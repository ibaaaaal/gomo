#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod logging;
mod notify;
mod paths;
mod single_instance;
mod state;
mod stats;
mod timer;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDate};
use slint::{ModelRc, VecModel};
use state::{AppState, DEFAULT_TASK, MAX_TASKS, MAX_TASK_NAME_LEN};
use stats::{HeatmapDay, StatsStore, HEATMAP_WEEKS};
use timer::{PomodoroEngine, TimerConfig, TimerMode, TimerState};

slint::include_modules!();

#[cfg(target_os = "windows")]
fn set_app_user_model_id() {
    const APP_ID: &str = "Gomo.StudyTimer";

    #[link(name = "shell32")]
    extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
    }

    let app_id: Vec<u16> = APP_ID.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
fn set_app_user_model_id() {}

fn calculate_gauge(cx: f32, cy: f32, r: f32, fraction: f32) -> (String, f32, f32) {
    let f = fraction.clamp(0.005, 0.999);
    let angle = -std::f32::consts::FRAC_PI_2 + f * 2.0 * std::f32::consts::PI;
    let end_x = cx + r * angle.cos();
    let end_y = cy + r * angle.sin();
    let large_arc = if f > 0.5 { 1 } else { 0 };

    let arc = format!(
        "M {} {} A {} {} 0 {} 1 {} {}",
        cx,
        cy - r,
        r,
        r,
        large_arc,
        end_x,
        end_y
    );

    let knob_x = end_x - 7.0;
    let knob_y = end_y - 7.0;

    (arc, knob_x, knob_y)
}

fn state_to_config(state: &AppState) -> TimerConfig {
    TimerConfig {
        focus_duration: Duration::from_secs((state.focus_duration_mins as u64) * 60),
        short_break_duration: Duration::from_secs((state.short_break_duration_mins as u64) * 60),
        long_break_duration: Duration::from_secs((state.long_break_duration_mins as u64) * 60),
        long_break_interval: state.long_break_interval,
        auto_start_breaks: state.auto_start_breaks,
        auto_start_focus: state.auto_start_focus,
    }
}

fn sync_engine_to_ui(ui: &MainWindow, engine: &PomodoroEngine) {
    ui.set_time_remaining_secs(engine.remaining.as_secs() as i32);
    ui.set_total_duration_secs(engine.total.as_secs() as i32);
    ui.set_time_display(engine.formatted_time().into());
    ui.set_active_mode(match engine.mode {
        TimerMode::Focus => 0,
        TimerMode::ShortBreak => 1,
        TimerMode::LongBreak => 2,
    });
    ui.set_is_running(engine.state == TimerState::Running);

    let frac = engine.progress_fraction();
    ui.set_progress_fraction(frac);

    let (arc, kx, ky) = calculate_gauge(110.0, 110.0, 90.0, frac);
    ui.set_arc_path(arc.into());
    ui.set_knob_x(kx);
    ui.set_knob_y(ky);

    ui.set_cycle_focus_count(engine.cycle_focus_count as i32);
    ui.set_sessions_until_long_break(engine.config.long_break_interval as i32);
}

fn sync_tray(tray: Option<&TrayIcon>, engine: &PomodoroEngine) {
    let Some(tray) = tray else { return };
    let mode = match engine.mode {
        TimerMode::Focus => "Study",
        TimerMode::ShortBreak => "Rest",
        TimerMode::LongBreak => "Deep Rest",
    };
    tray.set_running(engine.state == TimerState::Running);
    tray.set_status_text(format!("{} · {}", mode, engine.formatted_time()).into());
}

fn sync_settings_to_ui(ui: &MainWindow, state: &AppState) {
    ui.set_focus_duration(state.focus_duration_mins as i32);
    ui.set_short_break_duration(state.short_break_duration_mins as i32);
    ui.set_long_break_duration(state.long_break_duration_mins as i32);
    ui.set_long_break_interval(state.long_break_interval as i32);
    ui.set_sound_enabled(state.sound_enabled);
    ui.set_sound_volume(state.sound_volume as i32);
    ui.set_auto_start_breaks(state.auto_start_breaks);
    ui.set_auto_start_focus(state.auto_start_focus);
}

fn plural_sessions(count: u32) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn short_date(date: NaiveDate) -> String {
    format!("{} {}", MONTHS[date.month0() as usize], date.day())
}

