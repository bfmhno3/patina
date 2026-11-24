//! Work rhythm and break reminders, derived from timer segments, breaks, and snoozes. Pure.
//!
//! Nothing here is stored: whether a break is due or running follows from the log and `now`.

use jiff::Timestamp;
use serde::Serialize;

use crate::event::{BreakKind, RhythmMode, TimerTarget};
use crate::state::State;
use crate::time::Settings;

/// The chosen rhythm mode (default fixed).
pub fn mode(state: &State) -> RhythmMode {
    state
        .rhythm
        .last()
        .map_or(RhythmMode::Fixed, |(_, mode)| *mode)
}

/// A break that should be taken now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct BreakDue {
    /// Which kind.
    pub kind: BreakKind,
    /// How long.
    pub minutes: u32,
}

/// A break in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct OnBreak {
    /// Which kind.
    pub kind: BreakKind,
    /// When it started.
    pub started_at: Timestamp,
    /// When it ends.
    pub ends_at: Timestamp,
}

/// Where the owner stands in the rhythm.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Status {
    /// The mode.
    pub mode: RhythmMode,
    /// Seconds of focus in the current block (0 when none).
    pub focus_secs: i64,
    /// Short breaks taken since the last long break or long pause.
    pub round: u32,
    /// A break is due.
    pub due: Option<BreakDue>,
    /// A break is running.
    pub on_break: Option<OnBreak>,
    /// What the latest segment timed.
    pub last_target: Option<TimerTarget>,
}

/// Consecutive segments separated by less than a short break.
struct Block {
    start: Timestamp,
    end: Timestamp,
    focus_secs: i64,
    running: bool,
}

fn blocks(state: &State, now: Timestamp, settings: &Settings) -> Vec<Block> {
    let short = i64::from(settings.rhythm.short_break_minutes) * 60;
    let mut segments: Vec<_> = state.segments.values().collect();
    segments.sort_by_key(|s| s.start);
    let mut out: Vec<Block> = Vec::new();
    for seg in segments {
        let end = seg.end.unwrap_or(now);
        let secs = end.duration_since(seg.start).as_secs().max(0);
        let running = seg.end.is_none();
        match out.last_mut() {
            Some(b)
                if seg.start.duration_since(b.end).as_secs() < short
                    && !state
                        .breaks
                        .iter()
                        .any(|k| k.at >= b.end && k.at <= seg.start) =>
            {
                b.end = b.end.max(end);
                b.focus_secs += secs;
                b.running |= running;
            }
            _ => out.push(Block {
                start: seg.start,
                end,
                focus_secs: secs,
                running,
            }),
        }
    }
    out
}

fn fixed_next(round: u32, settings: &Settings) -> (BreakKind, u32) {
    let r = &settings.rhythm;
    if round + 1 >= r.rounds_before_long {
        (BreakKind::Long, r.long_break_minutes)
    } else {
        (BreakKind::Short, r.short_break_minutes)
    }
}

fn flow_minutes(focus_secs: i64, settings: &Settings) -> u32 {
    let focus_minutes = u32::try_from(focus_secs / 60).unwrap_or(u32::MAX);
    (focus_minutes / settings.rhythm.flow_ratio).max(1)
}

