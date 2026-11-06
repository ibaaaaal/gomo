#[path = "../src/paths.rs"]
#[allow(dead_code)]
mod paths;

#[path = "../src/state.rs"]
#[allow(dead_code)]
mod state;

use state::{AppState, DEFAULT_TASK, MAX_TASKS};

#[test]
fn missing_settings_fields_keep_tasks_and_use_defaults() {
    let json = r#"{
        "daily_goal_minutes": 200,
        "tasks": ["General", "Belajar Jepang"],
        "active_task": "Belajar Jepang"
    }"#;

    let state: AppState = serde_json::from_str(json).unwrap();

    assert_eq!(state.focus_duration_mins, 25);
    assert_eq!(state.short_break_duration_mins, 5);
    assert_eq!(state.long_break_duration_mins, 15);
    assert_eq!(state.long_break_interval, 4);
    assert!(state.sound_enabled);
    assert_eq!(state.sound_volume, 100);
    assert_eq!(state.daily_goal_minutes, 200);
    assert_eq!(state.weekly_goal_minutes, 500);
    assert!(!state.auto_start_breaks);
    assert!(!state.auto_start_focus);
    assert_eq!(state.completed_sessions, 0);
    assert_eq!(state.cycle_focus_count, 0);
    assert_eq!(state.tasks, vec!["General", "Belajar Jepang"]);
    assert_eq!(state.active_task, "Belajar Jepang");
}

#[test]
fn old_config_without_tasks_gets_defaults() {
    let json = r#"{
        "focus_duration_mins": 25,
        "short_break_duration_mins": 5,
        "long_break_duration_mins": 15,
        "long_break_interval": 4,
        "sound_enabled": true,
        "auto_start_breaks": false,
        "completed_sessions": 3
    }"#;

    let state: AppState = serde_json::from_str(json).unwrap();
    assert_eq!(state.tasks, vec![DEFAULT_TASK.to_string()]);
    assert_eq!(state.active_task, DEFAULT_TASK);
}

#[test]
fn sanitize_keeps_general_and_removes_duplicates() {
    let mut state = AppState {
        tasks: vec![
            "  Deep Work ".into(),
            "deep work".into(),
            "".into(),
            "Belajar Jepang".into(),
        ],
        active_task: "deep WORK".into(),
        ..AppState::default()
    };

    state.sanitize();

    assert_eq!(state.tasks, vec!["General", "Deep Work", "Belajar Jepang"]);
    assert_eq!(state.active_task, "Deep Work");
}

#[test]
fn sanitize_falls_back_to_general_for_unknown_active_task() {
    let mut state = AppState {
        tasks: vec!["General".into(), "Coding".into()],
        active_task: "Missing".into(),
        ..AppState::default()
    };

    state.sanitize();

    assert_eq!(state.active_task, DEFAULT_TASK);
}

#[test]
fn add_task_validates_and_selects() {
    let mut state = AppState::default();

    assert!(state.add_task("Deep Work"));
    assert_eq!(state.active_task, "Deep Work");
    assert!(!state.add_task("deep work"));
    assert!(!state.add_task("   "));
    assert!(!state.add_task(&"x".repeat(25)));

    while state.tasks.len() < MAX_TASKS {
        let next = format!("Task {}", state.tasks.len());
        assert!(state.add_task(&next));
    }
    assert_eq!(state.tasks.len(), MAX_TASKS);
    assert!(!state.add_task("Overflow"));
}

#[test]
fn delete_task_cannot_remove_general_and_resets_active() {
    let mut state = AppState::default();
    assert!(state.add_task("Deep Work"));
    assert_eq!(state.active_task, "Deep Work");

    assert!(!state.delete_task(DEFAULT_TASK));
    assert!(state.delete_task("deep work"));
    assert_eq!(state.tasks, vec![DEFAULT_TASK.to_string()]);
    assert_eq!(state.active_task, DEFAULT_TASK);
}

#[test]
fn select_task_is_case_insensitive_and_rejects_unknown() {
    let mut state = AppState::default();
    assert!(state.add_task("Belajar Jepang"));

    assert!(state.select_task("belajar jepang"));
    assert_eq!(state.active_task, "Belajar Jepang");
    assert!(!state.select_task("Coding"));
    assert_eq!(state.active_task, "Belajar Jepang");
}
