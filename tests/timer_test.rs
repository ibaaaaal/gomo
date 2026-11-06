#[path = "../src/timer.rs"]
mod timer;

use std::time::Duration;
use timer::{PomodoroEngine, TimerConfig, TimerMode, TimerState};

#[test]
fn test_initial_state() {
    let config = TimerConfig::default();
    let engine = PomodoroEngine::new(config);

    assert_eq!(engine.mode, TimerMode::Focus);
    assert_eq!(engine.state, TimerState::Idle);
    assert_eq!(engine.remaining, Duration::from_secs(25 * 60));
    assert_eq!(engine.formatted_time(), "25:00");
    assert_eq!(engine.progress_fraction(), 1.0);
    assert_eq!(engine.cycle_focus_count, 0);
}

#[test]
fn test_custom_durations() {
    let custom_config = TimerConfig {
        focus_duration: Duration::from_secs(50 * 60),
        short_break_duration: Duration::from_secs(10 * 60),
        long_break_duration: Duration::from_secs(30 * 60),
        long_break_interval: 2,
        auto_start_breaks: false,
        auto_start_focus: false,
    };

    let mut engine = PomodoroEngine::new(custom_config);
    assert_eq!(engine.remaining, Duration::from_secs(50 * 60));
    assert_eq!(engine.formatted_time(), "50:00");

    // Switch to short break
    engine.set_mode(TimerMode::ShortBreak);
    assert_eq!(engine.remaining, Duration::from_secs(10 * 60));
    assert_eq!(engine.formatted_time(), "10:00");

    // Switch to long break
    engine.set_mode(TimerMode::LongBreak);
    assert_eq!(engine.remaining, Duration::from_secs(30 * 60));
    assert_eq!(engine.formatted_time(), "30:00");
}

#[test]
fn test_cycle_progression() {
    let config = TimerConfig {
        long_break_interval: 4,
        ..Default::default()
    };
    let mut engine = PomodoroEngine::new(config);

    // 1st Focus -> Short Break
    assert_eq!(engine.mode, TimerMode::Focus);
    let next = engine.transition_next(true);
    assert_eq!(next, TimerMode::ShortBreak);
    assert_eq!(engine.completed_sessions, 1);
    assert_eq!(engine.cycle_focus_count, 1);

    // Short Break -> 2nd Focus
    assert_eq!(engine.transition_next(true), TimerMode::Focus);

    // 2nd Focus -> Short Break
    assert_eq!(engine.transition_next(true), TimerMode::ShortBreak);
    assert_eq!(engine.completed_sessions, 2);
    assert_eq!(engine.cycle_focus_count, 2);

    // Short Break -> 3rd Focus
    assert_eq!(engine.transition_next(true), TimerMode::Focus);

    // 3rd Focus -> Short Break
    assert_eq!(engine.transition_next(true), TimerMode::ShortBreak);
    assert_eq!(engine.completed_sessions, 3);
    assert_eq!(engine.cycle_focus_count, 3);

    // Short Break -> 4th Focus
    assert_eq!(engine.transition_next(true), TimerMode::Focus);

    // 4th Focus -> LONG BREAK! The cycle is cleared as soon as the long break is earned.
    assert_eq!(engine.transition_next(true), TimerMode::LongBreak);
    assert_eq!(engine.completed_sessions, 4);
    assert_eq!(engine.cycle_focus_count, 0);

    // Long Break -> Focus, cycle stays reset
    assert_eq!(engine.transition_next(true), TimerMode::Focus);
    assert_eq!(engine.cycle_focus_count, 0);
}

#[test]
fn test_zero_long_break_interval_falls_back_to_one() {
    let config = TimerConfig {
        long_break_interval: 0,
        ..Default::default()
    };
    let mut engine = PomodoroEngine::new(config);

    assert_eq!(engine.transition_next(true), TimerMode::LongBreak);
}

