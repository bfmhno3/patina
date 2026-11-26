//! Proposals: a reviewable batch of plan changes made by a restricted client. Pure types and
//! helpers; the commands that submit and accept them live in [`crate::command`].

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::command::{AddItem, Command, CommandError, Edit, Link, Move, Note, Unlink, Update};
use crate::ids::ItemId;
use crate::resolve;
use crate::state::State;

/// One step of a proposal.
///
/// Item references may be `@<key>`, naming an `add_item` step earlier in the same proposal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProposalOp {
    /// Create an item, to be referred to as `@key` by later steps.
    AddItem {
        /// The key, `^[A-Za-z0-9_-]{1,32}$`, unique within the proposal.
        key: String,
        /// The item.
        item: AddItem,
    },
    /// Edit an item.
    Edit(Box<Edit>),
    /// Add a note.
    Note(Note),
    /// Add a dependency.
    Link(Link),
    /// Remove a dependency.
    Unlink(Unlink),
    /// Reparent an item.
    Move(Move),
}

/// Calls `f` on every item reference of `op`.
fn visit_refs<E>(
    op: &mut ProposalOp,
    mut f: impl FnMut(&mut String) -> Result<(), E>,
) -> Result<(), E> {
    match op {
        ProposalOp::AddItem { item, .. } => {
            if let Some(p) = item.parent.as_mut() {
                f(p)?;
            }
        }
        ProposalOp::Edit(e) => f(&mut e.item)?,
        ProposalOp::Note(n) => f(&mut n.item)?,
        ProposalOp::Link(l) => {
            f(&mut l.item)?;
            f(&mut l.depends_on)?;
        }
        ProposalOp::Unlink(u) => {
            f(&mut u.item)?;
            f(&mut u.depends_on)?;
        }
        ProposalOp::Move(m) => {
            f(&mut m.item)?;
            if let Some(p) = m.parent.as_mut() {
                f(p)?;
            }
        }
    }
    Ok(())
}

/// The keys `op` refers to with `@key` references.
fn ref_keys(op: &ProposalOp) -> Vec<String> {
    let mut keys = Vec::new();
    let mut copy = op.clone();
    let _ = visit_refs::<()>(&mut copy, |r| {
        if let Some(k) = r.strip_prefix('@') {
            keys.push(k.to_string());
        }
        Ok(())
    });
    keys
}

/// Steps that are accepted or left out together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Indices into the proposal's steps.
    pub ops: Vec<usize>,
    /// Earlier units this one needs.
    pub needs: Vec<usize>,
}

/// Groups `ops` into units.
///
/// Every `add_item` starts a unit. A step that references keys joins the unit of the key whose
/// `add_item` comes latest; the other referenced units become needs of that unit. A step
/// without keys is a unit of its own. Needs always point to earlier units.
pub fn units(ops: &[ProposalOp]) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::new();
    let mut key_unit: HashMap<String, usize> = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        let referenced: BTreeSet<usize> = ref_keys(op)
            .iter()
            .filter_map(|k| key_unit.get(k).copied())
            .collect();
        if let ProposalOp::AddItem { key, .. } = op {
            key_unit.insert(key.clone(), out.len());
            out.push(Unit {
                ops: vec![i],
                needs: referenced.into_iter().collect(),
            });
            continue;
        }
        match referenced.iter().next_back().copied() {
            None => out.push(Unit {
                ops: vec![i],
                needs: Vec::new(),
            }),
            Some(latest) => {
                out[latest].ops.push(i);
                for u in referenced {
                    if u != latest && !out[latest].needs.contains(&u) {
                        out[latest].needs.push(u);
                    }
                }
                out[latest].needs.sort_unstable();
            }
        }
    }
    out
}

/// `picked` plus everything it needs, transitively.
pub fn with_needs(units: &[Unit], picked: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let mut stack: Vec<usize> = picked.iter().copied().collect();
    while let Some(u) = stack.pop() {
        if u < units.len() && out.insert(u) {
            stack.extend(units[u].needs.iter().copied());
        }
    }
    out
}

