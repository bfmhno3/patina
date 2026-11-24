//! The review's gauges and statistics over a window: time allocation, interruptions,
//! habit trends, estimate calibration, project completion, and notes. Pure.

use std::collections::{BTreeMap, BTreeSet};

use jiff::civil::Date;
use jiff::{Span, Timestamp};
use serde::Serialize;

use crate::calibration::{self, Conversion};
use crate::error::TimeError;
use crate::event::{InterruptOutcome, TimerTarget};
use crate::habit;
use crate::query::{HabitView, View};
use crate::review::{ProjectProgress, progress};
use crate::state::ItemStatus;
use crate::time::{logical_day, week_start_of};

type Interval = (Timestamp, Timestamp);

const WEEK_SECS: i64 = 7 * 24 * 3600;

fn overlap_secs(a: Interval, b: Interval) -> i64 {
    let start = a.0.max(b.0);
    let end = a.1.min(b.1);
    end.duration_since(start).as_secs().max(0)
}

fn day_of(v: &View<'_>, t: Timestamp) -> Result<Date, TimeError> {
    logical_day(&t.to_zoned_datetime(v.now.time_zone().clone()), v.settings)
}

fn add_days(day: Date, n: i64) -> Result<Date, TimeError> {
    Ok(day.checked_add(Span::new().days(n))?)
}

/// Time spent and planned for one identity in the window.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IdentityTime {
    /// Identity name.
    pub identity: String,
    /// Minutes tracked on its items and habits.
    pub tracked_minutes: i64,
    /// The weekly target prorated to the window.
    pub target_minutes: Option<i64>,
    /// Tracked minus target.
    pub deviation_minutes: Option<i64>,
    /// Minutes spent in this identity's mode, idle gaps without timing excluded.
    pub mode_minutes: i64,
}

fn minutes(secs: i64) -> i64 {
    (secs + 30).div_euclid(60)
}

/// Idle spans in `[.., now]`: stretches longer than the away limit without activity,
/// minus every timed segment.
fn away_spans(v: &View<'_>, segments: &[Interval]) -> Vec<Interval> {
    let now = v.now.timestamp();
    let limit = jiff::SignedDuration::from_mins(i64::from(v.settings.away_after_minutes));
    let mut activity = v.state.activity.clone();
    activity.sort();
    let mut spans = Vec::new();
    for pair in activity.windows(2) {
        if pair[1].duration_since(pair[0]) > limit {
            spans.push((pair[0] + limit, pair[1]));
        }
    }
    if let Some(last) = activity.last()
        && now.duration_since(*last) > limit
    {
        spans.push((*last + limit, now));
    }
    let mut cuts = segments.to_vec();
    cuts.sort();
    let mut out = Vec::new();
    for (mut s, e) in spans {
        for (cs, ce) in &cuts {
            if *ce <= s || *cs >= e {
                continue;
            }
            if *cs > s {
                out.push((s, *cs));
            }
            s = s.max(*ce);
        }
        if s < e {
            out.push((s, e));
        }
    }
    out
}

/// Tracked time, targets, and mode time per identity, sorted by name.
pub(crate) fn time(v: &View<'_>, window_start: Timestamp) -> Vec<IdentityTime> {
    let state = v.state;
    let now = v.now.timestamp();
    let window = (window_start, now);
    let window_secs = now.duration_since(window_start).as_secs().max(0);
    let segments: Vec<(Interval, TimerTarget)> = state
        .segments
        .values()
        .map(|s| ((s.start, s.end.unwrap_or(now)), s.target))
        .collect();
    let seg_intervals: Vec<Interval> = segments.iter().map(|(i, _)| *i).collect();
    let away = away_spans(v, &seg_intervals);

    let mut tracked: BTreeMap<_, i64> = BTreeMap::new();
    for (interval, target) in &segments {
        let identity = match target {
            TimerTarget::Item(id) => state.items.get(id).and_then(|i| i.identity),
            TimerTarget::Habit(id) => state.habits.get(id).map(|h| h.identity),
        };
        if let Some(identity) = identity {
            *tracked.entry(identity).or_insert(0) += overlap_secs(*interval, window);
        }
    }

    let mut mode_secs: BTreeMap<_, i64> = BTreeMap::new();
    for (i, m) in state.modes.iter().enumerate() {
        let Some(identity) = m.identity else { continue };
        let end = state.modes.get(i + 1).map_or(now, |n| n.at);
        let start = m.at.max(window_start);
        let end = end.min(now);
        if end <= start {
            continue;
        }
        let idle: i64 = away.iter().map(|a| overlap_secs((start, end), *a)).sum();
        let secs = end.duration_since(start).as_secs() - idle;
        *mode_secs.entry(identity).or_insert(0) += secs.max(0);
    }

    let mut out: Vec<IdentityTime> = state
        .identities
        .values()
        .map(|i| {
            let tracked_minutes = minutes(tracked.get(&i.id).copied().unwrap_or(0));
            let target_minutes = i
                .weekly_target_minutes
                .map(|t| (f64::from(t) * window_secs as f64 / WEEK_SECS as f64).round() as i64);
            IdentityTime {
                identity: i.name.clone(),
                tracked_minutes,
                target_minutes,
                deviation_minutes: target_minutes.map(|t| tracked_minutes - t),
                mode_minutes: minutes(mode_secs.get(&i.id).copied().unwrap_or(0)),
            }
        })
        .collect();
    out.sort_by(|a, b| a.identity.cmp(&b.identity));
    out
}

