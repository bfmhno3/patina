//! The v1 event wire format: envelope, payloads, and read-time upgrade.

use std::fmt;

use jiff::Timestamp;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::ids::{CommandId, DeviceId, EventId, HabitId, IdentityId, ItemId, SegmentId};
use crate::recommend::{CardKind, Reason};
use crate::time::Weekday;

/// One immutable log entry with its full envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Unique event id (UUIDv7).
    pub id: EventId,
    /// Device that wrote the event.
    pub device: DeviceId,
    /// When the change takes effect.
    pub effective_at: Timestamp,
    /// When the event was recorded.
    pub recorded_at: Timestamp,
    /// Which front end produced it.
    pub source: Source,
    /// The command that produced it.
    pub command: CommandId,
    /// Domain change or observation.
    pub category: Category,
    /// The typed payload.
    pub data: EventData,
}

/// Whether an event changes the domain or merely observes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// A domain change.
    Domain,
    /// An observation (survives undo of its command).
    Observation,
}

/// Which client produced an event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Source {
    /// The command-line interface.
    Cli,
    /// The graphical interface.
    Gui,
    /// An MCP client.
    Mcp {
        /// Client name.
        client: String,
    },
    /// An AI agent using the CLI.
    Agent {
        /// Agent name.
        name: String,
    },
}

impl Source {
    /// Proposal-level sources may only capture.
    pub fn is_proposal_level(&self) -> bool {
        matches!(self, Source::Mcp { .. } | Source::Agent { .. })
    }
}

/// A source string could not be parsed.
#[derive(Debug, thiserror::Error)]
#[error("invalid source {0:?}")]
pub struct SourceParseError(String);

impl From<Source> for String {
    fn from(s: Source) -> String {
        match s {
            Source::Cli => "cli".into(),
            Source::Gui => "gui".into(),
            Source::Mcp { client } => format!("mcp:{client}"),
            Source::Agent { name } => format!("agent:{name}"),
        }
    }
}

impl TryFrom<String> for Source {
    type Error = SourceParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.as_str() {
            "cli" => Ok(Source::Cli),
            "gui" => Ok(Source::Gui),
            _ => match s.split_once(':') {
                Some(("mcp", c)) if !c.is_empty() => Ok(Source::Mcp { client: c.into() }),
                Some(("agent", n)) if !n.is_empty() => Ok(Source::Agent { name: n.into() }),
                _ => Err(SourceParseError(s)),
            },
        }
    }
}

/// Old and new value of an edited field (old is audit information).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change<T> {
    /// Value before the edit.
    pub old: T,
    /// Value after the edit.
    pub new: T,
}

/// Why an item was abandoned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbandonReason {
    /// No longer important.
    NoLongerImportant,
    /// Replaced by something else.
    Superseded,
    /// Too hard.
    TooHard,
    /// Anything else.
    Other,
}

/// How a habit is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HabitKind {
    /// Done or not.
    Check,
    /// Counted toward a daily target.
    Count {
        /// Count that makes a day complete.
        target: u32,
    },
    /// Tracked time toward a daily target in minutes; a check-in also completes the day.
    Duration {
        /// Minutes that make a day complete.
        minutes: u32,
    },
}

/// What happened to an interruption.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptOutcome {
    /// Handled right now: the timer switched to it.
    Now,
    /// Parked in the inbox; the timer keeps running.
    Inbox,
}

/// What a timer segment times.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerTarget {
    /// A task.
    Item(ItemId),
    /// A duration habit.
    Habit(HabitId),
}

/// How often a habit is due.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frequency {
    /// Every day.
    Daily,
    /// N days per week.
    Weekly {
        /// Times per week, 1..=7.
        times: u8,
    },
    /// On listed weekdays.
    Weekdays {
        /// The weekdays.
        days: Vec<Weekday>,
    },
}

/// T-shirt size of an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    /// Small.
    S,
    /// Medium.
    M,
    /// Large.
    L,
}

