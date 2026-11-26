//! Commands: the serializable write protocol, and their translation to events.

use std::collections::{BTreeSet, HashMap, HashSet};

use jiff::Span;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use uuid::{ContextV7, Uuid};

use crate::calibration;
use crate::error::TimeError;
use crate::event::*;
use crate::ids::{
    CommandId, DeviceId, EventId, HabitId, IdentityId, ItemId, ProposalId, SegmentId, new_v7,
};
use crate::mode;
use crate::proposal::{self, ProposalOp, Unit};
use crate::recommend::{self, Card, CardKind, NextView};
use crate::resolve::{self, ResolveError};
use crate::rhythm;
use crate::state::{Habit, Item, ItemStatus, Proposal, Rejection, Segment, State, project};
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

impl<T> Update<T> {
    /// Maps the set value, keeping `Clear`.
    pub fn map_ref<U>(&self, f: impl FnOnce(&T) -> U) -> Update<U> {
        match self {
            Update::Set(v) => Update::Set(f(v)),
            Update::Clear => Update::Clear,
        }
    }
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
    /// A source that may only capture and propose.
    #[error("this source may only capture into the inbox or submit proposals")]
    ProposalOnly,
    /// A read-only client tried to write.
    #[error("this client may only read")]
    ReadOnly,
    /// A command reserved for the owner.
    #[error("only the owner can do this")]
    OwnerOnly,
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
    /// Habit graduated that day.
    #[error("habit has graduated on {day}")]
    HabitGraduated {
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
    /// A break is already running.
    #[error("a break is already running")]
    OnBreak,
    /// No break reminder is pending.
    #[error("no break is due")]
    NoBreakDue,
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
    /// Interrupt text with a hand-over detail but no hand-over.
    #[error("--left-off and an identity only apply when handling the interruption now")]
    NeedsHandleNow,
    /// The item is already an external node.
    #[error("the item is already waiting on something external")]
    AlreadyWaiting,
    /// Two weekly-focus items share an identity.
    #[error("more than one focus item for identity {identity}")]
    DuplicateFocus {
        /// The identity with two focus items.
        identity: IdentityId,
    },
    /// Nothing recommended on this device to act on.
    #[error("no recommendation to act on; run 'patina next' first")]
    NoRecommendation,
    /// A proposal step names a key no earlier step defines.
    #[error("unknown proposal key @{key}")]
    UnknownProposalKey {
        /// The key.
        key: String,
    },
    /// A proposal key is not 1 to 32 of letters, digits, `_`, `-`.
    #[error("invalid proposal key {key:?}")]
    InvalidProposalKey {
        /// The key.
        key: String,
    },
    /// Two steps define the same key.
    #[error("duplicate proposal key @{key}")]
    DuplicateProposalKey {
        /// The key.
        key: String,
    },
    /// A step of a proposal failed.
    #[error("proposal step {step}: {error}")]
    ProposalStep {
        /// 1-based step number.
        step: usize,
        /// Why.
        error: Box<CommandError>,
    },
    /// A part depends on a part that cannot be applied.
    #[error("depends on part {unit}, which cannot be applied")]
    StaleDependency {
        /// 1-based part number.
        unit: usize,
    },
    /// An item changed after the proposal was made.
    #[error("{item} changed after the proposal was made")]
    ChangedSince {
        /// The item's title.
        item: String,
    },
    /// A requested part does not exist.
    #[error("no part {unit}")]
    UnknownUnit {
        /// 1-based part number.
        unit: usize,
    },
    /// No selected part can be applied.
    #[error("every selected part is out of date; reject the proposal instead")]
    ProposalStale,
    /// Bulk undo needs an `mcp:` or `agent:` source.
    #[error("only mcp:<client> and agent:<name> sources can be undone in bulk")]
    BulkUndoSource {
        /// The source as given.
        input: String,
    },
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
    /// Set or clear an identity's weekly time target.
    pub struct SetIdentityTarget { pub identity: String, pub weekly_minutes: Option<u32>, }
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
        #[serde(default)]
        pub important: bool,
        pub size: Option<Size>,
        pub repeat: Option<Repeat>,
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
        #[serde(default)]
        pub important: bool,
        pub size: Option<Size>,
        pub repeat: Option<Repeat>,
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
        pub important: Option<bool>,
        pub size: Option<Update<Size>>,
        pub repeat: Option<Update<Repeat>>,
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
    /// Graduate a habit.
    pub struct GraduateHabit { pub habit: String, pub from: Option<Date>, }
    /// Reinstate a graduated habit.
    pub struct ReinstateHabit { pub habit: String, pub from: Option<Date>, }
    /// Switch identity mode (no identity means away), optionally backdated.
    pub struct SwitchMode { pub identity: Option<String>, pub at: Option<jiff::Timestamp>, }
    /// Dismiss the identity-switch suggestion for an identity.
    pub struct DismissSuggestion { pub identity: String, }
    /// Start timing (stopping any running timer), optionally backdated.
    pub struct StartTimer { pub target: TimerRef, pub at: Option<jiff::Timestamp>, }
    /// Stop the running timer, optionally at an earlier time.
    pub struct StopTimer { pub at: Option<jiff::Timestamp>, }
    /// Interrupt the running timer with a new matter.
    pub struct Interrupt {
        pub text: String,
        #[serde(default)]
        pub now: bool,
        pub identity: Option<String>,
        pub left_off: Option<String>,
    }
    /// Block an item on something external and stop timing it.
    pub struct Stuck {
        pub item: Option<String>,
        pub waiting_for: String,
        pub follow_up: Option<Date>,
    }
    /// Record the weekly review with at most one focus item per identity.
    pub struct CompleteReview {
        pub conclusion: Option<String>,
        #[serde(default)]
        pub focus: Vec<String>,
        #[serde(default)]
        pub minimal: bool,
    }
    /// Act on the recommendation card.
    pub struct NextCmd { pub action: NextAction, }
    /// Choose the rhythm mode.
    pub struct SetRhythm { pub mode: RhythmMode, }
    /// Submit a proposal for the owner to review.
    pub struct Propose { pub summary: String, pub ops: Vec<ProposalOp>, }
    /// Accept a proposal; no units means all.
    pub struct AcceptProposal { pub proposal: String, pub units: Option<Vec<usize>>, }
    /// Reject a proposal.
    pub struct RejectProposal { pub proposal: String, pub note: Option<String>, }
    /// Undo every command of an `mcp:` or `agent:` source.
    pub struct UndoSource { pub source: String, pub since: Option<jiff::Timestamp>, }
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
    /// Set an identity's weekly target.
    SetIdentityTarget(SetIdentityTarget),
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
    /// Graduate.
    GraduateHabit(GraduateHabit),
    /// Reinstate.
    ReinstateHabit(ReinstateHabit),
    /// Undo this device's latest command.
    Undo,
    /// Switch identity mode.
    SwitchMode(SwitchMode),
    /// Dismiss a mode suggestion.
    DismissSuggestion(DismissSuggestion),
    /// Start a timer.
    StartTimer(StartTimer),
    /// Stop the running timer.
    StopTimer(StopTimer),
    /// Interrupt the running timer.
    Interrupt(Interrupt),
    /// Mark an item as stuck on something external.
    Stuck(Stuck),
    /// Record the weekly review.
    CompleteReview(CompleteReview),
    /// Show a card or act on the last one.
    Next(NextCmd),
    /// Choose the rhythm mode.
    SetRhythm(SetRhythm),
    /// Start a break, stopping the running timer.
    StartBreak,
    /// Postpone the pending break reminder.
    SnoozeBreak,
    /// Submit a proposal.
    Propose(Propose),
    /// Accept a proposal.
    AcceptProposal(AcceptProposal),
    /// Reject a proposal.
    RejectProposal(RejectProposal),
    /// Undo all commands of one source.
    UndoSource(UndoSource),
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

/// What to do with the recommendation card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum NextAction {
    /// Compute and show the next card.
    Show {
        /// Restrict to one identity (overrides the mode).
        identity: Option<String>,
        /// Ignore the identity mode.
        #[serde(default)]
        all: bool,
    },
    /// Start timing the shown item.
    Start,
    /// Skip the shown card and show the next one.
    Skip,
    /// The shown item is too big: add its first step and show that.
    Split {
        /// The first step.
        text: String,
    },
}

fn shown_data(view: &NextView, scope: Option<IdentityId>) -> EventData {
    let (item, score, reasons) = match &view.card {
        Card::Item {
            item,
            score,
            reasons,
        }
        | Card::BreakDown {
            item,
            score,
            reasons,
        } => (Some(ItemId::from_uuid(item.id)), *score, reasons.clone()),
        _ => (None, None, Vec::new()),
    };
    EventData::RecommendationShown(RecommendationShown {
        card: view.card.kind(),
        item,
        score,
        reasons,
        scope,
    })
}

/// Applies `data` to a scratch copy so the next card is computed after it.
fn apply_scratch(
    state: &mut State,
    device: DeviceId,
    ctx: &Context,
    gen_: &Gen,
    data: &EventData,
) -> Result<(), CommandError> {
    let now = ctx.now.timestamp();
    let event = Event {
        id: EventId::from_uuid(gen_.uuid()),
        device,
        effective_at: now,
        recorded_at: now,
        source: ctx.source.clone(),
        command: CommandId::from_uuid(gen_.uuid()),
        category: data.category(),
        data: data.clone(),
    };
    state.apply(&event)?;
    Ok(())
}

/// The open item on this device's last item or break-down card.
fn shown_item(state: &State, device: DeviceId) -> Result<&Item, CommandError> {
    state
        .last_shown
        .get(&device)
        .filter(|s| matches!(s.card, CardKind::Item | CardKind::BreakDown))
        .and_then(|s| s.item)
        .and_then(|id| state.items.get(&id))
        .filter(|i| i.status == ItemStatus::Todo)
        .ok_or(CommandError::NoRecommendation)
}

/// An `item.created` payload with every optional field empty.
fn new_created(item: ItemId, title: String, identity: IdentityId) -> ItemCreated {
    ItemCreated {
        item,
        title,
        identity,
        parent: None,
        tags: Vec::new(),
        defer: None,
        scheduled: None,
        deadline: None,
        waiting_for: None,
        follow_up: None,
        important: false,
        estimate: None,
        repeat: None,
        repeat_of: None,
    }
}

/// The estimate for `size` on an item with `tags`, calibrated from finished work.
fn learned(state: &State, ctx: &Context, tags: &BTreeSet<String>, size: Size) -> Estimate {
    calibration::estimate(
        &calibration::samples(state, ctx.now.timestamp()),
        tags,
        size,
        &ctx.settings,
    )
}

/// The `item.created` payload for the occurrence that follows `item` once it is completed.
fn next_occurrence(
    state: &State,
    item: &Item,
    repeat: Repeat,
    today: Date,
    ctx: &Context,
    id: ItemId,
) -> Result<ItemCreated, CommandError> {
    let identity = item.identity.ok_or(CommandError::IdentityRequired)?;
    let shift = |d: Option<Date>, span: Span| -> Result<Option<Date>, CommandError> {
        d.map(|d| d.checked_add(span).map_err(|e| TimeError::from(e).into()))
            .transpose()
    };
    let (defer, scheduled, deadline) = match repeat {
        Repeat::Every { every, unit } => {
            let n = i64::from(every);
            let span = match unit {
                RepeatUnit::Day => Span::new().days(n),
                RepeatUnit::Week => Span::new().weeks(n),
                RepeatUnit::Month => Span::new().months(n),
            };
            if item.defer.is_none() && item.scheduled.is_none() && item.deadline.is_none() {
                let due = shift(Some(today), span)?;
                (due, due, None)
            } else {
                (
                    shift(item.defer, span)?,
                    shift(item.scheduled, span)?,
                    shift(item.deadline, span)?,
                )
            }
        }
        Repeat::AfterCompletion { days } => {
            let target = add_days(today, i64::from(days))?;
            match item.scheduled.or(item.deadline).or(item.defer) {
                None => (Some(target), Some(target), None),
                Some(anchor) => {
                    let delta = anchor.until(target).map_err(TimeError::from)?.get_days();
                    let span = Span::new().days(i64::from(delta));
                    (
                        shift(item.defer, span)?,
                        shift(item.scheduled, span)?,
                        shift(item.deadline, span)?,
                    )
                }
            }
        }
    };
    Ok(ItemCreated {
        parent: item.parent,
        tags: item.tags.iter().cloned().collect(),
        defer,
        scheduled,
        deadline,
        important: item.important,
        estimate: item
            .estimate
            .map(|e| learned(state, ctx, &item.tags, e.size)),
        repeat: Some(repeat),
        repeat_of: Some(item.id),
        ..new_created(id, item.title.clone(), identity)
    })
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
    /// The card a `Next` command displays.
    pub card: Option<Box<NextView>>,
}

/// What `build` decided: payloads plus the shared effective time.
struct Built {
    datas: Vec<EventData>,
    effective_at: Option<jiff::Timestamp>,
    card: Option<Box<NextView>>,
    /// The first `n` payloads are attributed to this source instead of the caller's.
    attributed: Option<(Source, usize)>,
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
    crate::access::check(crate::access::access(&ctx.source, &ctx.settings.mcp), cmd)?;
    if let Command::Capture(c) = cmd {
        return Ok(Executed {
            events: vec![capture(device, &c.text, ctx)?],
            card: None,
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
    for (i, data) in built.datas.into_iter().enumerate() {
        let source = match &built.attributed {
            Some((s, n)) if i < *n => s.clone(),
            _ => ctx.source.clone(),
        };
        let event = Event {
            id: EventId::from_uuid(gen_.uuid()),
            device,
            effective_at,
            recorded_at: now,
            source,
            command,
            category: data.category(),
            data,
        };
        scratch.apply(&event)?;
        events.push(event);
    }
    Ok(Executed {
        events,
        card: built.card,
    })
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
    let now = ctx.now.timestamp();
    let mut effective_at: Option<jiff::Timestamp> = None;
    let mut card: Option<Box<NextView>> = None;
    let mut attributed: Option<(Source, usize)> = None;
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
        Command::SetIdentityTarget(c) => {
            let identity = resolve::identity(state, &c.identity)?;
            let old = state
                .identities
                .get(&identity)
                .and_then(|i| i.weekly_target_minutes);
            if old == c.weekly_minutes {
                return Err(CommandError::NoChanges);
            }
            vec![EventData::IdentityTargetChanged(IdentityTargetChanged {
                identity,
                old,
                new: c.weekly_minutes,
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
                parent: parent.map(|p| p.id),
                tags: normalize_tags(&c.tags),
                defer: c.defer,
                scheduled: c.scheduled,
                deadline: c.deadline,
                waiting_for,
                follow_up: c.follow_up,
                important: c.important,
                estimate: c.size.map(|s| {
                    let tags = normalize_tags(&c.tags).into_iter().collect();
                    learned(state, ctx, &tags, s)
                }),
                repeat: c.repeat,
                ..new_created(ItemId::from_uuid(gen_.uuid()), text(&c.title)?, identity)
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
            let tags: BTreeSet<String> = item
                .tags
                .iter()
                .cloned()
                .chain(normalize_tags(&c.tags))
                .collect();
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
                important: c.important.then_some(Change {
                    old: false,
                    new: true,
                }),
                estimate: c.size.map(|s| Change {
                    old: None,
                    new: Some(learned(state, ctx, &tags, s)),
                }),
                repeat: c.repeat.map(|r| Change {
                    old: None,
                    new: Some(r),
                }),
            };
            let any = edit.title.is_some()
                || !edit.tags_added.is_empty()
                || edit.defer.is_some()
                || edit.scheduled.is_some()
                || edit.deadline.is_some()
                || edit.follow_up.is_some()
                || edit.waiting_for.is_some()
                || edit.important.is_some()
                || edit.estimate.is_some()
                || edit.repeat.is_some();
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
            let tags: BTreeSet<String> = item
                .tags
                .iter()
                .filter(|t| !tags_removed.contains(t))
                .chain(&tags_added)
                .cloned()
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
                important: c
                    .important
                    .filter(|i| *i != item.important)
                    .map(|new| Change {
                        old: item.important,
                        new,
                    }),
                estimate: upd(
                    &c.size
                        .as_ref()
                        .map(|u| u.map_ref(|s| learned(state, ctx, &tags, *s))),
                    &item.estimate,
                )
                .filter(|ch| ch.new.map(|e| e.size) != ch.old.map(|e| e.size)),
                repeat: upd(&c.repeat, &item.repeat),
            };
            let none = edit.title.is_none()
                && edit.identity.is_none()
                && edit.tags_added.is_empty()
                && edit.tags_removed.is_empty()
                && edit.defer.is_none()
                && edit.scheduled.is_none()
                && edit.deadline.is_none()
                && edit.follow_up.is_none()
                && edit.waiting_for.is_none()
                && edit.important.is_none()
                && edit.estimate.is_none()
                && edit.repeat.is_none();
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
            if let Some(repeat) = item.repeat {
                out.push(EventData::ItemCreated(next_occurrence(
                    state,
                    item,
                    repeat,
                    today,
                    ctx,
                    ItemId::from_uuid(gen_.uuid()),
                )?));
            }
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
            if crate::habit::is_graduated(habit, day) {
                return Err(CommandError::HabitGraduated { day });
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
            if crate::habit::is_graduated(habit, day) {
                return Err(CommandError::HabitGraduated { day });
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
        Command::GraduateHabit(c) => vec![EventData::HabitGraduated(HabitGraduated {
            habit: habit_ref(state, &c.habit)?.id,
            from: c.from.unwrap_or(today),
        })],
        Command::ReinstateHabit(c) => vec![EventData::HabitReinstated(HabitReinstated {
            habit: habit_ref(state, &c.habit)?.id,
            from: c.from.unwrap_or(today),
        })],
        Command::Propose(c) => {
            let summary = text(&c.summary)?;
            if c.ops.is_empty() {
                return Err(Rejection::EmptyProposal.into());
            }
            let ops = proposal::normalize(state, &c.ops)?;
            let mut scratch = state.clone();
            let mut keys = HashMap::new();
            for (i, op) in ops.iter().enumerate() {
                run_op(&mut scratch, log, device, op, &mut keys, ctx, gen_).map_err(|error| {
                    CommandError::ProposalStep {
                        step: i + 1,
                        error: Box::new(error),
                    }
                })?;
            }
            vec![EventData::ProposalSubmitted(ProposalSubmitted {
                proposal: ProposalId::from_uuid(gen_.uuid()),
                summary,
                ops,
            })]
        }
        Command::AcceptProposal(c) => {
            let id = resolve::proposal(state, &c.proposal)?;
            let p = state
                .proposals
                .get(&id)
                .ok_or(Rejection::UnknownProposal(id))?;
            if p.resolution.is_some() {
                return Err(Rejection::ProposalResolved(id).into());
            }
            let units = proposal::units(&p.ops);
            let requested: BTreeSet<usize> = match &c.units {
                Some(list) => {
                    if let Some(bad) = list.iter().find(|u| **u >= units.len()) {
                        return Err(CommandError::UnknownUnit { unit: bad + 1 });
                    }
                    list.iter().copied().collect()
                }
                None => (0..units.len()).collect(),
            };
            let selected = proposal::with_needs(&units, &requested);
            let outcomes = proposal_outcomes(state, log, device, p, &units, &selected, ctx, gen_);
            let mut datas = Vec::new();
            let mut accepted = Vec::new();
            let mut stale = Vec::new();
            for (i, outcome) in outcomes.into_iter().enumerate() {
                match outcome {
                    UnitOutcome::Ready(d) => {
                        datas.extend(d);
                        accepted.push(u32::try_from(i).unwrap_or(u32::MAX));
                    }
                    UnitOutcome::Stale(_) => stale.push(u32::try_from(i).unwrap_or(u32::MAX)),
                    UnitOutcome::Skipped => {}
                }
            }
            if accepted.is_empty() {
                return Err(CommandError::ProposalStale);
            }
            attributed = Some((p.source.clone(), datas.len()));
            datas.push(EventData::ProposalAccepted(ProposalAccepted {
                proposal: id,
                accepted,
                stale,
            }));
            datas
        }
        Command::RejectProposal(c) => {
            let id = resolve::proposal(state, &c.proposal)?;
            let p = state
                .proposals
                .get(&id)
                .ok_or(Rejection::UnknownProposal(id))?;
            if p.resolution.is_some() {
                return Err(Rejection::ProposalResolved(id).into());
            }
            vec![EventData::ProposalRejected(ProposalRejected {
                proposal: id,
                note: opt_text(&c.note)?,
            })]
        }
        Command::UndoSource(c) => {
            let bad = || CommandError::BulkUndoSource {
                input: c.source.clone(),
            };
            let source = Source::try_from(c.source.clone()).map_err(|_| bad())?;
            if !matches!(source, Source::Mcp { .. } | Source::Agent { .. }) {
                return Err(bad());
            }
            let candidates = bulk_candidates(log, &source, c.since);
            if candidates.is_empty() {
                return Err(CommandError::NothingToUndo);
            }
            let before: HashSet<EventId> = project(log).warnings.iter().map(|w| w.event).collect();
            let rest: Vec<Event> = log
                .iter()
                .filter(|e| !candidates.contains(&e.command) || e.category == Category::Observation)
                .cloned()
                .collect();
            let blocked: Vec<EventId> = project(&rest)
                .warnings
                .iter()
                .map(|w| w.event)
                .filter(|id| !before.contains(id))
                .collect();
            if !blocked.is_empty() {
                return Err(CommandError::UndoBlocked { events: blocked });
            }
            candidates
                .into_iter()
                .map(|command| EventData::CommandUndone(CommandUndone { command }))
                .collect()
        }
        Command::Undo => vec![EventData::CommandUndone(CommandUndone {
            command: undo_target(log, device)?,
        })],
        Command::DismissSuggestion(c) => {
            vec![EventData::SuggestionDismissed(SuggestionDismissed {
                identity: resolve::identity(state, &c.identity)?,
            })]
        }
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
        Command::Interrupt(c) => {
            let seg = running_segment(state).ok_or(CommandError::NoRunningTimer)?;
            if !c.now && (c.left_off.is_some() || c.identity.is_some()) {
                return Err(CommandError::NeedsHandleNow);
            }
            let title = text(&c.text)?;
            let left_off = opt_text(&c.left_off)?;
            let item = ItemId::from_uuid(gen_.uuid());
            if c.now {
                let running_identity = match seg.target {
                    TimerTarget::Item(id) => state.items.get(&id).and_then(|i| i.identity),
                    TimerTarget::Habit(_) => None,
                };
                let identity = match &c.identity {
                    Some(name) => resolve::identity(state, name)?,
                    None => mode::current(state, ctx.now.timestamp(), &ctx.settings)
                        .identity
                        .or(running_identity)
                        .ok_or(CommandError::IdentityRequired)?,
                };
                vec![
                    EventData::ItemCreated(new_created(item, title, identity)),
                    EventData::TimerInterrupted(TimerInterrupted {
                        item,
                        outcome: InterruptOutcome::Now,
                        interrupted: seg.id,
                        left_off,
                    }),
                    EventData::TimerStopped(TimerStopped { segment: seg.id }),
                    EventData::TimerStarted(TimerStarted {
                        segment: SegmentId::from_uuid(gen_.uuid()),
                        target: TimerTarget::Item(item),
                    }),
                ]
            } else {
                vec![
                    EventData::ItemCaptured(ItemCaptured { item, text: title }),
                    EventData::TimerInterrupted(TimerInterrupted {
                        item,
                        outcome: InterruptOutcome::Inbox,
                        interrupted: seg.id,
                        left_off: None,
                    }),
                ]
            }
        }
        Command::Stuck(c) => {
            let item = match &c.item {
                Some(r) => item_ref(state, r)?,
                None => match running_segment(state).map(|s| s.target) {
                    Some(TimerTarget::Item(id)) => {
                        state.items.get(&id).ok_or(Rejection::UnknownItem(id))?
                    }
                    _ => return Err(CommandError::NoRunningTimer),
                },
            };
            if item.is_external() {
                return Err(CommandError::AlreadyWaiting);
            }
            let waiting = text(&c.waiting_for)?;
            let identity = item.identity.ok_or(CommandError::IdentityRequired)?;
            let blocker = ItemId::from_uuid(gen_.uuid());
            let mut out = Vec::new();
            if let Some(seg) = running_segment(state)
                && seg.target == TimerTarget::Item(item.id)
            {
                out.push(EventData::TimerStopped(TimerStopped { segment: seg.id }));
            }
            out.push(EventData::ItemCreated(ItemCreated {
                parent: item.parent,
                waiting_for: Some(waiting.clone()),
                follow_up: c.follow_up,
                ..new_created(blocker, waiting, identity)
            }));
            out.push(EventData::EdgeAdded(EdgeAdded {
                item: item.id,
                depends_on: blocker,
            }));
            out
        }
        Command::CompleteReview(c) => {
            let mut focus: Vec<ItemId> = Vec::new();
            let mut identities: Vec<IdentityId> = Vec::new();
            for r in &c.focus {
                let item = item_ref(state, r)?;
                if item.status != ItemStatus::Todo {
                    return Err(Rejection::WrongStatus {
                        item: item.id,
                        status: item.status,
                    }
                    .into());
                }
                if focus.contains(&item.id) {
                    continue;
                }
                if let Some(identity) = item.identity {
                    if identities.contains(&identity) {
                        return Err(CommandError::DuplicateFocus { identity });
                    }
                    identities.push(identity);
                }
                focus.push(item.id);
            }
            vec![EventData::ReviewCompleted(ReviewCompleted {
                conclusion: opt_text(&c.conclusion)?,
                focus,
                minimal: c.minimal,
            })]
        }
        Command::SetRhythm(c) => {
            if rhythm::mode(state) == c.mode {
                return Err(CommandError::NoChanges);
            }
            vec![EventData::RhythmSwitched(RhythmSwitched { mode: c.mode })]
        }
        Command::StartBreak => {
            let status = rhythm::status(state, now, &ctx.settings);
            if status.on_break.is_some() {
                return Err(CommandError::OnBreak);
            }
            let (kind, minutes) = rhythm::next_break(&status, state, now, &ctx.settings);
            let mut out = Vec::new();
            if let Some(seg) = running_segment(state) {
                out.push(EventData::TimerStopped(TimerStopped { segment: seg.id }));
            }
            out.push(EventData::BreakStarted(BreakStarted { kind, minutes }));
            out
        }
        Command::SnoozeBreak => {
            if rhythm::status(state, now, &ctx.settings).due.is_none() {
                return Err(CommandError::NoBreakDue);
            }
            vec![EventData::BreakSnoozed(BreakSnoozed {
                minutes: ctx.settings.rhythm.snooze_minutes,
            })]
        }
        Command::Next(c) => match &c.action {
            NextAction::Show { identity, all } => {
                let scope = if let Some(n) = identity {
                    Some(resolve::identity(state, n)?)
                } else if *all {
                    None
                } else {
                    mode::current(state, ctx.now.timestamp(), &ctx.settings).effective()
                };
                let view = recommend::next(state, &ctx.now, &ctx.settings, scope, None)?;
                let shown = shown_data(&view, scope);
                card = Some(Box::new(view));
                vec![shown]
            }
            NextAction::Start => {
                let item = shown_item(state, device)?;
                let target = TimerTarget::Item(item.id);
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
                out.push(EventData::RecommendationAccepted(RecommendationAccepted {
                    item: item.id,
                }));
                out
            }
            NextAction::Skip => {
                let shown = state
                    .last_shown
                    .get(&device)
                    .filter(|s| s.card != CardKind::Empty)
                    .ok_or(CommandError::NoRecommendation)?;
                let skipped = EventData::RecommendationSkipped(RecommendationSkipped {
                    card: shown.card,
                    item: shown.item,
                });
                let mut scratch = state.clone();
                apply_scratch(&mut scratch, device, ctx, gen_, &skipped)?;
                let view = recommend::next(&scratch, &ctx.now, &ctx.settings, shown.scope, None)?;
                let next_shown = shown_data(&view, shown.scope);
                card = Some(Box::new(view));
                vec![skipped, next_shown]
            }
            NextAction::Split { text: step } => {
                let parent = shown_item(state, device)?;
                let scope = state.last_shown.get(&device).and_then(|s| s.scope);
                let identity = parent.identity.ok_or(CommandError::IdentityRequired)?;
                let child = ItemId::from_uuid(gen_.uuid());
                let created = EventData::ItemCreated(ItemCreated {
                    parent: Some(parent.id),
                    ..new_created(child, text(step)?, identity)
                });
                let mut scratch = state.clone();
                apply_scratch(&mut scratch, device, ctx, gen_, &created)?;
                let view = recommend::next(&scratch, &ctx.now, &ctx.settings, scope, Some(child))?;
                let next_shown = shown_data(&view, scope);
                card = Some(Box::new(view));
                vec![created, next_shown]
            }
        },
    };
    Ok(Built {
        datas,
        effective_at,
        card,
        attributed,
    })
}

/// Runs one proposal step on `scratch`, returning its payloads.
///
/// A step that adds an item records the new id under its key.
fn run_op(
    scratch: &mut State,
    log: &[Event],
    device: DeviceId,
    op: &ProposalOp,
    keys: &mut HashMap<String, ItemId>,
    ctx: &Context,
    gen_: &Gen,
) -> Result<Vec<EventData>, CommandError> {
    let cmd = proposal::to_command(op, keys)?;
    let built = build(scratch, log, device, &cmd, ctx, gen_)?;
    for data in &built.datas {
        apply_scratch(scratch, device, ctx, gen_, data)?;
    }
    if let ProposalOp::AddItem { key, .. } = op
        && let Some(item) = built.datas.iter().find_map(|d| match d {
            EventData::ItemCreated(c) => Some(c.item),
            _ => None,
        })
    {
        keys.insert(key.clone(), item);
    }
    Ok(built.datas)
}

/// What applying one unit of a proposal would do.
pub(crate) enum UnitOutcome {
    /// The payloads to append.
    Ready(Vec<EventData>),
    /// Cannot be applied now.
    Stale(CommandError),
    /// Not selected.
    Skipped,
}

/// Walks the units in order on one scratch state.
///
/// A unit is stale when a unit it needs is not ready, when an item it edits or moves changed
/// after the proposal was made, or when a step fails.
#[allow(clippy::too_many_arguments)] // one proposal walk needs the whole command context
fn proposal_outcomes(
    state: &State,
    log: &[Event],
    device: DeviceId,
    p: &Proposal,
    units: &[Unit],
    selected: &BTreeSet<usize>,
    ctx: &Context,
    gen_: &Gen,
) -> Vec<UnitOutcome> {
    let mut scratch = state.clone();
    let mut keys: HashMap<String, ItemId> = HashMap::new();
    let mut out: Vec<UnitOutcome> = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        if !selected.contains(&index) {
            out.push(UnitOutcome::Skipped);
            continue;
        }
        if let Some(need) = unit
            .needs
            .iter()
            .find(|n| !matches!(out.get(**n), Some(UnitOutcome::Ready(_))))
        {
            out.push(UnitOutcome::Stale(CommandError::StaleDependency {
                unit: need + 1,
            }));
            continue;
        }
        let mut trial = scratch.clone();
        let mut trial_keys = keys.clone();
        let mut datas = Vec::new();
        let result = (|| -> Result<(), CommandError> {
            for &i in &unit.ops {
                let op = &p.ops[i];
                let target = match op {
                    ProposalOp::Edit(e) => Some(e.item.as_str()),
                    ProposalOp::Move(m) => Some(m.item.as_str()),
                    _ => None,
                };
                if let Some(t) = target.filter(|t| !t.starts_with('@'))
                    && let Ok(id) = resolve::item(state, t)
                    && let Some(item) = state.items.get(&id)
                    && item.changed_at > p.submitted_at
                {
                    return Err(CommandError::ChangedSince {
                        item: item.title.clone(),
                    });
                }
                datas.extend(run_op(
                    &mut trial,
                    log,
                    device,
                    op,
                    &mut trial_keys,
                    ctx,
                    gen_,
                )?);
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                scratch = trial;
                keys = trial_keys;
                out.push(UnitOutcome::Ready(datas));
            }
            Err(e) => out.push(UnitOutcome::Stale(e)),
        }
    }
    out
}

/// What accepting every part of `p` would do right now, for display.
pub(crate) fn preview_proposal(
    state: &State,
    p: &Proposal,
    now: &jiff::ZonedDateTime,
    settings: &Settings,
) -> (Vec<Unit>, Vec<UnitOutcome>) {
    let ctx = Context {
        now: now.clone(),
        source: p.source.clone(),
        settings: settings.clone(),
    };
    let gen_ = Gen {
        ctx: ContextV7::new(),
        now: now.timestamp(),
    };
    let units = proposal::units(&p.ops);
    let all: BTreeSet<usize> = (0..units.len()).collect();
    let device = DeviceId::from_uuid(Uuid::nil());
    let outcomes = proposal_outcomes(state, &[], device, p, &units, &all, &ctx, &gen_);
    (units, outcomes)
}

/// Commands with at least one domain event from `source` (recorded at or after `since`) that
/// are neither undo commands nor already undone, newest first.
pub(crate) fn bulk_candidates(
    log: &[Event],
    source: &Source,
    since: Option<jiff::Timestamp>,
) -> Vec<CommandId> {
    let mut undo_commands = HashSet::new();
    let mut undone = HashSet::new();
    for e in log {
        if let EventData::CommandUndone(u) = &e.data {
            undo_commands.insert(e.command);
            undone.insert(u.command);
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in log.iter().rev() {
        if e.category == Category::Domain
            && e.source == *source
            && since.is_none_or(|s| e.recorded_at >= s)
            && !undo_commands.contains(&e.command)
            && !undone.contains(&e.command)
            && seen.insert(e.command)
        {
            out.push(e.command);
        }
    }
    out
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
            important: false,
            size: None,
            repeat: None,
        }))
    }

    fn link(a: &str, b: &str) -> Command {
        Command::Link(Link {
            item: a.into(),
            depends_on: b.into(),
        })
    }

    #[test]
    fn start_break_stops_the_timer_and_undo_restores_it() {
        let mut log = Vec::new();
        init(&mut log);
        run(&mut log, dev(1), add("A", None)).unwrap();
        run(
            &mut log,
            dev(1),
            Command::StartTimer(StartTimer {
                target: TimerRef::Item("A".into()),
                at: None,
            }),
        )
        .unwrap();
        run(&mut log, dev(1), Command::StartBreak).unwrap();
        let state = project(&log);
        assert!(state.running.is_none());
        assert_eq!(state.breaks.len(), 1);
        assert!(matches!(
            run(&mut log, dev(1), Command::StartBreak),
            Err(CommandError::OnBreak)
        ));
        run(&mut log, dev(1), Command::Undo).unwrap();
        let state = project(&log);
        assert!(state.running.is_some());
        assert!(state.breaks.is_empty());
        assert!(matches!(
            run(&mut log, dev(1), Command::SnoozeBreak),
            Err(CommandError::NoBreakDue)
        ));
    }

    #[test]
    fn identity_target_sets_and_rejects() {
        let mut log = Vec::new();
        init(&mut log);
        let set = |m| {
            Command::SetIdentityTarget(SetIdentityTarget {
                identity: "work".into(),
                weekly_minutes: m,
            })
        };
        run(&mut log, dev(1), set(Some(450))).unwrap();
        let state = project(&log);
        let work = resolve::identity(&state, "work").unwrap();
        assert_eq!(state.identities[&work].weekly_target_minutes, Some(450));
        assert!(matches!(
            run(&mut log, dev(1), set(Some(450))),
            Err(CommandError::NoChanges)
        ));
        for bad in [0, 10081] {
            assert!(matches!(
                run(&mut log, dev(1), set(Some(bad))),
                Err(CommandError::Rejected(Rejection::InvalidTarget(_)))
            ));
        }
        run(&mut log, dev(1), set(None)).unwrap();
        assert_eq!(project(&log).identities[&work].weekly_target_minutes, None);
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

    fn chore(
        title: &str,
        repeat: Repeat,
        scheduled: Option<&str>,
        deadline: Option<&str>,
    ) -> Command {
        Command::AddItem(Box::new(AddItem {
            title: title.into(),
            identity: Some("life".into()),
            parent: None,
            tags: vec!["home".into()],
            defer: None,
            scheduled: scheduled.map(|d| d.parse().unwrap()),
            deadline: deadline.map(|d| d.parse().unwrap()),
            waiting_for: None,
            follow_up: None,
            important: true,
            size: Some(Size::M),
            repeat: Some(repeat),
        }))
    }

    fn done(title: &str) -> Command {
        Command::Complete(Complete {
            item: title.into(),
            note: None,
        })
    }

    fn open_item<'a>(state: &'a State, title: &str) -> &'a Item {
        state
            .items
            .values()
            .find(|i| i.title == title && i.status == ItemStatus::Todo)
            .unwrap()
    }

    #[test]
    fn monthly_repeat_clamps_month_end_and_moves_exactly_one_interval() {
        let mut log = Vec::new();
        init(&mut log);
        let monthly = Repeat::Every {
            every: 1,
            unit: RepeatUnit::Month,
        };
        run(
            &mut log,
            dev(1),
            chore("pay rent", monthly, Some("2027-01-30"), Some("2027-01-31")),
        )
        .unwrap();
        run(&mut log, dev(1), done("pay rent")).unwrap();
        let state = project(&log);
        assert!(state.warnings.is_empty(), "{:?}", state.warnings);
        let next = open_item(&state, "pay rent");
        assert_eq!(next.deadline, Some("2027-02-28".parse().unwrap()));
        assert_eq!(next.scheduled, Some("2027-02-28".parse().unwrap()));
        assert_eq!(next.repeat, Some(monthly));
        assert_eq!(
            next.estimate.map(|e| (e.size, e.minutes)),
            Some((Size::M, 30))
        );
        assert!(next.important);
        assert!(next.tags.contains("home"));
        assert!(next.repeat_of.is_some());
        // A late completion still moves only one interval.
        run(&mut log, dev(1), done("pay rent")).unwrap();
        let state = project(&log);
        let after = open_item(&state, "pay rent");
        assert_eq!(after.deadline, Some("2027-03-28".parse().unwrap()));
    }

    #[test]
    fn after_completion_repeat_counts_from_today() {
        let mut log = Vec::new();
        init(&mut log);
        run(
            &mut log,
            dev(1),
            chore(
                "water plants",
                Repeat::AfterCompletion { days: 90 },
                None,
                None,
            ),
        )
        .unwrap();
        run(&mut log, dev(1), done("water plants")).unwrap();
        let state = project(&log);
        let next = open_item(&state, "water plants");
        let target: Date = "2027-02-22".parse().unwrap();
        assert_eq!((next.scheduled, next.defer), (Some(target), Some(target)));

        run(
            &mut log,
            dev(1),
            chore(
                "oil bike",
                Repeat::AfterCompletion { days: 7 },
                Some("2026-11-01"),
                Some("2026-11-10"),
            ),
        )
        .unwrap();
        run(&mut log, dev(1), done("oil bike")).unwrap();
        let state = project(&log);
        let next = open_item(&state, "oil bike");
        assert_eq!(next.scheduled, Some("2026-12-01".parse().unwrap()));
        assert_eq!(next.deadline, Some("2026-12-10".parse().unwrap()));
    }

    #[test]
    fn abandoning_a_recurring_item_ends_the_series() {
        let mut log = Vec::new();
        init(&mut log);
        let weekly = Repeat::Every {
            every: 1,
            unit: RepeatUnit::Week,
        };
        run(&mut log, dev(1), chore("laundry", weekly, None, None)).unwrap();
        run(
            &mut log,
            dev(1),
            Command::Abandon(Abandon {
                item: "laundry".into(),
                reason: AbandonReason::NoLongerImportant,
                note: None,
            }),
        )
        .unwrap();
        let state = project(&log);
        assert_eq!(state.items.len(), 1);
        assert!(state.warnings.is_empty());
    }

    #[test]
    fn recurring_items_take_no_children_and_no_zero_interval() {
        let mut log = Vec::new();
        init(&mut log);
        let weekly = Repeat::Every {
            every: 1,
            unit: RepeatUnit::Week,
        };
        run(&mut log, dev(1), chore("laundry", weekly, None, None)).unwrap();
        let err = run(&mut log, dev(1), add("fold", Some("laundry"))).unwrap_err();
        assert!(
            matches!(err, CommandError::Rejected(Rejection::ParentIsRecurring(_))),
            "{err:?}"
        );
        let zero = Repeat::AfterCompletion { days: 0 };
        let err = run(&mut log, dev(1), chore("bad", zero, None, None)).unwrap_err();
        assert!(matches!(
            err,
            CommandError::Rejected(Rejection::InvalidRepeat)
        ));
        run(&mut log, dev(1), add("plain", None)).unwrap();
        run(&mut log, dev(1), add("kid", Some("plain"))).unwrap();
        let err = run(
            &mut log,
            dev(1),
            Command::Edit(Box::new(Edit {
                item: "plain".into(),
                title: None,
                identity: None,
                add_tags: vec![],
                remove_tags: vec![],
                defer: None,
                scheduled: None,
                deadline: None,
                follow_up: None,
                waiting_for: None,
                important: None,
                size: None,
                repeat: Some(Update::Set(weekly)),
            })),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CommandError::Rejected(Rejection::RecurringWithChildren(_))
        ));
    }

    #[test]
    fn editing_to_the_same_size_is_no_change() {
        let mut log = Vec::new();
        init(&mut log);
        let weekly = Repeat::Every {
            every: 1,
            unit: RepeatUnit::Week,
        };
        run(&mut log, dev(1), chore("laundry", weekly, None, None)).unwrap();
        let edit = |size: Option<Update<Size>>, important: Option<bool>| {
            Command::Edit(Box::new(Edit {
                item: "laundry".into(),
                title: None,
                identity: None,
                add_tags: vec![],
                remove_tags: vec![],
                defer: None,
                scheduled: None,
                deadline: None,
                follow_up: None,
                waiting_for: None,
                important,
                size,
                repeat: None,
            }))
        };
        let err = run(
            &mut log,
            dev(1),
            edit(Some(Update::Set(Size::M)), Some(true)),
        )
        .unwrap_err();
        assert!(matches!(err, CommandError::NoChanges));
        run(&mut log, dev(1), edit(Some(Update::Set(Size::L)), None)).unwrap();
        let state = project(&log);
        assert_eq!(open_item(&state, "laundry").estimate.unwrap().minutes, 60);
        run(&mut log, dev(1), edit(Some(Update::Clear), Some(false))).unwrap();
        let state = project(&log);
        let item = open_item(&state, "laundry");
        assert!(item.estimate.is_none() && !item.important);
    }
}
