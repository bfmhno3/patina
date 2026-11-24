//! Projection of the event log into current state.
//!
//! [`State::apply`] is the single validation path: replay turns its errors into
//! warnings, commands turn them into refusals.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use jiff::Timestamp;
use jiff::civil::Date;
use serde::Serialize;
use uuid::Uuid;

use crate::event::{AbandonReason, Category, Event, EventData, Frequency, HabitKind};
use crate::ids::{CommandId, EventId, HabitId, IdentityId, ItemId};

/// An event that cannot be applied to the current state.
#[derive(Clone, Debug, PartialEq, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Rejection {
    /// An id that must be new already exists.
    #[error("id {0} already exists")]
    DuplicateId(Uuid),
    /// No such item.
    #[error("unknown item {0}")]
    UnknownItem(ItemId),
    /// No such habit.
    #[error("unknown habit {0}")]
    UnknownHabit(HabitId),
    /// No such identity.
    #[error("unknown identity {0}")]
    UnknownIdentity(IdentityId),
    /// No such command in the log.
    #[error("unknown command {0}")]
    UnknownCommand(CommandId),
    /// Identity names are unique, case-insensitively.
    #[error("identity name {0:?} is already taken")]
    IdentityNameTaken(String),
    /// Habit names are unique, case-insensitively.
    #[error("habit name {0:?} is already taken")]
    HabitNameTaken(String),
    /// Text or name is empty.
    #[error("text must not be empty")]
    EmptyText,
    /// The item's status does not allow this change.
    #[error("item {item} is {status:?}")]
    WrongStatus {
        /// The item.
        item: ItemId,
        /// Its status.
        status: ItemStatus,
    },
    /// Children are still open.
    #[error("item {item} has open children")]
    OpenChildren {
        /// The parent.
        item: ItemId,
        /// The open children.
        children: Vec<ItemId>,
    },
    /// Parent must be an open task.
    #[error("parent {0} is not an open task")]
    ParentNotOpen(ItemId),
    /// External nodes cannot have children.
    #[error("parent {0} is an external node")]
    ParentIsExternal(ItemId),
    /// Items with children cannot become external.
    #[error("item {0} has children and cannot be external")]
    ExternalWithChildren(ItemId),
    /// A move would create a parent loop.
    #[error("cannot move {0} into its own subtree")]
    MoveIntoOwnSubtree(ItemId),
    /// Items with dependency edges cannot move.
    #[error("item {0} has dependency edges; remove them before moving")]
    HasEdges(ItemId),
    /// An item cannot depend on itself.
    #[error("item {0} cannot depend on itself")]
    SelfDependency(ItemId),
    /// Edges only connect siblings.
    #[error("items {item} and {depends_on} are not siblings")]
    NotSiblings {
        /// Dependent item.
        item: ItemId,
        /// Prerequisite.
        depends_on: ItemId,
    },
    /// Edge exists already.
    #[error("{item} already depends on {depends_on}")]
    DuplicateEdge {
        /// Dependent item.
        item: ItemId,
        /// Prerequisite.
        depends_on: ItemId,
    },
    /// Edge does not exist.
    #[error("{item} does not depend on {depends_on}")]
    UnknownEdge {
        /// Dependent item.
        item: ItemId,
        /// Prerequisite.
        depends_on: ItemId,
    },
    /// The edge would close a cycle.
    #[error("dependency cycle")]
    Cycle {
        /// Items along the cycle, first equal to last.
        path: Vec<ItemId>,
    },
    /// Bad frequency or target.
    #[error("invalid habit definition")]
    InvalidHabit,
    /// Count presence does not match the habit kind.
    #[error("count does not match the kind of habit {0}")]
    CountMismatch(HabitId),
    /// Habit already paused.
    #[error("habit {0} is already paused")]
    AlreadyPaused(HabitId),
    /// Habit not paused.
    #[error("habit {0} is not paused")]
    NotPaused(HabitId),
    /// Pause or resume date out of order.
    #[error("pause or resume date out of order for habit {0}")]
    PauseOrder(HabitId),
    /// Undo of an undo command.
    #[error("command {0} is an undo and cannot be undone")]
    UndoOfUndo(CommandId),
    /// Command already undone.
    #[error("command {0} is already undone")]
    AlreadyUndone(CommandId),
}