/// A size estimate with the minutes it meant when written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estimate {
    /// The size.
    pub size: Size,
    /// Minutes the size stood for at write time.
    pub minutes: u32,
}

/// Unit of a fixed-interval repeat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatUnit {
    /// Days.
    Day,
    /// Weeks.
    Week,
    /// Months.
    Month,
}

/// How a recurring chore comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Repeat {
    /// Every N units, counted from the dates it had.
    Every {
        /// Interval length.
        every: u32,
        /// Interval unit.
        unit: RepeatUnit,
    },
    /// A number of days after each completion.
    AfterCompletion {
        /// Days after completion.
        days: u32,
    },
}

impl Repeat {
    /// Intervals must be positive.
    pub fn is_valid(&self) -> bool {
        match self {
            Repeat::Every { every, .. } => *every > 0,
            Repeat::AfterCompletion { days } => *days > 0,
        }
    }
}

macro_rules! payloads {
    ($($(#[$m:meta])* pub struct $name:ident { $($(#[$fm:meta])* pub $f:ident : $t:ty,)* })*) => {
        $(
            $(#[$m])*
            #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
            pub struct $name { $($(#[$fm])* pub $f: $t,)* }
        )*
    };
}

payloads! {
    /// `identity.created`
    pub struct IdentityCreated { pub identity: IdentityId, pub name: String, }
    /// `identity.renamed`
    pub struct IdentityRenamed { pub identity: IdentityId, pub old: String, pub new: String, }
    /// `item.captured`
    pub struct ItemCaptured { pub item: ItemId, pub text: String, }
    /// `item.created`
    pub struct ItemCreated {
        pub item: ItemId,
        pub title: String,
        pub identity: IdentityId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub parent: Option<ItemId>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub defer: Option<Date>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub scheduled: Option<Date>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub deadline: Option<Date>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub waiting_for: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub follow_up: Option<Date>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pub important: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub estimate: Option<Estimate>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub repeat: Option<Repeat>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub repeat_of: Option<ItemId>,
    }
    /// `item.triaged`
    pub struct ItemTriaged { pub item: ItemId, pub identity: IdentityId, }
    /// `item.edited`
    pub struct ItemEdited {
        pub item: ItemId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub title: Option<Change<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub identity: Option<Change<IdentityId>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub tags_added: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub tags_removed: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub defer: Option<Change<Option<Date>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub scheduled: Option<Change<Option<Date>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub deadline: Option<Change<Option<Date>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub follow_up: Option<Change<Option<Date>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub waiting_for: Option<Change<Option<String>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub important: Option<Change<bool>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub estimate: Option<Change<Option<Estimate>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub repeat: Option<Change<Option<Repeat>>>,
    }
    /// `item.note_added`
    pub struct ItemNoteAdded { pub item: ItemId, pub text: String, }
    /// `item.started`
    pub struct ItemStarted { pub item: ItemId, }
    /// `item.completed`
    pub struct ItemCompleted {
        pub item: ItemId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub note: Option<String>,
    }
    /// `item.abandoned`
    pub struct ItemAbandoned {
        pub item: ItemId,
        pub reason: AbandonReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub note: Option<String>,
    }
    /// `item.discarded`
    pub struct ItemDiscarded { pub item: ItemId, }
    /// `item.moved`
    pub struct ItemMoved {
        pub item: ItemId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub old_parent: Option<ItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub new_parent: Option<ItemId>,
    }
    /// `edge.added`
    pub struct EdgeAdded { pub item: ItemId, pub depends_on: ItemId, }
    /// `edge.removed`
    pub struct EdgeRemoved { pub item: ItemId, pub depends_on: ItemId, }
    /// `habit.created`
    pub struct HabitCreated {
        pub habit: HabitId,
        pub name: String,
        pub identity: IdentityId,
        pub kind: HabitKind,
        pub frequency: Frequency,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cue: Option<String>,
        pub start: Date,
    }
    /// `habit.edited`
    pub struct HabitEdited {
        pub habit: HabitId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<Change<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub identity: Option<Change<IdentityId>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cue: Option<Change<Option<String>>>,
    }
    /// `habit.checked_in`
    pub struct HabitCheckedIn {
        pub habit: HabitId,
        pub day: Date,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub count: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub note: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pub backfilled: bool,
    }
    /// `habit.skipped`
    pub struct HabitSkipped { pub habit: HabitId, pub day: Date, }
    /// `timer.started`; the segment starts at the envelope effective time.
    pub struct TimerStarted { pub segment: SegmentId, pub target: TimerTarget, }
    /// `timer.stopped`; the segment ends at the envelope effective time.
    pub struct TimerStopped { pub segment: SegmentId, }
    /// `timer.interrupted`; `item` is the item made for the interruption.
    pub struct TimerInterrupted {
        pub item: ItemId,
        pub outcome: InterruptOutcome,
        pub interrupted: SegmentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub left_off: Option<String>,
    }
    /// `review.completed`
    pub struct ReviewCompleted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub conclusion: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub focus: Vec<ItemId>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pub minimal: bool,
    }
    /// `recommendation.shown` (observation)
    pub struct RecommendationShown {
        pub card: CardKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub item: Option<ItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub score: Option<f64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub reasons: Vec<Reason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub scope: Option<IdentityId>,
    }
    /// `recommendation.accepted` (observation)
    pub struct RecommendationAccepted { pub item: ItemId, }
    /// `recommendation.skipped` (observation)
    pub struct RecommendationSkipped {
        pub card: CardKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub item: Option<ItemId>,
    }
    /// `habit.paused`
    pub struct HabitPaused { pub habit: HabitId, pub from: Date, }
    /// `habit.resumed`
    pub struct HabitResumed { pub habit: HabitId, pub from: Date, }
    /// `command.undone`
    pub struct CommandUndone { pub command: CommandId, }
    /// `mode.switched`; no identity means away.
    pub struct ModeSwitched {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub identity: Option<IdentityId>,
    }
    /// `identity.target_changed`; minutes per week.
    pub struct IdentityTargetChanged {
        pub identity: IdentityId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub old: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub new: Option<u32>,
    }
}

/// Typed event payload; serialized as `{"type": ..., "data": ...}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EventData {
    /// An identity was created.
    #[serde(rename = "identity.created")]
    IdentityCreated(IdentityCreated),
    /// An identity was renamed.
    #[serde(rename = "identity.renamed")]
    IdentityRenamed(IdentityRenamed),
    /// Text captured into the inbox.
    #[serde(rename = "item.captured")]
    ItemCaptured(ItemCaptured),
    /// A task created directly.
    #[serde(rename = "item.created")]
    ItemCreated(ItemCreated),
    /// An inbox entry became a task.
    #[serde(rename = "item.triaged")]
    ItemTriaged(ItemTriaged),
    /// Fields edited.
    #[serde(rename = "item.edited")]
    ItemEdited(ItemEdited),
    /// Note appended.
    #[serde(rename = "item.note_added")]
    ItemNoteAdded(ItemNoteAdded),
    /// Work started.
    #[serde(rename = "item.started")]
    ItemStarted(ItemStarted),
    /// Completed.
    #[serde(rename = "item.completed")]
    ItemCompleted(ItemCompleted),
    /// Abandoned.
    #[serde(rename = "item.abandoned")]
    ItemAbandoned(ItemAbandoned),
    /// Inbox entry discarded.
    #[serde(rename = "item.discarded")]
    ItemDiscarded(ItemDiscarded),
    /// Reparented.
    #[serde(rename = "item.moved")]
    ItemMoved(ItemMoved),
    /// Dependency edge added.
    #[serde(rename = "edge.added")]
    EdgeAdded(EdgeAdded),
    /// Dependency edge removed.
    #[serde(rename = "edge.removed")]
    EdgeRemoved(EdgeRemoved),
    /// Habit created.
    #[serde(rename = "habit.created")]
    HabitCreated(HabitCreated),
    /// Habit edited.
    #[serde(rename = "habit.edited")]
    HabitEdited(HabitEdited),
    /// Habit checked in.
    #[serde(rename = "habit.checked_in")]
    HabitCheckedIn(HabitCheckedIn),
    /// Habit day skipped.
    #[serde(rename = "habit.skipped")]
    HabitSkipped(HabitSkipped),
    /// Timer segment started.
    #[serde(rename = "timer.started")]
    TimerStarted(TimerStarted),
    /// Timer segment stopped.
    #[serde(rename = "timer.stopped")]
    TimerStopped(TimerStopped),
    /// A running timer was interrupted.
    #[serde(rename = "timer.interrupted")]
    TimerInterrupted(TimerInterrupted),
    /// A weekly review was recorded.
    #[serde(rename = "review.completed")]
    ReviewCompleted(ReviewCompleted),
    /// A recommendation card was shown.
    #[serde(rename = "recommendation.shown")]
    RecommendationShown(RecommendationShown),
    /// A recommendation was taken.
    #[serde(rename = "recommendation.accepted")]
    RecommendationAccepted(RecommendationAccepted),
    /// A recommendation was skipped.
    #[serde(rename = "recommendation.skipped")]
    RecommendationSkipped(RecommendationSkipped),
    /// Habit paused.
    #[serde(rename = "habit.paused")]
    HabitPaused(HabitPaused),
    /// Habit resumed.
    #[serde(rename = "habit.resumed")]
    HabitResumed(HabitResumed),
    /// A command was undone.
    #[serde(rename = "command.undone")]
    CommandUndone(CommandUndone),
    /// Identity mode switched (or set to away).
    #[serde(rename = "mode.switched")]
    ModeSwitched(ModeSwitched),
    /// A weekly time target changed.
    #[serde(rename = "identity.target_changed")]
    IdentityTargetChanged(IdentityTargetChanged),
}

const KINDS: &[&str] = &[
    "identity.created",
    "identity.renamed",
    "item.captured",
    "item.created",
    "item.triaged",
    "item.edited",
    "item.note_added",
    "item.started",
    "item.completed",
    "item.abandoned",
    "item.discarded",
    "item.moved",
    "edge.added",
    "edge.removed",
    "habit.created",
    "habit.edited",
    "habit.checked_in",
    "habit.skipped",
    "timer.started",
    "timer.stopped",
    "timer.interrupted",
    "review.completed",
    "recommendation.shown",
    "recommendation.accepted",
    "recommendation.skipped",
    "habit.paused",
    "habit.resumed",
    "command.undone",
    "mode.switched",
    "identity.target_changed",
];

impl EventData {
    /// The stable type key.
    pub fn kind(&self) -> &'static str {
        match self {
            EventData::IdentityCreated(_) => "identity.created",
            EventData::IdentityRenamed(_) => "identity.renamed",
            EventData::ItemCaptured(_) => "item.captured",
            EventData::ItemCreated(_) => "item.created",
            EventData::ItemTriaged(_) => "item.triaged",
            EventData::ItemEdited(_) => "item.edited",
            EventData::ItemNoteAdded(_) => "item.note_added",
            EventData::ItemStarted(_) => "item.started",
            EventData::ItemCompleted(_) => "item.completed",
            EventData::ItemAbandoned(_) => "item.abandoned",
            EventData::ItemDiscarded(_) => "item.discarded",
            EventData::ItemMoved(_) => "item.moved",
            EventData::EdgeAdded(_) => "edge.added",
            EventData::EdgeRemoved(_) => "edge.removed",
            EventData::HabitCreated(_) => "habit.created",
            EventData::HabitEdited(_) => "habit.edited",
            EventData::HabitCheckedIn(_) => "habit.checked_in",
            EventData::HabitSkipped(_) => "habit.skipped",
            EventData::TimerStarted(_) => "timer.started",
            EventData::TimerStopped(_) => "timer.stopped",
            EventData::TimerInterrupted(_) => "timer.interrupted",
            EventData::ReviewCompleted(_) => "review.completed",
            EventData::RecommendationShown(_) => "recommendation.shown",
            EventData::RecommendationAccepted(_) => "recommendation.accepted",
            EventData::RecommendationSkipped(_) => "recommendation.skipped",
            EventData::HabitPaused(_) => "habit.paused",
            EventData::HabitResumed(_) => "habit.resumed",
            EventData::CommandUndone(_) => "command.undone",
            EventData::ModeSwitched(_) => "mode.switched",
            EventData::IdentityTargetChanged(_) => "identity.target_changed",
        }
    }

    /// The schema version written for this payload.
    pub fn version(&self) -> u32 {
        match self {
            EventData::IdentityCreated(_)
            | EventData::IdentityRenamed(_)
            | EventData::ItemCaptured(_)
            | EventData::ItemCreated(_)
            | EventData::ItemTriaged(_)
            | EventData::ItemEdited(_)
            | EventData::ItemNoteAdded(_)
            | EventData::ItemStarted(_)
            | EventData::ItemCompleted(_)
            | EventData::ItemAbandoned(_)
            | EventData::ItemDiscarded(_)
            | EventData::ItemMoved(_)
            | EventData::EdgeAdded(_)
            | EventData::EdgeRemoved(_)
            | EventData::HabitCreated(_)
            | EventData::HabitEdited(_)
            | EventData::HabitCheckedIn(_)
            | EventData::HabitSkipped(_)
            | EventData::TimerStarted(_)
            | EventData::TimerStopped(_)
            | EventData::TimerInterrupted(_)
            | EventData::ReviewCompleted(_)
            | EventData::RecommendationShown(_)
            | EventData::RecommendationAccepted(_)
            | EventData::RecommendationSkipped(_)
            | EventData::HabitPaused(_)
            | EventData::HabitResumed(_)
            | EventData::ModeSwitched(_)
            | EventData::IdentityTargetChanged(_)
            | EventData::CommandUndone(_) => 1,
        }
    }

    /// Whether the event changes the domain or only observes.
    ///
    /// Observations survive the undo of their command.
    pub fn category(&self) -> Category {
        match self {
            EventData::RecommendationShown(_)
            | EventData::RecommendationAccepted(_)
            | EventData::RecommendationSkipped(_) => Category::Observation,
            _ => Category::Domain,
        }
    }
}

/// Wire form: one JSON object per line.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredEvent {
    id: EventId,
    device: DeviceId,
    effective_at: Timestamp,
    recorded_at: Timestamp,
    source: Source,
    command: CommandId,
    #[serde(rename = "type")]
    kind: String,
    version: u32,
    category: Category,
    data: serde_json::Value,
}

/// A stored event could not be turned into an [`Event`].
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum UpgradeError {
    /// Written by a newer version of the program.
    #[error("unsupported version {version} of event type {kind}")]
    UnsupportedVersion {
        /// Event type key.
        kind: String,
        /// Stored version.
        version: u32,
    },
    /// Unknown event type key.
    #[error("unknown event type {kind}")]
    UnknownType {
        /// Event type key.
        kind: String,
    },
    /// Payload or envelope does not parse.
    #[error("malformed event: {message}")]
    Malformed {
        /// Parser message.
        message: String,
    },
}

fn current_version(kind: &str) -> Option<u32> {
    KINDS.contains(&kind).then_some(1)
}

/// Upgrades a stored event to the current in-memory form.
pub(crate) fn upgrade(stored: StoredEvent) -> Result<Event, UpgradeError> {
    let StoredEvent {
        id,
        device,
        effective_at,
        recorded_at,
        source,
        command,
        kind,
        version,
        category,
        data,
    } = stored;
    let current =
        current_version(&kind).ok_or_else(|| UpgradeError::UnknownType { kind: kind.clone() })?;
    if version > current {
        return Err(UpgradeError::UnsupportedVersion { kind, version });
    }
    // Old-version upcasters go here: rewrite `data` per (kind, version) before parsing.
    let data = serde_json::from_value::<EventData>(serde_json::json!({"type": kind, "data": data}))
        .map_err(|e| UpgradeError::Malformed {
            message: e.to_string(),
        })?;
    Ok(Event {
        id,
        device,
        effective_at,
        recorded_at,
        source,
        command,
        category,
        data,
    })
}

impl Event {
    fn to_stored(&self) -> Result<StoredEvent, serde_json::Error> {
        let mut value = serde_json::to_value(&self.data)?;
        let kind = self.data.kind().to_string();
        let data = value
            .get_mut("data")
            .map(serde_json::Value::take)
            .unwrap_or_default();
        Ok(StoredEvent {
            id: self.id,
            device: self.device,
            effective_at: self.effective_at,
            recorded_at: self.recorded_at,
            source: self.source.clone(),
            command: self.command,
            kind,
            version: self.data.version(),
            category: self.category,
            data,
        })
    }

    /// Serializes to one JSON line (no trailing newline).
    pub(crate) fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.to_stored()?)
    }

    /// Parses one JSON line.
    pub(crate) fn from_line(line: &str) -> Result<Event, UpgradeError> {
        let stored: StoredEvent =
            serde_json::from_str(line).map_err(|e| UpgradeError::Malformed {
                message: e.to_string(),
            })?;
        upgrade(stored)
    }
}

