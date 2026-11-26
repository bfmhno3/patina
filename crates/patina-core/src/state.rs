//! Projection of the event log into current state.
//!
//! [`State::apply`] is the single validation path: replay turns its errors into
//! warnings, commands turn them into refusals.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use jiff::Timestamp;
use jiff::civil::Date;
use serde::Serialize;
use uuid::Uuid;

use crate::event::{
    AbandonReason, BreakKind, Category, Estimate, Event, EventData, Frequency, HabitKind,
    InterruptOutcome, Repeat, RhythmMode, TimerTarget,
};
use crate::ids::{CommandId, DeviceId, EventId, HabitId, IdentityId, ItemId, SegmentId};
use crate::recommend::CardKind;

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
    /// Habit already graduated.
    #[error("habit {0} has graduated")]
    AlreadyGraduated(HabitId),
    /// Habit not graduated.
    #[error("habit {0} has not graduated")]
    NotGraduated(HabitId),
    /// Graduation or reinstatement date out of order.
    #[error("graduation or reinstatement date out of order for habit {0}")]
    GraduationOrder(HabitId),
    /// Undo of an undo command.
    #[error("command {0} is an undo and cannot be undone")]
    UndoOfUndo(CommandId),
    /// Command already undone.
    #[error("command {0} is already undone")]
    AlreadyUndone(CommandId),
    /// A second timer cannot start while one runs.
    #[error("timer segment {0} is still running")]
    TimerRunning(SegmentId),
    /// External nodes cannot be timed.
    #[error("item {0} is an external node and cannot be timed")]
    TimerOnExternal(ItemId),
    /// Only duration habits can be timed.
    #[error("habit {0} is not a duration habit")]
    NotDurationHabit(HabitId),
    /// A segment starts before an earlier one ended.
    #[error("segment overlaps segment {0}")]
    SegmentOverlap(SegmentId),
    /// No such segment.
    #[error("unknown segment {0}")]
    UnknownSegment(SegmentId),
    /// The segment is not the running one.
    #[error("segment {0} is not running")]
    SegmentNotRunning(SegmentId),
    /// A segment would end before it started.
    #[error("segment {0} would end before it started")]
    SegmentEndBeforeStart(SegmentId),
    /// Repeat interval of zero.
    #[error("repeat intervals must be positive")]
    InvalidRepeat,
    /// Recurring chores are leaves.
    #[error("parent {0} is a recurring item")]
    ParentIsRecurring(ItemId),
    /// Weekly target out of range.
    #[error("weekly target must be 1..=10080 minutes, got {0}")]
    InvalidTarget(u32),
    /// Items with children cannot recur.
    #[error("item {0} has children and cannot recur")]
    RecurringWithChildren(ItemId),
}

/// An identity (a way of being: work, life, study).
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    /// Id.
    pub id: IdentityId,
    /// Display name.
    pub name: String,
    /// Weekly time target in minutes.
    pub weekly_target_minutes: Option<u32>,
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
    /// Some timer segment targets this item.
    pub timed: bool,
    /// Where work stopped when an interruption took over; cleared when timing resumes.
    pub left_off: Option<String>,
    /// Marked important.
    pub important: bool,
    /// Size estimate.
    pub estimate: Option<Estimate>,
    /// Recurrence rule.
    pub repeat: Option<Repeat>,
    /// The occurrence this one followed.
    pub repeat_of: Option<ItemId>,
    /// Last time the item was created or its defer, scheduled, or deadline changed.
    pub replanned_at: Timestamp,
    /// Since when the item has been waiting on something external.
    pub waiting_since: Option<Timestamp>,
    /// Times a recommendation of this item was skipped.
    pub skips: Vec<Timestamp>,
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
    /// Graduation intervals `[from, reinstated)`.
    pub graduations: Vec<(Date, Option<Date>)>,
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

/// The latest recommendation card a device showed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// When it was shown.
    pub at: Timestamp,
    /// Which card.
    pub card: CardKind,
    /// The item on an item card.
    pub item: Option<ItemId>,
    /// The identity scope it was computed for.
    pub scope: Option<IdentityId>,
}

