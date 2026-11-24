//! Commands: the serializable write protocol, and their translation to events.

use std::collections::HashSet;

use jiff::Span;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use uuid::{ContextV7, Uuid};

use crate::error::TimeError;
use crate::event::*;
use crate::ids::{CommandId, DeviceId, EventId, HabitId, IdentityId, ItemId, SegmentId, new_v7};
use crate::mode;
use crate::resolve::{self, ResolveError};
use crate::state::{Habit, Item, ItemStatus, Rejection, Segment, State, project};
use crate::time::{BACKFILL_DAYS, Settings, logical_day, week_start_of};

/// Set or clear an optional field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Update<T> {
    /// Set to a value.
    Set(T),
    /// Clear the field.
    Clear,
}

/// What a timer command refers to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerRef {
    /// A task reference.
    Item(String),
    /// A duration habit reference.
    Habit(String),
}

/// Why a command was refused.
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum CommandError {
    /// The resulting events do not fit the state.
    #[error(transparent)]
    Rejected(#[from] Rejection),
    /// A reference did not resolve.
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    /// Date arithmetic failed.
    #[error(transparent)]
    Time(#[from] TimeError),
    /// Proposal-level sources may only capture.
    #[error("this source may only capture into the inbox")]
    ProposalOnly,
    /// Identities exist already.
    #[error("already initialized")]
    AlreadyInitialized,
    /// No identity given and none inherited.
    #[error("an identity is required")]
    IdentityRequired,
    /// Follow-up date needs `waiting_for`.
    #[error("a follow-up date needs a waiting-for note")]
    FollowUpWithoutWaiting,
    /// Nothing would change.
    #[error("no changes")]
    NoChanges,
    /// Already started.
    #[error("already started")]
    AlreadyStarted,
    /// Day is outside the backfill window.
    #[error("day {day} is outside the backfill window {earliest}..{latest}")]
    OutsideBackfillWindow {
        /// Requested day.
        day: Date,
        /// Earliest allowed.
        earliest: Date,
        /// Latest allowed.
        latest: Date,
    },
    /// Day precedes the habit's start.
    #[error("day is before the habit started")]
    BeforeHabitStart,
    /// Habit paused that day.
    #[error("habit is paused on {day}")]
    HabitPaused {
        /// The day.
        day: Date,
    },
    /// Already checked in.
    #[error("already checked in on {day}")]
    AlreadyCheckedIn {
        /// The day.
        day: Date,
    },
    /// Already skipped.
    #[error("already skipped on {day}")]
    AlreadySkipped {
        /// The day.
        day: Date,
    },
    /// Habit is not due that day.
    #[error("habit is not due on {day}")]
    NotDue {
        /// The day.
        day: Date,
    },
    /// No command of this device to undo.
    #[error("nothing to undo")]
    NothingToUndo,
    /// Later events depend on the command.
    #[error("later events depend on this command")]
    UndoBlocked {
        /// Events that would become invalid.
        events: Vec<EventId>,
    },
    /// Text is empty.
    #[error("text must not be empty")]
    EmptyText,
    /// A time argument lies after the current time.
    #[error("time {at} is in the future")]
    FutureTime {
        /// The offending time.
        at: jiff::Timestamp,
    },
    /// A backdated mode switch precedes the latest switch.
    #[error("mode switch is earlier than the latest switch")]
    BeforeLastModeSwitch,
    /// The timer already runs on this target.
    #[error("a timer is already running on this target")]
    AlreadyTiming,
    /// No timer runs.
    #[error("no timer is running")]
    NoRunningTimer,
}

macro_rules! cmd_structs {
    ($($(#[$m:meta])* pub struct $name:ident { $($(#[$fm:meta])* pub $f:ident : $t:ty,)* })*) => {
        $(
            $(#[$m])*
            #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
            pub struct $name { $($(#[$fm])* pub $f: $t,)* }
        )*
    };
}

cmd_structs! {
    /// Add an identity.
    pub struct AddIdentity { pub name: String, }
    /// Rename an identity.
    pub struct RenameIdentity { pub identity: String, pub name: String, }
    /// Quick capture.
    pub struct Capture { pub text: String, }
    /// Create a task directly.
    pub struct AddItem {
        pub title: String,
        pub identity: Option<String>,
        pub parent: Option<String>,
        #[serde(default)]
        pub tags: Vec<String>,
        pub defer: Option<Date>,
        pub scheduled: Option<Date>,
        pub deadline: Option<Date>,
        pub waiting_for: Option<String>,
        pub follow_up: Option<Date>,
    }
    /// Turn an inbox entry into a task.
    pub struct Triage {
        pub item: String,
        pub identity: Option<String>,
        pub title: Option<String>,
        pub parent: Option<String>,
        #[serde(default)]
        pub tags: Vec<String>,
        pub defer: Option<Date>,
        pub scheduled: Option<Date>,
        pub deadline: Option<Date>,
        pub waiting_for: Option<String>,
        pub follow_up: Option<Date>,
    }
    /// Discard an inbox entry.
    pub struct Discard { pub item: String, }
    /// Merge an inbox entry into another item's notes.
    pub struct Merge { pub item: String, pub into: String, }
    /// Edit item fields.
    pub struct Edit {
        pub item: String,
        pub title: Option<String>,
        pub identity: Option<String>,
        #[serde(default)]
        pub add_tags: Vec<String>,
        #[serde(default)]
        pub remove_tags: Vec<String>,
        pub defer: Option<Update<Date>>,
        pub scheduled: Option<Update<Date>>,
        pub deadline: Option<Update<Date>>,
        pub follow_up: Option<Update<Date>>,
        pub waiting_for: Option<Update<String>>,
    }
    /// Add a note.
    pub struct Note { pub item: String, pub text: String, }
    /// Start work.
    pub struct Start { pub item: String, }
    /// Complete.
    pub struct Complete { pub item: String, pub note: Option<String>, }
    /// Abandon (with open descendants).
    pub struct Abandon { pub item: String, pub reason: AbandonReason, pub note: Option<String>, }
    /// Reparent; no parent means top level.
    pub struct Move { pub item: String, pub parent: Option<String>, }
    /// Add a dependency edge.
    pub struct Link { pub item: String, pub depends_on: String, }
    /// Remove a dependency edge.
    pub struct Unlink { pub item: String, pub depends_on: String, }
    /// Create a habit.
    pub struct AddHabit {
        pub name: String,
        pub identity: String,
        pub kind: HabitKind,
        pub frequency: Frequency,
        pub cue: Option<String>,
    }
    /// Edit a habit.
    pub struct EditHabit {
        pub habit: String,
        pub name: Option<String>,
        pub identity: Option<String>,
        pub cue: Option<Update<String>>,
    }
    /// Check in a habit.
    pub struct CheckIn {
        pub habit: String,
        pub day: Option<Date>,
        pub count: Option<u32>,
        pub note: Option<String>,
    }
    /// Skip a habit day.
    pub struct SkipHabit { pub habit: String, pub day: Option<Date>, }
    /// Pause a habit.
    pub struct PauseHabit { pub habit: String, pub from: Option<Date>, }
    /// Resume a habit.
    pub struct ResumeHabit { pub habit: String, pub from: Option<Date>, }
    /// Switch identity mode (no identity means away), optionally backdated.
    pub struct SwitchMode { pub identity: Option<String>, pub at: Option<jiff::Timestamp>, }
    /// Start timing (stopping any running timer), optionally backdated.
    pub struct StartTimer { pub target: TimerRef, pub at: Option<jiff::Timestamp>, }
    /// Stop the running timer, optionally at an earlier time.
    pub struct StopTimer { pub at: Option<jiff::Timestamp>, }
}

/// A write request; the same type is the CLI JSON, MCP, and IPC protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Create the default identities.
    Init,
    /// Add an identity.
    AddIdentity(AddIdentity),
    /// Rename an identity.
    RenameIdentity(RenameIdentity),
    /// Quick capture.
    Capture(Capture),
    /// Create a task.
    AddItem(Box<AddItem>),
    /// Triage an inbox entry.
    Triage(Box<Triage>),
    /// Discard an inbox entry.
    Discard(Discard),
    /// Merge an inbox entry.
    Merge(Merge),
    /// Edit an item.
    Edit(Box<Edit>),
    /// Add a note.
    Note(Note),
    /// Start work.
    Start(Start),
    /// Complete.
    Complete(Complete),
    /// Abandon.
    Abandon(Abandon),
    /// Reparent.
    Move(Move),
    /// Add an edge.
    Link(Link),
    /// Remove an edge.
    Unlink(Unlink),
    /// Create a habit.
    AddHabit(AddHabit),
    /// Edit a habit.
    EditHabit(EditHabit),
    /// Check in.
    CheckIn(CheckIn),
    /// Skip a day.
    SkipHabit(SkipHabit),
    /// Pause.
    PauseHabit(PauseHabit),
    /// Resume.
    ResumeHabit(ResumeHabit),
    /// Undo this device's latest command.
    Undo,
    /// Switch identity mode.
    SwitchMode(SwitchMode),
    /// Start a timer.
    StartTimer(StartTimer),
    /// Stop the running timer.
    StopTimer(StopTimer),
}

/// Everything a command needs besides state.
#[derive(Clone, Debug)]
pub struct Context {
    /// Current time.
    pub now: jiff::ZonedDateTime,
    /// Producing client.
    pub source: Source,
    /// Settings.
    pub settings: Settings,
}

struct Gen {
    ctx: ContextV7,
    now: jiff::Timestamp,
}

impl Gen {
    fn uuid(&self) -> Uuid {
        new_v7(self.now, &self.ctx)
    }
}

fn text(s: &str) -> Result<String, CommandError> {
    let t = s.trim();
    if t.is_empty() {
        Err(CommandError::EmptyText)
    } else {
        Ok(t.to_string())
    }
}

fn opt_text(s: &Option<String>) -> Result<Option<String>, CommandError> {
    s.as_deref().map(text).transpose()
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim();
        let t = t.strip_prefix('#').unwrap_or(t).trim();
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    out
}

fn add_days(day: Date, n: i64) -> Result<Date, CommandError> {
    day.checked_add(Span::new().days(n))
        .map_err(|e| TimeError::from(e).into())
}

fn check_not_future(at: jiff::Timestamp, now: jiff::Timestamp) -> Result<(), CommandError> {
    if at > now {
        Err(CommandError::FutureTime { at })
    } else {
        Ok(())
    }
}

fn running_segment(state: &State) -> Option<&Segment> {
    state.running.and_then(|id| state.segments.get(&id))
}

fn upd<T: PartialEq + Clone>(
    u: &Option<Update<T>>,
    current: &Option<T>,
) -> Option<Change<Option<T>>> {
    let new = match u.as_ref()? {
        Update::Set(v) => Some(v.clone()),
        Update::Clear => None,
    };
    (new != *current).then(|| Change {
        old: current.clone(),
        new,
    })
}

fn item_ref<'a>(state: &'a State, input: &str) -> Result<&'a Item, CommandError> {
    let id = resolve::item(state, input)?;
    state
        .items
        .get(&id)
        .ok_or(Rejection::UnknownItem(id).into())
}

fn habit_ref<'a>(state: &'a State, input: &str) -> Result<&'a Habit, CommandError> {
    let id = resolve::habit(state, input)?;
    state
        .habits
        .get(&id)
        .ok_or(Rejection::UnknownHabit(id).into())
}

fn identity_for(
    state: &State,
    given: &Option<String>,
    parent: Option<&Item>,
    fallback: Option<IdentityId>,
) -> Result<IdentityId, CommandError> {
    if let Some(name) = given {
        return Ok(resolve::identity(state, name)?);
    }
    parent
        .and_then(|p| p.identity)
        .or(fallback)
        .ok_or(CommandError::IdentityRequired)
}

fn post_order_open(state: &State, id: ItemId, out: &mut Vec<ItemId>, seen: &mut HashSet<ItemId>) {
    if !seen.insert(id) {
        return;
    }
    let Some(item) = state.items.get(&id) else {
        return;
    };
    for c in &item.children {
        post_order_open(state, *c, out, seen);
    }
    if item.status == ItemStatus::Todo {
        out.push(id);
    }
}

/// Builds the event for a quick capture without needing any state.
///
/// # Errors
/// [`CommandError::EmptyText`] for blank text.
pub fn capture(device: DeviceId, raw: &str, ctx: &Context) -> Result<Event, CommandError> {
    let text = text(raw)?;
    let now = ctx.now.timestamp();
    let gen_ = Gen {
        ctx: ContextV7::new(),
        now,
    };
    let item = ItemId::from_uuid(gen_.uuid());
    Ok(Event {
        id: EventId::from_uuid(gen_.uuid()),
        device,
        effective_at: now,
        recorded_at: now,
        source: ctx.source.clone(),
        command: CommandId::from_uuid(gen_.uuid()),
        category: Category::Domain,
        data: EventData::ItemCaptured(ItemCaptured { item, text }),
    })
}

/// Result of a successful [`execute`].
#[derive(Debug)]
pub struct Executed {
    /// The events to append, in order.
    pub events: Vec<Event>,
}

/// What `build` decided: payloads plus the shared effective time.
struct Built {
    datas: Vec<EventData>,
    effective_at: Option<jiff::Timestamp>,
}

/// Validates `cmd` against `state` and produces its events.
///
/// # Errors
/// [`CommandError`] when the command is refused; nothing is produced.
pub fn execute(
    state: &State,
    log: &[Event],
    device: DeviceId,
    cmd: &Command,
    ctx: &Context,
) -> Result<Executed, CommandError> {
    if ctx.source.is_proposal_level() && !matches!(cmd, Command::Capture(_)) {
        return Err(CommandError::ProposalOnly);
    }
    if let Command::Capture(c) = cmd {
        return Ok(Executed {
            events: vec![capture(device, &c.text, ctx)?],
        });
    }
    let now = ctx.now.timestamp();
    let gen_ = Gen {
        ctx: ContextV7::new(),
        now,
    };
    let built = build(state, log, device, cmd, ctx, &gen_)?;
    let effective_at = built.effective_at.unwrap_or(now);
    let command = CommandId::from_uuid(gen_.uuid());
    let mut scratch = state.clone();
    let mut events = Vec::with_capacity(built.datas.len());
    for data in built.datas {
        let event = Event {
            id: EventId::from_uuid(gen_.uuid()),
            device,
            effective_at,
            recorded_at: now,
            source: ctx.source.clone(),
            command,
            category: data.category(),
            data,
        };
        scratch.apply(&event)?;
        events.push(event);
    }
    Ok(Executed { events })
}

fn build(
    state: &State,
    log: &[Event],
    device: DeviceId,
    cmd: &Command,
    ctx: &Context,
    gen_: &Gen,
) -> Result<Built, CommandError> {
    let today = logical_day(&ctx.now, &ctx.settings)?;
    let mut effective_at: Option<jiff::Timestamp> = None;
    let datas = match cmd {
        Command::Capture(_) => Vec::new(),
        Command::Init => {
            if !state.identities.is_empty() {
                return Err(CommandError::AlreadyInitialized);
            }
            ["work", "life", "study"]
                .into_iter()
                .map(|name| {
                    EventData::IdentityCreated(IdentityCreated {
                        identity: IdentityId::from_uuid(gen_.uuid()),
                        name: name.into(),
                    })
                })
                .collect()
        }
        Command::AddIdentity(c) => vec![EventData::IdentityCreated(IdentityCreated {
            identity: IdentityId::from_uuid(gen_.uuid()),
            name: text(&c.name)?,
        })],
        Command::RenameIdentity(c) => {
            let identity = resolve::identity(state, &c.identity)?;
            let old = state
                .identities
                .get(&identity)
                .map(|i| i.name.clone())
                .unwrap_or_default();
            vec![EventData::IdentityRenamed(IdentityRenamed {
                identity,
                old,
                new: text(&c.name)?,
            })]
        }
        Command::AddItem(c) => {
            let parent = c
                .parent
                .as_deref()
                .map(|p| item_ref(state, p))
                .transpose()?;
            let identity = identity_for(state, &c.identity, parent, None)?;
            let waiting_for = opt_text(&c.waiting_for)?;
            if c.follow_up.is_some() && waiting_for.is_none() {
                return Err(CommandError::FollowUpWithoutWaiting);
            }
            vec![EventData::ItemCreated(ItemCreated {
                item: ItemId::from_uuid(gen_.uuid()),
                title: text(&c.title)?,
                identity,
                parent: parent.map(|p| p.id),
                tags: normalize_tags(&c.tags),
                defer: c.defer,
                scheduled: c.scheduled,
                deadline: c.deadline,
                waiting_for,
                follow_up: c.follow_up,
            })]
        }
        Command::Triage(c) => {
            let item = item_ref(state, &c.item)?;
            let parent = c
                .parent
                .as_deref()
                .map(|p| item_ref(state, p))
                .transpose()?;
            let identity = identity_for(
                state,
                &c.identity,
                parent,
                mode::mode_at(state, item.created_at),
            )?;
            let waiting_for = opt_text(&c.waiting_for)?;
            if c.follow_up.is_some() && waiting_for.is_none() {
                return Err(CommandError::FollowUpWithoutWaiting);
            }
            let mut out = vec![EventData::ItemTriaged(ItemTriaged {
                item: item.id,
                identity,
            })];
            let title = opt_text(&c.title)?;
            let edit = ItemEdited {
                item: item.id,
                title: title.filter(|t| *t != item.title).map(|new| Change {
                    old: item.title.clone(),
                    new,
                }),
                identity: None,
                tags_added: normalize_tags(&c.tags),
                tags_removed: Vec::new(),
                defer: c.defer.map(|d| Change {
                    old: None,
                    new: Some(d),
                }),
                scheduled: c.scheduled.map(|d| Change {
                    old: None,
                    new: Some(d),
                }),
                deadline: c.deadline.map(|d| Change {
                    old: None,
                    new: Some(d),
                }),
                follow_up: c.follow_up.map(|d| Change {
                    old: None,
                    new: Some(d),
                }),
                waiting_for: waiting_for.map(|w| Change {
                    old: None,
                    new: Some(w),
                }),
            };
            let any = edit.title.is_some()
                || !edit.tags_added.is_empty()
                || edit.defer.is_some()
                || edit.scheduled.is_some()
                || edit.deadline.is_some()
                || edit.follow_up.is_some()
                || edit.waiting_for.is_some();
            if any {
                out.push(EventData::ItemEdited(edit));
            }
            if let Some(p) = parent {
                out.push(EventData::ItemMoved(ItemMoved {
                    item: item.id,
                    old_parent: None,
                    new_parent: Some(p.id),
                }));
            }
            out
        }
        Command::Discard(c) => {
            vec![EventData::ItemDiscarded(ItemDiscarded {
                item: item_ref(state, &c.item)?.id,
            })]
        }
        Command::Merge(c) => {
            let source = item_ref(state, &c.item)?;
            let target = item_ref(state, &c.into)?;
            if source.status != ItemStatus::Inbox {
                return Err(Rejection::WrongStatus {
                    item: source.id,
                    status: source.status,
                }
                .into());
            }
            vec![
                EventData::ItemNoteAdded(ItemNoteAdded {
                    item: target.id,
                    text: source.title.clone(),
                }),
                EventData::ItemDiscarded(ItemDiscarded { item: source.id }),
            ]
        }
        Command::Edit(c) => {
            let item = item_ref(state, &c.item)?;
            let title = opt_text(&c.title)?.filter(|t| *t != item.title);
            let identity = match &c.identity {
                Some(name) => {
                    let new = resolve::identity(state, name)?;
                    (Some(new) != item.identity).then(|| Change {
                        old: item.identity.unwrap_or(new),
                        new,
                    })
                }
                None => None,
            };
            let tags_added: Vec<String> = normalize_tags(&c.add_tags)
                .into_iter()
                .filter(|t| !item.tags.contains(t))
                .collect();
            let tags_removed: Vec<String> = normalize_tags(&c.remove_tags)
                .into_iter()
                .filter(|t| item.tags.contains(t))
                .collect();
            let waiting_update = match &c.waiting_for {
                Some(Update::Set(w)) => Some(Update::Set(text(w)?)),
                Some(Update::Clear) => Some(Update::Clear),
                None => None,
            };
            let waiting_for = upd(&waiting_update, &item.waiting_for);
            let follow_up = upd(&c.follow_up, &item.follow_up);
            let final_waiting = waiting_for.as_ref().map_or(&item.waiting_for, |c| &c.new);
            let final_follow = follow_up.as_ref().map_or(&item.follow_up, |c| &c.new);
            if final_follow.is_some() && final_waiting.is_none() {
                return Err(CommandError::FollowUpWithoutWaiting);
            }
            let edit = ItemEdited {
                item: item.id,
                title: title.map(|new| Change {
                    old: item.title.clone(),
                    new,
                }),
                identity,
                tags_added,
                tags_removed,
                defer: upd(&c.defer, &item.defer),
                scheduled: upd(&c.scheduled, &item.scheduled),
                deadline: upd(&c.deadline, &item.deadline),
                follow_up,
                waiting_for,
            };
            let none = edit.title.is_none()
                && edit.identity.is_none()
                && edit.tags_added.is_empty()
                && edit.tags_removed.is_empty()
                && edit.defer.is_none()
                && edit.scheduled.is_none()
                && edit.deadline.is_none()
                && edit.follow_up.is_none()
                && edit.waiting_for.is_none();
            if none {
                return Err(CommandError::NoChanges);
            }
            vec![EventData::ItemEdited(edit)]
        }
        Command::Note(c) => vec![EventData::ItemNoteAdded(ItemNoteAdded {
            item: item_ref(state, &c.item)?.id,
            text: text(&c.text)?,
        })],
        Command::Start(c) => {
            let item = item_ref(state, &c.item)?;
            if item.started {
                return Err(CommandError::AlreadyStarted);
            }
            vec![EventData::ItemStarted(ItemStarted { item: item.id })]
        }
        Command::Complete(c) => {
            let item = item_ref(state, &c.item)?;
            let mut out = Vec::new();
            if let Some(seg) = running_segment(state)
                && seg.target == TimerTarget::Item(item.id)
            {
                out.push(EventData::TimerStopped(TimerStopped { segment: seg.id }));
            }
            out.push(EventData::ItemCompleted(ItemCompleted {
                item: item.id,
                note: opt_text(&c.note)?,
            }));
            out
        }
        Command::Abandon(c) => {
            let item = item_ref(state, &c.item)?;
            let note = opt_text(&c.note)?;
            let mut ids = Vec::new();
            let mut seen = HashSet::new();
            post_order_open(state, item.id, &mut ids, &mut seen);
            if ids.last() != Some(&item.id) {
                ids.push(item.id);
            }
            let mut out = Vec::new();
            if let Some(seg) = running_segment(state)
                && let TimerTarget::Item(t) = seg.target
                && ids.contains(&t)
            {
                out.push(EventData::TimerStopped(TimerStopped { segment: seg.id }));
            }
            out.extend(ids.into_iter().map(|id| {
                EventData::ItemAbandoned(ItemAbandoned {
                    item: id,
                    reason: c.reason,
                    note: note.clone(),
                })
            }));
            out
        }
        Command::Move(c) => {
            let item = item_ref(state, &c.item)?;
            let new_parent = c
                .parent
                .as_deref()
                .map(|p| item_ref(state, p))
                .transpose()?;
            let new_parent = new_parent.map(|p| p.id);
            if new_parent == item.parent {
                return Err(CommandError::NoChanges);
            }
            vec![EventData::ItemMoved(ItemMoved {
                item: item.id,
                old_parent: item.parent,
                new_parent,
            })]
        }
        Command::Link(c) => vec![EventData::EdgeAdded(EdgeAdded {
            item: item_ref(state, &c.item)?.id,
            depends_on: item_ref(state, &c.depends_on)?.id,
        })],
        Command::Unlink(c) => vec![EventData::EdgeRemoved(EdgeRemoved {
            item: item_ref(state, &c.item)?.id,
            depends_on: item_ref(state, &c.depends_on)?.id,
        })],
        Command::AddHabit(c) => vec![EventData::HabitCreated(HabitCreated {
            habit: HabitId::from_uuid(gen_.uuid()),
            name: text(&c.name)?,
            identity: resolve::identity(state, &c.identity)?,
            kind: c.kind,
            frequency: c.frequency.clone(),
            cue: opt_text(&c.cue)?,
            start: today,
        })],
        Command::EditHabit(c) => {
            let habit = habit_ref(state, &c.habit)?;
            let name = opt_text(&c.name)?.filter(|n| *n != habit.name);
            let identity = match &c.identity {
                Some(n) => {
                    let new = resolve::identity(state, n)?;
                    (new != habit.identity).then_some(Change {
                        old: habit.identity,
                        new,
                    })
                }
                None => None,
            };
            let cue_update = match &c.cue {
                Some(Update::Set(v)) => Some(Update::Set(text(v)?)),
                Some(Update::Clear) => Some(Update::Clear),
                None => None,
            };
            let cue = upd(&cue_update, &habit.cue);
            if name.is_none() && identity.is_none() && cue.is_none() {
                return Err(CommandError::NoChanges);
            }
            vec![EventData::HabitEdited(HabitEdited {
                habit: habit.id,
                name: name.map(|new| Change {
                    old: habit.name.clone(),
                    new,
                }),
                identity,
                cue,
            })]
        }
        Command::CheckIn(c) => {
            let habit = habit_ref(state, &c.habit)?;
            let day = c.day.unwrap_or(today);
            let earliest = add_days(today, -BACKFILL_DAYS)?;
            if day < earliest || day > today {
                return Err(CommandError::OutsideBackfillWindow {
                    day,
                    earliest,
                    latest: today,
                });
            }
            if day < habit.start {
                return Err(CommandError::BeforeHabitStart);
            }
            if crate::habit::is_paused(habit, day) {
                return Err(CommandError::HabitPaused { day });
            }
            if !matches!(habit.kind, HabitKind::Count { .. }) && habit.checkins.contains_key(&day) {
                return Err(CommandError::AlreadyCheckedIn { day });
            }
            vec![EventData::HabitCheckedIn(HabitCheckedIn {
                habit: habit.id,
                day,
                count: c.count,
                note: opt_text(&c.note)?,
                backfilled: day < today,
            })]
        }
        Command::SkipHabit(c) => {
            let habit = habit_ref(state, &c.habit)?;
            let day = c.day.unwrap_or(today);
            let earliest = add_days(today, -BACKFILL_DAYS)?;
            if day < earliest {
                return Err(CommandError::OutsideBackfillWindow {
                    day,
                    earliest,
                    latest: today,
                });
            }
            if day < habit.start {
                return Err(CommandError::BeforeHabitStart);
            }
            match &habit.frequency {
                Frequency::Weekdays { days }
                    if !days.contains(&crate::time::Weekday::from_jiff(day.weekday())) =>
                {
                    return Err(CommandError::NotDue { day });
                }
                Frequency::Weekly { .. } => {
                    let ws = week_start_of(day, &ctx.settings)?;
                    let we = add_days(ws, 6)?;
                    if habit.skips.range(ws..=we).next().is_some() {
                        return Err(CommandError::AlreadySkipped { day });
                    }
                }
                _ => {}
            }
            if !matches!(habit.frequency, Frequency::Weekly { .. }) {
                if habit.skips.contains(&day) {
                    return Err(CommandError::AlreadySkipped { day });
                }
                if habit.checkins.contains_key(&day) {
                    return Err(CommandError::AlreadyCheckedIn { day });
                }
            }
            vec![EventData::HabitSkipped(HabitSkipped {
                habit: habit.id,
                day,
            })]
        }
        Command::PauseHabit(c) => vec![EventData::HabitPaused(HabitPaused {
            habit: habit_ref(state, &c.habit)?.id,
            from: c.from.unwrap_or(today),
        })],
        Command::ResumeHabit(c) => vec![EventData::HabitResumed(HabitResumed {
            habit: habit_ref(state, &c.habit)?.id,
            from: c.from.unwrap_or(today),
        })],
        Command::Undo => vec![EventData::CommandUndone(CommandUndone {
            command: undo_target(log, device)?,
        })],
        Command::SwitchMode(c) => {
            let identity = c
                .identity
                .as_deref()
                .map(|n| resolve::identity(state, n))
                .transpose()?;
            let now_ts = ctx.now.timestamp();
            match c.at {
                Some(at) => {
                    check_not_future(at, now_ts)?;
                    if state.modes.last().is_some_and(|m| at < m.at) {
                        return Err(CommandError::BeforeLastModeSwitch);
                    }
                    if identity == mode::mode_at(state, at) {
                        return Err(CommandError::NoChanges);
                    }
                    effective_at = Some(at);
                }
                None => {
                    let cur = mode::current(state, now_ts, &ctx.settings);
                    if identity == cur.identity && !cur.auto_away {
                        return Err(CommandError::NoChanges);
                    }
                }
            }
            vec![EventData::ModeSwitched(ModeSwitched { identity })]
        }
        Command::StartTimer(c) => {
            let target = match &c.target {
                TimerRef::Item(r) => TimerTarget::Item(item_ref(state, r)?.id),
                TimerRef::Habit(r) => TimerTarget::Habit(habit_ref(state, r)?.id),
            };
            if let Some(at) = c.at {
                check_not_future(at, ctx.now.timestamp())?;
                effective_at = Some(at);
            }
            let mut out = Vec::new();
            if let Some(seg) = running_segment(state) {
                if seg.target == target {
                    return Err(CommandError::AlreadyTiming);
                }
                out.push(EventData::TimerStopped(TimerStopped { segment: seg.id }));
            }
            out.push(EventData::TimerStarted(TimerStarted {
                segment: SegmentId::from_uuid(gen_.uuid()),
                target,
            }));
            out
        }
        Command::StopTimer(c) => {
            let seg = running_segment(state).ok_or(CommandError::NoRunningTimer)?;
            if let Some(at) = c.at {
                check_not_future(at, ctx.now.timestamp())?;
                effective_at = Some(at);
            }
            vec![EventData::TimerStopped(TimerStopped { segment: seg.id })]
        }
    };
    Ok(Built { datas, effective_at })
}

/// The command `Undo` would undo, after checking nothing later depends on it.
fn undo_target(log: &[Event], device: DeviceId) -> Result<CommandId, CommandError> {
    let mut undo_commands = HashSet::new();
    let mut undone = HashSet::new();
    for e in log {
        if let EventData::CommandUndone(u) = &e.data {
            undo_commands.insert(e.command);
            undone.insert(u.command);
        }
    }
    let candidate = log
        .iter()
        .rev()
        .find(|e| {
            e.device == device
                && e.category == Category::Domain
                && !undo_commands.contains(&e.command)
                && !undone.contains(&e.command)
        })
        .map(|e| e.command)
        .ok_or(CommandError::NothingToUndo)?;
    let before: HashSet<EventId> = project(log).warnings.iter().map(|w| w.event).collect();
    let rest: Vec<Event> = log
        .iter()
        .filter(|e| e.command != candidate || e.category == Category::Observation)
        .cloned()
        .collect();
    let blocked: Vec<EventId> = project(&rest)
        .warnings
        .iter()
        .map(|w| w.event)
        .filter(|id| !before.contains(id))
        .collect();
    if blocked.is_empty() {
        Ok(candidate)
    } else {
        Err(CommandError::UndoBlocked { events: blocked })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(now: &str, source: Source) -> Context {
        Context {
            now: now.parse().unwrap(),
            source,
            settings: Settings::default(),
        }
    }

    const NOW: &str = "2026-11-24T09:00:00+08:00[Asia/Shanghai]";

    fn dev(n: u128) -> DeviceId {
        DeviceId::from_uuid(Uuid::from_u128(n))
    }

    /// Runs a command and appends its events to `log`.
    fn run(log: &mut Vec<Event>, device: DeviceId, cmd: Command) -> Result<(), CommandError> {
        run_as(log, device, cmd, Source::Cli)
    }

    fn run_as(
        log: &mut Vec<Event>,
        device: DeviceId,
        cmd: Command,
        source: Source,
    ) -> Result<(), CommandError> {
        let state = project(log);
        let events = execute(&state, log, device, &cmd, &ctx(NOW, source))?.events;
        log.extend(events);
        Ok(())
    }

    fn init(log: &mut Vec<Event>) {
        run(log, dev(1), Command::Init).unwrap();
    }

    fn add(title: &str, parent: Option<&str>) -> Command {
        Command::AddItem(Box::new(AddItem {
            title: title.into(),
            identity: Some("work".into()),
            parent: parent.map(String::from),
            tags: vec![],
            defer: None,
            scheduled: None,
            deadline: None,
            waiting_for: None,
            follow_up: None,
        }))
    }

    fn link(a: &str, b: &str) -> Command {
        Command::Link(Link {
            item: a.into(),
            depends_on: b.into(),
        })
    }

    #[test]
    fn undo_walks_back_through_own_commands() {
        let mut log = Vec::new();
        init(&mut log);
        run(&mut log, dev(1), add("A", None)).unwrap();
        run(&mut log, dev(1), add("B", Some("A"))).unwrap();
        run(&mut log, dev(1), Command::Undo).unwrap();
        assert_eq!(project(&log).items.len(), 1);
        run(&mut log, dev(1), Command::Undo).unwrap();
        let state = project(&log);
        assert!(state.items.is_empty());
        assert!(state.warnings.is_empty());
    }

    #[test]
    fn undo_blocked_by_other_devices_dependent_event() {
        let mut log = Vec::new();
        init(&mut log);
        run(&mut log, dev(1), add("A", None)).unwrap();
        run(&mut log, dev(2), add("B", Some("A"))).unwrap();
        // Device 1's latest command is the creation of A; B depends on it.
        let err = run(&mut log, dev(1), Command::Undo).unwrap_err();
        assert!(matches!(err, CommandError::UndoBlocked { .. }), "{err:?}");
    }

    #[test]
    fn agent_may_only_capture() {
        let mut log = Vec::new();
        init(&mut log);
        let agent = Source::Agent { name: "t".into() };
        let err = run_as(&mut log, dev(1), add("A", None), agent.clone()).unwrap_err();
        assert!(matches!(err, CommandError::ProposalOnly));
        run_as(
            &mut log,
            dev(1),
            Command::Capture(Capture {
                text: "idea".into(),
            }),
            agent,
        )
        .unwrap();
        assert_eq!(project(&log).items.len(), 1);
    }

    #[test]
    fn check_in_outside_window_is_refused() {
        let mut log = Vec::new();
        init(&mut log);
        run(
            &mut log,
            dev(1),
            Command::AddHabit(AddHabit {
                name: "read".into(),
                identity: "study".into(),
                kind: HabitKind::Check,
                frequency: Frequency::Daily,
                cue: None,
            }),
        )
        .unwrap();
        let check = |day: &str| {
            Command::CheckIn(CheckIn {
                habit: "read".into(),
                day: Some(day.parse().unwrap()),
                count: None,
                note: None,
            })
        };
        let err = run(&mut log, dev(1), check("2026-11-21")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "day 2026-11-21 is outside the backfill window 2026-11-22..2026-11-24"
        );
        assert!(matches!(
            run(&mut log, dev(1), check("2026-11-23")).unwrap_err(),
            CommandError::BeforeHabitStart
        ));
        run(&mut log, dev(1), check("2026-11-24")).unwrap();
        assert!(matches!(
            run(&mut log, dev(1), check("2026-11-24")).unwrap_err(),
            CommandError::AlreadyCheckedIn { .. }
        ));
    }

    #[test]
    fn abandon_composite_abandons_children_first() {
        let mut log = Vec::new();
        init(&mut log);
        run(&mut log, dev(1), add("P", None)).unwrap();
        run(&mut log, dev(1), add("A", Some("P"))).unwrap();
        run(&mut log, dev(1), add("B", Some("P"))).unwrap();
        run(
            &mut log,
            dev(1),
            Command::Complete(Complete {
                item: "B".into(),
                note: None,
            }),
        )
        .unwrap();
        let before = log.len();
        run(
            &mut log,
            dev(1),
            Command::Abandon(Abandon {
                item: "P".into(),
                reason: AbandonReason::Other,
                note: None,
            }),
        )
        .unwrap();
        let state = project(&log);
        assert!(state.warnings.is_empty());
        let titles: Vec<&str> = log[before..]
            .iter()
            .map(|e| match &e.data {
                EventData::ItemAbandoned(a) => state.items[&a.item].title.as_str(),
                _ => "?",
            })
            .collect();
        assert_eq!(titles, ["A", "P"]);
        assert_eq!(
            state
                .items
                .values()
                .filter(|i| i.status == ItemStatus::Abandoned)
                .count(),
            2
        );
    }

    #[test]
    fn three_node_cycle_is_rejected() {
        let mut log = Vec::new();
        init(&mut log);
        for t in ["A", "B", "C"] {
            run(&mut log, dev(1), add(t, None)).unwrap();
        }
        run(&mut log, dev(1), link("B", "A")).unwrap();
        run(&mut log, dev(1), link("C", "B")).unwrap();
        let err = run(&mut log, dev(1), link("A", "C")).unwrap_err();
        assert!(
            matches!(err, CommandError::Rejected(Rejection::Cycle { .. })),
            "{err:?}"
        );
    }

    fn mark_observation(log: &mut [Event], kind: &str) {
        for e in log.iter_mut().filter(|e| e.data.kind() == kind) {
            e.category = Category::Observation;
        }
    }

    #[test]
    fn undo_keeps_observation_events_of_the_undone_command() {
        let mut log = Vec::new();
        init(&mut log);
        run(&mut log, dev(1), add("target", None)).unwrap();
        run(
            &mut log,
            dev(1),
            Command::Capture(Capture {
                text: "inbox entry".into(),
            }),
        )
        .unwrap();
        run(
            &mut log,
            dev(1),
            Command::Merge(Merge {
                item: "inbox entry".into(),
                into: "target".into(),
            }),
        )
        .unwrap();
        mark_observation(&mut log, "item.note_added");
        run(&mut log, dev(1), Command::Undo).unwrap();
        let state = project(&log);
        assert!(state.warnings.is_empty());
        let target = state.items.values().find(|i| i.title == "target").unwrap();
        assert_eq!(target.notes.len(), 1);
        let entry = state
            .items
            .values()
            .find(|i| i.title == "inbox entry")
            .unwrap();
        assert_eq!(entry.status, ItemStatus::Inbox);
    }

    #[test]
    fn undo_skips_observation_only_commands() {
        let mut log = Vec::new();
        init(&mut log);
        run(&mut log, dev(1), add("A", None)).unwrap();
        run(&mut log, dev(1), add("B", None)).unwrap();
        run(
            &mut log,
            dev(1),
            Command::Note(Note {
                item: "A".into(),
                text: "seen".into(),
            }),
        )
        .unwrap();
        mark_observation(&mut log, "item.note_added");
        run(&mut log, dev(1), Command::Undo).unwrap();
        let state = project(&log);
        let titles: Vec<&str> = state.items.values().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["A"]);
        assert_eq!(state.items.values().next().unwrap().notes.len(), 1);
    }
}
