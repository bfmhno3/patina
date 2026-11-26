//! Dependency and hierarchy queries over a projected state. Pure.

use std::collections::{HashMap, HashSet};

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
    unmet_with(state, id, None)
}

/// [`unmet_prerequisites`] pretending `assume_done` is done.
fn unmet_with(state: &State, id: ItemId, assume_done: Option<ItemId>) -> Vec<ItemId> {
    let mut out = Vec::new();
    for i in std::iter::once(id).chain(ancestors(state, id)) {
        let Some(item) = state.items.get(&i) else {
            continue;
        };
        for dep in &item.depends_on {
            let done = Some(*dep) == assume_done
                || state
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
    if item.started || item.timed {
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

/// `id` and all its descendants, loop-safe, parents before children.
pub fn subtree(state: &State, id: ItemId) -> Vec<ItemId> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![id];
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        out.push(i);
        if let Some(item) = state.items.get(&i) {
            stack.extend(item.children.iter().rev().copied());
        }
    }
    out
}

/// The layer of each of `ids` within the dependency graph restricted to `ids`.
///
/// A node without prerequisites among `ids` is layer 0, otherwise one more than its
/// deepest prerequisite. An edge that would close a cycle is ignored.
pub fn layers(state: &State, ids: &[ItemId]) -> HashMap<ItemId, u32> {
    fn layer_of(
        state: &State,
        id: ItemId,
        set: &HashSet<ItemId>,
        memo: &mut HashMap<ItemId, u32>,
        stack: &mut HashSet<ItemId>,
    ) -> u32 {
        if let Some(l) = memo.get(&id) {
            return *l;
        }
        stack.insert(id);
        let mut layer = 0;
        if let Some(item) = state.items.get(&id) {
            for p in &item.depends_on {
                if set.contains(p) && !stack.contains(p) {
                    layer = layer.max(layer_of(state, *p, set, memo, stack) + 1);
                }
            }
        }
        stack.remove(&id);
        memo.insert(id, layer);
        layer
    }
    let set: HashSet<ItemId> = ids.iter().copied().collect();
    let mut memo = HashMap::new();
    let mut stack = HashSet::new();
    for id in ids {
        layer_of(state, *id, &set, &mut memo, &mut stack);
    }
    memo
}

/// Open, non-external items that can be worked on now.
///
/// A composite with open children is never in the frontier; its open leaves are.
pub fn frontier(state: &State, today: Date) -> Vec<ItemId> {
    state
        .items
        .keys()
        .copied()
        .filter(|id| actionable_with(state, *id, today, None))
        .collect()
}

/// The frontier predicate, optionally pretending `assume_done` is already done.
fn actionable_with(state: &State, id: ItemId, today: Date, assume_done: Option<ItemId>) -> bool {
    let Some(i) = state.items.get(&id) else {
        return false;
    };
    if i.status != ItemStatus::Todo || i.is_external() || Some(id) == assume_done {
        return false;
    }
    if hidden_by_defer(state, id, today) || !unmet_with(state, id, assume_done).is_empty() {
        return false;
    }
    i.children.is_empty()
        || i.children
            .iter()
            .all(|c| Some(*c) == assume_done || state.items.get(c).is_none_or(|c| c.is_settled()))
}

/// How many items would join the frontier if `id` were completed.
///
/// Candidates are the items depending on `id`, their descendants, and `id`'s parent
/// (which becomes ready to wrap up when `id` was its last open child).
pub fn unlocks(state: &State, id: ItemId, today: Date, frontier: &HashSet<ItemId>) -> u32 {
    let mut affected: HashSet<ItemId> = HashSet::new();
    for dependent in state
        .items
        .values()
        .filter(|o| o.depends_on.contains(&id))
        .map(|o| o.id)
    {
        affected.extend(subtree(state, dependent));
    }
    if let Some(parent) = state.items.get(&id).and_then(|i| i.parent) {
        affected.insert(parent);
    }
    let count = affected
        .into_iter()
        .filter(|a| *a != id && !frontier.contains(a))
        .filter(|a| actionable_with(state, *a, today, Some(id)))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Size of the transitive reverse-dependency closure of `id` over open items.
pub fn downstream(state: &State, id: ItemId) -> u32 {
    let mut seen: HashSet<ItemId> = HashSet::from([id]);
    let mut stack = vec![id];
    while let Some(cur) = stack.pop() {
        for o in state
            .items
            .values()
            .filter(|o| o.status == ItemStatus::Todo && o.depends_on.contains(&cur))
        {
            if seen.insert(o.id) {
                stack.push(o.id);
            }
        }
    }
    u32::try_from(seen.len() - 1).unwrap_or(u32::MAX)
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
            important: None,
            estimate: None,
            repeat: None,
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

    #[test]
    fn unlocks_counts_what_joins_the_frontier_and_downstream_the_whole_chain() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let a = log.item("a", id, None);
        let b = log.item("b", id, None);
        let c = log.item("c", id, None);
        log.edge(b, a);
        log.edge(c, b);
        let today = date("2026-11-24");
        let s = log.state();
        let front: HashSet<ItemId> = frontier(&s, today).into_iter().collect();
        assert_eq!(front, HashSet::from([a]));
        assert_eq!(unlocks(&s, a, today, &front), 1);
        assert_eq!(unlocks(&s, b, today, &front), 1);
        assert_eq!(unlocks(&s, c, today, &front), 0);
        assert_eq!(downstream(&s, a), 2);
        assert_eq!(downstream(&s, c), 0);
    }

    #[test]
    fn finishing_the_last_open_child_unlocks_the_parents_wrap_up() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let p = log.item("p", id, None);
        let a = log.item("a", id, Some(p));
        let b = log.item("b", id, Some(p));
        log.complete(a);
        let today = date("2026-11-24");
        let s = log.state();
        let front: HashSet<ItemId> = frontier(&s, today).into_iter().collect();
        assert_eq!(front, HashSet::from([b]));
        assert_eq!(unlocks(&s, b, today, &front), 1);
    }

    #[test]
    fn layers_follow_prerequisite_chains_and_survive_cycles() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let a = log.item("a", id, None);
        let b = log.item("b", id, None);
        let c = log.item("c", id, None);
        let d = log.item("d", id, None);
        log.edge(b, a);
        log.edge(c, b);
        let mut s = log.state();
        let l = layers(&s, &[a, b, c, d]);
        assert_eq!((l[&a], l[&b], l[&c], l[&d]), (0, 1, 2, 0));
        // A merged log can hold a cycle that the projection did not reject.
        s.items.get_mut(&a).unwrap().depends_on.insert(c);
        let l = layers(&s, &[a, b, c]);
        assert_eq!(l.len(), 3);
    }
}