/// An identity (a way of being: work, life, study).
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    /// Id.
    pub id: IdentityId,
    /// Display name.
    pub name: String,
}

/// Stored item state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Captured, not yet triaged.
    Inbox,
    /// Open task.
    Todo,
    /// Completed.
    Done,
    /// Abandoned.
    Abandoned,
    /// Discarded from the inbox.
    Discarded,
}

/// A dated note.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    /// When it was added.
    pub at: Timestamp,
    /// Text.
    pub text: String,
}

/// A task, composite node, or inbox entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Id.
    pub id: ItemId,
    /// Title (inbox: the captured text).
    pub title: String,
    /// Identity; `None` while in the inbox.
    pub identity: Option<IdentityId>,
    /// Stored status.
    pub status: ItemStatus,
    /// Work has started.
    pub started: bool,
    /// Parent node.
    pub parent: Option<ItemId>,
    /// Children in creation/move order.
    pub children: Vec<ItemId>,
    /// Tags.
    pub tags: BTreeSet<String>,
    /// Hidden until this day.
    pub defer: Option<Date>,
    /// Planned day.
    pub scheduled: Option<Date>,
    /// Due day.
    pub deadline: Option<Date>,
    /// External node: what is being waited for.
    pub waiting_for: Option<String>,
    /// Follow-up day for external nodes.
    pub follow_up: Option<Date>,
    /// Sibling prerequisites.
    pub depends_on: BTreeSet<ItemId>,
    /// Notes.
    pub notes: Vec<Note>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Completion or abandonment time.
    pub settled_at: Option<Timestamp>,
    /// Note given on completion.
    pub completion_note: Option<String>,
    /// Abandon reason and note.
    pub abandon: Option<(AbandonReason, Option<String>)>,
}

impl Item {
    /// Waiting on something outside.
    pub fn is_external(&self) -> bool {
        self.waiting_for.is_some()
    }

    /// Done, abandoned, or discarded.
    pub fn is_settled(&self) -> bool {
        matches!(
            self.status,
            ItemStatus::Done | ItemStatus::Abandoned | ItemStatus::Discarded
        )
    }
}

/// One habit check-in.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckIn {
    /// Count for count habits.
    pub count: Option<u32>,
    /// Note.
    pub note: Option<String>,
    /// Entered after the day.
    pub backfilled: bool,
}

/// A habit.
#[derive(Clone, Debug, PartialEq)]
pub struct Habit {
    /// Id.
    pub id: HabitId,
    /// Name.
    pub name: String,
    /// Identity.
    pub identity: IdentityId,
    /// Kind.
    pub kind: HabitKind,
    /// Frequency.
    pub frequency: Frequency,
    /// Cue.
    pub cue: Option<String>,
    /// First day.
    pub start: Date,
    /// Pause intervals `[from, resume)`.
    pub pauses: Vec<(Date, Option<Date>)>,
    /// Check-ins by day.
    pub checkins: BTreeMap<Date, CheckIn>,
    /// Skipped days.
    pub skips: BTreeSet<Date>,
}

/// An event that was skipped during projection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Warning {
    /// The skipped event.
    pub event: EventId,
    /// Why.
    pub rejection: Rejection,
}

/// One identity-mode switch (`None` is away).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSwitch {
    /// Effective time of the switch.
    pub at: Timestamp,
    /// The identity switched to.
    pub identity: Option<IdentityId>,
}

