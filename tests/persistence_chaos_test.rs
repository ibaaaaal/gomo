#[path = "../src/paths.rs"]
#[allow(dead_code)]
mod paths;

#[path = "../src/state.rs"]
#[allow(dead_code)]
mod state;

#[path = "../src/stats.rs"]
#[allow(dead_code)]
mod stats;

use chrono::{Duration as ChronoDuration, Local, TimeZone};
use state::AppState;
use stats::StatsStore;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "gomo-persistence-{}-{}-{}",
        std::process::id(),
        label,
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sidecar(path: &std::path::Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn with_temp_localappdata<T>(test: impl FnOnce(&PathBuf) -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let root = temp_dir("env");
    let local = root.join("local");
    fs::create_dir_all(&local).unwrap();
    let previous = std::env::var_os("LOCALAPPDATA");
    std::env::set_var("LOCALAPPDATA", &local);
    let result = test(&local);
    match previous {
        Some(value) => std::env::set_var("LOCALAPPDATA", value),
        None => std::env::remove_var("LOCALAPPDATA"),
    }
    let _ = fs::remove_dir_all(&root);
    result
}

#[test]
fn backup_roundtrip_preserves_focus_and_break_sessions() {
    let dir = temp_dir("roundtrip");
    let source_db = dir.join("source.db");
    let restored_db = dir.join("restored.db");
    let backup_path = dir.join("backup.json");
    let start = Local.with_ymd_and_hms(2026, 11, 6, 10, 0, 0).unwrap();

    {
        let mut store = StatsStore::open_at(&source_db).unwrap();
        store
            .record_focus_session(
                start,
                start + ChronoDuration::seconds(1500),
                1500,
                "Deep Work",
            )
            .unwrap();
        store
            .record_session(
                start + ChronoDuration::minutes(25),
                start + ChronoDuration::minutes(30),
                "short_break",
                "",
                300,
            )
            .unwrap();
        store.export_json(&backup_path).unwrap();
    }

    let mut restored = StatsStore::open_at(&restored_db).unwrap();
    restored.import_json(&backup_path).unwrap();

    let sessions = restored.sessions();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].task, "Deep Work");
    assert_eq!(sessions[1].mode, "short_break");
    assert_eq!(sessions[1].task, "");
    drop(restored);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_database_recovers_from_previous_backup() {
    with_temp_localappdata(|local| {
        let data_dir = local.join("Gomo");
        fs::create_dir_all(&data_dir).unwrap();
        let db_path = data_dir.join("stats.db");
        let today = Local::now().date_naive();

        {
            let mut store = StatsStore::open_at(&db_path).unwrap();
            store
                .record_focus_session(Local::now(), Local::now(), 1500, "Recovery")
                .unwrap();
            assert!(store.backup().is_some());
        }
        let backup_path = db_path.with_extension("db.bak");
        assert!(backup_path.is_file());
        assert!(fs::metadata(&backup_path).unwrap().len() > 0);

        fs::write(&db_path, b"this is not a sqlite database").unwrap();
        let store = StatsStore::load();
        let (sessions, seconds) = store.totals_between(today, today);
        assert_eq!(sessions, 1, "session history was lost after DB corruption");
        assert_eq!(seconds, 1500);
        assert!(db_path.with_extension("db.corrupt").is_file());
        assert!(backup_path.is_file());
    });
}

#[test]
fn config_load_falls_back_to_backup_and_defaults() {
    with_temp_localappdata(|local| {
        let data_dir = local.join("Gomo");
        fs::create_dir_all(&data_dir).unwrap();
        let config_path = data_dir.join("config.json");
        let backup_path = config_path.with_extension("json.bak");

        let saved = AppState {
            focus_duration_mins: 42,
            ..AppState::default()
        };
        let corrupted = b"{not valid json";
        fs::write(&config_path, corrupted).unwrap();
        fs::write(&backup_path, serde_json::to_vec_pretty(&saved).unwrap()).unwrap();

        let loaded = AppState::load();
        assert_eq!(loaded.focus_duration_mins, 42);
        let repaired: AppState = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(repaired.focus_duration_mins, 42);
        assert_eq!(
            fs::read(config_path.with_extension("json.corrupt")).unwrap(),
            corrupted
        );

        let mut changed = loaded;
        changed.focus_duration_mins = 50;
        changed.save().unwrap();
        let backup: AppState = serde_json::from_slice(&fs::read(&backup_path).unwrap()).unwrap();
        assert_eq!(backup.focus_duration_mins, 42);

        fs::write(&config_path, "{still broken").unwrap();
        fs::write(&backup_path, "{also broken").unwrap();
        let loaded = AppState::load();
        assert_eq!(
            loaded.focus_duration_mins,
            AppState::default().focus_duration_mins
        );
    });
}