#[test]
fn test_pause_and_resume() {
    let config = TimerConfig::default();
    let mut engine = PomodoroEngine::new(config);

    engine.start();
    assert_eq!(engine.state, TimerState::Running);

    assert!(!engine.pause());
    assert_eq!(engine.state, TimerState::Paused);

    assert!(!engine.toggle()); // Should resume
    assert_eq!(engine.state, TimerState::Running);

    assert!(!engine.toggle()); // Should pause
    assert_eq!(engine.state, TimerState::Paused);
}

#[test]
fn test_pause_at_zero_completes_instead_of_pausing() {
    let mut engine = PomodoroEngine::new(TimerConfig::default());
    engine.start();
    engine.remaining = Duration::ZERO;

    assert!(engine.pause());
    assert_eq!(engine.state, TimerState::Completed);
}

#[test]
fn test_toggle_at_zero_completes() {
    let mut engine = PomodoroEngine::new(TimerConfig::default());
    engine.start();
    engine.remaining = Duration::ZERO;

    assert!(engine.toggle());
    assert_eq!(engine.state, TimerState::Completed);
}

#[test]
fn test_formatted_time_rounds_up() {
    let mut engine = PomodoroEngine::new(TimerConfig::default());

    engine.remaining = Duration::from_millis(1_499_900);
    assert_eq!(engine.formatted_time(), "25:00");

    engine.remaining = Duration::from_millis(59_100);
    assert_eq!(engine.formatted_time(), "01:00");

    engine.remaining = Duration::ZERO;
    assert_eq!(engine.formatted_time(), "00:00");
}

#[test]
fn test_skipping_focus_does_not_earn_long_break() {
    let config = TimerConfig {
        long_break_interval: 4,
        ..Default::default()
    };
    let mut engine = PomodoroEngine::new(config);

    assert_eq!(engine.skip(), TimerMode::ShortBreak);
    assert_eq!(engine.completed_sessions, 0);
    assert_eq!(engine.cycle_focus_count, 0);
}

#[test]
fn test_skipping_long_break_resets_cycle() {
    let mut engine = PomodoroEngine::new(TimerConfig::default());
    engine.cycle_focus_count = 4;
    engine.set_mode(TimerMode::LongBreak);

    assert_eq!(engine.skip(), TimerMode::Focus);
    assert_eq!(engine.cycle_focus_count, 0);
}

#[test]
fn test_config_changes_apply_to_next_session_when_running() {
    let mut engine = PomodoroEngine::new(TimerConfig::default());
    engine.start();
    engine.remaining = Duration::from_secs(10 * 60);
    let old_total = engine.total;

    let config = TimerConfig {
        focus_duration: Duration::from_secs(50 * 60),
        ..Default::default()
    };
    engine.update_config(config);

    assert_eq!(engine.remaining, Duration::from_secs(10 * 60));
    assert_eq!(engine.total, old_total);

    engine.reset_current_mode();
    assert_eq!(engine.total, Duration::from_secs(50 * 60));
    assert_eq!(engine.remaining, Duration::from_secs(50 * 60));
}

#[test]
fn test_config_changes_apply_immediately_when_idle() {
    let mut engine = PomodoroEngine::new(TimerConfig::default());
    let config = TimerConfig {
        focus_duration: Duration::from_secs(50 * 60),
        ..Default::default()
    };

    engine.update_config(config);

    assert_eq!(engine.total, Duration::from_secs(50 * 60));
    assert_eq!(engine.remaining, Duration::from_secs(50 * 60));
}

#[test]
fn test_auto_start_focus() {
    let config = TimerConfig {
        auto_start_focus: true,
        ..Default::default()
    };
    let mut engine = PomodoroEngine::new(config);
    engine.set_mode(TimerMode::ShortBreak);

    assert_eq!(engine.transition_next(true), TimerMode::Focus);
    assert_eq!(engine.state, TimerState::Running);
}

#[test]
fn test_auto_start_breaks() {
    let config = TimerConfig {
        auto_start_breaks: true,
        ..Default::default()
    };
    let mut engine = PomodoroEngine::new(config);

    assert_eq!(engine.transition_next(true), TimerMode::ShortBreak);
    assert_eq!(engine.state, TimerState::Running);
}