/// Projected state.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// Identities.
    pub identities: BTreeMap<IdentityId, Identity>,
    /// Items.
    pub items: BTreeMap<ItemId, Item>,
    /// Habits.
    pub habits: BTreeMap<HabitId, Habit>,
    /// Events skipped during projection.
    pub warnings: Vec<Warning>,
    /// Recorded time of every event, in log order (idle detection and active days).
    pub activity: Vec<Timestamp>,
    /// Mode switches sorted by effective time.
    pub modes: Vec<ModeSwitch>,
}

fn habit_shape_ok(kind: &HabitKind, frequency: &Frequency) -> bool {
    let kind_ok = !matches!(kind, HabitKind::Count { target: 0 });
    let freq_ok = match frequency {
        Frequency::Daily => true,
        Frequency::Weekly { times } => (1..=7).contains(times),
        Frequency::Weekdays { days } => {
            let set: BTreeSet<_> = days.iter().collect();
            !days.is_empty() && set.len() == days.len()
        }
    };
    kind_ok && freq_ok
}

fn clean(text: &str) -> Result<(), Rejection> {
    if text.trim().is_empty() {
        Err(Rejection::EmptyText)
    } else {
        Ok(())
    }
}

impl State {
    /// The item with `id`.
    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.get(&id)
    }

    fn item_ok(&self, id: ItemId) -> Result<&Item, Rejection> {
        self.items.get(&id).ok_or(Rejection::UnknownItem(id))
    }

    fn open_item(&self, id: ItemId) -> Result<&Item, Rejection> {
        let item = self.item_ok(id)?;
        if item.status == ItemStatus::Todo {
            Ok(item)
        } else {
            Err(Rejection::WrongStatus {
                item: id,
                status: item.status,
            })
        }
    }

    fn habit_ok(&self, id: HabitId) -> Result<&Habit, Rejection> {
        self.habits.get(&id).ok_or(Rejection::UnknownHabit(id))
    }

    fn identity_ok(&self, id: IdentityId) -> Result<(), Rejection> {
        if self.identities.contains_key(&id) {
            Ok(())
        } else {
            Err(Rejection::UnknownIdentity(id))
        }
    }

    fn identity_name_ok(&self, name: &str, exclude: Option<IdentityId>) -> Result<(), Rejection> {
        clean(name)?;
        let lower = name.trim().to_lowercase();
        let taken = self
            .identities
            .values()
            .any(|i| Some(i.id) != exclude && i.name.trim().to_lowercase() == lower);
        if taken {
            Err(Rejection::IdentityNameTaken(name.trim().to_string()))
        } else {
            Ok(())
        }
    }

    fn habit_name_ok(&self, name: &str, exclude: Option<HabitId>) -> Result<(), Rejection> {
        clean(name)?;
        let lower = name.trim().to_lowercase();
        let taken = self
            .habits
            .values()
            .any(|h| Some(h.id) != exclude && h.name.trim().to_lowercase() == lower);
        if taken {
            Err(Rejection::HabitNameTaken(name.trim().to_string()))
        } else {
            Ok(())
        }
    }

    /// True when `candidate` is `ancestor` or lies below it.
    fn in_subtree(&self, ancestor: ItemId, candidate: ItemId) -> bool {
        let mut seen = HashSet::new();
        let mut cur = Some(candidate);
        while let Some(id) = cur {
            if id == ancestor {
                return true;
            }
            if !seen.insert(id) {
                return false;
            }
            cur = self.items.get(&id).and_then(|i| i.parent);
        }
        false
    }

    /// Path `from -> ... -> to` along `depends_on` edges.
    fn dependency_path(&self, from: ItemId, to: ItemId) -> Option<Vec<ItemId>> {
        let mut seen = HashSet::new();
        let mut stack = vec![vec![from]];
        while let Some(path) = stack.pop() {
            let last = *path.last()?;
            if last == to {
                return Some(path);
            }
            if !seen.insert(last) {
                continue;
            }
            if let Some(item) = self.items.get(&last) {
                for dep in &item.depends_on {
                    let mut next = path.clone();
                    next.push(*dep);
                    stack.push(next);
                }
            }
        }
        None
    }

    fn open_children(&self, item: &Item) -> Vec<ItemId> {
        item.children
            .iter()
            .copied()
            .filter(|c| self.items.get(c).is_some_and(|c| !c.is_settled()))
            .collect()
    }

    /// Applies one event; on error the state is unchanged.
    ///
    /// # Errors
    /// Returns a [`Rejection`] when the event does not fit the current state.
    pub fn apply(&mut self, event: &Event) -> Result<(), Rejection> {
        match &event.data {
            EventData::IdentityCreated(d) => {
                if self.identities.contains_key(&d.identity) {
                    return Err(Rejection::DuplicateId(d.identity.as_uuid()));
                }
                self.identity_name_ok(&d.name, None)?;
                self.identities.insert(
                    d.identity,
                    Identity {
                        id: d.identity,
                        name: d.name.trim().to_string(),
                    },
                );
            }
            EventData::IdentityRenamed(d) => {
                if !self.identities.contains_key(&d.identity) {
                    return Err(Rejection::UnknownIdentity(d.identity));
                }
                self.identity_name_ok(&d.new, Some(d.identity))?;
                if let Some(i) = self.identities.get_mut(&d.identity) {
                    i.name = d.new.trim().to_string();
                }
            }
            EventData::ItemCaptured(d) => {
                if self.items.contains_key(&d.item) {
                    return Err(Rejection::DuplicateId(d.item.as_uuid()));
                }
                clean(&d.text)?;
                self.items
                    .insert(d.item, new_item(d.item, d.text.clone(), event.effective_at));
            }
            EventData::ItemCreated(d) => {
                if self.items.contains_key(&d.item) {
                    return Err(Rejection::DuplicateId(d.item.as_uuid()));
                }
                clean(&d.title)?;
                self.identity_ok(d.identity)?;
                if let Some(w) = &d.waiting_for {
                    clean(w)?;
                }
                if let Some(p) = d.parent {
                    let parent = self.item_ok(p)?;
                    if parent.status != ItemStatus::Todo {
                        return Err(Rejection::ParentNotOpen(p));
                    }
                    if parent.is_external() {
                        return Err(Rejection::ParentIsExternal(p));
                    }
                }
                let mut item = new_item(d.item, d.title.clone(), event.effective_at);
                item.status = ItemStatus::Todo;
                item.identity = Some(d.identity);
                item.parent = d.parent;
                item.tags = d.tags.iter().cloned().collect();
                item.defer = d.defer;
                item.scheduled = d.scheduled;
                item.deadline = d.deadline;
                item.waiting_for = d.waiting_for.clone();
                item.follow_up = d.follow_up;
                if let Some(p) = d.parent.and_then(|p| self.items.get_mut(&p)) {
                    p.children.push(d.item);
                }
                self.items.insert(d.item, item);
            }
            EventData::ItemTriaged(d) => {
                let item = self.item_ok(d.item)?;
                if item.status != ItemStatus::Inbox {
                    return Err(Rejection::WrongStatus {
                        item: d.item,
                        status: item.status,
                    });
                }
                self.identity_ok(d.identity)?;
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.status = ItemStatus::Todo;
                    i.identity = Some(d.identity);
                }
            }
            EventData::ItemEdited(d) => {
                let item = self.item_ok(d.item)?;
                if item.status == ItemStatus::Discarded {
                    return Err(Rejection::WrongStatus {
                        item: d.item,
                        status: item.status,
                    });
                }
                if let Some(c) = &d.title {
                    clean(&c.new)?;
                }
                if let Some(c) = &d.identity {
                    self.identity_ok(c.new)?;
                }
                if let Some(c) = &d.waiting_for
                    && let Some(w) = &c.new
                {
                    clean(w)?;
                    if !item.children.is_empty() {
                        return Err(Rejection::ExternalWithChildren(d.item));
                    }
                }
                if let Some(i) = self.items.get_mut(&d.item) {
                    if let Some(c) = &d.title {
                        i.title = c.new.trim().to_string();
                    }
                    if let Some(c) = &d.identity {
                        i.identity = Some(c.new);
                    }
                    for t in &d.tags_added {
                        i.tags.insert(t.clone());
                    }
                    for t in &d.tags_removed {
                        i.tags.remove(t);
                    }
                    if let Some(c) = &d.defer {
                        i.defer = c.new;
                    }
                    if let Some(c) = &d.scheduled {
                        i.scheduled = c.new;
                    }
                    if let Some(c) = &d.deadline {
                        i.deadline = c.new;
                    }
                    if let Some(c) = &d.follow_up {
                        i.follow_up = c.new;
                    }
                    if let Some(c) = &d.waiting_for {
                        i.waiting_for = c.new.clone();
                    }
                }
            }
            EventData::ItemNoteAdded(d) => {
                let item = self.item_ok(d.item)?;
                if item.status == ItemStatus::Discarded {
                    return Err(Rejection::WrongStatus {
                        item: d.item,
                        status: item.status,
                    });
                }
                clean(&d.text)?;
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.notes.push(Note {
                        at: event.effective_at,
                        text: d.text.clone(),
                    });
                }
            }
            EventData::ItemStarted(d) => {
                self.open_item(d.item)?;
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.started = true;
                }
            }
            EventData::ItemCompleted(d) => {
                let item = self.open_item(d.item)?;
                let open = self.open_children(item);
                if !open.is_empty() {
                    return Err(Rejection::OpenChildren {
                        item: d.item,
                        children: open,
                    });
                }
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.status = ItemStatus::Done;
                    i.settled_at = Some(event.effective_at);
                    i.completion_note = d.note.clone();
                }
            }
            EventData::ItemAbandoned(d) => {
                let item = self.open_item(d.item)?;
                let open = self.open_children(item);
                if !open.is_empty() {
                    return Err(Rejection::OpenChildren {
                        item: d.item,
                        children: open,
                    });
                }
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.status = ItemStatus::Abandoned;
                    i.settled_at = Some(event.effective_at);
                    i.abandon = Some((d.reason, d.note.clone()));
                }
            }
            EventData::ItemDiscarded(d) => {
                let item = self.item_ok(d.item)?;
                if item.status != ItemStatus::Inbox {
                    return Err(Rejection::WrongStatus {
                        item: d.item,
                        status: item.status,
                    });
                }
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.status = ItemStatus::Discarded;
                }
            }
            EventData::ItemMoved(d) => {
                let item = self.open_item(d.item)?;
                let has_dependents = self.items.values().any(|o| o.depends_on.contains(&d.item));
                if !item.depends_on.is_empty() || has_dependents {
                    return Err(Rejection::HasEdges(d.item));
                }
                if let Some(p) = d.new_parent {
                    let parent = self.item_ok(p)?;
                    if parent.status != ItemStatus::Todo {
                        return Err(Rejection::ParentNotOpen(p));
                    }
                    if parent.is_external() {
                        return Err(Rejection::ParentIsExternal(p));
                    }
                    if self.in_subtree(d.item, p) {
                        return Err(Rejection::MoveIntoOwnSubtree(d.item));
                    }
                }
                let old = item.parent;
                if let Some(p) = old.and_then(|p| self.items.get_mut(&p)) {
                    p.children.retain(|c| *c != d.item);
                }
                if let Some(p) = d.new_parent.and_then(|p| self.items.get_mut(&p)) {
                    p.children.push(d.item);
                }
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.parent = d.new_parent;
                }
            }
            EventData::EdgeAdded(d) => {
                let item = self.item_ok(d.item)?;
                let dep = self.item_ok(d.depends_on)?;
                if d.item == d.depends_on {
                    return Err(Rejection::SelfDependency(d.item));
                }
                if item.status != ItemStatus::Todo {
                    return Err(Rejection::WrongStatus {
                        item: d.item,
                        status: item.status,
                    });
                }
                if !matches!(
                    dep.status,
                    ItemStatus::Todo | ItemStatus::Done | ItemStatus::Abandoned
                ) {
                    return Err(Rejection::WrongStatus {
                        item: d.depends_on,
                        status: dep.status,
                    });
                }
                if item.parent != dep.parent {
                    return Err(Rejection::NotSiblings {
                        item: d.item,
                        depends_on: d.depends_on,
                    });
                }
                if item.depends_on.contains(&d.depends_on) {
                    return Err(Rejection::DuplicateEdge {
                        item: d.item,
                        depends_on: d.depends_on,
                    });
                }
                if let Some(back) = self.dependency_path(d.depends_on, d.item) {
                    let mut path = vec![d.item];
                    path.extend(back);
                    return Err(Rejection::Cycle { path });
                }
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.depends_on.insert(d.depends_on);
                }
            }
            EventData::EdgeRemoved(d) => {
                let item = self.item_ok(d.item)?;
                if !item.depends_on.contains(&d.depends_on) {
                    return Err(Rejection::UnknownEdge {
                        item: d.item,
                        depends_on: d.depends_on,
                    });
                }
                if let Some(i) = self.items.get_mut(&d.item) {
                    i.depends_on.remove(&d.depends_on);
                }
            }
            EventData::HabitCreated(d) => {
                if self.habits.contains_key(&d.habit) {
                    return Err(Rejection::DuplicateId(d.habit.as_uuid()));
                }
                self.habit_name_ok(&d.name, None)?;
                self.identity_ok(d.identity)?;
                if !habit_shape_ok(&d.kind, &d.frequency) {
                    return Err(Rejection::InvalidHabit);
                }
                self.habits.insert(
                    d.habit,
                    Habit {
                        id: d.habit,
                        name: d.name.trim().to_string(),
                        identity: d.identity,
                        kind: d.kind,
                        frequency: d.frequency.clone(),
                        cue: d.cue.clone(),
                        start: d.start,
                        pauses: Vec::new(),
                        checkins: BTreeMap::new(),
                        skips: BTreeSet::new(),
                    },
                );
            }
            EventData::HabitEdited(d) => {
                self.habit_ok(d.habit)?;
                if let Some(c) = &d.name {
                    self.habit_name_ok(&c.new, Some(d.habit))?;
                }
                if let Some(c) = &d.identity {
                    self.identity_ok(c.new)?;
                }
                if let Some(h) = self.habits.get_mut(&d.habit) {
                    if let Some(c) = &d.name {
                        h.name = c.new.trim().to_string();
                    }
                    if let Some(c) = &d.identity {
                        h.identity = c.new;
                    }
                    if let Some(c) = &d.cue {
                        h.cue = c.new.clone();
                    }
                }
            }
            EventData::HabitCheckedIn(d) => {
                let h = self.habit_ok(d.habit)?;
                if d.count.is_some() != matches!(h.kind, HabitKind::Count { .. }) {
                    return Err(Rejection::CountMismatch(d.habit));
                }
                if let Some(h) = self.habits.get_mut(&d.habit) {
                    h.checkins.insert(
                        d.day,
                        CheckIn {
                            count: d.count,
                            note: d.note.clone(),
                            backfilled: d.backfilled,
                        },
                    );
                }
            }
            EventData::HabitSkipped(d) => {
                self.habit_ok(d.habit)?;
                if let Some(h) = self.habits.get_mut(&d.habit) {
                    h.skips.insert(d.day);
                }
            }
            EventData::HabitPaused(d) => {
                let h = self.habit_ok(d.habit)?;
                match h.pauses.last() {
                    Some((_, None)) => return Err(Rejection::AlreadyPaused(d.habit)),
                    Some((_, Some(resume))) if d.from < *resume => {
                        return Err(Rejection::PauseOrder(d.habit));
                    }
                    _ => {}
                }
                if let Some(h) = self.habits.get_mut(&d.habit) {
                    h.pauses.push((d.from, None));
                }
            }
            EventData::HabitResumed(d) => {
                let h = self.habit_ok(d.habit)?;
                match h.pauses.last() {
                    Some((from, None)) if d.from > *from => {}
                    Some((_, None)) => return Err(Rejection::PauseOrder(d.habit)),
                    _ => return Err(Rejection::NotPaused(d.habit)),
                }
                if let Some(last) = self
                    .habits
                    .get_mut(&d.habit)
                    .and_then(|h| h.pauses.last_mut())
                {
                    last.1 = Some(d.from);
                }
            }
            EventData::ModeSwitched(d) => {
                if let Some(id) = d.identity {
                    self.identity_ok(id)?;
                }
                let at = event.effective_at;
                let pos = self.modes.partition_point(|m| m.at <= at);
                self.modes.insert(
                    pos,
                    ModeSwitch {
                        at,
                        identity: d.identity,
                    },
                );
            }
            EventData::CommandUndone(_) => {}
        }
        Ok(())
    }
}

