//! Simple recommendation: one card at a time, scored from weighted terms. Pure.

use std::collections::HashSet;

use jiff::{SignedDuration, Timestamp, ZonedDateTime};
use serde::{Deserialize, Serialize};

use crate::dag;
use crate::error::TimeError;
use crate::event::Size;
use crate::ids::{IdentityId, ItemId};
use crate::query::{Banner, ItemView, ModeView, TimerView, View};
use crate::review;
use crate::state::{ItemStatus, State};
use crate::time::{Settings, logical_day};

/// Which kind of card was shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// Do this item.
    Item,
    /// Big and vague: break it down first.
    BreakDown,
    /// The weekly review is due.
    Review,
    /// The inbox needs triage.
    Triage,
    /// Nothing to recommend.
    Empty,
}

/// Why an item was recommended; stable keys, no UI text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reason {
    /// Deadline close (negative days: overdue).
    Deadline {
        /// Days until the deadline.
        days: i64,
    },
    /// Planned for today or earlier.
    Scheduled {
        /// Days past the planned day.
        days_late: i64,
    },
    /// Completing it makes other items actionable.
    Unlocks {
        /// How many.
        count: u32,
    },
    /// Open items depend on it, directly or not.
    Downstream {
        /// How many.
        count: u32,
    },
    /// In the weekly focus.
    Focus,
    /// Marked important.
    Important,
    /// Small enough to finish quickly.
    Small {
        /// The size.
        size: Size,
    },
    /// Skipped recently.
    RecentlySkipped {
        /// How many times within the cooldown.
        count: u32,
    },
    /// Forced as the first step of a split.
    FirstStep,
}

/// How many actionable items another identity has.
#[derive(Clone, Debug, Serialize)]
pub struct ElsewhereCount {
    /// Identity name.
    pub identity: String,
    /// Actionable items.
    pub actionable: usize,
}

/// The recommendation card.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Card {
    /// Do this item.
    Item {
        /// The item.
        item: ItemView,
        /// Its score; none when forced.
        score: Option<f64>,
        /// Why.
        reasons: Vec<Reason>,
    },
    /// Break this item down first.
    BreakDown {
        /// The item.
        item: ItemView,
        /// Its score; none when forced.
        score: Option<f64>,
        /// Why.
        reasons: Vec<Reason>,
    },
    /// Do the weekly review.
    Review {
        /// When the last review was recorded.
        since: Option<Timestamp>,
    },
    /// Triage the inbox.
    Triage {
        /// Inbox size.
        inbox: usize,
    },
    /// Nothing actionable in scope.
    Empty {
        /// Actionable items of other identities.
        elsewhere: Vec<ElsewhereCount>,
    },
}

impl Card {
    /// The stable kind key of the card.
    pub fn kind(&self) -> CardKind {
        match self {
            Card::Item { .. } => CardKind::Item,
            Card::BreakDown { .. } => CardKind::BreakDown,
            Card::Review { .. } => CardKind::Review,
            Card::Triage { .. } => CardKind::Triage,
            Card::Empty { .. } => CardKind::Empty,
        }
    }
}

/// A card with the context shown beside it.
#[derive(Clone, Debug, Serialize)]
pub struct NextView {
    /// The logical day.
    pub today: jiff::civil::Date,
    /// The identity the card is scoped to.
    pub scope: Option<String>,
    /// The card.
    pub card: Card,
    /// Urgent items of the other identities.
    pub banner: Banner,
    /// Current identity mode.
    pub mode: ModeView,
    /// The running timer.
    pub timer: Option<TimerView>,
}

/// The next card for `scope` at `now`.
///
/// `forced` makes that item the card (the first step after a split).
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub fn next(
    state: &State,
    now: &ZonedDateTime,
    settings: &Settings,
    scope: Option<IdentityId>,
    forced: Option<ItemId>,
) -> Result<NextView, TimeError> {
    let today = logical_day(now, settings)?;
    let v = View::new(state, settings, today, now);
    let card = decide(&v, scope, forced)?;
    Ok(NextView {
        today,
        scope: v.identity_name(scope),
        card,
        banner: v.banner(scope)?,
        mode: v.mode_view(),
        timer: v.timer_view(),
    })
}

