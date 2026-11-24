//! The minimal weekly review: due date, postponement counts, and the review summary. Pure.

use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::dag;
use crate::engine::Error;
use crate::error::TimeError;
use crate::query::{InboxEntry, ItemView, View};
use crate::state::{Item, ItemStatus, State};
use crate::time::{Settings, last_on_or_before, logical_day};

/// When the last review was recorded.
pub fn since(state: &State) -> Option<Timestamp> {
    state.reviews.last().map(|r| r.at)
}

fn day_of(ts: Timestamp, tz: &TimeZone, settings: &Settings) -> Result<Date, TimeError> {
    logical_day(&ts.to_zoned_datetime(tz.clone()), settings)
}

/// A review is due when the latest review day has come and no review happened on or after it.
///
/// Nothing is due before the first recorded activity reaches a review day.
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub fn due(
    state: &State,
    today: Date,
    tz: &TimeZone,
    settings: &Settings,
) -> Result<bool, TimeError> {
    let occurrence = last_on_or_before(today, settings.review_day)?;
    let Some(first) = state.activity.iter().min() else {
        return Ok(false);
    };
    if day_of(*first, tz, settings)? > occurrence {
        return Ok(false);
    }
    for r in &state.reviews {
        if day_of(r.at, tz, settings)? >= occurrence {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Logical days on which any event was recorded.
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub fn active_days(
    state: &State,
    tz: &TimeZone,
    settings: &Settings,
) -> Result<BTreeSet<Date>, TimeError> {
    state
        .activity
        .iter()
        .map(|a| day_of(*a, tz, settings))
        .collect()
}

/// How often the plan for `item` slipped: active days past its scheduled day since it was
/// last replanned. Days the app was not used do not count.
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub fn postponements(
    item: &Item,
    today: Date,
    active: &BTreeSet<Date>,
    tz: &TimeZone,
    settings: &Settings,
) -> Result<u32, TimeError> {
    let Some(scheduled) = item.scheduled else {
        return Ok(0);
    };
    let from = scheduled.max(day_of(item.replanned_at, tz, settings)?);
    if from >= today {
        return Ok(0);
    }
    let days = active.range(from..today).count();
    Ok(u32::try_from(days).unwrap_or(u32::MAX))
}

/// Items completed in the window, per identity.
#[derive(Clone, Debug, Serialize)]
pub struct CompletedGroup {
    /// Identity name.
    pub identity: String,
    /// Items by completion time.
    pub items: Vec<ItemView>,
}

/// Progress of a project (a top-level item with children) in the window.
#[derive(Clone, Debug, Serialize)]
pub struct ProjectProgress {
    /// The project.
    pub project: ItemView,
    /// Leaves completed in the window.
    pub completed: usize,
    /// Leaves done in total.
    pub done: usize,
    /// Leaves still open.
    pub open: usize,
}

/// An item postponed again and again.
#[derive(Clone, Debug, Serialize)]
pub struct PostponedView {
    /// The item.
    pub item: ItemView,
    /// Times postponed.
    pub count: u32,
}

/// An external item that has waited long.
#[derive(Clone, Debug, Serialize)]
pub struct WaitingView {
    /// The item.
    pub item: ItemView,
    /// Days of waiting.
    pub days: i64,
}

/// The weekly review summary.
#[derive(Clone, Debug, Serialize)]
pub struct ReviewView {
    /// The logical day.
    pub today: Date,
    /// When the last review was recorded.
    pub since: Option<Timestamp>,
    /// A review is due.
    pub due: bool,
    /// Done in the window, per identity.
    pub completed: Vec<CompletedGroup>,
    /// Project progress in the window.
    pub projects: Vec<ProjectProgress>,
    /// Items postponed at least the threshold.
    pub postponed: Vec<PostponedView>,
    /// External items waiting past the alert age.
    pub waiting: Vec<WaitingView>,
    /// Untriaged captures.
    pub inbox: Vec<InboxEntry>,
    /// Last review's focus items that are still open.
    pub focus: Vec<ItemView>,
}

/// Builds the review summary.
///
/// # Errors
/// [`Error`] on date arithmetic overflow.
pub(crate) fn view(v: &View<'_>) -> Result<ReviewView, Error> {
    let (state, settings, today) = (v.state, v.settings, v.today);
    let tz = v.now.time_zone();
    let now = v.now.timestamp();
    let since = since(state);
    let in_window =
        |t: Option<Timestamp>| t.is_some_and(|t| since.is_none_or(|s| t > s) && t <= now);

    let mut done: Vec<&Item> = state
        .items
        .values()
        .filter(|i| i.status == ItemStatus::Done && in_window(i.settled_at))
        .collect();
    done.sort_by_key(|i| i.settled_at);
    let mut groups: BTreeMap<String, Vec<ItemView>> = BTreeMap::new();
    for i in done {
        let name = v.identity_name(i.identity).unwrap_or_default();
        groups.entry(name).or_default().push(v.item(i));
    }
    let completed = groups
        .into_iter()
        .map(|(identity, items)| CompletedGroup { identity, items })
        .collect();

    let mut projects = Vec::new();
    for p in state
        .items
        .values()
        .filter(|i| i.parent.is_none() && !i.children.is_empty())
        .filter(|i| i.status != ItemStatus::Discarded)
    {
        let leaves: Vec<&Item> = dag::subtree(state, p.id)
            .into_iter()
            .filter(|id| *id != p.id)
            .filter_map(|id| state.items.get(&id))
            .filter(|i| i.children.is_empty() && i.status != ItemStatus::Discarded)
            .collect();
        let completed = leaves
            .iter()
            .filter(|i| i.status == ItemStatus::Done && in_window(i.settled_at))
            .count();
        if completed == 0 {
            continue;
        }
        projects.push(ProjectProgress {
            project: v.item(p),
            completed,
            done: leaves
                .iter()
                .filter(|i| i.status == ItemStatus::Done)
                .count(),
            open: leaves
                .iter()
                .filter(|i| i.status == ItemStatus::Todo)
                .count(),
        });
    }

    let active = active_days(state, tz, settings)?;
    let mut postponed = Vec::new();
    for i in state
        .items
        .values()
        .filter(|i| i.status == ItemStatus::Todo)
    {
        let count = postponements(i, today, &active, tz, settings)?;
        if count >= settings.postpone_threshold {
            postponed.push((count, i));
        }
    }
    postponed.sort_by_key(|(count, i)| (std::cmp::Reverse(*count), i.created_at));
    let postponed = postponed
        .into_iter()
        .map(|(count, i)| PostponedView {
            item: v.item(i),
            count,
        })
        .collect();

    let mut waiting = Vec::new();
    for i in state
        .items
        .values()
        .filter(|i| i.status == ItemStatus::Todo && i.is_external())
    {
        let Some(ws) = i.waiting_since else { continue };
        let wait = day_of(ws, tz, settings)?
            .until(today)
            .map_err(TimeError::from)?;
        let days = i64::from(wait.get_days());
        if days >= i64::from(settings.waiting_alert_days) {
            waiting.push((days, i));
        }
    }
    waiting.sort_by_key(|(days, i)| (std::cmp::Reverse(*days), i.created_at));
    let waiting = waiting
        .into_iter()
        .map(|(days, i)| WaitingView {
            item: v.item(i),
            days,
        })
        .collect();

    let focus = state.reviews.last().map_or_else(Vec::new, |r| {
        v.items(r.focus.iter().copied().filter(|id| {
            state
                .items
                .get(id)
                .is_some_and(|i| i.status == ItemStatus::Todo)
        }))
    });

    Ok(ReviewView {
        today,
        since,
        due: due(state, today, tz, settings)?,
        completed,
        projects,
        postponed,
        waiting,
        inbox: v.inbox_entries(),
        focus,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::testutil::{TestLog, date};

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn tz() -> TimeZone {
        TimeZone::get("Asia/Shanghai").unwrap()
    }

    fn edit_scheduled(log: &mut TestLog, item: crate::ids::ItemId, day: &str, at: &str) {
        log.push_at(
            EventData::ItemEdited(ItemEdited {
                item,
                title: None,
                identity: None,
                tags_added: vec![],
                tags_removed: vec![],
                defer: None,
                scheduled: Some(Change {
                    old: None,
                    new: Some(date(day)),
                }),
                deadline: None,
                follow_up: None,
                waiting_for: None,
                important: None,
                estimate: None,
                repeat: None,
            }),
            ts(at),
            ts(at),
        );
    }

    fn touch(log: &mut TestLog, id: crate::ids::IdentityId, at: &str) {
        log.push_at(
            EventData::IdentityRenamed(IdentityRenamed {
                identity: id,
                old: "work".into(),
                new: "work".into(),
            }),
            ts(at),
            ts(at),
        );
    }

    #[test]
    fn only_days_the_app_was_used_count_as_postponements() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let item = log.item("report", id, None);
        // Scheduled for 2026-11-20; replanned (edited) at 2026-11-20 09:00 local.
        edit_scheduled(&mut log, item, "2026-11-20", "2026-11-20T01:00:00Z");
        // App used on the 21st and 23rd, not on the 22nd; today is the 24th.
        touch(&mut log, id, "2026-11-21T02:00:00Z");
        touch(&mut log, id, "2026-11-23T02:00:00Z");
        let state = log.state();
        let active = active_days(&state, &tz(), &Settings::default()).unwrap();
        let n = postponements(
            &state.items[&item],
            date("2026-11-24"),
            &active,
            &tz(),
            &Settings::default(),
        )
        .unwrap();
        // The 20th itself (the edit) was an active day too, so 20, 21, 23 count.
        assert_eq!(n, 3);
        let none = postponements(
            &state.items[&item],
            date("2026-11-21"),
            &active,
            &tz(),
            &Settings::default(),
        )
        .unwrap();
        assert_eq!(none, 1);
    }

    #[test]
    fn review_is_due_on_review_day_until_recorded() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        touch(&mut log, id, "2026-11-17T02:00:00Z");
        let state = log.state();
        let s = Settings::default();
        // 2026-11-24 is a Tuesday: the last Sunday (22nd) has passed with no review.
        assert!(due(&state, date("2026-11-24"), &tz(), &s).unwrap());
        // Before the first activity reached a Sunday, nothing is due.
        assert!(!due(&state, date("2026-11-16"), &tz(), &s).unwrap());
        log.push_at(
            EventData::ReviewCompleted(ReviewCompleted {
                conclusion: None,
                focus: vec![],
            }),
            ts("2026-11-22T12:00:00Z"),
            ts("2026-11-22T12:00:00Z"),
        );
        let state = log.state();
        assert!(!due(&state, date("2026-11-24"), &tz(), &s).unwrap());
        // The following Sunday it is due again.
        assert!(due(&state, date("2026-11-29"), &tz(), &s).unwrap());
    }
}
