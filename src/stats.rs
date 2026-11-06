use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const HEATMAP_WEEKS: u32 = 26;
const SCHEMA_VERSION: i64 = 2;

macro_rules! stats_log {
    ($($arg:tt)*) => {
        log_stats_error(&format!($($arg)*))
    };
}

#[cfg(not(test))]
fn log_stats_error(message: &str) {
    eprintln!("[stats] {message}");
    crate::logging::error("stats", message);
}

#[cfg(test)]
fn log_stats_error(message: &str) {
    eprintln!("[stats] {message}");
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DayStats {
    pub focus_sessions: u32,
    pub focus_seconds: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub today_sessions: u32,
    pub today_seconds: u64,
    pub total_sessions: u32,
    pub total_seconds: u64,
    pub current_streak: u32,
    pub best_day: Option<(NaiveDate, DayStats)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub started_at: String,
    pub ended_at: String,
    pub mode: String,
    pub task: String,
    pub seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeatmapDay {
    pub date: NaiveDate,
    pub level: i32,
    pub minutes: u32,
    pub sessions: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupFile {
    version: u32,
    #[serde(default)]
    exported_at: String,
    #[serde(default)]
    days: Vec<BackupDay>,
    #[serde(default)]
    day_tasks: Vec<BackupTask>,
    #[serde(default)]
    sessions: Vec<BackupSession>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupDay {
    date: String,
    focus_sessions: u32,
    focus_seconds: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupTask {
    date: String,
    task: String,
    focus_sessions: u32,
    focus_seconds: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupSession {
    started_at: String,
    ended_at: String,
    mode: String,
    task: String,
    seconds: u64,
}

#[derive(Debug, Deserialize)]
struct LegacyStore {
    #[serde(default)]
    days: BTreeMap<NaiveDate, LegacyDay>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyDay {
    #[serde(default)]
    focus_sessions: u32,
    #[serde(default)]
    focus_seconds: u64,
    #[serde(default)]
    tasks: BTreeMap<String, LegacyTask>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyTask {
    #[serde(default)]
    focus_sessions: u32,
    #[serde(default)]
    focus_seconds: u64,
}

pub struct StatsStore {
    conn: Connection,
    db_path: Option<PathBuf>,
}

fn unique_corrupt_path(path: &Path) -> PathBuf {
    let base = path.with_extension("db.corrupt");
    if !base.exists() {
        return base;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    for attempt in 0..1000 {
        let candidate = path.with_extension(format!("db.corrupt.{stamp}.{attempt}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn quarantine_sidecars(path: &Path, corrupt: &Path) {
    for suffix in ["-wal", "-shm"] {
        let source = sidecar_path(path, suffix);
        if source.exists() {
            let destination = sidecar_path(corrupt, suffix);
            if fs::rename(&source, &destination).is_err() {
                let _ = fs::remove_file(&source);
            }
        }
    }
}

fn remove_sidecars(path: &Path) {
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(sidecar_path(path, suffix));
    }
}

impl StatsStore {
    fn data_path() -> PathBuf {
        let dir = crate::paths::data_dir();
        let _ = fs::create_dir_all(&dir);
        dir.join("stats.db")
    }

    pub fn load() -> Self {
        let path = Self::data_path();
        match Self::open_at(&path) {
            Ok(store) => {
                store.backup();
                store
            }
            Err(err) => {
                stats_log!("failed to open {}: {err}", path.display());
                if let Some(store) = Self::recover_from_backup(&path) {
                    stats_log!(
                        "recovered database from {}",
                        path.with_extension("db.bak").display()
                    );
                    return store;
                }
                let corrupt = unique_corrupt_path(&path);
                let _ = fs::rename(&path, &corrupt);
                quarantine_sidecars(&path, &corrupt);
                match Self::open_at(&path) {
                    Ok(store) => {
                        if !path.with_extension("db.bak").exists() {
                            store.backup();
                        }
                        store
                    }
                    Err(retry_err) => {
                        stats_log!("falling back to in-memory database: {retry_err}");
                        Self::open_in_memory()
                            .unwrap_or_else(|e| panic!("cannot open in-memory SQLite: {e}"))
                    }
                }
            }
        }
    }

    fn recover_from_backup(path: &Path) -> Option<Self> {
        let backup = path.with_extension("db.bak");
        if !backup.is_file() {
            return None;
        }

        let corrupt = unique_corrupt_path(path);
        if fs::rename(path, &corrupt).is_err() {
            return None;
        }
        quarantine_sidecars(path, &corrupt);
        if let Err(err) = fs::copy(&backup, path) {
            stats_log!("could not restore backup: {err}");
            return None;
        }

        match Self::open_at(path) {
            Ok(store) => Some(store),
            Err(err) => {
                stats_log!("backup recovery failed: {err}");
                let _ = fs::remove_file(path);
                None
            }
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn recover_from_backup_for_test(path: &Path) -> Option<Self> {
        Self::recover_from_backup(path)
    }

    pub fn open_at(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::configure(&conn)?;
        let mut store = Self {
            conn,
            db_path: Some(path.to_path_buf()),
        };
        store.init_schema()?;
        store.import_legacy_json()?;
        Ok(store)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn)?;
        let store = Self {
            conn,
            db_path: None,
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn is_persistent(&self) -> bool {
        self.db_path.is_some()
    }

    fn configure(conn: &Connection) -> rusqlite::Result<()> {
        conn.busy_timeout(Duration::from_secs(3))?;
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }

    fn init_schema(&self) -> rusqlite::Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS days (
                date TEXT PRIMARY KEY,
                focus_sessions INTEGER NOT NULL DEFAULT 0,
                focus_seconds INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS day_tasks (
                date TEXT NOT NULL,
                task TEXT NOT NULL,
                focus_sessions INTEGER NOT NULL DEFAULT 0,
                focus_seconds INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, task),
                FOREIGN KEY (date) REFERENCES days(date) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_day_tasks_task_date
                ON day_tasks(task, date);
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at TEXT NOT NULL,
                ended_at TEXT NOT NULL,
                mode TEXT NOT NULL,
                task TEXT NOT NULL,
                seconds INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_started_at
                ON sessions(started_at);",
        )?;
        if version < SCHEMA_VERSION {
            self.conn
                .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version > SCHEMA_VERSION {
            stats_log!("database schema v{version} is newer than supported v{SCHEMA_VERSION}");
        }
        Ok(())
    }

    fn import_legacy_json(&mut self) -> rusqlite::Result<()> {
        let Some(db_path) = self.db_path.clone() else {
            return Ok(());
        };
        let json_path = db_path.with_file_name("stats.json");
        if !json_path.exists() {
            return Ok(());
        }
        let existing: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM days", [], |row| row.get(0))?;
        if existing > 0 {
            return Ok(());
        }

        let text = match fs::read_to_string(&json_path) {
            Ok(text) => text,
            Err(err) => {
                stats_log!("cannot read legacy stats.json: {err}");
                return Ok(());
            }
        };
        let legacy: LegacyStore = match serde_json::from_str(&text) {
            Ok(legacy) => legacy,
            Err(err) => {
                stats_log!("cannot parse legacy stats.json: {err}");
                return Ok(());
            }
        };

        let tx = self.conn.transaction()?;
        for (date, day) in &legacy.days {
            let date = date_string(*date);
            tx.execute(
                "INSERT INTO days(date, focus_sessions, focus_seconds)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(date) DO UPDATE SET
                    focus_sessions = excluded.focus_sessions,
                    focus_seconds = excluded.focus_seconds",
                params![
                    date,
                    to_i64(day.focus_sessions as u64),
                    to_i64(day.focus_seconds)
                ],
            )?;
            for (task, stats) in &day.tasks {
                tx.execute(
                    "INSERT INTO day_tasks(date, task, focus_sessions, focus_seconds)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(date, task) DO UPDATE SET
                        focus_sessions = excluded.focus_sessions,
                        focus_seconds = excluded.focus_seconds",
                    params![
                        date,
                        task,
                        to_i64(stats.focus_sessions as u64),
                        to_i64(stats.focus_seconds)
                    ],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('legacy_json_imported', '1')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )?;
        tx.commit()?;

        let imported = db_path.with_file_name("stats.imported.json");
        if let Err(err) = fs::rename(&json_path, &imported) {
            stats_log!("imported legacy JSON but could not rename it: {err}");
        }
        Ok(())
    }

    pub fn backup(&self) -> Option<PathBuf> {
        let db_path = self.db_path.as_ref()?;
        let backup_path = db_path.with_extension("db.bak");
        let temp_path = db_path.with_extension("db.bak.tmp");
        let _ = fs::remove_file(&temp_path);
        let temp = temp_path.to_string_lossy().to_string();
        if let Err(err) = self.conn.execute("VACUUM INTO ?1", params![temp]) {
            stats_log!("backup failed: {err}");
            let _ = fs::remove_file(&temp_path);
            return None;
        }

        let previous_path = db_path.with_extension("db.bak.prev");
        let _ = fs::remove_file(&previous_path);
        if backup_path.exists() {
            if let Err(err) = fs::rename(&backup_path, &previous_path) {
                stats_log!("backup failed to stage previous snapshot: {err}");
                let _ = fs::remove_file(&temp_path);
                return None;
            }
        }
        if let Err(err) = fs::rename(&temp_path, &backup_path) {
            stats_log!("backup failed to activate new snapshot: {err}");
            if previous_path.exists() {
                if let Err(restore_err) = fs::rename(&previous_path, &backup_path) {
                    stats_log!("backup restore failed: {restore_err}");
                }
            }
            let _ = fs::remove_file(&temp_path);
            return None;
        }
        let _ = fs::remove_file(&previous_path);
        Some(backup_path)
    }

    /// Replaces the live database with the latest .bak snapshot, keeping the
    /// current database in stats.db.pre-restore first.
    pub fn restore_backup(&mut self) -> Result<PathBuf, String> {
        let db_path = self
            .db_path
            .clone()
            .ok_or_else(|| "statistics are using in-memory storage".to_string())?;
        let backup_path = db_path.with_extension("db.bak");
        if !backup_path.is_file() {
            return Err("no backup snapshot available".to_string());
        }

        let staging_path = db_path.with_extension("db.restore.tmp");
        let safety_path = db_path.with_extension("db.pre-restore");
        let _ = fs::remove_file(&staging_path);
        remove_sidecars(&staging_path);
        let _ = fs::remove_file(&safety_path);

        fs::copy(&backup_path, &staging_path)
            .map_err(|err| format!("could not stage backup: {err}"))?;
        if let Err(err) = Self::open_at(&staging_path) {
            let _ = fs::remove_file(&staging_path);
            remove_sidecars(&staging_path);
            return Err(format!("backup is not readable: {err}"));
        }
        remove_sidecars(&staging_path);

        let safety_text = safety_path.to_string_lossy().to_string();
        if let Err(err) = self.conn.execute("VACUUM INTO ?1", params![safety_text]) {
            let _ = fs::remove_file(&staging_path);
            return Err(format!("could not create pre-restore snapshot: {err}"));
        }
        if let Err(err) = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
            let _ = fs::remove_file(&staging_path);
            let _ = fs::remove_file(&safety_path);
            return Err(format!("could not checkpoint current database: {err}"));
        }

        let placeholder = Connection::open_in_memory()
            .and_then(|conn| {
                Self::configure(&conn)?;
                Ok(conn)
            })
            .map_err(|err| format!("could not prepare restore: {err}"))?;
        let old_connection = std::mem::replace(&mut self.conn, placeholder);
        drop(old_connection);
        remove_sidecars(&db_path);

        if let Err(err) = fs::remove_file(&db_path) {
            if let Ok(store) = Self::open_at(&db_path) {
                self.conn = store.conn;
            }
            let _ = fs::remove_file(&staging_path);
            return Err(format!("could not replace current database: {err}"));
        }
        if let Err(err) = fs::rename(&staging_path, &db_path) {
            let _ = fs::copy(&safety_path, &db_path);
            if let Ok(store) = Self::open_at(&db_path) {
                self.conn = store.conn;
            }
            let _ = fs::remove_file(&staging_path);
            return Err(format!("could not activate restored database: {err}"));
        }

        match Self::open_at(&db_path) {
            Ok(store) => {
                self.conn = store.conn;
                Ok(safety_path)
            }
            Err(err) => {
                let _ = fs::copy(&safety_path, &db_path);
                if let Ok(store) = Self::open_at(&db_path) {
                    self.conn = store.conn;
                }
                Err(format!(
                    "restored backup could not be opened: {err}; pre-restore snapshot kept"
                ))
            }
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn record_focus(
        &mut self,
        date: NaiveDate,
        duration_secs: u64,
        task: &str,
    ) -> rusqlite::Result<()> {
        let task = normalized_task(task);
        let date = date_string(date);
        let seconds = to_i64(duration_secs);

        let tx = self.conn.transaction()?;
        upsert_aggregates(&tx, &date, seconds, task)?;
        tx.commit()
    }

    pub fn record_focus_session(
        &mut self,
        started_at: DateTime<Local>,
        ended_at: DateTime<Local>,
        duration_secs: u64,
        task: &str,
    ) -> rusqlite::Result<()> {
        let task = normalized_task(task);
        let date = date_string(started_at.date_naive());
        let seconds = to_i64(duration_secs);
        let ended_at = consistent_end(started_at, ended_at, duration_secs);

        let tx = self.conn.transaction()?;
        upsert_aggregates(&tx, &date, seconds, task)?;
        tx.execute(
            "INSERT INTO sessions(started_at, ended_at, mode, task, seconds)
             VALUES (?1, ?2, 'focus', ?3, ?4)",
            params![
                started_at.to_rfc3339(),
                ended_at.to_rfc3339(),
                task,
                seconds
            ],
        )?;
        tx.commit()
    }

    pub fn record_session(
        &mut self,
        started_at: DateTime<Local>,
        ended_at: DateTime<Local>,
        mode: &str,
        task: &str,
        duration_secs: u64,
    ) -> rusqlite::Result<()> {
        let ended_at = consistent_end(started_at, ended_at, duration_secs);
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO sessions(started_at, ended_at, mode, task, seconds)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                started_at.to_rfc3339(),
                ended_at.to_rfc3339(),
                mode,
                task,
                to_i64(duration_secs)
            ],
        )?;
        tx.commit()
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn break_sessions_for_test(&self) -> rusqlite::Result<()> {
        self.conn.execute("DROP TABLE sessions", [])?;
        Ok(())
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn sessions(&self) -> Vec<SessionRow> {
        match self.try_sessions() {
            Ok(rows) => rows,
            Err(err) => {
                stats_log!("read sessions: {err}");
                Vec::new()
            }
        }
    }

    fn try_sessions(&self) -> rusqlite::Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT started_at, ended_at, mode, task, seconds
             FROM sessions ORDER BY started_at, id",
        )?;
        let rows = stmt.query_map([], |row| {
            let seconds: i64 = row.get(4)?;
            Ok(SessionRow {
                started_at: row.get(0)?,
                ended_at: row.get(1)?,
                mode: row.get(2)?,
                task: row.get(3)?,
                seconds: seconds.max(0) as u64,
            })
        })?;
        rows.collect()
    }

    pub fn export_json(&self, path: &Path) -> Result<(), String> {
        let backup = self.build_backup().map_err(|err| err.to_string())?;
        let json = serde_json::to_string_pretty(&backup).map_err(|err| err.to_string())?;
        fs::write(path, json).map_err(|err| err.to_string())
    }

    pub fn export_sessions_csv(&self, path: &Path) -> Result<(), String> {
        let sessions = self.try_sessions().map_err(|err| err.to_string())?;
        let mut output = String::from("\u{feff}started_at,ended_at,mode,task,seconds\n");
        for session in sessions {
            output.push_str(&format!(
                "{},{},{},{},{}\n",
                csv_field(&session.started_at),
                csv_field(&session.ended_at),
                csv_field(&session.mode),
                csv_field(&session.task),
                session.seconds
            ));
        }
        fs::write(path, output).map_err(|err| err.to_string())
    }

    pub fn import_json(&mut self, path: &Path) -> Result<(), String> {
        let text = fs::read_to_string(path).map_err(|err| err.to_string())?;
        let backup: BackupFile = serde_json::from_str(&text).map_err(|err| err.to_string())?;
        if backup.version == 0 || backup.version > SCHEMA_VERSION as u32 {
            return Err(format!("unsupported backup version {}", backup.version));
        }
        if backup.days.is_empty() && backup.day_tasks.is_empty() && backup.sessions.is_empty() {
            return Err("backup contains no data".to_string());
        }
        for day in &backup.days {
            if parse_date(&day.date).is_none() {
                return Err(format!("invalid day date: {}", day.date));
            }
        }
        for task in &backup.day_tasks {
            if parse_date(&task.date).is_none() || task.task.trim().is_empty() {
                return Err(format!("invalid task row for date {}", task.date));
            }
        }
        for session in &backup.sessions {
            let mode = session.mode.trim();
            let valid_mode = matches!(mode, "focus" | "short_break" | "long_break");
            let valid_times = DateTime::parse_from_rfc3339(&session.started_at).is_ok()
                && DateTime::parse_from_rfc3339(&session.ended_at).is_ok();
            let valid_task = mode != "focus" || !session.task.trim().is_empty();
            if !valid_mode || !valid_times || !valid_task {
                return Err(format!("invalid session row in backup: {mode}"));
            }
        }
        if self.backup().is_none() {
            return Err("cannot create a backup before importing".to_string());
        }

        let tx = self.conn.transaction().map_err(|err| err.to_string())?;
        tx.execute("DELETE FROM sessions", [])
            .map_err(|err| err.to_string())?;
        tx.execute("DELETE FROM day_tasks", [])
            .map_err(|err| err.to_string())?;
        tx.execute("DELETE FROM days", [])
            .map_err(|err| err.to_string())?;
        for day in &backup.days {
            tx.execute(
                "INSERT INTO days(date, focus_sessions, focus_seconds) VALUES (?1, ?2, ?3)",
                params![
                    day.date,
                    to_i64(day.focus_sessions as u64),
                    to_i64(day.focus_seconds)
                ],
            )
            .map_err(|err| err.to_string())?;
        }
        for task in &backup.day_tasks {
            tx.execute(
                "INSERT INTO day_tasks(date, task, focus_sessions, focus_seconds)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    task.date,
                    task.task,
                    to_i64(task.focus_sessions as u64),
                    to_i64(task.focus_seconds)
                ],
            )
            .map_err(|err| err.to_string())?;
        }
        for session in &backup.sessions {
            tx.execute(
                "INSERT INTO sessions(started_at, ended_at, mode, task, seconds)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    session.started_at,
                    session.ended_at,
                    session.mode,
                    session.task,
                    to_i64(session.seconds)
                ],
            )
            .map_err(|err| err.to_string())?;
        }
        tx.commit().map_err(|err| err.to_string())
    }

    fn build_backup(&self) -> rusqlite::Result<BackupFile> {
        let mut days_stmt = self
            .conn
            .prepare("SELECT date, focus_sessions, focus_seconds FROM days ORDER BY date")?;
        let days = days_stmt
            .query_map([], |row| {
                let sessions: i64 = row.get(1)?;
                let seconds: i64 = row.get(2)?;
                Ok(BackupDay {
                    date: row.get(0)?,
                    focus_sessions: u32::try_from(sessions.max(0)).unwrap_or(u32::MAX),
                    focus_seconds: seconds.max(0) as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut tasks_stmt = self.conn.prepare(
            "SELECT date, task, focus_sessions, focus_seconds
             FROM day_tasks ORDER BY date, task",
        )?;
        let day_tasks = tasks_stmt
            .query_map([], |row| {
                let sessions: i64 = row.get(2)?;
                let seconds: i64 = row.get(3)?;
                Ok(BackupTask {
                    date: row.get(0)?,
                    task: row.get(1)?,
                    focus_sessions: u32::try_from(sessions.max(0)).unwrap_or(u32::MAX),
                    focus_seconds: seconds.max(0) as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let sessions = self
            .try_sessions()?
            .into_iter()
            .map(|session| BackupSession {
                started_at: session.started_at,
                ended_at: session.ended_at,
                mode: session.mode,
                task: session.task,
                seconds: session.seconds,
            })
            .collect();

        Ok(BackupFile {
            version: SCHEMA_VERSION as u32,
            exported_at: Local::now().to_rfc3339(),
            days,
            day_tasks,
            sessions,
        })
    }

    pub fn summary(&self, today: NaiveDate) -> Summary {
        or_log(self.try_summary(today), "read summary")
    }

    pub fn heatmap(&self, today: NaiveDate, weeks: u32) -> Vec<HeatmapDay> {
        or_log(self.try_heatmap(today, weeks), "read heatmap")
    }

    pub fn totals_between(&self, start: NaiveDate, end: NaiveDate) -> (u32, u64) {
        or_log(self.try_totals_between(start, end), "read totals")
    }

    pub fn best_day_between(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Option<(NaiveDate, DayStats)> {
        or_log(self.try_best_day_between(start, end), "read best day")
    }

    pub fn task_totals_between(&self, start: NaiveDate, end: NaiveDate) -> Vec<(String, u32, u64)> {
        or_log(self.try_task_totals_between(start, end), "read task totals")
    }

    pub fn daily_series(&self, today: NaiveDate, days: u32) -> Vec<(NaiveDate, u64)> {
        or_log(self.try_daily_series(today, days), "read daily series")
    }

    pub fn monthly_series(&self, today: NaiveDate, months: u32) -> Vec<(NaiveDate, u64)> {
        or_log(
            self.try_monthly_series(today, months),
            "read monthly series",
        )
    }

    fn try_day_stats(&self, date: NaiveDate) -> rusqlite::Result<DayStats> {
        self.conn
            .query_row(
                "SELECT focus_sessions, focus_seconds FROM days WHERE date = ?1",
                params![date_string(date)],
                |row| read_day_at(row, 0),
            )
            .optional()
            .map(|stats| stats.unwrap_or_default())
    }

    fn try_summary(&self, today: NaiveDate) -> rusqlite::Result<Summary> {
        let today_stats = self.try_day_stats(today)?;
        let (total_sessions, total_seconds) = self.read_all_totals()?;
        let best_day = self.try_best_day_between(
            NaiveDate::from_ymd_opt(1, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(9999, 12, 31).unwrap(),
        )?;
        Ok(Summary {
            today_sessions: today_stats.focus_sessions,
            today_seconds: today_stats.focus_seconds,
            total_sessions,
            total_seconds,
            current_streak: self.try_current_streak(today)?,
            best_day,
        })
    }

    fn read_all_totals(&self) -> rusqlite::Result<(u32, u64)> {
        self.conn.query_row(
            "SELECT COALESCE(SUM(focus_sessions), 0), COALESCE(SUM(focus_seconds), 0)
             FROM days",
            [],
            read_totals,
        )
    }

    fn try_current_streak(&self, today: NaiveDate) -> rusqlite::Result<u32> {
        let mut stmt = self
            .conn
            .prepare("SELECT date FROM days WHERE focus_sessions > 0")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let dates: HashSet<NaiveDate> = rows
            .filter_map(|row| row.ok())
            .filter_map(|date| parse_date(&date))
            .collect();

        let mut day = if dates.contains(&today) {
            today
        } else {
            match today.pred_opt() {
                Some(yesterday) if dates.contains(&yesterday) => yesterday,
                _ => return Ok(0),
            }
        };

        let mut streak = 0u32;
        while dates.contains(&day) {
            streak = streak.saturating_add(1);
            match day.pred_opt() {
                Some(previous) => day = previous,
                None => break,
            }
        }
        Ok(streak)
    }

    fn try_heatmap(&self, today: NaiveDate, weeks: u32) -> rusqlite::Result<Vec<HeatmapDay>> {
        let weeks = weeks.max(1) as i64;
        let days_from_monday = today.weekday().num_days_from_monday() as i64;
        let start = today - ChronoDuration::days(days_from_monday + 7 * (weeks - 1));
        let start_string = date_string(start);
        let end_string = date_string(today);

        let mut stmt = self.conn.prepare(
            "SELECT date, focus_sessions, focus_seconds FROM days
             WHERE date BETWEEN ?1 AND ?2",
        )?;
        let rows = stmt.query_map(params![start_string, end_string], |row| {
            let date: String = row.get(0)?;
            let stats = read_day_at(row, 1)?;
            Ok((date, stats))
        })?;
        let mut by_date: HashMap<NaiveDate, DayStats> = HashMap::new();
        for row in rows {
            let (date, stats) = row?;
            if let Some(date) = parse_date(&date) {
                by_date.insert(date, stats);
            }
        }

        let mut cells = Vec::with_capacity((weeks * 7) as usize);
        for week in 0..weeks {
            for row in 0..7 {
                let date = start + ChronoDuration::days(week * 7 + row);
                if date > today {
                    cells.push(HeatmapDay {
                        date,
                        level: -1,
                        minutes: 0,
                        sessions: 0,
                    });
                    continue;
                }
                let stats = by_date.get(&date).copied().unwrap_or_default();
                cells.push(HeatmapDay {
                    date,
                    level: level_for(stats.focus_seconds),
                    minutes: minutes_from_secs(stats.focus_seconds),
                    sessions: stats.focus_sessions,
                });
            }
        }
        Ok(cells)
    }

    fn try_totals_between(&self, start: NaiveDate, end: NaiveDate) -> rusqlite::Result<(u32, u64)> {
        self.conn.query_row(
            "SELECT COALESCE(SUM(focus_sessions), 0), COALESCE(SUM(focus_seconds), 0)
             FROM days WHERE date BETWEEN ?1 AND ?2",
            params![date_string(start), date_string(end)],
            read_totals,
        )
    }

    fn try_best_day_between(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> rusqlite::Result<Option<(NaiveDate, DayStats)>> {
        let row = self
            .conn
            .query_row(
                "SELECT date, focus_sessions, focus_seconds FROM days
                 WHERE focus_sessions > 0 AND date BETWEEN ?1 AND ?2
                 ORDER BY focus_seconds DESC, date DESC LIMIT 1",
                params![date_string(start), date_string(end)],
                |row| {
                    let date: String = row.get(0)?;
                    let stats = read_day_at(row, 1)?;
                    Ok((date, stats))
                },
            )
            .optional()?;
        Ok(row.and_then(|(date, stats)| parse_date(&date).map(|date| (date, stats))))
    }

    fn try_task_totals_between(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> rusqlite::Result<Vec<(String, u32, u64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT task, COALESCE(SUM(focus_sessions), 0), COALESCE(SUM(focus_seconds), 0)
             FROM day_tasks WHERE date BETWEEN ?1 AND ?2
             GROUP BY task
             ORDER BY SUM(focus_seconds) DESC, task ASC",
        )?;
        let rows = stmt.query_map(params![date_string(start), date_string(end)], |row| {
            let task: String = row.get(0)?;
            let sessions: i64 = row.get(1)?;
            let seconds: i64 = row.get(2)?;
            Ok((
                task,
                u32::try_from(sessions.max(0)).unwrap_or(u32::MAX),
                seconds.max(0) as u64,
            ))
        })?;
        rows.collect()
    }

    fn try_daily_series(
        &self,
        today: NaiveDate,
        days: u32,
    ) -> rusqlite::Result<Vec<(NaiveDate, u64)>> {
        let days = days.max(1) as i64;
        let start = today - ChronoDuration::days(days - 1);
        let mut stmt = self.conn.prepare(
            "SELECT date, focus_seconds FROM days
             WHERE date BETWEEN ?1 AND ?2",
        )?;
        let rows = stmt.query_map(params![date_string(start), date_string(today)], |row| {
            let date: String = row.get(0)?;
            let seconds: i64 = row.get(1)?;
            Ok((date, seconds.max(0) as u64))
        })?;
        let mut by_date: HashMap<NaiveDate, u64> = HashMap::new();
        for row in rows {
            let (date, seconds) = row?;
            if let Some(date) = parse_date(&date) {
                by_date.insert(date, seconds);
            }
        }
        Ok((0..days)
            .map(|offset| {
                let date = start + ChronoDuration::days(offset);
                (date, by_date.get(&date).copied().unwrap_or(0))
            })
            .collect())
    }

    fn try_monthly_series(
        &self,
        today: NaiveDate,
        months: u32,
    ) -> rusqlite::Result<Vec<(NaiveDate, u64)>> {
        let months = months.max(1) as i64;
        let current_month =
            NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);
        let first = shift_months(current_month, -(months - 1));
        let after = shift_months(current_month, 1);

        let mut stmt = self.conn.prepare(
            "SELECT substr(date, 1, 7) AS month, COALESCE(SUM(focus_seconds), 0)
             FROM days WHERE date >= ?1 AND date < ?2
             GROUP BY month",
        )?;
        let rows = stmt.query_map(params![date_string(first), date_string(after)], |row| {
            let month: String = row.get(0)?;
            let seconds: i64 = row.get(1)?;
            Ok((month, seconds.max(0) as u64))
        })?;
        let mut by_month: HashMap<String, u64> = HashMap::new();
        for row in rows {
            let (month, seconds) = row?;
            by_month.insert(month, seconds);
        }

        Ok((0..months)
            .map(|offset| {
                let month = shift_months(first, offset);
                let key = format!("{:04}-{:02}", month.year(), month.month());
                (month, by_month.get(&key).copied().unwrap_or(0))
            })
            .collect())
    }
}

fn normalized_task(task: &str) -> &str {
    if task.trim().is_empty() {
        "General"
    } else {
        task.trim()
    }
}

fn consistent_end(
    started_at: DateTime<Local>,
    ended_at: DateTime<Local>,
    duration_secs: u64,
) -> DateTime<Local> {
    if ended_at < started_at {
        started_at + ChronoDuration::seconds(duration_secs.min(i64::MAX as u64) as i64)
    } else {
        ended_at
    }
}

// days and day_tasks are read by different screens, so both have to move together
fn upsert_aggregates(
    tx: &Transaction<'_>,
    date: &str,
    seconds: i64,
    task: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO days(date, focus_sessions, focus_seconds)
         VALUES (?1, 1, ?2)
         ON CONFLICT(date) DO UPDATE SET
            focus_sessions = focus_sessions + 1,
            focus_seconds = focus_seconds + excluded.focus_seconds",
        params![date, seconds],
    )?;
    tx.execute(
        "INSERT INTO day_tasks(date, task, focus_sessions, focus_seconds)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(date, task) DO UPDATE SET
            focus_sessions = focus_sessions + 1,
            focus_seconds = focus_seconds + excluded.focus_seconds",
        params![date, task, seconds],
    )?;
    Ok(())
}

fn csv_field(value: &str) -> String {
    let mut value = value.to_string();
    let first = value.trim_start().chars().next();
    if matches!(first, Some('=' | '+' | '-' | '@')) {
        value.insert(0, '\'');
    }
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn read_totals(row: &Row<'_>) -> rusqlite::Result<(u32, u64)> {
    let sessions: i64 = row.get(0)?;
    let seconds: i64 = row.get(1)?;
    Ok((
        u32::try_from(sessions.max(0)).unwrap_or(u32::MAX),
        seconds.max(0) as u64,
    ))
}

fn read_day_at(row: &Row<'_>, offset: usize) -> rusqlite::Result<DayStats> {
    let sessions: i64 = row.get(offset)?;
    let seconds: i64 = row.get(offset + 1)?;
    Ok(DayStats {
        focus_sessions: u32::try_from(sessions.max(0)).unwrap_or(u32::MAX),
        focus_seconds: seconds.max(0) as u64,
    })
}

fn date_string(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn or_log<T: Default>(result: rusqlite::Result<T>, context: &str) -> T {
    match result {
        Ok(value) => value,
        Err(err) => {
            stats_log!("{context}: {err}");
            T::default()
        }
    }
}

fn level_for(seconds: u64) -> i32 {
    let minutes = seconds / 60;
    if minutes == 0 {
        0
    } else if minutes < 25 {
        1
    } else if minutes < 50 {
        2
    } else if minutes < 100 {
        3
    } else {
        4
    }
}

fn minutes_from_secs(seconds: u64) -> u32 {
    (seconds / 60).min(u32::MAX as u64) as u32
}

fn shift_months(date: NaiveDate, delta: i64) -> NaiveDate {
    let total = date.year() as i64 * 12 + date.month0() as i64 + delta;
    NaiveDate::from_ymd_opt(
        total.div_euclid(12) as i32,
        total.rem_euclid(12) as u32 + 1,
        1,
    )
    .unwrap_or(date)
}