/// A recorded weekly review.
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    /// Effective time.
    pub at: Timestamp,
    /// Conclusion in the owner's words.
    pub conclusion: Option<String>,
    /// Items chosen as the coming week's focus.
    pub focus: Vec<ItemId>,
    /// The minimal (1/5/7) version of the review.
    pub minimal: bool,
}

/// An interruption of a running timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interruption {
    /// Effective time.
    pub at: Timestamp,
    /// The item made for the interruption.
    pub item: ItemId,
    /// What became of it.
    pub outcome: InterruptOutcome,
}

/// One timer segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Id.
    pub id: SegmentId,
    /// What it times.
    pub target: TimerTarget,
    /// Start time.
    pub start: Timestamp,
    /// End time; none while running.
    pub end: Option<Timestamp>,
}

/// One identity-mode switch (`None` is away).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSwitch {
    /// Effective time of the switch.
    pub at: Timestamp,
    /// The identity switched to.
    pub identity: Option<IdentityId>,
}

/// A break that started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Break {
    /// Effective time.
    pub at: Timestamp,
    /// Kind.
    pub kind: BreakKind,
    /// Planned length in minutes.
    pub minutes: u32,
}

/// A snoozed break reminder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snooze {
    /// Effective time.
    pub at: Timestamp,
    /// Minutes the reminder was postponed.
    pub minutes: u32,
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
    /// Timer segments.
    pub segments: BTreeMap<SegmentId, Segment>,
    /// The running segment.
    pub running: Option<SegmentId>,
    /// Weekly reviews sorted by effective time.
    pub reviews: Vec<Review>,
    /// Rhythm mode switches sorted by effective time.
    pub rhythm: Vec<(Timestamp, RhythmMode)>,
    /// Breaks sorted by effective time.
    pub breaks: Vec<Break>,
    /// Snoozed reminders sorted by effective time.
    pub snoozes: Vec<Snooze>,
    /// Interruptions sorted by effective time.
    pub interruptions: Vec<Interruption>,
    /// Latest card shown per device.
    pub last_shown: BTreeMap<DeviceId, Shown>,
    /// Latest skip of the review and triage cards.
    pub card_skips: BTreeMap<CardKind, Timestamp>,
    /// Latest dismissal of a mode suggestion per identity (effective time).
    pub suggestion_dismissals: BTreeMap<IdentityId, Timestamp>,
}