fn format_total_focus(seconds: u64) -> String {
    let total_mins = seconds / 60;
    let hours = total_mins / 60;
    let mins = total_mins % 60;
    if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{} min", mins)
    }
}

fn heatmap_label(day: &HeatmapDay) -> String {
    if day.level < 0 {
        return String::new();
    }
    let date = short_date(day.date);
    if day.sessions == 0 {
        format!("{} · no focus", date)
    } else {
        format!(
            "{} · {} min · {} session{}",
            date,
            day.minutes,
            day.sessions,
            plural_sessions(day.sessions)
        )
    }
}

fn sync_stats_to_ui(ui: &MainWindow, store: &StatsStore, range: i32) {
    let today = Local::now().date_naive();
    let summary = store.summary(today);
    let range = range.clamp(0, 2);

    let (title, range_label, series): (&str, &str, Vec<(NaiveDate, u64)>) = match range {
        0 => ("LAST 7 DAYS", "Last 7 days", store.daily_series(today, 7)),
        1 => (
            "LAST 30 DAYS",
            "Last 30 days",
            store.daily_series(today, 30),
        ),
        _ => (
            "LAST 12 MONTHS",
            "Last 12 months",
            store.monthly_series(today, 12),
        ),
    };
    let start = series.first().map(|(date, _)| *date).unwrap_or(today);
    let (range_sessions, range_seconds) = store.totals_between(start, today);

    ui.set_stat_range_title(title.into());
    ui.set_stat_range_value(format_total_focus(range_seconds).into());
    ui.set_stat_range_sub(
        format!(
            "{} session{}",
            range_sessions,
            plural_sessions(range_sessions)
        )
        .into(),
    );
    ui.set_stat_streak_value(
        format!(
            "{} day{}",
            summary.current_streak,
            if summary.current_streak == 1 { "" } else { "s" }
        )
        .into(),
    );
    ui.set_stat_streak_sub(
        if summary.current_streak == 0 {
            "Start a streak"
        } else {
            "Consecutive days"
        }
        .into(),
    );

    match store.best_day_between(start, today) {
        Some((date, day)) => {
            ui.set_stat_best_value(format!("{} min", day.focus_seconds / 60).into());
            ui.set_stat_best_sub(short_date(date).into());
        }
        None => {
            ui.set_stat_best_value("—".into());
            ui.set_stat_best_sub("No sessions".into());
        }
    }

    let max_seconds = series
        .iter()
        .map(|(_, seconds)| *seconds)
        .max()
        .unwrap_or(0);
    let bars: Vec<ChartBar> = series
        .iter()
        .enumerate()
        .map(|(index, (date, seconds))| {
            let axis = match range {
                0 => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                    [date.weekday().num_days_from_monday() as usize]
                    .to_string(),
                1 => {
                    if index % 5 == 0 || index + 1 == series.len() {
                        date.day().to_string()
                    } else {
                        String::new()
                    }
                }
                _ => MONTHS[date.month0() as usize].to_string(),
            };
            let height_px = if max_seconds == 0 {
                0
            } else {
                (*seconds * 78).div_ceil(max_seconds) as i32
            };
            ChartBar {
                axis: axis.into(),
                height_px,
                width_px: match range {
                    0 => 20,
                    1 => 6,
                    _ => 14,
                },
            }
        })
        .collect();
    ui.set_chart_range_label(range_label.into());
    ui.set_chart_bars(ModelRc::new(VecModel::from(bars)));

    let task_totals = store.task_totals_between(start, today);
    let max_task_seconds = task_totals
        .first()
        .map(|(_, _, seconds)| *seconds)
        .unwrap_or(0);
    let slices: Vec<TaskSlice> = task_totals
        .iter()
        .take(4)
        .map(|(name, _sessions, seconds)| TaskSlice {
            name: name.clone().into(),
            minutes: (seconds / 60).min(i32::MAX as u64) as i32,
            fraction: if max_task_seconds == 0 {
                0.0
            } else {
                *seconds as f32 / max_task_seconds as f32
            },
        })
        .collect();
    ui.set_task_slice_count(slices.len() as i32);
    ui.set_task_slices(ModelRc::new(VecModel::from(slices)));

    let days = store.heatmap(today, HEATMAP_WEEKS);
    let heatmap_start = days.first().map(|day| day.date).unwrap_or(today);
    ui.set_heatmap_range(format!("{} – {}", short_date(heatmap_start), short_date(today)).into());
    ui.set_heatmap_weeks(HEATMAP_WEEKS as i32);

    let cells: Vec<HeatCell> = days
        .iter()
        .enumerate()
        .map(|(index, day)| HeatCell {
            level: day.level,
            label: heatmap_label(day).into(),
            col: (index / 7) as i32,
            row: (index % 7) as i32,
        })
        .collect();
    ui.set_heatmap_cells(ModelRc::new(VecModel::from(cells)));
}

