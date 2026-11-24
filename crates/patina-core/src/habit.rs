//! Habit strength and today status. Pure: `today` and settings are parameters.

use jiff::Span;
use jiff::civil::Date;

use crate::error::TimeError;
use crate::event::{Frequency, HabitKind};
use crate::state::Habit;
use crate::time::{BACKFILL_DAYS, Settings, Weekday, week_start_of};

/// Today's standing of one habit.
#[derive(Clone, Debug, PartialEq)]
pub struct HabitToday {
    /// Still expected today.
    pub due: bool,
    /// Today's completion, 0.0..=1.0.
    pub value: f64,
    /// Today's count, for count habits.
    pub count: Option<u32>,
    /// Weekly habits: check-in value so far this week.
    pub week_value: Option<f64>,
    /// Weekly habits: times per week.
    pub week_target: Option<u8>,
}

/// Completion of `day`: check habits 1.0 or 0.0, count habits `count / target` capped at 1.
pub fn day_value(habit: &Habit, day: Date) -> f64 {
    match habit.checkins.get(&day) {
        None => 0.0,
        Some(c) => match habit.kind {
            HabitKind::Check => 1.0,
            HabitKind::Count { target } => {
                (f64::from(c.count.unwrap_or(0)) / f64::from(target.max(1))).min(1.0)
            }
        },
    }
}

/// Whether `day` falls in a pause interval.
pub fn is_paused(habit: &Habit, day: Date) -> bool {
    habit
        .pauses
        .iter()
        .any(|(from, resume)| day >= *from && resume.is_none_or(|r| day < r))
}

fn add_days(day: Date, n: i64) -> Result<Date, TimeError> {
    Ok(day.checked_add(Span::new().days(n))?)
}

fn weekday_listed(days: &[Weekday], day: Date) -> bool {
    days.contains(&Weekday::from_jiff(day.weekday()))
}

/// One scoring unit: its last day, completion, and time weight in days.
struct Unit {
    last_day: Date,
    x: f64,
    delta: f64,
}

fn week_sum(habit: &Habit, from: Date, to: Date) -> Result<f64, TimeError> {
    let mut sum = 0.0;
    let mut d = from;
    while d <= to {
        sum += day_value(habit, d);
        d = d.tomorrow()?;
    }
    Ok(sum)
}

/// Habit strength in 0.0..=1.0 (exponential moving average, recomputed from scratch).
///
/// # Errors
/// Returns [`TimeError`] on date arithmetic overflow.
pub fn strength(habit: &Habit, today: Date, settings: &Settings) -> Result<f64, TimeError> {
    let mut units = Vec::new();
    match &habit.frequency {
        Frequency::Daily | Frequency::Weekdays { .. } => {
            let delta = match &habit.frequency {
                Frequency::Weekdays { days } => 7.0 / days.len().max(1) as f64,
                _ => 1.0,
            };
            let mut d = habit.start;
            while d <= today {
                let counts = match &habit.frequency {
                    Frequency::Weekdays { days } => weekday_listed(days, d),
                    _ => true,
                };
                if counts && !is_paused(habit, d) && !habit.skips.contains(&d) {
                    units.push(Unit {
                        last_day: d,
                        x: day_value(habit, d),
                        delta,
                    });
                }
                d = d.tomorrow()?;
            }
        }
        Frequency::Weekly { times } => {
            let mut ws = week_start_of(habit.start, settings)?;
            let last_ws = week_start_of(today, settings)?;
            while ws <= last_ws {
                let we = add_days(ws, 6)?;
                let from = ws.max(habit.start);
                let mut skipped = false;
                let mut available = 0u8;
                let mut d = ws;
                while d <= we {
                    skipped |= habit.skips.contains(&d);
                    if d >= from && !is_paused(habit, d) {
                        available += 1;
                    }
                    d = d.tomorrow()?;
                }
                let target = (*times).min(available);
                if !skipped && target > 0 {
                    let sum = week_sum(habit, from, we.min(today))?;
                    units.push(Unit {
                        last_day: we,
                        x: (sum / f64::from(target)).min(1.0),
                        delta: 7.0,
                    });
                }
                ws = add_days(ws, 7)?;
            }
        }
    }
    let settled_by = add_days(today, -(BACKFILL_DAYS + 1))?;
    let h = settings.habit_half_life_days;
    let mut s = 0.0;
    for u in units {
        if u.last_day <= settled_by || u.x >= 1.0 {
            let alpha = (-u.delta / h).exp2();
            s = alpha * s + (1.0 - alpha) * u.x;
        }
    }
    Ok(s)
}

