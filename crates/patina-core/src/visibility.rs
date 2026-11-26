//! What an MCP client may see: an allowlist of identities, applied by redacting the state
//! before any query runs. Pure.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::access::McpLevel;
use crate::dag;
use crate::event::{Source, TimerTarget};
use crate::ids::{IdentityId, ItemId};
use crate::mode;
use crate::state::{ItemStatus, State};

/// MCP exposure settings (`[mcp]`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpSettings {
    /// Identity names whose items, habits, and time MCP clients may read.
    pub visible_identities: Vec<String>,
    /// What a client without an entry in `clients` may do.
    pub default_level: McpLevel,
    /// Access level per MCP client name.
    pub clients: BTreeMap<String, McpLevel>,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            visible_identities: vec!["life".into(), "study".into()],
            default_level: McpLevel::Propose,
            clients: BTreeMap::new(),
        }
    }
}

/// Identities whose trimmed, lowercased name equals a trimmed, lowercased entry of `names`.
pub fn visible_ids(state: &State, names: &[String]) -> BTreeSet<IdentityId> {
    let wanted: BTreeSet<String> = names.iter().map(|n| n.trim().to_lowercase()).collect();
    state
        .identities
        .values()
        .filter(|i| wanted.contains(&i.name.trim().to_lowercase()))
        .map(|i| i.id)
        .collect()
}

/// A copy of `state` without anything that belongs to an identity outside `visible`.
///
/// Dependency edges stay, so an item whose prerequisite disappeared remains blocked
/// without revealing the prerequisite.
pub fn redact(state: &State, visible: &BTreeSet<IdentityId>) -> State {
    let mut out = state.clone();
    out.identities.retain(|id, _| visible.contains(id));

    let hidden_items: BTreeSet<ItemId> = state
        .items
        .values()
        .filter(|item| {
            if item.status == ItemStatus::Discarded {
                return true;
            }
            let hidden = |identity: Option<IdentityId>| match identity {
                Some(id) => !visible.contains(&id),
                None => false,
            };
            if hidden(item.identity)
                || dag::ancestors(state, item.id)
                    .into_iter()
                    .any(|a| hidden(state.items.get(&a).and_then(|a| a.identity)))
            {
                return true;
            }
            item.identity.is_none()
                && !mode::mode_at(state, item.created_at).is_some_and(|m| visible.contains(&m))
        })
        .map(|i| i.id)
        .collect();
    out.items.retain(|id, _| !hidden_items.contains(id));
    for item in out.items.values_mut() {
        item.children.retain(|c| !hidden_items.contains(c));
    }
    out.habits.retain(|_, h| visible.contains(&h.identity));

    let target_gone = |t: &TimerTarget| match t {
        TimerTarget::Item(id) => !out.items.contains_key(id),
        TimerTarget::Habit(id) => !out.habits.contains_key(id),
    };
    let dropped: Vec<_> = out
        .segments
        .values()
        .filter(|s| target_gone(&s.target))
        .map(|s| s.id)
        .collect();
    for id in dropped {
        out.segments.remove(&id);
        if out.running == Some(id) {
            out.running = None;
        }
    }

    for m in &mut out.modes {
        if m.identity.is_some_and(|i| !visible.contains(&i)) {
            m.identity = None;
        }
    }
    out.suggestion_dismissals
        .retain(|id, _| visible.contains(id));
    out.proposals
        .retain(|_, p| matches!(p.source, Source::Mcp { .. }));
    out.interruptions
        .retain(|i| out.items.contains_key(&i.item));
    for r in &mut out.reviews {
        r.focus.retain(|f| out.items.contains_key(f));
        r.conclusion = None;
    }
    out.last_shown
        .retain(|_, s| s.item.is_none_or(|i| out.items.contains_key(&i)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::ids::SegmentId;
    use crate::testutil::TestLog;

    #[test]
    fn hidden_identities_vanish_and_blocked_stays_truthful() {
        let mut log = TestLog::new();
        let work = log.identity("work");
        let life = log.identity("Life");
        log.push(EventData::ModeSwitched(ModeSwitched {
            identity: Some(work),
        }));
        let secret = log.item("secret", work, None);
        let mixed_child = log.item("mixed child", life, Some(secret));
        let milk = log.item("milk", life, None);
        log.edge(milk, secret);
        let capture = ItemId::from_uuid(log.uuid());
        log.push(EventData::ItemCaptured(ItemCaptured {
            item: capture,
            text: "work thought".into(),
        }));
        let habit = log.habit("deep", work, Frequency::Daily, "2026-11-01");
        let segment = SegmentId::from_uuid(log.uuid());
        log.push(EventData::TimerStarted(TimerStarted {
            segment,
            target: TimerTarget::Item(secret),
        }));
        let state = log.state();

        let visible = visible_ids(&state, &["life".to_string(), " STUDY ".to_string()]);
        assert_eq!(visible, BTreeSet::from([life]));
        let red = redact(&state, &visible);

        assert!(!red.identities.contains_key(&work));
        assert!(!red.items.contains_key(&secret));
        assert!(!red.items.contains_key(&mixed_child));
        assert!(!red.items.contains_key(&capture));
        assert!(!red.habits.contains_key(&habit));
        assert!(red.running.is_none() && red.segments.is_empty());
        assert!(red.modes.iter().all(|m| m.identity.is_none()));
        assert!(red.items.contains_key(&milk));

        let now: jiff::ZonedDateTime = "2026-11-24T09:00:00+08:00[Asia/Shanghai]".parse().unwrap();
        let out = crate::query::run(
            &crate::query::Query::List { identity: None },
            &red,
            &log.events,
            &[],
            &now,
            &crate::time::Settings::default(),
        )
        .unwrap();
        // The only item left is blocked by the removed one, so nothing is actionable.
        let crate::query::QueryOutput::List(items) = out else {
            panic!("expected a list");
        };
        assert!(items.is_empty());
        let shown = crate::query::run(
            &crate::query::Query::Show {
                item: "milk".into(),
            },
            &red,
            &log.events,
            &[],
            &now,
            &crate::time::Settings::default(),
        )
        .unwrap();
        let crate::query::QueryOutput::Show(detail) = shown else {
            panic!("expected a detail");
        };
        assert_eq!(detail.item.blocked_by.len(), 1);
        assert!(detail.prerequisites.is_empty());
    }
}