/// One interruption in the window.
#[derive(Clone, Debug, Serialize)]
pub struct InterruptionView {
    /// The interruption's title.
    pub title: String,
    /// Identity of the interruption item; none while in the inbox.
    pub identity: Option<String>,
    /// When it happened.
    pub at: Timestamp,
    /// What became of it.
    pub outcome: InterruptOutcome,
}

/// Interruptions per source identity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SourceCount {
    /// Identity name; none for inbox entries.
    pub identity: Option<String>,
    /// Interruptions.
    pub count: usize,
}

/// Interruptions in the window and their sources.
pub(crate) fn interruptions(
    v: &View<'_>,
    window_start: Timestamp,
) -> (Vec<InterruptionView>, Vec<SourceCount>) {
    let now = v.now.timestamp();
    let items: Vec<InterruptionView> = v
        .state
        .interruptions
        .iter()
        .filter(|i| i.at > window_start && i.at <= now)
        .filter_map(|i| {
            let item = v.state.items.get(&i.item)?;
            Some(InterruptionView {
                title: item.title.clone(),
                identity: v.identity_name(item.identity),
                at: i.at,
                outcome: i.outcome,
            })
        })
        .collect();
    let mut counts: BTreeMap<Option<String>, usize> = BTreeMap::new();
    for i in &items {
        *counts.entry(i.identity.clone()).or_insert(0) += 1;
    }
    let mut sources: Vec<SourceCount> = counts
        .into_iter()
        .map(|(identity, count)| SourceCount { identity, count })
        .collect();
    sources.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.identity.cmp(&b.identity))
    });
    (items, sources)
}

/// Habit strength on one day.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StrengthPoint {
    /// The day.
    pub day: Date,
    /// Strength 0..=1.
    pub strength: f64,
}

/// A habit's strength now, at the window start, and over the last weeks.
#[derive(Clone, Debug, Serialize)]
pub struct HabitTrend {
    /// The habit today.
    pub habit: HabitView,
    /// Strength on the first day of the window.
    pub start_strength: f64,
    /// Strength now minus at the start.
    pub change: f64,
    /// Strength once a week for the last eight weeks, oldest first.
    pub curve: Vec<StrengthPoint>,
}

/// Every habit's trend, sorted by identity then name.
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub(crate) fn habits(v: &View<'_>, window_start: Timestamp) -> Result<Vec<HabitTrend>, TimeError> {
    let start_day = day_of(v, window_start)?;
    let mut out = Vec::new();
    for h in v.state.habits.values() {
        let timed = v.habit_timed(h)?;
        let view = v.habit(h)?;
        let start_strength = if start_day < h.start {
            0.0
        } else {
            habit::strength(h, start_day, v.settings, &timed)?
        };
        let mut curve = Vec::new();
        for k in (0..=7).rev() {
            let day = add_days(v.today, -7 * k)?;
            if day >= h.start {
                curve.push(StrengthPoint {
                    day,
                    strength: habit::strength(h, day, v.settings, &timed)?,
                });
            }
        }
        out.push(HabitTrend {
            change: view.strength - start_strength,
            habit: view,
            start_strength,
            curve,
        });
    }
    out.sort_by(|a, b| (&a.habit.identity, &a.habit.name).cmp(&(&b.habit.identity, &b.habit.name)));
    Ok(out)
}

/// Mean calibration error of one tag.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TagError {
    /// The tag.
    pub tag: String,
    /// Mean absolute log2 error.
    pub error: f64,
    /// Samples.
    pub samples: usize,
}