fn sync_goals_to_ui(ui: &MainWindow, state: &AppState, store: &StatsStore) {
    let today = Local::now().date_naive();
    let week_start = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let (_, today_secs) = store.totals_between(today, today);
    let (_, week_secs) = store.totals_between(week_start, today);
    let today_mins = today_secs / 60;
    let week_mins = week_secs / 60;
    let daily_goal = state.daily_goal_minutes as u64;
    let weekly_goal = state.weekly_goal_minutes as u64;

    ui.set_daily_goal(state.daily_goal_minutes as i32);
    ui.set_weekly_goal(state.weekly_goal_minutes as i32);
    ui.set_daily_goal_visible(state.daily_goal_minutes > 0);
    ui.set_daily_goal_text(format!("{} / {} min", today_mins, daily_goal).into());
    ui.set_daily_goal_fraction(if daily_goal == 0 {
        0.0
    } else {
        (today_mins as f32 / daily_goal as f32).min(1.0)
    });
    ui.set_daily_goal_progress(
        if daily_goal == 0 {
            format!("{} min today · goal off", today_mins)
        } else {
            format!("{} / {} min today", today_mins, daily_goal)
        }
        .into(),
    );
    ui.set_daily_goal_progress_fraction(if daily_goal == 0 {
        0.0
    } else {
        (today_mins as f32 / daily_goal as f32).min(1.0)
    });
    ui.set_weekly_goal_progress(
        if weekly_goal == 0 {
            format!("{} min this week · goal off", week_mins)
        } else {
            format!("{} / {} min this week", week_mins, weekly_goal)
        }
        .into(),
    );
    ui.set_weekly_goal_progress_fraction(if weekly_goal == 0 {
        0.0
    } else {
        (week_mins as f32 / weekly_goal as f32).min(1.0)
    });
}

fn sync_tasks_to_ui(ui: &MainWindow, state: &AppState) {
    ui.set_task_limit(MAX_TASKS as i32);
    ui.set_active_task(state.active_task.clone().into());
    let items: Vec<TaskItem> = state
        .tasks
        .iter()
        .map(|name| TaskItem {
            name: name.clone().into(),
            deletable: !name.eq_ignore_ascii_case(DEFAULT_TASK),
        })
        .collect();
    ui.set_task_list(ModelRc::new(VecModel::from(items)));
}

struct SessionMeta {
    started_at: chrono::DateTime<Local>,
    task: String,
}

fn new_focus_session(state: &AppState) -> SessionMeta {
    SessionMeta {
        started_at: Local::now(),
        task: state.active_task.clone(),
    }
}

fn ensure_focus_session(
    engine: &PomodoroEngine,
    state: &AppState,
    session: &Rc<RefCell<Option<SessionMeta>>>,
) {
    let needs_session = session.borrow().is_none();
    if needs_session && engine.state == TimerState::Running && engine.mode == TimerMode::Focus {
        *session.borrow_mut() = Some(new_focus_session(state));
    }
}

