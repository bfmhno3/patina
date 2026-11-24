//! Dependency and hierarchy queries over a projected state. Pure.

use std::collections::HashSet;

use jiff::civil::Date;

use crate::ids::ItemId;
use crate::state::{ItemStatus, State};

/// Ancestors of `id`, parent first. Stops on a parent loop from a bad log.
pub fn ancestors(state: &State, id: ItemId) -> Vec<ItemId> {
    let mut out = Vec::new();
    let mut seen = HashSet::from([id]);
    let mut cur = state.items.get(&id).and_then(|i| i.parent);
    while let Some(p) = cur {
        if !seen.insert(p) {
            break;
        }
        out.push(p);
        cur = state.items.get(&p).and_then(|i| i.parent);
    }
    out
}

/// True when the item or any ancestor is deferred past `today`.
pub fn hidden_by_defer(state: &State, id: ItemId, today: Date) -> bool {
    std::iter::once(id)
        .chain(ancestors(state, id))
        .filter_map(|i| state.items.get(&i))
        .any(|i| i.defer.is_some_and(|d| d > today))
}

/// Prerequisites not done, from the item and every ancestor (blocking is inherited).
pub fn unmet_prerequisites(state: &State, id: ItemId) -> Vec<ItemId> {
    let mut out = Vec::new();
    for i in std::iter::once(id).chain(ancestors(state, id)) {
        let Some(item) = state.items.get(&i) else {
            continue;
        };
        for dep in &item.depends_on {
            let done = state
                .items
                .get(dep)
                .is_some_and(|d| d.status == ItemStatus::Done);
            if !done && !out.contains(dep) {
                out.push(*dep);
            }
        }
    }
    out
}

/// The item's own prerequisites that were abandoned.
pub fn abandoned_prerequisites(state: &State, id: ItemId) -> Vec<ItemId> {
    let Some(item) = state.items.get(&id) else {
        return Vec::new();
    };
    item.depends_on
        .iter()
        .copied()
        .filter(|d| {
            state
                .items
                .get(d)
                .is_some_and(|d| d.status == ItemStatus::Abandoned)
        })
        .collect()
}

/// Has children and all are settled: the composite itself is ready to close.
pub fn is_wrap_up(state: &State, id: ItemId) -> bool {
    state.items.get(&id).is_some_and(|i| {
        !i.children.is_empty()
            && i.children
                .iter()
                .all(|c| state.items.get(c).is_none_or(|c| c.is_settled()))
    })
}

/// Started, or some descendant is done.
pub fn is_in_progress(state: &State, id: ItemId) -> bool {
    let Some(item) = state.items.get(&id) else {
        return false;
    };
    if item.started {
        return true;
    }
    let mut seen = HashSet::from([id]);
    let mut stack = item.children.clone();
    while let Some(c) = stack.pop() {
        if !seen.insert(c) {
            continue;
        }
        if let Some(child) = state.items.get(&c) {
            if child.status == ItemStatus::Done {
                return true;
            }
            stack.extend(child.children.iter().copied());
        }
    }
    false
}

/// Open, non-external items that can be worked on now.
///
/// A composite with open children is never in the frontier; its open leaves are.
pub fn frontier(state: &State, today: Date) -> Vec<ItemId> {
    state
        .items
        .values()
        .filter(|i| i.status == ItemStatus::Todo && !i.is_external())
        .filter(|i| !hidden_by_defer(state, i.id, today))
        .filter(|i| unmet_prerequisites(state, i.id).is_empty())
        .filter(|i| i.children.is_empty() || is_wrap_up(state, i.id))
        .map(|i| i.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::testutil::{TestLog, date};

    fn edit_dates(log: &mut TestLog, item: ItemId, defer: Option<Date>, waiting: Option<&str>) {
        log.push(EventData::ItemEdited(ItemEdited {
            item,
            title: None,
            identity: None,
            tags_added: vec![],
            tags_removed: vec![],
            defer: defer.map(|d| Change {
                old: None,
                new: Some(d),
            }),
            scheduled: None,
            deadline: None,
            follow_up: None,
            waiting_for: waiting.map(|w| Change {
                old: None,
                new: Some(w.to_string()),
            }),
        }));
    }

    #[test]
    fn blocked_child_of_blocked_composite_is_excluded() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let a = log.item("a", id, None);
        let c = log.item("c", id, None);
        let leaf = log.item("leaf", id, Some(c));
        log.edge(c, a);
        let s = log.state();
        assert_eq!(frontier(&s, date("2026-11-24")), vec![a]);
        assert_eq!(unmet_prerequisites(&s, leaf), vec![a]);
        log.complete(a);
        let s = log.state();
        let f = frontier(&s, date("2026-11-24"));
        assert_eq!(f, vec![leaf]);
    }

    #[test]
    fn deferred_ancestor_hides_subtree() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let p = log.item("p", id, None);
        let leaf = log.item("leaf", id, Some(p));
        edit_dates(&mut log, p, Some(date("2026-12-01")), None);
        let s = log.state();
        assert!(hidden_by_defer(&s, leaf, date("2026-11-24")));
        assert!(frontier(&s, date("2026-11-24")).is_empty());
        assert_eq!(frontier(&s, date("2026-12-01")), vec![leaf]);
    }

    #[test]
    fn external_node_is_not_actionable() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let x = log.item("x", id, None);
        edit_dates(&mut log, x, None, Some("mentor"));
        assert!(frontier(&log.state(), date("2026-11-24")).is_empty());
    }

    #[test]
    fn composite_with_all_children_done_is_wrap_up() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let p = log.item("p", id, None);
        let a = log.item("a", id, Some(p));
        assert!(!is_wrap_up(&log.state(), p));
        log.complete(a);
        let s = log.state();
        assert!(is_wrap_up(&s, p));
        assert!(is_in_progress(&s, p));
        assert_eq!(frontier(&s, date("2026-11-24")), vec![p]);
    }

    #[test]
    fn abandoned_prerequisite_keeps_dependent_blocked() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let a = log.item("a", id, None);
        let b = log.item("b", id, None);
        log.edge(b, a);
        log.push(EventData::ItemAbandoned(ItemAbandoned {
            item: a,
            reason: AbandonReason::Other,
            note: None,
        }));
        let s = log.state();
        assert!(frontier(&s, date("2026-11-24")).is_empty());
        assert_eq!(abandoned_prerequisites(&s, b), vec![a]);
    }
}
