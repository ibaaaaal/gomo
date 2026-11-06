#[path = "../src/paths.rs"]
#[allow(dead_code)]
mod paths;

#[path = "../src/stats.rs"]
#[allow(dead_code)]
mod stats;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone};
use stats::{StatsStore, HEATMAP_WEEKS};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn mem_store() -> StatsStore {
    StatsStore::open_in_memory().unwrap()
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "gomo-sqlite-{}-{}-{}",
        std::process::id(),
        label,
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn csv_export_reports_query_failure_instead_of_empty_success() {
    let mut store = mem_store();
    store
        .record_focus(date(2026, 11, 6), 1500, "General")
        .unwrap();
    store.break_sessions_for_test().unwrap();

    let dir = temp_dir("csv-error");
    let path = dir.join("sessions.csv");
    assert!(store.export_sessions_csv(&path).is_err());
    assert!(!path.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn in_memory_store_is_not_persistent() {
    let store = mem_store();
    assert!(!store.is_persistent());

    let dir = temp_dir("persistent");
    let store = StatsStore::open_at(&dir.join("stats.db")).unwrap();
    assert!(store.is_persistent());
    drop(store);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn record_focus_aggregates_by_day_and_task() {
    let mut store = mem_store();
    store
        .record_focus(date(2026, 11, 6), 1500, "Deep Work")
        .unwrap();
    store
        .record_focus(date(2026, 11, 6), 1500, "Deep Work")
        .unwrap();
    store
        .record_focus(date(2026, 11, 6), 600, "General")
        .unwrap();
    store
        .record_focus(date(2026, 11, 5), 3000, "Deep Work")
        .unwrap();

    let (sessions, seconds) = store.totals_between(date(2026, 11, 6), date(2026, 11, 6));
    assert_eq!(sessions, 3);
    assert_eq!(seconds, 3600);

    let tasks = store.task_totals_between(date(2026, 11, 6), date(2026, 11, 6));
    assert_eq!(
        tasks,
        vec![
            ("Deep Work".to_string(), 2, 3000),
            ("General".to_string(), 1, 600),
        ]
    );
}

#[test]
fn summary_totals_and_best_day() {
    let mut store = mem_store();
    store
        .record_focus(date(2026, 11, 4), 1500, "General")
        .unwrap();
    store
        .record_focus(date(2026, 11, 4), 1500, "General")
        .unwrap();
    store
        .record_focus(date(2026, 11, 5), 1500, "General")
        .unwrap();

    let summary = store.summary(date(2026, 11, 5));
    assert_eq!(summary.today_sessions, 1);
    assert_eq!(summary.today_seconds, 1500);
    assert_eq!(summary.total_sessions, 3);
    assert_eq!(summary.total_seconds, 4500);
    assert_eq!(summary.current_streak, 2);
    let (best_date, best_day) = summary.best_day.unwrap();
    assert_eq!(best_date, date(2026, 11, 4));
    assert_eq!(best_day.focus_sessions, 2);
    assert_eq!(best_day.focus_seconds, 3000);
}

#[test]
fn streak_survives_an_empty_today() {
    let mut store = mem_store();
    store
        .record_focus(date(2026, 11, 4), 1500, "General")
        .unwrap();
    store
        .record_focus(date(2026, 11, 5), 1500, "General")
        .unwrap();

    assert_eq!(store.summary(date(2026, 11, 6)).current_streak, 2);
}

#[test]
fn streak_resets_after_a_gap() {
    let mut store = mem_store();
    store
        .record_focus(date(2026, 11, 3), 1500, "General")
        .unwrap();
    store
        .record_focus(date(2026, 11, 5), 1500, "General")
        .unwrap();

    assert_eq!(store.summary(date(2026, 11, 6)).current_streak, 1);
}

#[test]
fn heatmap_aligns_weeks_on_monday_and_ends_today() {
    let today = date(2026, 11, 6);
    let mut store = mem_store();
    store.record_focus(today, 1500, "General").unwrap();

    let cells = store.heatmap(today, HEATMAP_WEEKS);
    assert_eq!(cells.len(), (HEATMAP_WEEKS * 7) as usize);
    assert_eq!(cells[0].date.weekday(), chrono::Weekday::Mon);

    let today_index = ((HEATMAP_WEEKS - 1) * 7 + today.weekday().num_days_from_monday()) as usize;
    assert_eq!(cells[today_index].date, today);
    assert_eq!(cells[today_index].level, 2);
    assert_eq!(cells[today_index].minutes, 25);
    assert_eq!(cells[today_index].sessions, 1);

    let expected_start = today
        - Duration::days(
            today.weekday().num_days_from_monday() as i64 + 7 * (HEATMAP_WEEKS as i64 - 1),
        );
    assert_eq!(cells[0].date, expected_start);

    for cell in &cells[today_index + 1..] {
        assert_eq!(cell.level, -1);
    }
}

#[test]
fn heatmap_levels_follow_focus_minutes() {
    let today = date(2026, 11, 6);
    let today_index = today.weekday().num_days_from_monday() as usize;
    let mut store = mem_store();
    store.record_focus(today, 24 * 60, "General").unwrap();
    assert_eq!(store.heatmap(today, 1)[today_index].level, 1);

    store.record_focus(today, 60, "General").unwrap();
    assert_eq!(store.heatmap(today, 1)[today_index].level, 2);

    store.record_focus(today, 25 * 60, "General").unwrap();
    assert_eq!(store.heatmap(today, 1)[today_index].level, 3);

    store.record_focus(today, 50 * 60, "General").unwrap();
    assert_eq!(store.heatmap(today, 1)[today_index].level, 4);
}

#[test]
fn task_totals_between_sorts_and_sums() {
    let mut store = mem_store();
    store
        .record_focus(date(2026, 11, 3), 3000, "Deep Work")
        .unwrap();
    store
        .record_focus(date(2026, 11, 4), 1500, "General")
        .unwrap();
    store
        .record_focus(date(2026, 11, 5), 1500, "Deep Work")
        .unwrap();
    store
        .record_focus(date(2026, 11, 6), 1500, "Belajar Jepang")
        .unwrap();
    store
        .record_focus(date(2026, 11, 7), 1500, "Deep Work")
        .unwrap();

    let rows = store.task_totals_between(date(2026, 11, 4), date(2026, 11, 6));
    assert_eq!(
        rows,
        vec![
            ("Belajar Jepang".to_string(), 1, 1500),
            ("Deep Work".to_string(), 1, 1500),
            ("General".to_string(), 1, 1500),
        ]
    );
}

#[test]
fn daily_series_is_ordered_and_ends_today() {
    let today = date(2026, 11, 6);
    let mut store = mem_store();
    store.record_focus(today, 1500, "General").unwrap();
    store
        .record_focus(date(2026, 11, 5), 3000, "General")
        .unwrap();

    let series = store.daily_series(today, 3);
    assert_eq!(
        series,
        vec![
            (date(2026, 11, 4), 0),
            (date(2026, 11, 5), 3000),
            (date(2026, 11, 6), 1500),
        ]
    );
}

#[test]
fn monthly_series_spans_year_boundary() {
    let today = date(2026, 2, 15);
    let mut store = mem_store();
    store
        .record_focus(date(2025, 12, 10), 1500, "General")
        .unwrap();
    store
        .record_focus(date(2026, 1, 5), 3000, "General")
        .unwrap();
    store
        .record_focus(date(2026, 2, 15), 4500, "General")
        .unwrap();

    let series = store.monthly_series(today, 4);
    assert_eq!(
        series,
        vec![
            (date(2025, 11, 1), 0),
            (date(2025, 12, 1), 1500),
            (date(2026, 1, 1), 3000),
            (date(2026, 2, 1), 4500),
        ]
    );
}

#[test]
fn persists_across_reopen() {
    let dir = temp_dir("persist");
    let db_path = dir.join("stats.db");
    {
        let mut store = StatsStore::open_at(&db_path).unwrap();
        store
            .record_focus(date(2026, 11, 6), 1500, "Deep Work")
            .unwrap();
    }
    {
        let store = StatsStore::open_at(&db_path).unwrap();
        assert_eq!(
            store.totals_between(date(2026, 11, 6), date(2026, 11, 6)),
            (1, 1500)
        );
        assert_eq!(
            store.task_totals_between(date(2026, 11, 6), date(2026, 11, 6)),
            vec![("Deep Work".to_string(), 1, 1500)]
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn imports_legacy_json_once_and_renames_it() {
    let dir = temp_dir("legacy");
    let json_path = dir.join("stats.json");
    let db_path = dir.join("stats.db");
    fs::write(
        &json_path,
        r#"{
            "version": 1,
            "days": {
                "2026-11-06": {
                    "focus_sessions": 2,
                    "focus_seconds": 3000,
                    "tasks": {
                        "Deep Work": { "focus_sessions": 2, "focus_seconds": 3000 }
                    }
                },
                "2026-11-05": { "focus_sessions": 1, "focus_seconds": 1500 }
            }
        }"#,
    )
    .unwrap();

    {
        let store = StatsStore::open_at(&db_path).unwrap();
        assert_eq!(
            store.totals_between(date(2026, 11, 5), date(2026, 11, 6)),
            (3, 4500)
        );
        assert_eq!(
            store.task_totals_between(date(2026, 11, 5), date(2026, 11, 6)),
            vec![("Deep Work".to_string(), 2, 3000)]
        );
    }

    assert!(!json_path.exists());
    assert!(dir.join("stats.imported.json").exists());

    {
        let store = StatsStore::open_at(&db_path).unwrap();
        assert_eq!(
            store.totals_between(date(2026, 11, 5), date(2026, 11, 6)),
            (3, 4500)
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn backup_creates_a_readable_snapshot() {
    let dir = temp_dir("backup");
    let db_path = dir.join("stats.db");
    let backup_path = db_path.with_extension("db.bak");
    {
        let mut store = StatsStore::open_at(&db_path).unwrap();
        store
            .record_focus(date(2026, 11, 6), 1500, "General")
            .unwrap();
        store.backup();
        assert!(backup_path.exists());
        assert!(fs::metadata(&backup_path).unwrap().len() > 0);

        let snapshot = StatsStore::open_at(&backup_path).unwrap();
        assert_eq!(
            snapshot.totals_between(date(2026, 11, 6), date(2026, 11, 6)),
            (1, 1500)
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn session_log_uses_start_date_and_metadata() {
    let mut store = mem_store();
    let start = Local.with_ymd_and_hms(2026, 11, 6, 23, 59, 0).unwrap();
    let end = start + Duration::seconds(60);

    store
        .record_focus_session(start, end, 60, "Deep Work")
        .unwrap();

    assert_eq!(
        store.totals_between(date(2026, 11, 6), date(2026, 11, 6)),
        (1, 60)
    );
    assert_eq!(
        store.totals_between(date(2026, 11, 7), date(2026, 11, 7)),
        (0, 0)
    );
    let sessions = store.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].mode, "focus");
    assert_eq!(sessions[0].task, "Deep Work");
    assert_eq!(sessions[0].seconds, 60);
    assert!(sessions[0].started_at.contains("2026-11-06"));

    let started = DateTime::parse_from_rfc3339(&sessions[0].started_at).unwrap();
    let ended = DateTime::parse_from_rfc3339(&sessions[0].ended_at).unwrap();
    assert_eq!(ended - started, Duration::seconds(60));
}

#[test]
fn export_import_json_and_csv_roundtrip() {
    let dir = temp_dir("export");
    let source_db = dir.join("source.db");
    let restored_db = dir.join("restored.db");
    let json_path = dir.join("backup.json");
    let csv_path = dir.join("sessions.csv");
    let start = Local.with_ymd_and_hms(2026, 11, 6, 10, 0, 0).unwrap();

    {
        let mut store = StatsStore::open_at(&source_db).unwrap();
        store
            .record_focus_session(
                start,
                start + Duration::seconds(1500),
                1500,
                "Deep, \"Work\"",
            )
            .unwrap();
        store.export_json(&json_path).unwrap();
        store.export_sessions_csv(&csv_path).unwrap();
    }

    {
        let mut store = StatsStore::open_at(&restored_db).unwrap();
        store.record_focus(date(2026, 1, 1), 1500, "Old").unwrap();
        store.import_json(&json_path).unwrap();

        assert_eq!(
            store.totals_between(date(2026, 1, 1), date(2026, 1, 1)),
            (0, 0)
        );
        assert_eq!(
            store.totals_between(date(2026, 11, 6), date(2026, 11, 6)),
            (1, 1500)
        );
        let sessions = store.sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].task, "Deep, \"Work\"");
    }

    let csv_text = fs::read_to_string(&csv_path).unwrap();
    assert!(csv_text.starts_with("\u{feff}started_at,ended_at,mode,task,seconds"));
    assert!(csv_text.contains("\"Deep, \"\"Work\"\""));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn csv_export_has_bom_and_neutralizes_formulas() {
    let mut store = mem_store();
    let start = Local.with_ymd_and_hms(2026, 11, 6, 10, 0, 0).unwrap();
    store
        .record_focus_session(start, start + Duration::seconds(60), 60, "=1+1")
        .unwrap();

    let dir = temp_dir("csv-formula");
    let path = dir.join("sessions.csv");
    store.export_sessions_csv(&path).unwrap();

    let bytes = fs::read(&path).unwrap();
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("'=1+1"));

    drop(store);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn session_log_can_record_break_modes() {
    let mut store = mem_store();
    let start = Local.with_ymd_and_hms(2026, 11, 6, 10, 0, 0).unwrap();
    store
        .record_session(
            start,
            start + Duration::seconds(300),
            "short_break",
            "",
            300,
        )
        .unwrap();

    let sessions = store.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].mode, "short_break");
    assert_eq!(sessions[0].task, "");
    assert_eq!(sessions[0].seconds, 300);
}