/// Calibration error of one calendar week.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WeekError {
    /// First day of the week.
    pub week_start: Date,
    /// Mean absolute log2 error; none without samples.
    pub error: Option<f64>,
    /// Samples.
    pub samples: usize,
}

/// The estimate-calibration gauge.
#[derive(Clone, Debug, Serialize)]
pub struct CalibrationView {
    /// Mean absolute log2 error over the window.
    pub error: Option<f64>,
    /// Samples in the window.
    pub samples: usize,
    /// Error per tag in the window, worst first.
    pub by_tag: Vec<TagError>,
    /// Error per calendar week for the last eight weeks, oldest first.
    pub weeks: Vec<WeekError>,
    /// Learned conversions over all samples.
    pub conversions: Vec<Conversion>,
}

/// Builds the calibration view.
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub(crate) fn calibration(
    v: &View<'_>,
    window_start: Timestamp,
) -> Result<CalibrationView, TimeError> {
    let now = v.now.timestamp();
    let all = calibration::samples(v.state, now);
    let window: Vec<_> = all
        .iter()
        .filter(|s| s.settled_at > window_start && s.settled_at <= now)
        .collect();
    let tags: BTreeSet<&String> = window.iter().flat_map(|s| &s.tags).collect();
    let mut by_tag: Vec<TagError> = tags
        .into_iter()
        .filter_map(|tag| {
            let of_tag: Vec<_> = window
                .iter()
                .copied()
                .filter(|s| s.tags.contains(tag))
                .collect();
            Some(TagError {
                tag: tag.clone(),
                error: calibration::error(of_tag.iter().copied())?,
                samples: of_tag.len(),
            })
        })
        .collect();
    by_tag.sort_by(|a, b| b.error.total_cmp(&a.error).then_with(|| a.tag.cmp(&b.tag)));

    let this_week = week_start_of(v.today, v.settings)?;
    let mut dated = Vec::with_capacity(all.len());
    for s in &all {
        dated.push((day_of(v, s.settled_at)?, s));
    }
    let mut weeks = Vec::new();
    for k in (0..=7).rev() {
        let ws = add_days(this_week, -7 * k)?;
        let we = add_days(ws, 6)?;
        let in_week: Vec<_> = dated
            .iter()
            .filter(|(d, _)| *d >= ws && *d <= we)
            .map(|(_, s)| *s)
            .collect();
        weeks.push(WeekError {
            week_start: ws,
            error: calibration::error(in_week.iter().copied()),
            samples: in_week.len(),
        });
    }
    Ok(CalibrationView {
        error: calibration::error(window.iter().copied()),
        samples: window.len(),
        by_tag,
        weeks,
        conversions: calibration::conversions(&all, v.settings),
    })
}

/// Open projects with their progress and how the open count moved in the window.
pub(crate) fn open_projects(v: &View<'_>, window_start: Timestamp) -> Vec<ProjectProgress> {
    let mut projects: Vec<_> = v
        .state
        .items
        .values()
        .filter(|i| i.parent.is_none() && !i.children.is_empty() && i.status == ItemStatus::Todo)
        .collect();
    projects.sort_by_key(|i| i.created_at);
    projects
        .into_iter()
        .map(|p| progress(v, p, window_start))
        .collect()
}

/// What a note belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// Written when completing an item.
    Completion,
    /// Written when checking in a habit.
    CheckIn,
}

/// A note written in the window.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WeekNote {
    /// Logical day.
    pub day: Date,
    /// Where it came from.
    pub kind: NoteKind,
    /// Item title or habit name.
    pub subject: String,
    /// The note.
    pub text: String,
}

/// Completion and check-in notes of the window, by day.
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub(crate) fn notes(v: &View<'_>, window_start: Timestamp) -> Result<Vec<WeekNote>, TimeError> {
    let now = v.now.timestamp();
    let mut out = Vec::new();
    for i in v.state.items.values() {
        let (Some(text), Some(at)) = (&i.completion_note, i.settled_at) else {
            continue;
        };
        if i.status == ItemStatus::Done && at > window_start && at <= now {
            out.push(WeekNote {
                day: day_of(v, at)?,
                kind: NoteKind::Completion,
                subject: i.title.clone(),
                text: text.clone(),
            });
        }
    }
    let first = day_of(v, window_start)?;
    for h in v.state.habits.values() {
        for (day, c) in h.checkins.range(first..=v.today) {
            if let Some(text) = &c.note {
                out.push(WeekNote {
                    day: *day,
                    kind: NoteKind::CheckIn,
                    subject: h.name.clone(),
                    text: text.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| (a.day, &a.subject).cmp(&(b.day, &b.subject)));
    Ok(out)
}