#[test]
fn import_rejects_invalid_sessions_and_rolls_back() {
    let dir = temp_dir("strict-import");
    let dest_db = dir.join("dest.db");
    let start = Local.with_ymd_and_hms(2026, 11, 6, 10, 0, 0).unwrap();
    let mut store = StatsStore::open_at(&dest_db).unwrap();
    store
        .record_focus_session(start, start + ChronoDuration::minutes(25), 1500, "Existing")
        .unwrap();

    let invalid_sessions = [
        (
            "bad-timestamp.json",
            r#"{"version":2,"days":[],"day_tasks":[],"sessions":[{"started_at":"not-a-date","ended_at":"2026-11-06T10:25:00+07:00","mode":"focus","task":"X","seconds":1500}]}"#,
        ),
        (
            "bad-mode.json",
            r#"{"version":2,"days":[],"day_tasks":[],"sessions":[{"started_at":"2026-11-06T10:00:00+07:00","ended_at":"2026-11-06T10:25:00+07:00","mode":"nap","task":"X","seconds":1500}]}"#,
        ),
        (
            "empty-focus-task.json",
            r#"{"version":2,"days":[],"day_tasks":[],"sessions":[{"started_at":"2026-11-06T10:00:00+07:00","ended_at":"2026-11-06T10:25:00+07:00","mode":"focus","task":"  ","seconds":1500}]}"#,
        ),
    ];

    for (name, json) in invalid_sessions {
        let path = dir.join(name);
        fs::write(&path, json).unwrap();
        assert!(store.import_json(&path).is_err(), "{name} was accepted");
        assert_eq!(
            store.totals_between(start.date_naive(), start.date_naive()),
            (1, 1500),
            "{name} changed existing data"
        );
    }

    let duplicate_days = dir.join("duplicate-days.json");
    fs::write(
        &duplicate_days,
        r#"{"version":2,"days":[{"date":"2026-11-06","focus_sessions":1,"focus_seconds":1500},{"date":"2026-11-06","focus_sessions":1,"focus_seconds":1500}],"day_tasks":[],"sessions":[]}"#,
    )
    .unwrap();
    assert!(store.import_json(&duplicate_days).is_err());
    assert_eq!(
        store.totals_between(start.date_naive(), start.date_naive()),
        (1, 1500)
    );

    let empty_backup = dir.join("empty.json");
    fs::write(&empty_backup, r#"{"version":2}"#).unwrap();
    assert!(store.import_json(&empty_backup).is_err());
    assert_eq!(
        store.totals_between(start.date_naive(), start.date_naive()),
        (1, 1500)
    );

    drop(store);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn failed_backup_preserves_the_previous_snapshot() {
    let dir = temp_dir("backup-failure");
    let db_path = dir.join("stats.db");
    let today = Local::now().date_naive();

    {
        let mut store = StatsStore::open_at(&db_path).unwrap();
        store
            .record_focus_session(Local::now(), Local::now(), 1500, "First")
            .unwrap();
        let backup_path = store.backup().expect("initial backup failed");
        let original_backup = fs::read(&backup_path).unwrap();

        store
            .record_focus_session(Local::now(), Local::now(), 600, "Second")
            .unwrap();
        fs::create_dir(db_path.with_extension("db.bak.tmp")).unwrap();
        assert!(store.backup().is_none());
        assert_eq!(fs::read(&backup_path).unwrap(), original_backup);

        let snapshot = StatsStore::open_at(&backup_path).unwrap();
        assert_eq!(snapshot.totals_between(today, today), (1, 1500));
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_database_and_backup_does_not_delete_evidence_or_overwrite_backup() {
    with_temp_localappdata(|local| {
        let data_dir = local.join("Gomo");
        fs::create_dir_all(&data_dir).unwrap();
        let db_path = data_dir.join("stats.db");

        {
            let mut store = StatsStore::open_at(&db_path).unwrap();
            store
                .record_focus_session(Local::now(), Local::now(), 1500, "Recovery")
                .unwrap();
            assert!(store.backup().is_some());
        }

        let backup_path = db_path.with_extension("db.bak");
        fs::write(&db_path, b"broken db").unwrap();
        fs::write(&backup_path, b"broken backup").unwrap();

        let store = StatsStore::load();
        let today = Local::now().date_naive();
        assert_eq!(store.totals_between(today, today), (0, 0));
        assert!(db_path.with_extension("db.corrupt").is_file());
        assert_eq!(fs::read(&backup_path).unwrap(), b"broken backup");
    });
}

#[test]
fn recovery_quarantines_stale_wal_and_shm() {
    let dir = temp_dir("recover-sidecars");
    let db_path = dir.join("stats.db");
    let today = Local::now().date_naive();

    {
        let mut store = StatsStore::open_at(&db_path).unwrap();
        store
            .record_focus_session(Local::now(), Local::now(), 1500, "Sidecar")
            .unwrap();
        assert!(store.backup().is_some());
    }

    fs::write(&db_path, b"broken db").unwrap();
    fs::write(sidecar(&db_path, "-wal"), b"stale-wal").unwrap();
    fs::write(sidecar(&db_path, "-shm"), b"stale-shm").unwrap();

    let store = StatsStore::recover_from_backup_for_test(&db_path).expect("recovery failed");
    assert_eq!(store.totals_between(today, today), (1, 1500));
    let corrupt = db_path.with_extension("db.corrupt");
    assert_eq!(fs::read(sidecar(&corrupt, "-wal")).unwrap(), b"stale-wal");
    assert_eq!(fs::read(sidecar(&corrupt, "-shm")).unwrap(), b"stale-shm");

    drop(store);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn restore_backup_replaces_live_stats_and_keeps_safety_snapshot() {
    let dir = temp_dir("restore-backup");
    let db_path = dir.join("stats.db");
    let today = Local::now().date_naive();

    {
        let mut store = StatsStore::open_at(&db_path).unwrap();
        store
            .record_focus_session(Local::now(), Local::now(), 1500, "Before backup")
            .unwrap();
        assert!(store.backup().is_some());
        store
            .record_focus_session(Local::now(), Local::now(), 600, "After backup")
            .unwrap();

        let safety = store.restore_backup().unwrap();
        assert_eq!(store.totals_between(today, today), (1, 1500));
        assert!(safety.is_file());
        let safety_store = StatsStore::open_at(&safety).unwrap();
        assert_eq!(safety_store.totals_between(today, today), (2, 2100));
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn restore_without_or_with_corrupt_backup_leaves_current_stats_intact() {
    let dir = temp_dir("restore-missing");
    let db_path = dir.join("stats.db");
    let today = Local::now().date_naive();

    {
        let mut store = StatsStore::open_at(&db_path).unwrap();
        store
            .record_focus_session(Local::now(), Local::now(), 1500, "Current")
            .unwrap();
        assert!(store.restore_backup().is_err());
        assert_eq!(store.totals_between(today, today), (1, 1500));

        fs::write(db_path.with_extension("db.bak"), b"not a database").unwrap();
        assert!(store.restore_backup().is_err());
        assert_eq!(store.totals_between(today, today), (1, 1500));
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn config_save_is_atomic_and_reports_blocked_writes() {
    with_temp_localappdata(|local| {
        let config_path = local.join("Gomo").join("config.json");
        let mut state = AppState::default();
        state.save().unwrap();
        assert!(config_path.is_file());

        state.focus_duration_mins = 30;
        state.save().unwrap();
        let saved: AppState = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(saved.focus_duration_mins, 30);
        assert!(config_path.with_extension("json.bak").is_file());

        fs::create_dir(config_path.with_extension("json.tmp")).unwrap();
        state.focus_duration_mins = 45;
        assert!(state.save().is_err());
        let untouched: AppState = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(untouched.focus_duration_mins, 30);
    });
}