fn handle_completion(
    ui: &MainWindow,
    engine: &mut PomodoroEngine,
    app_state: &Rc<RefCell<AppState>>,
    stats_store: &Rc<RefCell<StatsStore>>,
    session: &Rc<RefCell<Option<SessionMeta>>>,
) {
    let finished_mode = engine.mode;
    let finished_secs = engine.total.as_secs();
    let was_hidden = !ui.window().is_visible();
    let finished_session = session.borrow_mut().take();
    let (sound, volume, active_task) = {
        let state = app_state.borrow();
        (
            state.sound_enabled,
            state.sound_volume,
            state.active_task.clone(),
        )
    };
    if sound {
        notify::play_alarm(volume);
    }

    let next_mode = engine.transition_next(true);
    let title = match next_mode {
        TimerMode::Focus => "Rest Finished",
        TimerMode::ShortBreak => "Study Session Complete!",
        TimerMode::LongBreak => "Cycle Complete!",
    };
    let desc = match next_mode {
        TimerMode::Focus => "Ready to jump back into study mode.",
        TimerMode::ShortBreak => "Great session! Time to take a breather.",
        TimerMode::LongBreak => "Amazing session! Take a deep restful break.",
    };
    if was_hidden {
        let _ = ui.show();
        ui.invoke_focus_shortcuts();
        single_instance::focus_existing();
    }
    ui.set_notice_text(title.into());
    ui.set_notice_visible(true);
    notify::notify_session_complete(title, desc);

    let ended_at = Local::now();
    let fallback_start =
        ended_at - chrono::Duration::seconds(finished_secs.min(i64::MAX as u64) as i64);
    if finished_mode == TimerMode::Focus {
        let (started_at, task) = match finished_session {
            Some(meta) => (meta.started_at, meta.task),
            None => (fallback_start, active_task),
        };
        let mut stats = stats_store.borrow_mut();
        if let Err(err) = stats.record_focus_session(started_at, ended_at, finished_secs, &task) {
            eprintln!("[stats] failed to record focus session: {err}");
            logging::error("stats", &format!("failed to record focus session: {err}"));
        }
        sync_stats_to_ui(ui, &stats, ui.get_stats_range());
    } else {
        let mode = match finished_mode {
            TimerMode::ShortBreak => "short_break",
            TimerMode::LongBreak => "long_break",
            TimerMode::Focus => unreachable!(),
        };
        let mut stats = stats_store.borrow_mut();
        if let Err(err) = stats.record_session(fallback_start, ended_at, mode, "", finished_secs) {
            eprintln!("[stats] failed to record break session: {err}");
            logging::error("stats", &format!("failed to record break session: {err}"));
        }
    }

    if engine.state == TimerState::Running && engine.mode == TimerMode::Focus {
        *session.borrow_mut() = Some(new_focus_session(&app_state.borrow()));
    }

    sync_goals_to_ui(ui, &app_state.borrow(), &stats_store.borrow());

    let mut state = app_state.borrow_mut();
    state.completed_sessions = engine.completed_sessions;
    state.cycle_focus_count = engine.cycle_focus_count;
    save_state(ui, &state);
}

fn toggle_timer(
    ui: &MainWindow,
    engine: &mut PomodoroEngine,
    app_state: &Rc<RefCell<AppState>>,
    stats_store: &Rc<RefCell<StatsStore>>,
    session: &Rc<RefCell<Option<SessionMeta>>>,
) {
    ui.set_notice_visible(false);
    notify::stop_alarm();
    if engine.toggle() {
        handle_completion(ui, engine, app_state, stats_store, session);
    } else {
        ensure_focus_session(engine, &app_state.borrow(), session);
    }
    sync_engine_to_ui(ui, engine);
}

fn reset_timer(
    ui: &MainWindow,
    engine: &mut PomodoroEngine,
    session: &Rc<RefCell<Option<SessionMeta>>>,
) {
    ui.set_notice_visible(false);
    notify::stop_alarm();
    engine.reset_current_mode();
    *session.borrow_mut() = None;
    sync_engine_to_ui(ui, engine);
}

fn skip_timer(
    ui: &MainWindow,
    engine: &mut PomodoroEngine,
    app_state: &Rc<RefCell<AppState>>,
    session: &Rc<RefCell<Option<SessionMeta>>>,
) {
    ui.set_notice_visible(false);
    notify::stop_alarm();
    engine.skip();
    *session.borrow_mut() = None;
    ensure_focus_session(engine, &app_state.borrow(), session);

    let mut state = app_state.borrow_mut();
    state.completed_sessions = engine.completed_sessions;
    state.cycle_focus_count = engine.cycle_focus_count;
    save_state(ui, &state);
    sync_engine_to_ui(ui, engine);
}