/// Computes the rhythm status at `now`.
pub fn status(state: &State, now: Timestamp, settings: &Settings) -> Status {
    let r = &settings.rhythm;
    let mode = mode(state);
    let blocks = blocks(state, now, settings);
    let short = i64::from(r.short_break_minutes) * 60;
    let long = i64::from(r.long_break_minutes) * 60;

    let last_target = state
        .segments
        .values()
        .max_by_key(|s| s.start)
        .map(|s| s.target);

    let on_break = state
        .breaks
        .iter()
        .rev()
        .find(|b| now < b.at + jiff::SignedDuration::from_mins(i64::from(b.minutes)))
        .filter(|b| !state.segments.values().any(|s| s.start > b.at))
        .map(|b| OnBreak {
            kind: b.kind,
            started_at: b.at,
            ends_at: b.at + jiff::SignedDuration::from_mins(i64::from(b.minutes)),
        });

    let current = blocks.last().filter(|b| {
        b.running
            || (now.duration_since(b.end).as_secs() < short
                && !state.breaks.iter().any(|k| k.at >= b.end))
    });
    let focus_secs = current.map_or(0, |b| b.focus_secs);

    let last_long = state
        .breaks
        .iter()
        .filter(|b| b.kind == BreakKind::Long)
        .map(|b| b.at)
        .max();
    let last_pause = blocks
        .iter()
        .enumerate()
        .filter(|(i, b)| *i == 0 || b.start.duration_since(blocks[i - 1].end).as_secs() >= long)
        .map(|(_, b)| b.start)
        .max();
    let reset = last_long.max(last_pause).unwrap_or(Timestamp::MIN);
    let round = u32::try_from(
        state
            .breaks
            .iter()
            .filter(|b| b.kind == BreakKind::Short && b.at >= reset)
            .count(),
    )
    .unwrap_or(u32::MAX);

    let due = match mode {
        RhythmMode::Off => None,
        RhythmMode::Fixed => current
            .filter(|b| b.running && on_break.is_none())
            .and_then(|b| {
                let snoozed: i64 = state
                    .snoozes
                    .iter()
                    .filter(|s| s.at >= b.start)
                    .map(|s| i64::from(s.minutes))
                    .sum();
                (b.focus_secs >= (i64::from(r.focus_minutes) + snoozed) * 60).then(|| {
                    let (kind, minutes) = fixed_next(round, settings);
                    BreakDue { kind, minutes }
                })
            }),
        RhythmMode::Flowtime => blocks
            .last()
            .filter(|b| {
                !b.running
                    && b.focus_secs >= 60
                    && !state.breaks.iter().any(|k| k.at >= b.end)
                    && !state.snoozes.iter().any(|s| s.at >= b.end)
            })
            .and_then(|b| {
                let minutes = flow_minutes(b.focus_secs, settings);
                (now.duration_since(b.end).as_secs() < i64::from(minutes) * 60).then_some(
                    BreakDue {
                        kind: BreakKind::Flow,
                        minutes,
                    },
                )
            }),
    };

    Status {
        mode,
        focus_secs,
        round,
        due,
        on_break,
        last_target,
    }
}