impl Serialize for Event {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_stored()
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.id, self.data.kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample() -> Event {
        let ts: Timestamp = "2026-11-24T01:00:00Z".parse().unwrap();
        Event {
            id: EventId(Uuid::from_u128(1)),
            device: DeviceId(Uuid::from_u128(2)),
            effective_at: ts,
            recorded_at: ts,
            source: Source::Cli,
            command: CommandId(Uuid::from_u128(3)),
            category: Category::Domain,
            data: EventData::ItemCaptured(ItemCaptured {
                item: ItemId(Uuid::from_u128(4)),
                text: "buy milk".into(),
            }),
        }
    }

    #[test]
    fn line_shape_and_round_trip() {
        let e = sample();
        let line = e.to_line().unwrap();
        assert_eq!(
            line,
            "{\"id\":\"00000000-0000-0000-0000-000000000001\",\"device\":\"00000000-0000-0000-0000-000000000002\",\"effective_at\":\"2026-11-24T01:00:00Z\",\"recorded_at\":\"2026-11-24T01:00:00Z\",\"source\":\"cli\",\"command\":\"00000000-0000-0000-0000-000000000003\",\"type\":\"item.captured\",\"version\":1,\"category\":\"domain\",\"data\":{\"item\":\"00000000-0000-0000-0000-000000000004\",\"text\":\"buy milk\"}}"
        );
        assert_eq!(Event::from_line(&line).unwrap(), e);
    }

    #[test]
    fn newer_version_and_unknown_type_are_reported() {
        let line = sample().to_line().unwrap();
        let v2 = line.replace("\"version\":1", "\"version\":2");
        assert!(matches!(
            Event::from_line(&v2),
            Err(UpgradeError::UnsupportedVersion { .. })
        ));
        let unk = line.replace("item.captured", "item.zapped");
        assert!(matches!(
            Event::from_line(&unk),
            Err(UpgradeError::UnknownType { .. })
        ));
    }

    #[test]
    fn source_strings() {
        assert_eq!(
            Source::try_from("agent:x".to_string()).unwrap(),
            Source::Agent { name: "x".into() }
        );
        assert!(Source::try_from("agent:".to_string()).is_err());
        assert!(Source::try_from("bogus".to_string()).is_err());
    }
}
