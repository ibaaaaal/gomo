#[path = "../src/timer.rs"]
#[allow(dead_code)]
mod timer;

use std::time::Duration;
use timer::{PomodoroEngine, TimerConfig, TimerMode, TimerState};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    fn pick(&mut self, max: u64) -> u64 {
        self.next() % max
    }

    fn flag(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

fn config(duration_secs: u64, interval: u32, auto_breaks: bool, auto_focus: bool) -> TimerConfig {
    TimerConfig {
        focus_duration: Duration::from_secs(duration_secs),
        short_break_duration: Duration::from_secs(duration_secs),
        long_break_duration: Duration::from_secs(duration_secs),
        long_break_interval: interval,
        auto_start_breaks: auto_breaks,
        auto_start_focus: auto_focus,
    }
}

fn duration_for(mode: TimerMode, config: &TimerConfig) -> Duration {
    match mode {
        TimerMode::Focus => config.focus_duration,
        TimerMode::ShortBreak => config.short_break_duration,
        TimerMode::LongBreak => config.long_break_duration,
    }
}

fn assert_engine_sane(engine: &PomodoroEngine) {
    assert!(
        engine.remaining <= engine.total,
        "remaining {:?} > total {:?} in {:?}/{:?}",
        engine.remaining,
        engine.total,
        engine.mode,
        engine.state
    );
    let fraction = engine.progress_fraction();
    assert!(
        (0.0..=1.0).contains(&fraction),
        "progress fraction out of range: {fraction}"
    );
    let display = engine.formatted_time();
    assert!(display.len() >= 5, "unexpected time display: {display}");
    assert_eq!(
        &display[display.len() - 3..display.len() - 2],
        ":",
        "unexpected time display: {display}"
    );
}

fn assert_mode_settings(engine: &PomodoroEngine) {
    assert_eq!(engine.total, duration_for(engine.mode, &engine.config));
    assert_eq!(engine.remaining, engine.total);
    assert_eq!(engine.state, TimerState::Idle);
}

#[test]
fn random_state_machine_sequences_keep_invariants() {
    for seed in 0..64 {
        let mut rng = Rng::new(seed);
        let cfg = config(3600, 1 + rng.pick(8) as u32, rng.flag(), rng.flag());
        let mut engine = PomodoroEngine::new(cfg);

        for _ in 0..2000 {
            match rng.pick(8) {
                0 => engine.start(),
                1 => {
                    let completed = engine.pause();
                    if completed {
                        assert_eq!(engine.state, TimerState::Completed);
                        engine.transition_next(true);
                    }
                }
                2 => engine.reset_current_mode(),
                3 => {
                    let before_mode = engine.mode;
                    let before = (engine.completed_sessions, engine.cycle_focus_count);
                    let mode = engine.skip();
                    let expected = match before_mode {
                        TimerMode::Focus => TimerMode::ShortBreak,
                        TimerMode::ShortBreak | TimerMode::LongBreak => TimerMode::Focus,
                    };
                    assert_eq!(mode, expected);
                    assert_eq!(engine.completed_sessions, before.0);
                    if before_mode == TimerMode::LongBreak {
                        assert_eq!(engine.cycle_focus_count, 0);
                    } else {
                        assert_eq!(engine.cycle_focus_count, before.1);
                    }
                    let auto = match mode {
                        TimerMode::Focus => engine.config.auto_start_focus,
                        TimerMode::ShortBreak | TimerMode::LongBreak => {
                            engine.config.auto_start_breaks
                        }
                    };
                    assert_eq!(
                        engine.state,
                        if auto {
                            TimerState::Running
                        } else {
                            TimerState::Idle
                        }
                    );
                }
                4 => {
                    let mode = match rng.pick(3) {
                        0 => TimerMode::Focus,
                        1 => TimerMode::ShortBreak,
                        _ => TimerMode::LongBreak,
                    };
                    engine.set_mode(mode);
                    assert_mode_settings(&engine);
                }
                5 => {
                    let new_cfg = config(3600, 1 + rng.pick(8) as u32, rng.flag(), rng.flag());
                    engine.update_config(new_cfg);
                    if engine.state == TimerState::Idle {
                        assert_mode_settings(&engine);
                    }
                }
                6 => {
                    if engine.state != TimerState::Running {
                        engine.start();
                    }
                    let before_mode = engine.mode;
                    let before = (engine.completed_sessions, engine.cycle_focus_count);
                    engine.remaining = Duration::ZERO;
                    assert!(engine.tick());
                    assert_eq!(engine.state, TimerState::Completed);

                    let next = engine.transition_next(true);
                    match before_mode {
                        TimerMode::Focus => {
                            assert_eq!(engine.completed_sessions, before.0 + 1);
                            let expected = if before.1 + 1 >= engine.config.long_break_interval {
                                TimerMode::LongBreak
                            } else {
                                TimerMode::ShortBreak
                            };
                            assert_eq!(next, expected);
                            assert_eq!(
                                engine.cycle_focus_count,
                                if expected == TimerMode::LongBreak {
                                    0
                                } else {
                                    before.1 + 1
                                }
                            );
                        }
                        TimerMode::ShortBreak => {
                            assert_eq!(engine.completed_sessions, before.0);
                            assert_eq!(engine.cycle_focus_count, before.1);
                            assert_eq!(next, TimerMode::Focus);
                        }
                        TimerMode::LongBreak => {
                            assert_eq!(engine.completed_sessions, before.0);
                            assert_eq!(engine.cycle_focus_count, 0);
                            assert_eq!(next, TimerMode::Focus);
                        }
                    }
                }
                _ => {
                    let _ = engine.toggle();
                }
            }
            assert_engine_sane(&engine);
        }
    }
}

#[test]
fn natural_focus_completion_counts_once_and_picks_long_break() {
    for interval in 1..=8 {
        for cycle_before in 0..interval {
            let mut engine = PomodoroEngine::new(config(60, interval, false, false));
            engine.cycle_focus_count = cycle_before;
            engine.completed_sessions = 10;
            engine.start();
            engine.remaining = Duration::ZERO;
            assert!(engine.tick());
            assert_eq!(engine.state, TimerState::Completed);

            let next = engine.transition_next(true);
            assert_eq!(engine.completed_sessions, 11);
            let expected = if cycle_before + 1 >= interval {
                TimerMode::LongBreak
            } else {
                TimerMode::ShortBreak
            };
            assert_eq!(next, expected);
            assert_eq!(
                engine.cycle_focus_count,
                if expected == TimerMode::LongBreak {
                    0
                } else {
                    cycle_before + 1
                }
            );
            assert_mode_settings(&engine);
        }
    }
}

#[test]
fn stale_cycle_at_or_above_interval_is_cleared_when_long_break_is_earned() {
    for cycle_before in [8, 9, 11] {
        let mut engine = PomodoroEngine::new(config(60, 8, false, false));
        engine.cycle_focus_count = cycle_before;

        engine.start();
        engine.remaining = Duration::ZERO;
        assert!(engine.tick());
        assert_eq!(engine.transition_next(true), TimerMode::LongBreak);
        assert_eq!(engine.cycle_focus_count, 0);
    }
}

#[test]
fn leaving_an_earned_long_break_manually_does_not_poison_the_cycle() {
    let mut engine = PomodoroEngine::new(config(60, 2, false, false));
    engine.cycle_focus_count = 1;

    engine.start();
    engine.remaining = Duration::ZERO;
    assert!(engine.tick());
    assert_eq!(engine.transition_next(true), TimerMode::LongBreak);
    assert_eq!(engine.cycle_focus_count, 0);

    engine.set_mode(TimerMode::Focus);
    assert_eq!(engine.cycle_focus_count, 0);

    engine.start();
    engine.remaining = Duration::ZERO;
    assert!(engine.tick());
    assert_eq!(engine.transition_next(true), TimerMode::ShortBreak);
    assert_eq!(engine.cycle_focus_count, 1);
}

#[test]
fn lowering_the_interval_below_the_current_cycle_recovers_at_the_next_long_break() {
    let mut engine = PomodoroEngine::new(config(60, 8, false, false));
    engine.cycle_focus_count = 7;
    engine.update_config(config(60, 2, false, false));

    engine.start();
    engine.remaining = Duration::ZERO;
    assert!(engine.tick());
    assert_eq!(engine.transition_next(true), TimerMode::LongBreak);
    assert_eq!(engine.cycle_focus_count, 0);

    assert_eq!(engine.transition_next(false), TimerMode::Focus);
    assert_eq!(engine.cycle_focus_count, 0);

    engine.start();
    engine.remaining = Duration::ZERO;
    assert!(engine.tick());
    assert_eq!(engine.transition_next(true), TimerMode::ShortBreak);
    assert_eq!(engine.cycle_focus_count, 1);
}

#[test]
fn skip_rules_and_auto_start_are_consistent() {
    let mut engine = PomodoroEngine::new(config(60, 4, true, true));
    engine.cycle_focus_count = 3;
    engine.completed_sessions = 7;

    engine.start();
    let skipped = engine.skip();
    assert_eq!(skipped, TimerMode::ShortBreak);
    assert_eq!(engine.completed_sessions, 7);
    assert_eq!(engine.cycle_focus_count, 3);
    assert_eq!(engine.state, TimerState::Running);

    let skipped = engine.skip();
    assert_eq!(skipped, TimerMode::Focus);
    assert_eq!(engine.cycle_focus_count, 3);
    assert_eq!(engine.state, TimerState::Running);

    engine.set_mode(TimerMode::LongBreak);
    engine.start();
    let skipped = engine.skip();
    assert_eq!(skipped, TimerMode::Focus);
    assert_eq!(engine.cycle_focus_count, 0);
    assert_eq!(engine.state, TimerState::Running);
}

#[test]
fn pause_and_toggle_at_zero_complete_exactly_once() {
    let mut engine = PomodoroEngine::new(config(60, 4, false, false));
    engine.start();
    engine.remaining = Duration::ZERO;
    assert!(engine.pause());
    assert_eq!(engine.state, TimerState::Completed);
    assert!(!engine.pause());

    let mut engine = PomodoroEngine::new(config(60, 4, false, false));
    engine.start();
    engine.remaining = Duration::ZERO;
    assert!(engine.toggle());
    assert_eq!(engine.state, TimerState::Completed);
    assert!(!engine.toggle());
    assert_eq!(engine.state, TimerState::Running);
}

#[test]
fn zero_long_break_interval_still_picks_a_long_break() {
    let mut engine = PomodoroEngine::new(config(60, 0, false, false));
    engine.start();
    engine.remaining = Duration::ZERO;
    assert!(engine.tick());
    assert_eq!(engine.transition_next(true), TimerMode::LongBreak);
}

#[test]
fn config_updates_apply_at_the_documented_moment() {
    let mut engine = PomodoroEngine::new(config(60, 4, false, false));

    engine.update_config(config(120, 5, false, false));
    assert_mode_settings(&engine);

    engine.start();
    engine.remaining = Duration::from_secs(42);
    engine.update_config(config(300, 6, false, false));
    assert_eq!(engine.total, Duration::from_secs(120));
    assert_eq!(engine.remaining, Duration::from_secs(42));

    engine.state = TimerState::Completed;
    engine.remaining = Duration::ZERO;
    engine.update_config(config(300, 6, false, false));
    assert_eq!(engine.total, Duration::from_secs(300));
    assert_eq!(engine.remaining, Duration::ZERO);
}

#[test]
fn formatted_time_is_ceiling_and_never_goes_negative() {
    let mut engine = PomodoroEngine::new(config(1, 1, false, false));
    for (duration, expected) in [
        (Duration::ZERO, "00:00"),
        (Duration::from_nanos(1), "00:01"),
        (Duration::from_millis(59_900), "01:00"),
        (Duration::from_secs(60), "01:00"),
        (Duration::from_secs(59), "00:59"),
        (Duration::from_secs(3_600), "60:00"),
        (Duration::MAX, "307445734561825860:15"),
    ] {
        engine.remaining = duration;
        assert_eq!(engine.formatted_time(), expected, "duration {duration:?}");
    }
}