/// The units whose needs closure contains `index` (including `index` itself).
pub fn dependents(units: &[Unit], index: usize) -> BTreeSet<usize> {
    (0..units.len())
        .filter(|u| with_needs(units, &BTreeSet::from([*u])).contains(&index))
        .collect()
}

/// The command for `op`, with every `@key` replaced by the id of the item it created.
///
/// # Errors
/// [`CommandError::UnknownProposalKey`] when a key has no created item.
pub fn to_command(
    op: &ProposalOp,
    keys: &HashMap<String, ItemId>,
) -> Result<Command, CommandError> {
    let mut op = op.clone();
    visit_refs::<CommandError>(&mut op, |r| {
        if let Some(k) = r.strip_prefix('@') {
            let id = keys
                .get(k)
                .ok_or_else(|| CommandError::UnknownProposalKey { key: k.to_string() })?;
            *r = id.to_string();
        }
        Ok(())
    })?;
    Ok(match op {
        ProposalOp::AddItem { item, .. } => Command::AddItem(Box::new(item)),
        ProposalOp::Edit(e) => Command::Edit(e),
        ProposalOp::Note(n) => Command::Note(n),
        ProposalOp::Link(l) => Command::Link(l),
        ProposalOp::Unlink(u) => Command::Unlink(u),
        ProposalOp::Move(m) => Command::Move(m),
    })
}

fn valid_key(key: &str) -> bool {
    (1..=32).contains(&key.len())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Checks keys and `@key` references, and rewrites every other item and identity reference
/// to a full id, so the stored proposal means the same thing later.
pub(crate) fn normalize(
    state: &State,
    ops: &[ProposalOp],
) -> Result<Vec<ProposalOp>, CommandError> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        let step = |error: CommandError| CommandError::ProposalStep {
            step: i + 1,
            error: Box::new(error),
        };
        let mut op = op.clone();
        visit_refs::<CommandError>(&mut op, |r| {
            if let Some(k) = r.strip_prefix('@') {
                if !seen.contains(k) {
                    return Err(CommandError::UnknownProposalKey { key: k.to_string() });
                }
            } else {
                *r = resolve::item(state, r)?.to_string();
            }
            Ok(())
        })
        .map_err(step)?;
        let identity = match &mut op {
            ProposalOp::AddItem { item, .. } => item.identity.as_mut(),
            ProposalOp::Edit(e) => e.identity.as_mut(),
            _ => None,
        };
        if let Some(name) = identity {
            *name = resolve::identity(state, name)
                .map_err(|e| step(e.into()))?
                .to_string();
        }
        if let ProposalOp::AddItem { key, .. } = &op {
            if !valid_key(key) {
                return Err(step(CommandError::InvalidProposalKey { key: key.clone() }));
            }
            if !seen.insert(key.clone()) {
                return Err(step(CommandError::DuplicateProposalKey {
                    key: key.clone(),
                }));
            }
        }
        out.push(op);
    }
    Ok(out)
}

/// One changed field.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FieldChange {
    /// Field key.
    pub field: &'static str,
    /// Value before (`null` when unset or not applicable).
    pub old: Value,
    /// Value after.
    pub new: Value,
}

/// What a step does, with references replaced by titles and names.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum ChangeView {
    /// A new item.
    AddItem {
        /// Key later steps use.
        key: String,
        /// Title.
        title: String,
        /// Parent title.
        parent: Option<String>,
        /// Identity name.
        identity: Option<String>,
        /// The other fields that are set.
        fields: Vec<FieldChange>,
    },
    /// Changed fields of an item.
    Edit {
        /// Item title.
        item: String,
        /// The changes.
        fields: Vec<FieldChange>,
    },
    /// A note.
    Note {
        /// Item title.
        item: String,
        /// Note text.
        text: String,
    },
    /// A new dependency.
    Link {
        /// The dependent.
        item: String,
        /// The prerequisite.
        depends_on: String,
    },
    /// A removed dependency.
    Unlink {
        /// The dependent.
        item: String,
        /// The former prerequisite.
        depends_on: String,
    },
    /// A reparenting.
    Move {
        /// Item title.
        item: String,
        /// New parent title; none is top level.
        parent: Option<String>,
    },
}