fn decide(
    v: &View<'_>,
    scope: Option<IdentityId>,
    forced: Option<ItemId>,
) -> Result<Card, TimeError> {
    let (state, settings, today) = (v.state, v.settings, v.today);
    let tz = v.now.time_zone();
    let ts = v.now.timestamp();
    let cooldown = SignedDuration::from_hours(i64::from(settings.skip_cooldown_hours));
    // A skip counts while it is strictly younger than the cooldown.
    let recent = |t: Timestamp| ts.duration_since(t) < cooldown;

    if let Some(item) = forced
        .and_then(|id| state.items.get(&id))
        .filter(|i| i.status == ItemStatus::Todo)
    {
        return Ok(Card::Item {
            item: v.item(item),
            score: None,
            reasons: vec![Reason::FirstStep],
        });
    }
    let skipped = |kind: CardKind| state.card_skips.get(&kind).is_some_and(|t| recent(*t));
    if review::due(state, today, tz, settings)? && !skipped(CardKind::Review) {
        return Ok(Card::Review {
            since: review::since(state),
        });
    }
    let inbox = state
        .items
        .values()
        .filter(|i| i.status == ItemStatus::Inbox)
        .count();
    if inbox >= settings.inbox_threshold && !skipped(CardKind::Triage) {
        return Ok(Card::Triage { inbox });
    }

    let candidates = v.actionable(scope);
    if candidates.is_empty() {
        let mut elsewhere: Vec<ElsewhereCount> = state
            .identities
            .values()
            .filter(|i| Some(i.id) != scope)
            .filter_map(|i| {
                let n = v.actionable(Some(i.id)).len();
                (n > 0).then(|| ElsewhereCount {
                    identity: i.name.clone(),
                    actionable: n,
                })
            })
            .collect();
        elsewhere.sort_by(|a, b| a.identity.cmp(&b.identity));
        return Ok(Card::Empty { elsewhere });
    }

    let frontier: HashSet<ItemId> = dag::frontier(state, today).into_iter().collect();
    let focus: HashSet<ItemId> = state
        .reviews
        .last()
        .map(|r| r.focus.iter().copied().collect())
        .unwrap_or_default();
    let w = &settings.recommend;
    let mut best: Option<(f64, Vec<Reason>, &crate::state::Item)> = None;
    for item in candidates {
        let mut score = 0.0;
        let mut reasons = Vec::new();
        let mut add = |term: f64, reason: Reason| {
            if term != 0.0 {
                score += term;
                reasons.push(reason);
            }
        };
        if let Some(d) = item.deadline {
            let days = i64::from(today.until(d).map_err(TimeError::from)?.get_days());
            if days <= 7 {
                let decay = if days <= 0 {
                    1.0
                } else {
                    1.0 / (1.0 + days as f64)
                };
                add(w.deadline * decay, Reason::Deadline { days });
            }
        }
        if let Some(s) = item.scheduled.filter(|s| *s <= today) {
            let days_late = i64::from(s.until(today).map_err(TimeError::from)?.get_days());
            add(w.scheduled, Reason::Scheduled { days_late });
        }
        let unlocks = dag::unlocks(state, item.id, today, &frontier);
        if unlocks > 0 {
            add(
                w.unlock * f64::from(unlocks),
                Reason::Unlocks { count: unlocks },
            );
        }
        let downstream = dag::downstream(state, item.id);
        if downstream > 0 {
            add(
                w.downstream * f64::from(downstream),
                Reason::Downstream { count: downstream },
            );
        }
        if std::iter::once(item.id)
            .chain(dag::ancestors(state, item.id))
            .any(|a| focus.contains(&a))
        {
            add(w.focus, Reason::Focus);
        }
        if item.important {
            add(w.important, Reason::Important);
        }
        if let Some(e) = item.estimate {
            match e.size {
                Size::S => add(w.size_s, Reason::Small { size: Size::S }),
                Size::M => add(w.size_m, Reason::Small { size: Size::M }),
                Size::L => {}
            }
        }
        let skips =
            u32::try_from(item.skips.iter().filter(|t| recent(**t)).count()).unwrap_or(u32::MAX);
        if skips > 0 {
            add(
                w.skip * f64::from(skips),
                Reason::RecentlySkipped { count: skips },
            );
        }
        if best.as_ref().is_none_or(|(s, ..)| score > *s) {
            best = Some((score, reasons, item));
        }
    }
    let Some((score, reasons, item)) = best else {
        return Ok(Card::Empty {
            elsewhere: Vec::new(),
        });
    };
    let big = item.estimate.is_some_and(|e| e.size == Size::L)
        && item.children.is_empty()
        && !dag::is_in_progress(state, item.id);
    let view = v.item(item);
    Ok(if big {
        Card::BreakDown {
            item: view,
            score: Some(score),
            reasons,
        }
    } else {
        Card::Item {
            item: view,
            score: Some(score),
            reasons,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_serialize_as_stable_keys() {
        let json = serde_json::to_string(&[
            Reason::Deadline { days: 0 },
            Reason::Small { size: Size::S },
            Reason::FirstStep,
        ])
        .unwrap();
        assert_eq!(
            json,
            r#"[{"kind":"deadline","days":0},{"kind":"small","size":"s"},{"kind":"first_step"}]"#
        );
        assert_eq!(
            serde_json::to_string(&CardKind::BreakDown).unwrap(),
            "\"break_down\""
        );
    }
}
