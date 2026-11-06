use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "Gomo";
const LEGACY_APP_DIR: &str = "Podomoro";
const CONFIG_FILES: [&str; 2] = ["config.json", "config.json.bak"];
const STATS_FILES: [&str; 7] = [
    "stats.db",
    "stats.db-wal",
    "stats.db-shm",
    "stats.db.bak",
    "stats.json",
    "stats.json.bak",
    "stats.imported.json",
];
const EXTRA_FILES: [&str; 1] = ["alarm.mp3"];

pub fn data_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        PathBuf::from(local).join(APP_DIR)
    } else if let Some(roaming) = std::env::var_os("APPDATA") {
        PathBuf::from(roaming).join(APP_DIR)
    } else {
        PathBuf::from(APP_DIR)
    }
}

fn legacy_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(LEGACY_APP_DIR));
    }
    if let Some(roaming) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(roaming).join(LEGACY_APP_DIR));
    }
    dirs
}

fn has_any(dir: &Path, files: &[&str]) -> bool {
    files.iter().any(|name| dir.join(name).is_file())
}

/// Copies legacy Podomoro data into the current folder once, never overwriting.
/// Safe to run again: only files that are still missing get copied.
pub fn migrate_legacy_data() -> io::Result<Option<PathBuf>> {
    let target = data_dir();
    let target_has_config = has_any(&target, &CONFIG_FILES);
    let target_has_stats = has_any(&target, &STATS_FILES);

    for legacy in legacy_data_dirs() {
        if legacy == target || !legacy.is_dir() {
            continue;
        }

        let need_config = !target_has_config && has_any(&legacy, &CONFIG_FILES);
        let need_stats = !target_has_stats && has_any(&legacy, &STATS_FILES);
        if !need_config && !need_stats {
            continue;
        }

        fs::create_dir_all(&target)?;
        let mut copied = false;
        if need_config {
            copied |= copy_if_missing(&legacy, &target, &CONFIG_FILES)?;
        }
        if need_stats {
            copied |= copy_if_missing(&legacy, &target, &STATS_FILES)?;
        }
        copied |= copy_if_missing(&legacy, &target, &EXTRA_FILES)?;
        if copied {
            return Ok(Some(legacy));
        }
    }
    Ok(None)
}

fn copy_if_missing(source: &Path, target: &Path, files: &[&str]) -> io::Result<bool> {
    let mut copied = false;
    for name in files {
        let src = source.join(name);
        let dst = target.join(name);
        if src.is_file() && !dst.exists() {
            fs::copy(&src, &dst)?;
            copied = true;
        }
    }
    Ok(copied)
}