fn label_item(state: &State, ops: &[ProposalOp], r: &str) -> String {
    if let Some(k) = r.strip_prefix('@') {
        return ops
            .iter()
            .find_map(|op| match op {
                ProposalOp::AddItem { key, item } if key == k => Some(item.title.clone()),
                _ => None,
            })
            .unwrap_or_else(|| r.to_string());
    }
    r.parse::<uuid::Uuid>()
        .ok()
        .and_then(|u| state.items.get(&ItemId::from_uuid(u)))
        .map_or_else(|| r.to_string(), |i| i.title.clone())
}

fn label_identity(state: &State, r: &str) -> String {
    r.parse::<uuid::Uuid>()
        .ok()
        .and_then(|u| state.identities.get(&crate::ids::IdentityId::from_uuid(u)))
        .map_or_else(|| r.to_string(), |i| i.name.clone())
}

fn update_json<T: Serialize>(u: &Update<T>) -> Value {
    match u {
        Update::Set(v) => json!(v),
        Update::Clear => Value::Null,
    }
}

fn add_item_fields(item: &AddItem) -> Vec<FieldChange> {
    let mut out = Vec::new();
    let mut set = |field: &'static str, new: Value| {
        out.push(FieldChange {
            field,
            old: Value::Null,
            new,
        });
    };
    if !item.tags.is_empty() {
        set("tags", json!(item.tags));
    }
    if let Some(d) = item.defer {
        set("defer", json!(d));
    }
    if let Some(d) = item.scheduled {
        set("scheduled", json!(d));
    }
    if let Some(d) = item.deadline {
        set("deadline", json!(d));
    }
    if let Some(d) = item.follow_up {
        set("follow_up", json!(d));
    }
    if let Some(w) = &item.waiting_for {
        set("waiting_for", json!(w));
    }
    if item.important {
        set("important", json!(true));
    }
    if let Some(s) = item.size {
        set("size", json!(s));
    }
    if let Some(r) = item.repeat {
        set("repeat", json!(r));
    }
    out
}

fn edit_fields(state: &State, e: &Edit) -> Vec<FieldChange> {
    let current = resolve::item(state, &e.item)
        .ok()
        .and_then(|id| state.items.get(&id));
    let mut out = Vec::new();
    let mut put = |field: &'static str, old: Value, new: Value| {
        out.push(FieldChange { field, old, new });
    };
    if let Some(t) = &e.title {
        put("title", json!(current.map(|i| &i.title)), json!(t));
    }
    if let Some(id) = &e.identity {
        let old = current
            .and_then(|i| i.identity)
            .and_then(|i| state.identities.get(&i))
            .map(|i| i.name.clone());
        put("identity", json!(old), json!(label_identity(state, id)));
    }
    let tags: Vec<&String> = current.map(|i| i.tags.iter().collect()).unwrap_or_default();
    if !e.add_tags.is_empty() {
        put("tags_added", json!(tags), json!(e.add_tags));
    }
    if !e.remove_tags.is_empty() {
        put("tags_removed", json!(tags), json!(e.remove_tags));
    }
    if let Some(u) = &e.defer {
        put(
            "defer",
            json!(current.and_then(|i| i.defer)),
            update_json(u),
        );
    }
    if let Some(u) = &e.scheduled {
        put(
            "scheduled",
            json!(current.and_then(|i| i.scheduled)),
            update_json(u),
        );
    }
    if let Some(u) = &e.deadline {
        put(
            "deadline",
            json!(current.and_then(|i| i.deadline)),
            update_json(u),
        );
    }
    if let Some(u) = &e.follow_up {
        put(
            "follow_up",
            json!(current.and_then(|i| i.follow_up)),
            update_json(u),
        );
    }
    if let Some(u) = &e.waiting_for {
        put(
            "waiting_for",
            json!(current.and_then(|i| i.waiting_for.clone())),
            update_json(u),
        );
    }
    if let Some(b) = e.important {
        put("important", json!(current.map(|i| i.important)), json!(b));
    }
    if let Some(u) = &e.size {
        put(
            "size",
            json!(current.and_then(|i| i.estimate).map(|s| s.size)),
            update_json(u),
        );
    }
    if let Some(u) = &e.repeat {
        put(
            "repeat",
            json!(current.and_then(|i| i.repeat)),
            update_json(u),
        );
    }
    out
}

