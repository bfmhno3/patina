//! Personal estimate calibration: what a size actually costs, from finished work.
//!
//! Pure: `now` is a parameter. A sample is a completed item with a size estimate and at
//! least a minute of tracked time (its subtree included).

use std::collections::BTreeSet;

use jiff::Timestamp;
use serde::Serialize;

use crate::event::{Estimate, Size};
use crate::ids::ItemId;
use crate::state::{ItemStatus, State};
use crate::time::Settings;
use crate::timer;

/// Samples a (tag, size) or size needs before it replaces the configured minutes.
pub const MIN_SAMPLES: usize = 5;

/// One finished, timed, estimated item.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// The item.
    pub item: ItemId,
    /// Its size.
    pub size: Size,
    /// Minutes the size stood for when estimated.
    pub estimate_minutes: u32,
    /// Minutes actually tracked on it and its descendants.
    pub actual_minutes: f64,
    /// Its tags.
    pub tags: Vec<String>,
    /// When it was completed.
    pub settled_at: Timestamp,
}

/// All samples in `state`.
pub fn samples(state: &State, now: Timestamp) -> Vec<Sample> {
    let direct = timer::item_seconds(state, now);
    let mut out = Vec::new();
    for item in state.items.values() {
        let (Some(est), Some(settled_at)) = (item.estimate, item.settled_at) else {
            continue;
        };
        if item.status != ItemStatus::Done || est.minutes == 0 {
            continue;
        }
        let secs = timer::subtree_seconds(state, item.id, &direct);
        if secs < 60 {
            continue;
        }
        out.push(Sample {
            item: item.id,
            size: est.size,
            estimate_minutes: est.minutes,
            actual_minutes: secs as f64 / 60.0,
            tags: item.tags.iter().cloned().collect(),
            settled_at,
        });
    }
    out
}

fn median_minutes(values: &mut [f64]) -> u32 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    let m = if n == 0 {
        0.0
    } else if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    };
    (m.round() as u32).max(1)
}

fn median_of<'a>(samples: impl Iterator<Item = &'a Sample>) -> (u32, usize) {
    let mut v: Vec<f64> = samples.map(|s| s.actual_minutes).collect();
    let n = v.len();
    (median_minutes(&mut v), n)
}

/// The estimate to store for `size` on an item with `tags`.
///
/// Uses the median of the tag with the most samples of this size (at least
/// [`MIN_SAMPLES`]), else the size-wide median, else the configured minutes.
pub fn estimate(
    samples: &[Sample],
    tags: &BTreeSet<String>,
    size: Size,
    settings: &Settings,
) -> Estimate {
    let of_size = || samples.iter().filter(|s| s.size == size);
    let mut best: Option<(usize, &String, u32)> = None;
    for tag in tags {
        let (minutes, n) = median_of(of_size().filter(|s| s.tags.contains(tag)));
        if n >= MIN_SAMPLES && best.is_none_or(|(bn, _, _)| n > bn) {
            best = Some((n, tag, minutes));
        }
    }
    let minutes = match best {
        Some((_, _, m)) => m,
        None => {
            let (m, n) = median_of(of_size());
            if n >= MIN_SAMPLES {
                m
            } else {
                settings.size_minutes.minutes(size)
            }
        }
    };
    Estimate { size, minutes }
}

/// Mean absolute log2 ratio of actual to estimated minutes; `None` without samples.
pub fn error<'a>(samples: impl IntoIterator<Item = &'a Sample>) -> Option<f64> {
    let (sum, n) = samples.into_iter().fold((0.0, 0usize), |(sum, n), s| {
        let ratio = s.actual_minutes / f64::from(s.estimate_minutes);
        (sum + ratio.log2().abs(), n + 1)
    });
    (n > 0).then(|| sum / n as f64)
}

/// A learned size-to-minutes conversion.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conversion {
    /// The tag it applies to; none for all items.
    pub tag: Option<String>,
    /// The size.
    pub size: Size,
    /// Median minutes actually spent.
    pub minutes: u32,
    /// The configured minutes for the size.
    pub default_minutes: u32,
    /// Samples behind it.
    pub samples: usize,
}

