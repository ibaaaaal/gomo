use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerMode {
    Focus,
    ShortBreak,
    LongBreak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    Idle,
    Running,
    Paused,
    Completed,
}

#[derive(Debug, Clone)]
pub struct TimerConfig {
    pub focus_duration: Duration,
    pub short_break_duration: Duration,
    pub long_break_duration: Duration,
    pub long_break_interval: u32,
    pub auto_start_breaks: bool,
    pub auto_start_focus: bool,
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            focus_duration: Duration::from_secs(25 * 60),
            short_break_duration: Duration::from_secs(5 * 60),
            long_break_duration: Duration::from_secs(15 * 60),
            long_break_interval: 4,
            auto_start_breaks: false,
            auto_start_focus: false,
        }
    }
}

pub struct PomodoroEngine {
    pub mode: TimerMode,
    pub state: TimerState,
    pub config: TimerConfig,
    pub remaining: Duration,
    pub total: Duration,
    pub completed_sessions: u32,
    pub cycle_focus_count: u32,
    last_tick: Option<Instant>,
}

impl PomodoroEngine {
    pub fn new(config: TimerConfig) -> Self {
        let total = config.focus_duration;
        Self {
            mode: TimerMode::Focus,
            state: TimerState::Idle,
            remaining: total,
            total,
            completed_sessions: 0,
            cycle_focus_count: 0,
            config,
            last_tick: None,
        }
    }

    pub fn start(&mut self) {
        if self.state == TimerState::Completed {
            self.reset_current_mode();
        }
        self.state = TimerState::Running;
        self.last_tick = Some(Instant::now());
    }

    /// True if this pause also finished the session.
    pub fn pause(&mut self) -> bool {
        if self.state != TimerState::Running {
            return false;
        }
        if self.tick() {
            return true;
        }
        self.state = TimerState::Paused;
        self.last_tick = None;
        false
    }

    /// Like pause(), but also starts the timer when it is not running. Returns
    /// true if the toggle ended up finishing the session.
    pub fn toggle(&mut self) -> bool {
        match self.state {
            TimerState::Running => self.pause(),
            TimerState::Idle | TimerState::Paused | TimerState::Completed => {
                self.start();
                false
            }
        }
    }

    pub fn reset_current_mode(&mut self) {
        self.state = TimerState::Idle;
        self.last_tick = None;
        self.total = self.duration_for_mode(self.mode);
        self.remaining = self.total;
    }

    pub fn set_mode(&mut self, mode: TimerMode) {
        self.mode = mode;
        self.reset_current_mode();
    }

    pub fn skip(&mut self) -> TimerMode {
        self.transition_next(false)
    }

    /// Advances by real elapsed time, so a late tick does not drift.
    /// Returns true if the session hit zero during this tick.
    pub fn tick(&mut self) -> bool {
        if self.state != TimerState::Running {
            return false;
        }

        let now = Instant::now();
        if let Some(last) = self.last_tick {
            let elapsed = now.saturating_duration_since(last);
            self.remaining = self.remaining.saturating_sub(elapsed);
        }
        self.last_tick = Some(now);

        if self.remaining.is_zero() {
            self.state = TimerState::Completed;
            self.last_tick = None;
            return true;
        }

        false
    }

    /// Moves to the next mode. A natural finish counts the session and can earn
    /// a long break; a skip does neither.
    pub fn transition_next(&mut self, is_natural_finish: bool) -> TimerMode {
        match self.mode {
            TimerMode::Focus => {
                if is_natural_finish {
                    self.completed_sessions = self.completed_sessions.saturating_add(1);
                    self.cycle_focus_count = self.cycle_focus_count.saturating_add(1);
                    let interval = self.config.long_break_interval.max(1);
                    if self.cycle_focus_count >= interval {
                        // reset when earned: leaving the long break by hand must not
                        // keep the counter above the interval
                        self.cycle_focus_count = 0;
                        self.mode = TimerMode::LongBreak;
                    } else {
                        self.mode = TimerMode::ShortBreak;
                    }
                } else {
                    // skipping a focus session never earns a long break
                    self.mode = TimerMode::ShortBreak;
                }
            }
            TimerMode::ShortBreak => {
                self.mode = TimerMode::Focus;
            }
            TimerMode::LongBreak => {
                self.mode = TimerMode::Focus;
                self.cycle_focus_count = 0;
            }
        }

        self.reset_current_mode();
        if self.should_auto_start() {
            self.start();
        }
        self.mode
    }

    /// New durations apply immediately when idle, and on the next session otherwise.
    pub fn update_config(&mut self, new_config: TimerConfig) {
        self.config = new_config;
        if self.state == TimerState::Idle {
            self.total = self.duration_for_mode(self.mode);
            self.remaining = self.total;
        } else if self.state == TimerState::Completed {
            self.total = self.duration_for_mode(self.mode);
        }
    }

    pub fn progress_fraction(&self) -> f32 {
        if self.total.is_zero() {
            return 0.0;
        }
        (self.remaining.as_secs_f32() / self.total.as_secs_f32()).clamp(0.0, 1.0)
    }

    pub fn formatted_time(&self) -> String {
        let total_secs = self
            .remaining
            .as_secs()
            .saturating_add(u64::from(self.remaining.subsec_nanos() > 0));
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{:02}:{:02}", mins, secs)
    }

    fn should_auto_start(&self) -> bool {
        match self.mode {
            TimerMode::Focus => self.config.auto_start_focus,
            TimerMode::ShortBreak | TimerMode::LongBreak => self.config.auto_start_breaks,
        }
    }

    fn duration_for_mode(&self, mode: TimerMode) -> Duration {
        match mode {
            TimerMode::Focus => self.config.focus_duration,
            TimerMode::ShortBreak => self.config.short_break_duration,
            TimerMode::LongBreak => self.config.long_break_duration,
        }
    }
}
