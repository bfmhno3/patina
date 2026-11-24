//! Time tracking over timer segments. Pure: `now` is a parameter.

use std::collections::{BTreeMap, HashMap};

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::dag;
use crate::error::TimeError;
use crate::event::TimerTarget;
use crate::ids::{HabitId, ItemId};
use crate::state::{Segment, State};
use crate::time::{Settings, logical_day};

/// Seconds a segment has run; a running segment counts up to `now`.
pub fn seconds(seg: &Segment, now: Timestamp) -> i64 {
    seg.end
        .unwrap_or(now)
        .duration_since(seg.start)
        .as_secs()
        .max(0)
}

/// Directly tracked seconds per item (item targets only).
pub fn item_seconds(state: &State, now: Timestamp) -> HashMap<ItemId, i64> {
    let mut out = HashMap::new();
    for seg in state.segments.values() {
        if let TimerTarget::Item(id) = seg.target {
            *out.entry(id).or_insert(0) += seconds(seg, now);
        }
    }
    out
}

/// Tracked seconds of an item and all its descendants.
pub fn subtree_seconds(state: &State, id: ItemId, direct: &HashMap<ItemId, i64>) -> i64 {
    dag::subtree(state, id)
        .iter()
        .filter_map(|i| direct.get(i))
        .sum()
}

/// Tracked seconds of a habit per logical day of the segment start.
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub fn habit_seconds(
    state: &State,
    habit: HabitId,
    tz: &TimeZone,
    settings: &Settings,
    now: Timestamp,
) -> Result<BTreeMap<Date, i64>, TimeError> {
    let mut out = BTreeMap::new();
    for seg in state.segments.values() {
        if seg.target == TimerTarget::Habit(habit) {
            let day = logical_day(&seg.start.to_zoned_datetime(tz.clone()), settings)?;
            *out.entry(day).or_insert(0) += seconds(seg, now);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::ids::SegmentId;
    use crate::testutil::TestLog;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn segment(log: &mut TestLog, target: TimerTarget, from: &str, to: Option<&str>) {
        let seg = SegmentId::from_uuid(log.uuid());
        log.push_at(
            EventData::TimerStarted(TimerStarted {
                segment: seg,
                target,
            }),
            ts(from),
            ts(from),
        );
        if let Some(to) = to {
            log.push_at(
                EventData::TimerStopped(TimerStopped { segment: seg }),
                ts(to),
                ts(to),
            );
        }
    }

    #[test]
    fn sums_per_subtree_and_counts_the_running_segment() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let parent = log.item("parent", id, None);
        let child = log.item("child", id, Some(parent));
        segment(
            &mut log,
            TimerTarget::Item(parent),
            "2026-11-24T01:00:00Z",
            Some("2026-11-24T01:10:00Z"),
        );
        segment(
            &mut log,
            TimerTarget::Item(child),
            "2026-11-24T02:00:00Z",
            None,
        );
        let state = log.state();
        assert!(state.warnings.is_empty());
        let now = ts("2026-11-24T02:05:00Z");
        let direct = item_seconds(&state, now);
        assert_eq!(direct[&parent], 600);
        assert_eq!(subtree_seconds(&state, parent, &direct), 900);
        assert_eq!(subtree_seconds(&state, child, &direct), 300);
    }

    #[test]
    fn habit_seconds_group_by_logical_day() {
        let mut log = TestLog::new();
        let id = log.identity("study");
        let h = log.habit_of(
            "read",
            id,
            HabitKind::Duration { minutes: 30 },
            Frequency::Daily,
            "2026-11-23",
        );
        // 02:00 local (+08:00) belongs to the previous logical day (day starts at 04:00).
        segment(
            &mut log,
            TimerTarget::Habit(h),
            "2026-11-23T18:00:00Z",
            Some("2026-11-23T18:10:00Z"),
        );
        segment(
            &mut log,
            TimerTarget::Habit(h),
            "2026-11-24T01:00:00Z",
            Some("2026-11-24T01:20:00Z"),
        );
        let state = log.state();
        assert!(state.warnings.is_empty(), "{:?}", state.warnings);
        let tz = TimeZone::get("Asia/Shanghai").unwrap();
        let by_day = habit_seconds(
            &state,
            h,
            &tz,
            &Settings::default(),
            ts("2026-11-24T03:00:00Z"),
        )
        .unwrap();
        assert_eq!(by_day[&"2026-11-23".parse().unwrap()], 600);
        assert_eq!(by_day[&"2026-11-24".parse().unwrap()], 1200);
    }
}