/// The changes of the steps in `unit`, in step order.
pub fn changes(state: &State, ops: &[ProposalOp], unit: &Unit) -> Vec<ChangeView> {
    let label = |r: &str| label_item(state, ops, r);
    unit.ops
        .iter()
        .filter_map(|i| ops.get(*i))
        .map(|op| match op {
            ProposalOp::AddItem { key, item } => ChangeView::AddItem {
                key: key.clone(),
                title: item.title.clone(),
                parent: item.parent.as_deref().map(label),
                identity: item.identity.as_deref().map(|i| label_identity(state, i)),
                fields: add_item_fields(item),
            },
            ProposalOp::Edit(e) => ChangeView::Edit {
                item: label(&e.item),
                fields: edit_fields(state, e),
            },
            ProposalOp::Note(n) => ChangeView::Note {
                item: label(&n.item),
                text: n.text.clone(),
            },
            ProposalOp::Link(l) => ChangeView::Link {
                item: label(&l.item),
                depends_on: label(&l.depends_on),
            },
            ProposalOp::Unlink(u) => ChangeView::Unlink {
                item: label(&u.item),
                depends_on: label(&u.depends_on),
            },
            ProposalOp::Move(m) => ChangeView::Move {
                item: label(&m.item),
                parent: m.parent.as_deref().map(label),
            },
        })
        .collect()
}

/// Plain text of a field value, shared by the CLI and the GUI.
pub fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "–".to_string(),
        Value::Bool(true) => "✓".to_string(),
        Value::Bool(false) => "✗".to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a.iter().map(value_text).collect::<Vec<_>>().join(", "),
        Value::Object(_) => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(key: &str, title: &str, parent: Option<&str>) -> ProposalOp {
        ProposalOp::AddItem {
            key: key.into(),
            item: AddItem {
                title: title.into(),
                identity: None,
                parent: parent.map(String::from),
                tags: vec![],
                defer: None,
                scheduled: None,
                deadline: None,
                waiting_for: None,
                follow_up: None,
                important: false,
                size: None,
                repeat: None,
            },
        }
    }

    fn link(a: &str, b: &str) -> ProposalOp {
        ProposalOp::Link(Link {
            item: a.into(),
            depends_on: b.into(),
        })
    }

    #[test]
    fn units_group_a_step_with_the_latest_key_it_names() {
        let ops = vec![
            add("a", "A", Some("P")),
            add("b", "B", Some("P")),
            link("@b", "@a"),
            add("c", "C", Some("@a")),
        ];
        let u = units(&ops);
        assert_eq!(u.len(), 3);
        assert_eq!((u[0].ops.clone(), u[0].needs.clone()), (vec![0], vec![]));
        assert_eq!(
            (u[1].ops.clone(), u[1].needs.clone()),
            (vec![1, 2], vec![0])
        );
        assert_eq!((u[2].ops.clone(), u[2].needs.clone()), (vec![3], vec![0]));
        assert_eq!(with_needs(&u, &BTreeSet::from([1])), BTreeSet::from([0, 1]));
        assert_eq!(dependents(&u, 0), BTreeSet::from([0, 1, 2]));
        assert_eq!(dependents(&u, 1), BTreeSet::from([1]));
    }

    #[test]
    fn ops_round_trip_as_tagged_json() {
        let text = r#"{"op":"add_item","key":"a","item":{"title":"Write tests","parent":"7f3a"}}"#;
        let op: ProposalOp = serde_json::from_str(text).unwrap();
        assert!(matches!(&op, ProposalOp::AddItem { key, .. } if key == "a"));
        let link: ProposalOp =
            serde_json::from_str(r#"{"op":"link","item":"@b","depends_on":"@a"}"#).unwrap();
        assert_eq!(link, super::tests::link("@b", "@a"));
    }
}