/// The break `start break` would start: the due one, else the one the mode would suggest.
pub fn next_break(
    status: &Status,
    state: &State,
    now: Timestamp,
    settings: &Settings,
) -> (BreakKind, u32) {
    if let Some(d) = status.due {
        return (d.kind, d.minutes);
    }
    match status.mode {
        RhythmMode::Fixed | RhythmMode::Off => fixed_next(status.round, settings),
        RhythmMode::Flowtime => match blocks(state, now, settings).last() {
            Some(b) => (BreakKind::Flow, flow_minutes(b.focus_secs, settings)),
            None => (BreakKind::Flow, settings.rhythm.short_break_minutes),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::ids::SegmentId;
    use crate::testutil::TestLog;

    fn ts(hhmm: &str) -> Timestamp {
        format!("2026-11-24T{hhmm}:00Z").parse().unwrap()
    }

    /// A segment `from..to` (open when `to` is `None`) on a fresh item.
    fn seg(log: &mut TestLog, from: &str, to: Option<&str>) {
        let identity = log
            .state()
            .identities
            .keys()
            .next()
            .copied()
            .unwrap_or_else(|| log.identity("work"));
        let item = log.item("t", identity, None);
        let segment = SegmentId::from_uuid(log.uuid());
        log.push_at(
            EventData::TimerStarted(TimerStarted {
                segment,
                target: TimerTarget::Item(item),
            }),
            ts(from),
            ts(from),
        );
        if let Some(to) = to {
            log.push_at(
                EventData::TimerStopped(TimerStopped { segment }),
                ts(to),
                ts(to),
            );
        }
    }

    fn brk(log: &mut TestLog, at: &str, kind: BreakKind, minutes: u32) {
        log.push_at(
            EventData::BreakStarted(BreakStarted { kind, minutes }),
            ts(at),
            ts(at),
        );
    }

    fn due(log: &TestLog, now: &str) -> Option<(BreakKind, u32)> {
        status(&log.state(), ts(now), &Settings::default())
            .due
            .map(|d| (d.kind, d.minutes))
    }

    #[test]
    fn fixed_block_joins_segments_across_a_short_gap() {
        let mut log = TestLog::new();
        seg(&mut log, "09:00", Some("09:15"));
        seg(&mut log, "09:17", None);
        assert_eq!(due(&log, "09:24"), None);
        assert_eq!(due(&log, "09:28"), Some((BreakKind::Short, 5)));
    }

    #[test]
    fn snooze_postpones_by_its_minutes() {
        let mut log = TestLog::new();
        seg(&mut log, "09:00", None);
        log.push_at(
            EventData::BreakSnoozed(BreakSnoozed { minutes: 10 }),
            ts("09:26"),
            ts("09:26"),
        );
        assert_eq!(due(&log, "09:34"), None);
        assert_eq!(due(&log, "09:35"), Some((BreakKind::Short, 5)));
    }

    #[test]
    fn long_break_follows_three_short_ones_and_a_long_pause_resets() {
        let mut log = TestLog::new();
        seg(&mut log, "09:00", Some("09:25"));
        brk(&mut log, "09:25", BreakKind::Short, 5);
        seg(&mut log, "09:30", Some("09:55"));
        brk(&mut log, "09:55", BreakKind::Short, 5);
        seg(&mut log, "10:00", Some("10:25"));
        brk(&mut log, "10:25", BreakKind::Short, 5);
        seg(&mut log, "10:30", None);
        assert_eq!(due(&log, "10:55"), Some((BreakKind::Long, 15)));

        let mut log = TestLog::new();
        seg(&mut log, "09:00", Some("09:25"));
        brk(&mut log, "09:25", BreakKind::Short, 5);
        seg(&mut log, "09:45", None);
        assert_eq!(due(&log, "10:10"), Some((BreakKind::Short, 5)));
        assert_eq!(
            status(&log.state(), ts("10:10"), &Settings::default()).round,
            0
        );
    }

    #[test]
    fn flowtime_suggests_a_proportional_break_right_after_stopping() {
        let mut log = TestLog::new();
        log.push_at(
            EventData::RhythmSwitched(RhythmSwitched {
                mode: RhythmMode::Flowtime,
            }),
            ts("08:00"),
            ts("08:00"),
        );
        seg(&mut log, "09:00", Some("09:40"));
        assert_eq!(due(&log, "09:41"), Some((BreakKind::Flow, 8)));
        assert_eq!(due(&log, "09:47"), Some((BreakKind::Flow, 8)));
        assert_eq!(due(&log, "09:48"), None);
        brk(&mut log, "09:42", BreakKind::Flow, 8);
        assert_eq!(due(&log, "09:43"), None);
    }

    #[test]
    fn break_ends_by_time_or_when_work_resumes() {
        let mut log = TestLog::new();
        seg(&mut log, "09:00", Some("09:25"));
        brk(&mut log, "09:25", BreakKind::Short, 5);
        let on =
            |log: &TestLog, now: &str| status(&log.state(), ts(now), &Settings::default()).on_break;
        assert_eq!(on(&log, "09:29").map(|b| b.ends_at), Some(ts("09:30")));
        assert!(on(&log, "09:30").is_none());
        seg(&mut log, "09:27", None);
        assert!(on(&log, "09:29").is_none());
    }
}
