#[path = "../src/paths.rs"]
#[allow(dead_code)]
mod paths;

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("gomo-paths-{}-{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn data_dir_is_gomo_and_legacy_podomoro_migrates_once() {
    let root = temp_root();
    let local = root.join("local");
    let roaming = root.join("roaming");
    fs::create_dir_all(&local).unwrap();
    fs::create_dir_all(&roaming).unwrap();

    std::env::set_var("LOCALAPPDATA", &local);
    std::env::set_var("APPDATA", &roaming);

    let target = local.join("Gomo");
    assert_eq!(paths::data_dir(), target);

    let legacy_local = local.join("Podomoro");
    fs::create_dir_all(&legacy_local).unwrap();
    fs::write(legacy_local.join("config.json"), "legacy-config").unwrap();
    fs::write(legacy_local.join("stats.db"), "legacy-stats").unwrap();
    fs::write(legacy_local.join("podomoro.log"), "old-log").unwrap();

    let roaming_legacy = roaming.join("Podomoro");
    fs::create_dir_all(&roaming_legacy).unwrap();
    fs::write(roaming_legacy.join("config.json"), "roaming-config").unwrap();

    let migrated = paths::migrate_legacy_data().unwrap();
    assert_eq!(migrated.as_deref(), Some(legacy_local.as_path()));
    assert_eq!(
        fs::read_to_string(target.join("config.json")).unwrap(),
        "legacy-config"
    );
    assert_eq!(
        fs::read_to_string(target.join("stats.db")).unwrap(),
        "legacy-stats"
    );
    assert!(target.join("stats.db").is_file());
    assert!(!target.join("podomoro.log").exists());
    assert!(legacy_local.join("config.json").is_file());
    assert!(roaming_legacy.join("config.json").is_file());

    fs::write(target.join("config.json"), "gomo-config").unwrap();
    assert_eq!(paths::migrate_legacy_data().unwrap(), None);
    assert_eq!(
        fs::read_to_string(target.join("config.json")).unwrap(),
        "gomo-config"
    );

    fs::remove_dir_all(&target).unwrap();
    fs::remove_dir_all(&legacy_local).unwrap();
    let migrated = paths::migrate_legacy_data().unwrap();
    assert_eq!(migrated.as_deref(), Some(roaming_legacy.as_path()));
    assert_eq!(
        fs::read_to_string(target.join("config.json")).unwrap(),
        "roaming-config"
    );

    fs::remove_dir_all(&target).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("config.json"), "gomo-config").unwrap();
    fs::create_dir_all(&legacy_local).unwrap();
    fs::write(legacy_local.join("config.json"), "legacy-config").unwrap();
    fs::write(legacy_local.join("stats.db"), "legacy-stats").unwrap();

    let migrated = paths::migrate_legacy_data().unwrap();
    assert_eq!(migrated.as_deref(), Some(legacy_local.as_path()));
    assert_eq!(
        fs::read_to_string(target.join("config.json")).unwrap(),
        "gomo-config"
    );
    assert_eq!(
        fs::read_to_string(target.join("stats.db")).unwrap(),
        "legacy-stats"
    );

    std::env::remove_var("LOCALAPPDATA");
    std::env::remove_var("APPDATA");
    let _ = fs::remove_dir_all(&root);
}
