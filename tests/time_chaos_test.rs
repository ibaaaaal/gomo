#[path = "../src/paths.rs"]
#[allow(dead_code)]
mod paths;

#[path = "../src/stats.rs"]
#[allow(dead_code)]
mod stats;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Weekday};
use stats::{StatsStore, HEATMAP_WEEKS};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn mem_store() -> StatsStore {
    StatsStore::open_in_memory().unwrap()
}

#[test]
fn midnight_sessions_are_attributed_to_the_start_date() {
    let mut store = mem_store();
    let start = Local.with_ymd_and_hms(2026, 11, 6, 23, 59, 0).unwrap();
    let end = Local.with_ymd_and_hms(2026, 11, 7, 0, 0, 30).unwrap();

    store
        .record_focus_session(start, end, 90, "Midnight")
        .unwrap();

    assert_eq!(
        store.totals_between(date(2026, 11, 6), date(2026, 11, 6)),
        (1, 90)
    );
    assert_eq!(
        store.totals_between(date(2026, 11, 7), date(2026, 11, 7)),
        (0, 0)
    );
    assert_eq!(
        store.task_totals_between(date(2026, 11, 6), date(2026, 11, 6)),
        vec![("Midnight".to_string(), 1, 90)]
    );
}

#[test]
fn backwards_clock_keeps_session_timestamps_consistent() {
    let mut store = mem_store();
    let start = Local.with_ymd_and_hms(2026, 11, 6, 10, 0, 0).unwrap();
    let backwards = Local.with_ymd_and_hms(2026, 11, 6, 9, 50, 0).unwrap();

    store
        .record_focus_session(start, backwards, 1500, "Clock")
        .unwrap();
    store
        .record_session(
            start + Duration::minutes(30),
            backwards,
            "short_break",
            "",
            300,
        )
        .unwrap();

    let sessions = store.sessions();
    assert_eq!(sessions.len(), 2);
    let focus_end = DateTime::parse_from_rfc3339(&sessions[0].ended_at).unwrap();
    assert_eq!(
        focus_end.with_timezone(&Local),
        start + Duration::minutes(25)
    );
    assert!(focus_end >= DateTime::parse_from_rfc3339(&sessions[0].started_at).unwrap());

    let break_start = DateTime::parse_from_rfc3339(&sessions[1].started_at).unwrap();
    let break_end = DateTime::parse_from_rfc3339(&sessions[1].ended_at).unwrap();
    assert_eq!(
        break_end.with_timezone(&Local),
        break_start.with_timezone(&Local) + Duration::minutes(5)
    );
    assert!(break_end >= break_start);
}

#[test]
fn leap_day_and_year_boundary_are_handled() {
    let mut store = mem_store();
    for day in [date(2023, 12, 31), date(2024, 1, 1), date(2024, 1, 2)] {
        store.record_focus(day, 1500, "General").unwrap();
    }

    let summary = store.summary(date(2024, 1, 2));
    assert_eq!(summary.current_streak, 3);
    assert_eq!(summary.today_sessions, 1);
    assert_eq!(
        summary.best_day,
        Some((
            date(2024, 1, 2),
            stats::DayStats {
                focus_sessions: 1,
                focus_seconds: 1500
            }
        ))
    );

    store.record_focus(date(2024, 2, 29), 1500, "Leap").unwrap();
    let summary = store.summary(date(2024, 3, 1));
    assert_eq!(summary.current_streak, 1);
    assert_eq!(summary.today_sessions, 0);
    let heatmap = store.heatmap(date(2024, 3, 1), 4);
    let leap_cell = heatmap
        .iter()
        .find(|cell| cell.date == date(2024, 2, 29))
        .unwrap();
    assert_eq!(leap_cell.level, 2);
    assert_eq!(leap_cell.sessions, 1);
}

#[test]
fn heatmap_ends_today_for_every_weekday() {
    let monday = date(2026, 11, 2);
    assert_eq!(monday.weekday(), Weekday::Mon);

    for offset in 0..7 {
        let today = monday + Duration::days(offset);
        let mut store = mem_store();
        store.record_focus(today, 1500, "General").unwrap();
        let cells = store.heatmap(today, HEATMAP_WEEKS);
        assert_eq!(cells.len(), (HEATMAP_WEEKS * 7) as usize);
        assert_eq!(cells[0].date.weekday(), Weekday::Mon);

        let index = ((HEATMAP_WEEKS - 1) * 7 + today.weekday().num_days_from_monday()) as usize;
        assert_eq!(cells[index].date, today);
        assert_eq!(cells[index].level, 2);
        for cell in &cells[index + 1..] {
            assert_eq!(cell.level, -1);
        }
    }
}

#[test]
fn future_days_do_not_extend_the_current_streak() {
    let today = date(2026, 11, 6);
    let mut store = mem_store();
    for day in [today - Duration::days(1), today, today + Duration::days(1)] {
        store.record_focus(day, 1500, "General").unwrap();
    }

    let summary = store.summary(today);
    assert_eq!(summary.current_streak, 2);
    assert_eq!(summary.total_sessions, 3);
}

#[test]
fn empty_history_and_zero_ranges_are_safe() {
    let store = mem_store();
    let today = date(2026, 11, 6);
    let summary = store.summary(today);
    assert_eq!(summary.total_sessions, 0);
    assert_eq!(summary.current_streak, 0);
    assert!(summary.best_day.is_none());

    assert_eq!(store.daily_series(today, 0).len(), 1);
    assert_eq!(store.monthly_series(today, 0).len(), 1);
    assert_eq!(store.heatmap(today, 0).len(), 7);
    assert!(store
        .daily_series(today, 7)
        .iter()
        .all(|(_, seconds)| *seconds == 0));
    assert!(store
        .monthly_series(today, 3)
        .iter()
        .all(|(_, seconds)| *seconds == 0));
}
