use chrono::Local;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

static LOG_LOCK: Mutex<()> = Mutex::new(());

pub fn info(context: &str, message: &str) {
    write("INFO", context, message);
}

pub fn error(context: &str, message: &str) {
    write("ERROR", context, message);
}

fn write(level: &str, context: &str, message: &str) {
    let dir = crate::paths::data_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("gomo.log");

    if fs::metadata(&path)
        .map(|meta| meta.len() > 1_000_000)
        .unwrap_or(false)
    {
        let rotated = dir.join("gomo.old.log");
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, &rotated);
    }

    let _guard = LOG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            file,
            "{} [{}] {}: {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            level,
            context,
            message
        );
    }
}
