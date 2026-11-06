use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_TASK: &str = "General";
pub const MAX_TASKS: usize = 8;
pub const MAX_TASK_NAME_LEN: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default = "default_focus_duration")]
    pub focus_duration_mins: u32,
    #[serde(default = "default_short_break")]
    pub short_break_duration_mins: u32,
    #[serde(default = "default_long_break")]
    pub long_break_duration_mins: u32,
    #[serde(default = "default_long_break_interval")]
    pub long_break_interval: u32,
    #[serde(default = "default_true")]
    pub sound_enabled: bool,
    #[serde(default = "default_sound_volume")]
    pub sound_volume: u8,
    #[serde(default = "default_daily_goal")]
    pub daily_goal_minutes: u32,
    #[serde(default = "default_weekly_goal")]
    pub weekly_goal_minutes: u32,
    #[serde(default)]
    pub auto_start_breaks: bool,
    #[serde(default)]
    pub auto_start_focus: bool,
    #[serde(default)]
    pub completed_sessions: u32,
    #[serde(default)]
    pub cycle_focus_count: u32,
    #[serde(default = "default_tasks")]
    pub tasks: Vec<String>,
    #[serde(default = "default_active_task")]
    pub active_task: String,
}

fn default_tasks() -> Vec<String> {
    vec![DEFAULT_TASK.to_string()]
}

fn default_active_task() -> String {
    DEFAULT_TASK.to_string()
}

fn default_sound_volume() -> u8 {
    100
}

fn default_true() -> bool {
    true
}

fn default_focus_duration() -> u32 {
    25
}

fn default_short_break() -> u32 {
    5
}

fn default_long_break() -> u32 {
    15
}

fn default_long_break_interval() -> u32 {
    4
}

fn default_daily_goal() -> u32 {
    100
}

fn default_weekly_goal() -> u32 {
    500
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            focus_duration_mins: 25,
            short_break_duration_mins: 5,
            long_break_duration_mins: 15,
            long_break_interval: 4,
            sound_enabled: true,
            sound_volume: default_sound_volume(),
            daily_goal_minutes: default_daily_goal(),
            weekly_goal_minutes: default_weekly_goal(),
            auto_start_breaks: false,
            auto_start_focus: false,
            completed_sessions: 0,
            cycle_focus_count: 0,
            tasks: default_tasks(),
            active_task: default_active_task(),
        }
    }
}

impl AppState {
    pub(crate) fn sanitize(&mut self) {
        self.focus_duration_mins = self.focus_duration_mins.clamp(1, 120);
        self.short_break_duration_mins = self.short_break_duration_mins.clamp(1, 30);
        self.long_break_duration_mins = self.long_break_duration_mins.clamp(1, 60);
        self.long_break_interval = self.long_break_interval.clamp(1, 8);
        // an old config can carry a counter from a bigger interval
        self.cycle_focus_count = self.cycle_focus_count.min(self.long_break_interval);
        self.sound_volume = self.sound_volume.min(100);
        self.daily_goal_minutes = self.daily_goal_minutes.min(1440);
        self.weekly_goal_minutes = self.weekly_goal_minutes.min(10080);

        self.tasks = sanitize_task_list(&self.tasks);
        let active = self.active_task.trim();
        self.active_task = self
            .tasks
            .iter()
            .find(|task| task.eq_ignore_ascii_case(active))
            .cloned()
            .unwrap_or_else(|| DEFAULT_TASK.to_string());
    }

    pub fn add_task(&mut self, raw: &str) -> bool {
        let name = raw.trim();
        if name.is_empty()
            || name.chars().count() > MAX_TASK_NAME_LEN
            || self.tasks.len() >= MAX_TASKS
        {
            return false;
        }
        if self
            .tasks
            .iter()
            .any(|task| task.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.tasks.push(name.to_string());
        self.active_task = name.to_string();
        true
    }

    pub fn delete_task(&mut self, raw: &str) -> bool {
        let name = raw.trim();
        if name.is_empty() || name.eq_ignore_ascii_case(DEFAULT_TASK) {
            return false;
        }
        let Some(index) = self
            .tasks
            .iter()
            .position(|task| task.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        let removed = self.tasks.remove(index);
        if self.active_task.eq_ignore_ascii_case(&removed) {
            self.active_task = DEFAULT_TASK.to_string();
        }
        true
    }

    pub fn select_task(&mut self, raw: &str) -> bool {
        let name = raw.trim();
        let Some(task) = self
            .tasks
            .iter()
            .find(|task| task.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        self.active_task = task.clone();
        true
    }

    fn config_path() -> PathBuf {
        let dir = crate::paths::data_dir();
        let _ = fs::create_dir_all(&dir);
        dir.join("config.json")
    }

    fn read_from(path: &std::path::Path) -> Option<AppState> {
        let data = fs::read_to_string(path).ok()?;
        let mut state = serde_json::from_str::<AppState>(&data).ok()?;
        state.sanitize();
        Some(state)
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Some(state) = Self::read_from(&path) {
            return state;
        }
        let backup = path.with_extension("json.bak");
        if let Some(state) = Self::read_from(&backup) {
            preserve_corrupt_config(&path);
            if let Err(err) = fs::copy(&backup, &path) {
                log_config_error(&format!("failed to repair config from backup: {err}"));
            }
            return state;
        }
        Self::default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let tmp = path.with_extension("json.tmp");
        // write tmp, keep the previous file as .bak, then swap: a failed write
        // must never leave us without a config
        fs::write(&tmp, json)?;

        let previous = path.with_extension("json.old");
        let _ = fs::remove_file(&previous);
        if path.exists() {
            if let Err(err) = fs::copy(&path, path.with_extension("json.bak")) {
                log_config_error(&format!("failed to refresh backup: {err}"));
            }
            if let Err(err) = fs::rename(&path, &previous) {
                let _ = fs::remove_file(&tmp);
                return Err(err);
            }
        }

        if let Err(err) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            if previous.exists() {
                let _ = fs::rename(&previous, &path);
            }
            return Err(err);
        }
        let _ = fs::remove_file(&previous);
        Ok(())
    }
}

fn preserve_corrupt_config(path: &Path) {
    if !path.exists() {
        return;
    }
    let corrupt = unique_config_corrupt_path(path);
    if let Err(err) = fs::copy(path, &corrupt) {
        log_config_error(&format!("failed to preserve corrupt config: {err}"));
    }
}

#[cfg(not(test))]
fn log_config_error(message: &str) {
    eprintln!("[config] {message}");
    crate::logging::error("config", message);
}

#[cfg(test)]
fn log_config_error(message: &str) {
    eprintln!("[config] {message}");
}

fn unique_config_corrupt_path(path: &Path) -> PathBuf {
    let base = path.with_extension("json.corrupt");
    if !base.exists() {
        return base;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    for attempt in 0..1000 {
        let candidate = path.with_extension(format!("json.corrupt.{stamp}.{attempt}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

fn sanitize_task_list(tasks: &[String]) -> Vec<String> {
    let mut result = vec![DEFAULT_TASK.to_string()];
    for task in tasks {
        let name = task.trim();
        if name.is_empty() || name.chars().count() > MAX_TASK_NAME_LEN {
            continue;
        }
        if result
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            continue;
        }
        result.push(name.to_string());
        if result.len() >= MAX_TASKS {
            break;
        }
    }
    result
}