fn habit_shape_ok(kind: &HabitKind, frequency: &Frequency) -> bool {
    let kind_ok = !matches!(
        kind,
        HabitKind::Count { target: 0 } | HabitKind::Duration { minutes: 0 }
    );
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
                        weekly_target_minutes: None,
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
            EventData::IdentityTargetChanged(d) => {
                if let Some(m) = d.new
                    && !(1..=10080).contains(&m)
                {
                    return Err(Rejection::InvalidTarget(m));
                }
                let identity = self
                    .identities
                    .get_mut(&d.identity)
                    .ok_or(Rejection::UnknownIdentity(d.identity))?;
                identity.weekly_target_minutes = d.new;
            }
            EventData::RhythmSwitched(d) => {
                let at = event.effective_at;
                let pos = self.rhythm.partition_point(|(t, _)| *t <= at);
                self.rhythm.insert(pos, (at, d.mode));
            }
            EventData::BreakStarted(d) => {
                let at = event.effective_at;
                let pos = self.breaks.partition_point(|b| b.at <= at);
                self.breaks.insert(
                    pos,
                    Break {
                        at,
                        kind: d.kind,
                        minutes: d.minutes,
                    },
                );
            }
            EventData::SuggestionDismissed(d) => {
                self.identity_ok(d.identity)?;
                self.suggestion_dismissals
                    .insert(d.identity, event.effective_at);
            }
            EventData::BreakSnoozed(d) => {
                let at = event.effective_at;
                let pos = self.snoozes.partition_point(|s| s.at <= at);
                self.snoozes.insert(
                    pos,
                    Snooze {
                        at,
                        minutes: d.minutes,
                    },
                );
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
                if let Some(r) = &d.repeat
                    && !r.is_valid()
                {
                    return Err(Rejection::InvalidRepeat);
                }
                if let Some(o) = d.repeat_of {
                    self.item_ok(o)?;
                }
                if let Some(p) = d.parent {
                    let parent = self.item_ok(p)?;
                    if parent.status != ItemStatus::Todo {
                        return Err(Rejection::ParentNotOpen(p));
                    }
                    if parent.is_external() {
                        return Err(Rejection::ParentIsExternal(p));
                    }
                    if parent.repeat.is_some() {
                        return Err(Rejection::ParentIsRecurring(p));
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
                item.important = d.important;
                item.estimate = d.estimate;
                item.repeat = d.repeat;
                item.repeat_of = d.repeat_of;
                item.waiting_since = d.waiting_for.is_some().then_some(event.effective_at);
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
                if let Some(c) = &d.repeat
                    && let Some(r) = &c.new
                {
                    if !r.is_valid() {
                        return Err(Rejection::InvalidRepeat);
                    }
                    if !item.children.is_empty() {
                        return Err(Rejection::RecurringWithChildren(d.item));
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
                    if d.defer.is_some() || d.scheduled.is_some() || d.deadline.is_some() {
                        i.replanned_at = event.effective_at;
                    }
                    if let Some(c) = &d.waiting_for {
                        match (&i.waiting_for, &c.new) {
                            (None, Some(_)) => i.waiting_since = Some(event.effective_at),
                            (_, None) => i.waiting_since = None,
                            _ => {}
                        }
                        i.waiting_for = c.new.clone();
                    }
                    if let Some(c) = &d.important {
                        i.important = c.new;
                    }
                    if let Some(c) = &d.estimate {
                        i.estimate = c.new;
                    }
                    if let Some(c) = &d.repeat {
                        i.repeat = c.new;
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
                    if parent.repeat.is_some() {
                        return Err(Rejection::ParentIsRecurring(p));
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
                        graduations: Vec::new(),
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
                if matches!(h.graduations.last(), Some((_, None))) {
                    return Err(Rejection::AlreadyGraduated(d.habit));
                }
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
            EventData::HabitGraduated(d) => {
                let h = self.habit_ok(d.habit)?;
                if matches!(h.pauses.last(), Some((_, None))) {
                    return Err(Rejection::AlreadyPaused(d.habit));
                }
                match h.graduations.last() {
                    Some((_, None)) => return Err(Rejection::AlreadyGraduated(d.habit)),
                    Some((_, Some(back))) if d.from < *back => {
                        return Err(Rejection::GraduationOrder(d.habit));
                    }
                    _ => {}
                }
                if let Some(h) = self.habits.get_mut(&d.habit) {
                    h.graduations.push((d.from, None));
                }
            }
            EventData::HabitReinstated(d) => {
                let h = self.habit_ok(d.habit)?;
                match h.graduations.last() {
                    Some((from, None)) if d.from > *from => {}
                    Some((_, None)) => return Err(Rejection::GraduationOrder(d.habit)),
                    _ => return Err(Rejection::NotGraduated(d.habit)),
                }
                if let Some(last) = self
                    .habits
                    .get_mut(&d.habit)
                    .and_then(|h| h.graduations.last_mut())
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
            EventData::TimerStarted(d) => {
                if self.segments.contains_key(&d.segment) {
                    return Err(Rejection::DuplicateId(d.segment.as_uuid()));
                }
                if let Some(running) = self.running {
                    return Err(Rejection::TimerRunning(running));
                }
                match d.target {
                    TimerTarget::Item(id) => {
                        if self.open_item(id)?.is_external() {
                            return Err(Rejection::TimerOnExternal(id));
                        }
                    }
                    TimerTarget::Habit(id) => {
                        if !matches!(self.habit_ok(id)?.kind, HabitKind::Duration { .. }) {
                            return Err(Rejection::NotDurationHabit(id));
                        }
                    }
                }
                let start = event.effective_at;
                if let Some(latest) = self
                    .segments
                    .values()
                    .filter(|s| s.end.is_some_and(|end| start < end))
                    .max_by_key(|s| s.end)
                {
                    return Err(Rejection::SegmentOverlap(latest.id));
                }
                self.segments.insert(
                    d.segment,
                    Segment {
                        id: d.segment,
                        target: d.target,
                        start,
                        end: None,
                    },
                );
                self.running = Some(d.segment);
                if let TimerTarget::Item(id) = d.target
                    && let Some(i) = self.items.get_mut(&id)
                {
                    i.timed = true;
                    i.left_off = None;
                }
            }
            EventData::TimerStopped(d) => {
                let seg = self
                    .segments
                    .get(&d.segment)
                    .ok_or(Rejection::UnknownSegment(d.segment))?;
                if self.running != Some(d.segment) {
                    return Err(Rejection::SegmentNotRunning(d.segment));
                }
                if event.effective_at < seg.start {
                    return Err(Rejection::SegmentEndBeforeStart(d.segment));
                }
                if let Some(seg) = self.segments.get_mut(&d.segment) {
                    seg.end = Some(event.effective_at);
                }
                self.running = None;
            }
            EventData::TimerInterrupted(d) => {
                self.item_ok(d.item)?;
                let seg = self
                    .segments
                    .get(&d.interrupted)
                    .ok_or(Rejection::UnknownSegment(d.interrupted))?;
                if let Some(l) = &d.left_off {
                    clean(l)?;
                }
                if let TimerTarget::Item(id) = seg.target
                    && let Some(i) = self.items.get_mut(&id)
                {
                    i.left_off = d.left_off.clone();
                }
                let at = event.effective_at;
                let pos = self.interruptions.partition_point(|i| i.at <= at);
                self.interruptions.insert(
                    pos,
                    Interruption {
                        at,
                        item: d.item,
                        outcome: d.outcome,
                    },
                );
            }
            EventData::ReviewCompleted(d) => {
                if let Some(c) = &d.conclusion {
                    clean(c)?;
                }
                for f in &d.focus {
                    self.item_ok(*f)?;
                }
                let at = event.effective_at;
                let pos = self.reviews.partition_point(|r| r.at <= at);
                self.reviews.insert(
                    pos,
                    Review {
                        at,
                        conclusion: d.conclusion.clone(),
                        focus: d.focus.clone(),
                        minimal: d.minimal,
                    },
                );
            }
            EventData::RecommendationShown(d) => {
                if let Some(i) = d.item {
                    self.item_ok(i)?;
                }
                if let Some(s) = d.scope {
                    self.identity_ok(s)?;
                }
                self.last_shown.insert(
                    event.device,
                    Shown {
                        at: event.effective_at,
                        card: d.card,
                        item: d.item,
                        scope: d.scope,
                    },
                );
            }
            EventData::RecommendationAccepted(d) => {
                self.item_ok(d.item)?;
            }
            EventData::RecommendationSkipped(d) => {
                if let Some(i) = d.item {
                    self.item_ok(i)?;
                }
                if let Some(item) = d.item.and_then(|i| self.items.get_mut(&i)) {
                    item.skips.push(event.effective_at);
                }
                if matches!(d.card, CardKind::Review | CardKind::Triage) {
                    self.card_skips.insert(d.card, event.effective_at);
                }
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
        timed: false,
        left_off: None,
        important: false,
        estimate: None,
        repeat: None,
        repeat_of: None,
        replanned_at: created_at,
        waiting_since: None,
        skips: Vec::new(),
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

    #[test]
    fn merged_log_with_two_running_timers_warns_once() {
        let mut log = TestLog::new();
        let id = log.identity("work");
        let a = log.item("a", id, None);
        let b = log.item("b", id, None);
        let s1 = SegmentId::from_uuid(log.uuid());
        let s2 = SegmentId::from_uuid(log.uuid());
        for (segment, item) in [(s1, a), (s2, b)] {
            log.push(EventData::TimerStarted(TimerStarted {
                segment,
                target: TimerTarget::Item(item),
            }));
        }
        let state = log.state();
        assert_eq!(state.warnings.len(), 1);
        assert_eq!(state.warnings[0].rejection, Rejection::TimerRunning(s1));
        assert_eq!(state.running, Some(s1));
        assert!(!state.items[&b].timed);
    }
}