fn select_mode(
    ui: &MainWindow,
    engine: &mut PomodoroEngine,
    mode: TimerMode,
    session: &Rc<RefCell<Option<SessionMeta>>>,
) {
    ui.set_notice_visible(false);
    notify::stop_alarm();
    engine.set_mode(mode);
    *session.borrow_mut() = None;
    sync_engine_to_ui(ui, engine);
}

fn save_state(ui: &MainWindow, state: &AppState) {
    if let Err(err) = state.save() {
        logging::error("config", &format!("failed to save config: {err}"));
        ui.set_notice_text("Could not save data · check disk".into());
        ui.set_notice_visible(true);
    }
}

fn report_data_result(ui: &MainWindow, result: Result<(), String>, prefix: &str, path: &Path) {
    match result {
        Ok(()) => {
            let message = format!("{}: {}", prefix, path.display());
            ui.set_data_status(message.clone().into());
            logging::info("data", &message);
        }
        Err(err) => {
            let message = format!("Failed: {err}");
            ui.set_data_status(message.clone().into());
            logging::error("data", &message);
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    set_app_user_model_id();

    if !single_instance::acquire() {
        return Ok(());
    }

    match paths::migrate_legacy_data() {
        Ok(Some(source)) => logging::info(
            "migration",
            &format!("copied legacy Podomoro data from {}", source.display()),
        ),
        Ok(None) => {}
        Err(err) => logging::error("migration", &err.to_string()),
    }

    let state = AppState::load();
    let config = state_to_config(&state);

    let mut engine = PomodoroEngine::new(config);
    engine.completed_sessions = state.completed_sessions;
    engine.cycle_focus_count = state.cycle_focus_count;
    let engine = Rc::new(RefCell::new(engine));
    let app_state = Rc::new(RefCell::new(state));
    let stats_store = Rc::new(RefCell::new(StatsStore::load()));
    let session_meta = Rc::new(RefCell::new(None::<SessionMeta>));

    let storage_persistent = stats_store.borrow().is_persistent();
    let main_window = MainWindow::new()?;
    let tray = match TrayIcon::new() {
        Ok(tray) => Some(Rc::new(tray)),
        Err(err) => {
            logging::error("tray", &format!("system tray unavailable: {err}"));
            main_window.set_notice_text("Tray unavailable · closing will exit".into());
            main_window.set_notice_visible(true);
            None
        }
    };

    if !storage_persistent {
        logging::error(
            "stats",
            "statistics storage unavailable; using in-memory data",
        );
        main_window.set_notice_text("Stats temporary · data folder not writable".into());
        main_window.set_notice_visible(true);
    }

    // initial paint
    sync_settings_to_ui(&main_window, &app_state.borrow());
    sync_engine_to_ui(&main_window, &engine.borrow());
    sync_stats_to_ui(&main_window, &stats_store.borrow(), 0);
    sync_goals_to_ui(&main_window, &app_state.borrow(), &stats_store.borrow());
    sync_tasks_to_ui(&main_window, &app_state.borrow());
    sync_tray(tray.as_deref(), &engine.borrow());

    // 250 ms tick
    let timer = slint::Timer::default();
    let ui_weak = main_window.as_weak();
    let engine_tick = Rc::clone(&engine);
    let state_tick = Rc::clone(&app_state);
    let stats_tick = Rc::clone(&stats_store);
    let session_tick = Rc::clone(&session_meta);
    let tray_tick = tray.clone();

    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut eng = engine_tick.borrow_mut();
            if eng.state != TimerState::Running {
                return;
            }

            if eng.tick() {
                handle_completion(&ui, &mut eng, &state_tick, &stats_tick, &session_tick);
            }

            sync_engine_to_ui(&ui, &eng);
            sync_tray(tray_tick.as_deref(), &eng);
        },
    );

    // start / pause button
    {
        let engine = Rc::clone(&engine);
        let state = Rc::clone(&app_state);
        let stats = Rc::clone(&stats_store);
        let session = Rc::clone(&session_meta);
        let tray = tray.clone();
        let ui_weak = main_window.as_weak();
        main_window.on_start_pause_clicked(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut eng = engine.borrow_mut();
            toggle_timer(&ui, &mut eng, &state, &stats, &session);
            sync_tray(tray.as_deref(), &eng);
        });
    }

    // reset current mode (icon at the top left)
    {
        let engine = Rc::clone(&engine);
        let session = Rc::clone(&session_meta);
        let tray = tray.clone();
        let ui_weak = main_window.as_weak();
        main_window.on_reset_clicked(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut eng = engine.borrow_mut();
            reset_timer(&ui, &mut eng, &session);
            sync_tray(tray.as_deref(), &eng);
        });
    }

    // skip to the next mode
    {
        let engine = Rc::clone(&engine);
        let state = Rc::clone(&app_state);
        let session = Rc::clone(&session_meta);
        let tray = tray.clone();
        let ui_weak = main_window.as_weak();
        main_window.on_skip_clicked(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut eng = engine.borrow_mut();
            skip_timer(&ui, &mut eng, &state, &session);
            sync_tray(tray.as_deref(), &eng);
        });
    }

    // mode pills
    {
        let engine = Rc::clone(&engine);
        let session = Rc::clone(&session_meta);
        let tray = tray.clone();
        let ui_weak = main_window.as_weak();
        main_window.on_mode_selected(move |mode_idx| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut eng = engine.borrow_mut();
            let mode = match mode_idx {
                0 => TimerMode::Focus,
                1 => TimerMode::ShortBreak,
                2 => TimerMode::LongBreak,
                _ => TimerMode::Focus,
            };
            select_mode(&ui, &mut eng, mode, &session);
            sync_tray(tray.as_deref(), &eng);
        });
    }

    // save settings
    {
        let engine = Rc::clone(&engine);
        let state = Rc::clone(&app_state);
        let tray = tray.clone();
        let ui_weak = main_window.as_weak();
        main_window.on_settings_saved(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut st = state.borrow_mut();

            st.focus_duration_mins = ui.get_focus_duration().max(1) as u32;
            st.short_break_duration_mins = ui.get_short_break_duration().max(1) as u32;
            st.long_break_duration_mins = ui.get_long_break_duration().max(1) as u32;
            st.long_break_interval = ui.get_long_break_interval().max(1) as u32;
            st.sound_enabled = ui.get_sound_enabled();
            st.sound_volume = ui.get_sound_volume().clamp(0, 100) as u8;
            st.auto_start_breaks = ui.get_auto_start_breaks();
            st.auto_start_focus = ui.get_auto_start_focus();
            save_state(&ui, &st);

            let new_config = state_to_config(&st);
            let mut eng = engine.borrow_mut();
            eng.update_config(new_config);
            sync_engine_to_ui(&ui, &eng);
            sync_tray(tray.as_deref(), &eng);
        });
    }

    // reset settings to defaults
    {
        let engine = Rc::clone(&engine);
        let state = Rc::clone(&app_state);
        let session = Rc::clone(&session_meta);
        let tray = tray.clone();
        let stats = Rc::clone(&stats_store);
        let ui_weak = main_window.as_weak();
        main_window.on_settings_reset_defaults(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let defaults = AppState::default();
            let new_config = state_to_config(&defaults);

            let keep_session = {
                let eng = engine.borrow();
                matches!(eng.state, TimerState::Running | TimerState::Paused)
                    && eng.mode == TimerMode::Focus
                    && session.borrow().is_some()
            };
            {
                let mut eng = engine.borrow_mut();
                eng.completed_sessions = 0;
                eng.cycle_focus_count = 0;
                eng.update_config(new_config);
            }

            let mut st = state.borrow_mut();
            *st = defaults;
            st.completed_sessions = 0;
            st.cycle_focus_count = 0;
            if !keep_session {
                *session.borrow_mut() = None;
            }
            save_state(&ui, &st);

            sync_settings_to_ui(&ui, &st);
            sync_tasks_to_ui(&ui, &st);
            {
                let eng = engine.borrow();
                sync_engine_to_ui(&ui, &eng);
                sync_tray(tray.as_deref(), &eng);
            }
            sync_goals_to_ui(&ui, &st, &stats.borrow());
        });
    }

    // dismiss the notice and stop the alarm
    {
        let ui_weak = main_window.as_weak();
        main_window.on_dismiss_notice(move || {
            notify::stop_alarm();
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_notice_visible(false);
            }
        });
    }

    // stats page
    {
        let stats_store = Rc::clone(&stats_store);
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_stats_clicked(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let stats = stats_store.borrow();
            sync_stats_to_ui(&ui, &stats, ui.get_stats_range());
            sync_goals_to_ui(&ui, &state.borrow(), &stats);
        });
    }

    // stats range tabs
    {
        let stats_store = Rc::clone(&stats_store);
        let ui_weak = main_window.as_weak();
        main_window.on_stats_range_selected(move |range| {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_stats_range(range);
            sync_stats_to_ui(&ui, &stats_store.borrow(), range);
        });
    }

    // tasks page
    {
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_tasks_clicked(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_task_error("".into());
            sync_tasks_to_ui(&ui, &state.borrow());
        });
    }

    {
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_task_selected(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if st.select_task(name.as_str()) {
                save_state(&ui, &st);
                sync_tasks_to_ui(&ui, &st);
                ui.set_task_error("".into());
            }
        });
    }

    {
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_task_added(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if st.add_task(name.as_str()) {
                save_state(&ui, &st);
                ui.set_task_error("".into());
            } else {
                ui.set_task_error(
                    format!(
                        "Use a unique name (max {MAX_TASK_NAME_LEN} chars, up to {MAX_TASKS} tasks)."
                    )
                    .into(),
                );
            }
            sync_tasks_to_ui(&ui, &st);
        });
    }

    {
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_task_deleted(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut st = state.borrow_mut();
            if st.delete_task(name.as_str()) {
                save_state(&ui, &st);
                ui.set_task_error("".into());
            }
            sync_tasks_to_ui(&ui, &st);
        });
    }

    // second instance: bring this window to the front
    let activation_timer = slint::Timer::default();
    let activation_ui = main_window.as_weak();
    activation_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(300),
        move || {
            if !single_instance::activation_requested() {
                return;
            }
            if let Some(ui) = activation_ui.upgrade() {
                let _ = ui.show();
                ui.invoke_focus_shortcuts();
                single_instance::focus_existing();
            }
        },
    );

    // data page
    {
        let ui_weak = main_window.as_weak();
        main_window.on_data_clicked(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_data_path(paths::data_dir().display().to_string().into());
                ui.set_data_status("".into());
            }
        });
    }
    {
        let stats = Rc::clone(&stats_store);
        let ui_weak = main_window.as_weak();
        main_window.on_data_export_csv(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("CSV", &["csv"])
                .set_file_name("gomo-sessions.csv")
                .save_file()
            else {
                return;
            };
            let result = stats.borrow().export_sessions_csv(&path);
            report_data_result(&ui, result, "Exported", &path);
        });
    }
    {
        let stats = Rc::clone(&stats_store);
        let ui_weak = main_window.as_weak();
        main_window.on_data_export_json(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .set_file_name("gomo-backup.json")
                .save_file()
            else {
                return;
            };
            let result = stats.borrow().export_json(&path);
            report_data_result(&ui, result, "Exported", &path);
        });
    }
    {
        let stats = Rc::clone(&stats_store);
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_data_import_json(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .pick_file()
            else {
                return;
            };
            let answer = rfd::MessageDialog::new()
                .set_title("Import backup")
                .set_description(
                    "Replace all current statistics with this backup? A backup of the current data is created first.",
                )
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if answer != rfd::MessageDialogResult::Yes {
                return;
            }
            let mut stats = stats.borrow_mut();
            match stats.import_json(&path) {
                Ok(()) => {
                    sync_stats_to_ui(&ui, &stats, ui.get_stats_range());
                    sync_goals_to_ui(&ui, &state.borrow(), &stats);
                    let message = format!("Imported: {}", path.display());
                    ui.set_data_status(message.clone().into());
                    logging::info("import", &message);
                }
                Err(err) => {
                    let message = format!("Import failed: {err}");
                    ui.set_data_status(message.clone().into());
                    logging::error("import", &message);
                }
            }
        });
    }
    {
        let stats = Rc::clone(&stats_store);
        let ui_weak = main_window.as_weak();
        main_window.on_data_backup(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let path = stats.borrow().backup();
            match path {
                Some(path) => {
                    let message = format!("Backup created: {}", path.display());
                    ui.set_data_status(message.clone().into());
                    logging::info("backup", &message);
                }
                None => {
                    ui.set_data_status("Backup failed".into());
                    logging::error("backup", "database backup failed");
                }
            }
        });
    }

    {
        let stats = Rc::clone(&stats_store);
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_data_restore_backup(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let answer = rfd::MessageDialog::new()
                .set_title("Restore last backup")
                .set_description(
                    "Replace current statistics with the last backup? A pre-restore snapshot is saved first.",
                )
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if answer != rfd::MessageDialogResult::Yes {
                return;
            }

            let safety = {
                let mut store = stats.borrow_mut();
                store.restore_backup()
            };
            match safety {
                Ok(path) => {
                    {
                        let store = stats.borrow();
                        let range = ui.get_stats_range();
                        sync_stats_to_ui(&ui, &store, range);
                        sync_goals_to_ui(&ui, &state.borrow(), &store);
                    }
                    let message = format!("Restored last backup. Previous data: {}", path.display());
                    ui.set_data_status(message.clone().into());
                    logging::info("restore", &message);
                }
                Err(err) => {
                    let message = format!("Restore failed: {err}");
                    ui.set_data_status(message.clone().into());
                    logging::error("restore", &message);
                }
            }
        });
    }

    // goals page
    {
        let stats = Rc::clone(&stats_store);
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_goals_clicked(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            sync_goals_to_ui(&ui, &state.borrow(), &stats.borrow());
        });
    }
    {
        let stats = Rc::clone(&stats_store);
        let state = Rc::clone(&app_state);
        let ui_weak = main_window.as_weak();
        main_window.on_goals_changed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let daily = ui.get_daily_goal().clamp(0, 600) as u32;
            let weekly = ui.get_weekly_goal().clamp(0, 4200) as u32;
            let mut st = state.borrow_mut();
            st.daily_goal_minutes = daily;
            st.weekly_goal_minutes = weekly;
            save_state(&ui, &st);
            sync_goals_to_ui(&ui, &st, &stats.borrow());
        });
    }

    // tray menu (skipped when there is no tray)
    if let Some(tray) = tray.as_ref() {
        {
            let window = main_window.clone_strong();
            tray.on_show_window(move || {
                let _ = window.show();
                window.invoke_focus_shortcuts();
                single_instance::focus_existing();
            });
        }
        {
            let window = main_window.clone_strong();
            tray.on_hide_window(move || {
                let _ = window.hide();
            });
        }
        {
            let engine = Rc::clone(&engine);
            let state = Rc::clone(&app_state);
            let stats = Rc::clone(&stats_store);
            let session = Rc::clone(&session_meta);
            let tray_clone = Rc::clone(tray);
            let ui_weak = main_window.as_weak();
            tray.on_toggle_timer(move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                let mut eng = engine.borrow_mut();
                toggle_timer(&ui, &mut eng, &state, &stats, &session);
                sync_tray(Some(tray_clone.as_ref()), &eng);
            });
        }
        {
            let engine = Rc::clone(&engine);
            let session = Rc::clone(&session_meta);
            let tray_clone = Rc::clone(tray);
            let ui_weak = main_window.as_weak();
            tray.on_reset_timer(move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                let mut eng = engine.borrow_mut();
                reset_timer(&ui, &mut eng, &session);
                sync_tray(Some(tray_clone.as_ref()), &eng);
            });
        }
        {
            let engine = Rc::clone(&engine);
            let state = Rc::clone(&app_state);
            let session = Rc::clone(&session_meta);
            let tray_clone = Rc::clone(tray);
            let ui_weak = main_window.as_weak();
            tray.on_skip_timer(move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                let mut eng = engine.borrow_mut();
                skip_timer(&ui, &mut eng, &state, &session);
                sync_tray(Some(tray_clone.as_ref()), &eng);
            });
        }
        {
            tray.on_quit_app(move || {
                let _ = slint::quit_event_loop();
            });
        }
    }

    // close hides to the tray, or quits when there is no tray
    let window_only = tray.is_none();
    let close_ui = main_window.as_weak();
    main_window.window().on_close_requested(move || {
        if let Some(ui) = close_ui.upgrade() {
            if ui.get_show_settings() {
                ui.invoke_settings_saved();
            }
        }
        if window_only {
            let _ = slint::quit_event_loop();
        }
        slint::CloseRequestResponse::HideWindow
    });

    main_window.show()?;
    if let Some(tray) = &tray {
        tray.show()?;
    }
    let result = slint::run_event_loop();
    notify::stop_alarm();
    stats_store.borrow().backup();
    result
}