/// Today's status of a habit.
///
/// # Errors
/// Returns [`TimeError`] on date arithmetic overflow.
pub fn today_status(
    habit: &Habit,
    today: Date,
    settings: &Settings,
) -> Result<HabitToday, TimeError> {
    let value = day_value(habit, today);
    let count = habit.checkins.get(&today).and_then(|c| c.count);
    let off = is_paused(habit, today) || habit.skips.contains(&today);
    let mut week_value = None;
    let mut week_target = None;
    let due = match &habit.frequency {
        Frequency::Daily => !off,
        Frequency::Weekdays { days } => weekday_listed(days, today) && !off,
        Frequency::Weekly { times } => {
            let ws = week_start_of(today, settings)?;
            let we = add_days(ws, 6)?;
            let mut skipped = false;
            let mut d = ws;
            while d <= we {
                skipped |= habit.skips.contains(&d);
                d = d.tomorrow()?;
            }
            let sum = week_sum(habit, ws.max(habit.start), today)?;
            week_value = Some(sum);
            week_target = Some(*times);
            !skipped && !is_paused(habit, today) && sum < f64::from(*times)
        }
    };
    Ok(HabitToday {
        due,
        value,
        count,
        week_value,
        week_target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::testutil::{TestLog, date};

    fn days(start: &str, n: i64) -> Vec<String> {
        (0..n)
            .map(|i| add_days(date(start), i).unwrap().to_string())
            .collect()
    }

    fn setup(freq: Frequency, start: &str) -> (TestLog, crate::ids::HabitId) {
        let mut log = TestLog::new();
        let id = log.identity("study");
        let h = log.habit("read", id, freq, start);
        (log, h)
    }

    fn strength_at(log: &TestLog, h: crate::ids::HabitId, today: &str) -> f64 {
        strength(&log.state().habits[&h], date(today), &Settings::default()).unwrap()
    }

    #[test]
    fn daily_streak_follows_closed_form() {
        for (n, above) in [(47, true), (46, false)] {
            let (mut log, h) = setup(Frequency::Daily, "2026-10-01");
            let ds = days("2026-10-01", n);
            for d in &ds {
                log.check(h, d);
            }
            let s = strength_at(&log, h, ds.last().unwrap());
            assert!((s - (1.0 - (-(n as f64) / 14.0).exp2())).abs() < 1e-9);
            assert_eq!(s > 0.9, above);
        }
    }

    #[test]
    fn skipped_day_leaves_strength_unchanged() {
        let (mut log, h) = setup(Frequency::Daily, "2026-10-01");
        for d in days("2026-10-01", 20) {
            log.check(h, &d);
        }
        let before = strength_at(&log, h, "2026-10-20");
        log.push(EventData::HabitSkipped(HabitSkipped {
            habit: h,
            day: date("2026-10-21"),
        }));
        assert!((strength_at(&log, h, "2026-10-21") - before).abs() < 1e-12);
    }

    #[test]
    fn missed_day_costs_only_after_backfill_window() {
        let (mut log, h) = setup(Frequency::Daily, "2026-10-01");
        for d in days("2026-10-01", 10) {
            log.check(h, &d);
        }
        let at10 = strength_at(&log, h, "2026-10-10");
        assert!((strength_at(&log, h, "2026-10-12") - at10).abs() < 1e-12);
        assert!(strength_at(&log, h, "2026-10-14") < at10);
    }

    #[test]
    fn weekly_unit_equals_seven_daily_units() {
        let (mut weekly, w) = setup(Frequency::Weekly { times: 3 }, "2026-11-23");
        for d in ["2026-11-23", "2026-11-25", "2026-11-28"] {
            weekly.check(w, d);
        }
        let (mut daily, d) = setup(Frequency::Daily, "2026-11-23");
        for day in days("2026-11-23", 7) {
            daily.check(d, &day);
        }
        let a = strength_at(&weekly, w, "2026-12-02");
        let b = strength_at(&daily, d, "2026-11-29");
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }

    #[test]
    fn weekly_due_until_target_met() {
        let (mut log, h) = setup(Frequency::Weekly { times: 2 }, "2026-11-23");
        let today = date("2026-11-25");
        let st = |log: &TestLog| {
            today_status(&log.state().habits[&h], today, &Settings::default()).unwrap()
        };
        assert!(st(&log).due);
        log.check(h, "2026-11-24");
        log.check(h, "2026-11-25");
        let s = st(&log);
        assert!(!s.due);
        assert_eq!((s.week_value, s.week_target), (Some(2.0), Some(2)));
    }
}