/// Every conversion with at least [`MIN_SAMPLES`] samples.
pub fn conversions(samples: &[Sample], settings: &Settings) -> Vec<Conversion> {
    let mut out = Vec::new();
    let tags: BTreeSet<&String> = samples.iter().flat_map(|s| &s.tags).collect();
    for size in [Size::S, Size::M, Size::L] {
        let default_minutes = settings.size_minutes.minutes(size);
        let groups = std::iter::once(None).chain(tags.iter().map(|t| Some((*t).clone())));
        for tag in groups {
            let (minutes, n) = median_of(
                samples
                    .iter()
                    .filter(|s| s.size == size && tag.as_ref().is_none_or(|t| s.tags.contains(t))),
            );
            if n >= MIN_SAMPLES {
                out.push(Conversion {
                    tag,
                    size,
                    minutes,
                    default_minutes,
                    samples: n,
                });
            }
        }
    }
    out.sort_by(|a, b| (a.tag.is_some(), &a.tag, a.size).cmp(&(b.tag.is_some(), &b.tag, b.size)));
    out
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

    /// A completed `size` item tagged `tags` with `minutes` of tracked time.
    fn done(log: &mut TestLog, tags: &[&str], size: Size, minutes: i64) {
        let identity = log
            .state()
            .identities
            .keys()
            .next()
            .copied()
            .unwrap_or_else(|| log.identity("work"));
        let item = ItemId::from_uuid(log.uuid());
        log.push(EventData::ItemCreated(ItemCreated {
            item,
            title: "t".into(),
            identity,
            parent: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            defer: None,
            scheduled: None,
            deadline: None,
            waiting_for: None,
            follow_up: None,
            important: false,
            estimate: Some(Estimate { size, minutes: 30 }),
            repeat: None,
            repeat_of: None,
        }));
        let seg = SegmentId::from_uuid(log.uuid());
        let k = seg.as_uuid().as_u128() as i64;
        let from = ts("2026-11-01T01:00:00Z") + jiff::SignedDuration::from_hours(3 * k);
        let to = from + jiff::SignedDuration::from_mins(minutes);
        log.push_at(
            EventData::TimerStarted(TimerStarted {
                segment: seg,
                target: TimerTarget::Item(item),
            }),
            from,
            from,
        );
        log.push_at(
            EventData::TimerStopped(TimerStopped { segment: seg }),
            to,
            to,
        );
        log.complete(item);
    }

    fn pick(log: &TestLog, tags: &[&str], size: Size) -> u32 {
        let state = log.state();
        let s = samples(&state, ts("2026-11-24T01:00:00Z"));
        let tags = tags.iter().map(|t| t.to_string()).collect();
        estimate(&s, &tags, size, &Settings::default()).minutes
    }

    #[test]
    fn tag_median_needs_five_samples() {
        let mut log = TestLog::new();
        for _ in 0..4 {
            done(&mut log, &["rust"], Size::M, 50);
        }
        assert_eq!(pick(&log, &["rust"], Size::M), 30);
        done(&mut log, &["rust"], Size::M, 50);
        assert_eq!(pick(&log, &["rust"], Size::M), 50);
        assert_eq!(pick(&log, &["go"], Size::M), 50);
        assert_eq!(pick(&log, &["rust"], Size::S), 15);
    }

    #[test]
    fn falls_back_to_size_wide_median_across_tags() {
        let mut log = TestLog::new();
        for tag in ["a", "b", "c", "d", "e"] {
            done(&mut log, &[tag], Size::M, 40);
        }
        assert_eq!(pick(&log, &["a"], Size::M), 40);
    }

    #[test]
    fn tag_with_more_samples_wins() {
        let mut log = TestLog::new();
        for _ in 0..5 {
            done(&mut log, &["x"], Size::L, 90);
        }
        for _ in 0..6 {
            done(&mut log, &["y"], Size::L, 120);
        }
        assert_eq!(pick(&log, &["x", "y"], Size::L), 120);
    }

    #[test]
    fn error_is_mean_absolute_log_ratio() {
        let mk = |actual: f64| Sample {
            item: ItemId::from_uuid(uuid::Uuid::from_u128(1)),
            size: Size::M,
            estimate_minutes: 30,
            actual_minutes: actual,
            tags: vec![],
            settled_at: ts("2026-11-20T01:00:00Z"),
        };
        let s = [mk(60.0), mk(15.0)];
        assert!((error(&s).unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(error(&[]), None);
    }

    #[test]
    fn add_item_uses_the_learned_minutes() {
        use crate::command::{AddItem, Command, Context, execute};
        let mut log = TestLog::new();
        for _ in 0..5 {
            done(&mut log, &["rust"], Size::M, 50);
        }
        let ctx = Context {
            now: "2026-11-24T09:00:00+08:00[Asia/Shanghai]".parse().unwrap(),
            source: Source::Cli,
            settings: Settings::default(),
        };
        let cmd = Command::AddItem(Box::new(AddItem {
            title: "new".into(),
            identity: Some("work".into()),
            parent: None,
            tags: vec!["rust".into()],
            defer: None,
            scheduled: None,
            deadline: None,
            waiting_for: None,
            follow_up: None,
            important: false,
            size: Some(Size::M),
            repeat: None,
        }));
        let state = log.state();
        let out = execute(&state, &log.events, log.device, &cmd, &ctx).unwrap();
        let EventData::ItemCreated(c) = &out.events[0].data else {
            panic!("expected item.created");
        };
        assert_eq!(c.estimate.map(|e| e.minutes), Some(50));
    }
}