fn new_item(id: ItemId, title: String, created_at: Timestamp) -> Item {
    Item {
        id,
        title: title.trim().to_string(),
        identity: None,
        status: ItemStatus::Inbox,
        started: false,
        parent: None,
        children: Vec::new(),
        tags: BTreeSet::new(),
        defer: None,
        scheduled: None,
        deadline: None,
        waiting_for: None,
        follow_up: None,
        depends_on: BTreeSet::new(),
        notes: Vec::new(),
        created_at,
        settled_at: None,
        completion_note: None,
        abandon: None,
    }
}

/// Projects the log into state. Pure and time-independent; never panics.
pub fn project(events: &[Event]) -> State {
    let mut state = State::default();
    let mut seen: HashSet<CommandId> = HashSet::new();
    let mut undo_commands: HashSet<CommandId> = HashSet::new();
    let mut undone: HashSet<CommandId> = HashSet::new();
    for e in events {
        if let EventData::CommandUndone(u) = &e.data {
            undo_commands.insert(e.command);
            let problem = if undo_commands.contains(&u.command) {
                Some(Rejection::UndoOfUndo(u.command))
            } else if !seen.contains(&u.command) {
                Some(Rejection::UnknownCommand(u.command))
            } else if undone.contains(&u.command) {
                Some(Rejection::AlreadyUndone(u.command))
            } else {
                None
            };
            match problem {
                Some(rejection) => state.warnings.push(Warning {
                    event: e.id,
                    rejection,
                }),
                None => {
                    undone.insert(u.command);
                }
            }
        }
        seen.insert(e.command);
    }
    for e in events {
        state.activity.push(e.recorded_at);
        let undone_domain = undone.contains(&e.command) && e.category == Category::Domain;
        if matches!(e.data, EventData::CommandUndone(_)) || undone_domain {
            continue;
        }
        if let Err(rejection) = state.apply(e) {
            state.warnings.push(Warning {
                event: e.id,
                rejection,
            });
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::testutil::TestLog;

    #[test]
    fn cycle_edge_is_rejected_with_path() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let a = log.item("a", id, None);
        let b = log.item("b", id, None);
        let c = log.item("c", id, None);
        log.edge(b, a);
        log.edge(c, b);
        let mut state = log.state();
        let mut probe = TestLog::new();
        probe.push(EventData::EdgeAdded(EdgeAdded {
            item: a,
            depends_on: c,
        }));
        match state.apply(&probe.events[0]) {
            Err(Rejection::Cycle { path }) => assert_eq!(path, vec![a, c, b, a]),
            other => panic!("expected cycle, got {other:?}"),
        }
    }

    #[test]
    fn cyclic_log_projects_with_one_warning() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let a = log.item("a", id, None);
        let b = log.item("b", id, None);
        log.edge(b, a);
        log.edge(a, b);
        let state = log.state();
        assert_eq!(state.warnings.len(), 1);
        assert!(matches!(
            state.warnings[0].rejection,
            Rejection::Cycle { .. }
        ));
        assert_eq!(state.items.len(), 2);
    }
}
