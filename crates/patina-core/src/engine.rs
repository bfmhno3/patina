//! The facade adapters use: load, project, execute, append.

use std::collections::HashSet;

use jiff::ZonedDateTime;
use serde::Serialize;
use uuid::Uuid;

use crate::command::{self, Command, CommandError, Context};
use crate::error::TimeError;
use crate::event::{Category, Estimate, Event, EventData, Source, TimerTarget};
use crate::ids::{self, CommandId};
use crate::query::{self, Query, QueryOutput};
use crate::state::{ItemStatus, State, project};
use crate::store::{Store, StoreError};
use crate::time::Settings;
use crate::visibility;

/// Facade error.
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Error {
    /// Storage failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Command refused.
    #[error(transparent)]
    Command(#[from] CommandError),
    /// Date arithmetic failure.
    #[error(transparent)]
    Time(#[from] TimeError),
    /// Name resolution failure inside a query.
    #[error(transparent)]
    Resolve(#[from] crate::resolve::ResolveError),
}

/// Kind of a [`Subject`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// A task or inbox entry.
    Item,
    /// A habit.
    Habit,
    /// An identity.
    Identity,
}

/// An entity a command touched.
#[derive(Clone, Debug, Serialize)]
pub struct Subject {
    /// What it is.
    pub kind: SubjectKind,
    /// Full id.
    pub id: Uuid,
    /// Short reference, when one exists.
    pub short: Option<String>,
    /// Title or name.
    pub label: String,
}

/// A timer segment that ended with a command.
#[derive(Clone, Debug, Serialize)]
pub struct StoppedView {
    /// What was timed.
    pub subject: Subject,
    /// Length of the segment in whole minutes.
    pub minutes: i64,
    /// Total tracked minutes: the item with its descendants, or the habit on that day.
    pub total_minutes: i64,
    /// The timed item's size estimate.
    pub estimate: Option<Estimate>,
    /// The item's total tracked time reached twice its estimate.
    pub over_estimate: bool,
}

/// Result of a successful command.
#[derive(Debug, Serialize)]
pub struct CommandOutcome {
    /// The command id.
    pub command: CommandId,
    /// Events appended.
    pub events: Vec<Event>,
    /// Entities touched, in first-reference order.
    pub subjects: Vec<Subject>,
    /// Abandon only: open dependents of abandoned items.
    pub needs_decision: Vec<Subject>,
    /// Undo only: event type keys of the undone command.
    pub undone: Vec<String>,
    /// Load and projection warnings.
    pub warnings: usize,
    /// A timer segment ended with this command.
    pub stopped: Option<StoppedView>,
    /// Timing started on an item that had a hand-over note: where work stopped.
    pub left_off: Option<String>,
    /// A `Next` command: the card to display.
    pub card: Option<Box<query::NextView>>,
}

/// Entry point for every front end.
#[derive(Debug)]
pub struct Patina {
    store: Store,
}

fn referenced(state: &State, data: &EventData) -> Vec<(SubjectKind, Uuid)> {
    use SubjectKind::{Habit, Identity, Item};
    match data {
        EventData::IdentityCreated(d) => vec![(Identity, d.identity.as_uuid())],
        EventData::IdentityRenamed(d) => vec![(Identity, d.identity.as_uuid())],
        EventData::ItemCaptured(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemCreated(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemTriaged(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemEdited(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemNoteAdded(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemStarted(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemCompleted(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemAbandoned(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemDiscarded(d) => vec![(Item, d.item.as_uuid())],
        EventData::ItemMoved(d) => vec![(Item, d.item.as_uuid())],
        EventData::EdgeAdded(d) => vec![(Item, d.item.as_uuid()), (Item, d.depends_on.as_uuid())],
        EventData::EdgeRemoved(d) => {
            vec![(Item, d.item.as_uuid()), (Item, d.depends_on.as_uuid())]
        }
        EventData::HabitCreated(d) => vec![(Habit, d.habit.as_uuid())],
        EventData::HabitEdited(d) => vec![(Habit, d.habit.as_uuid())],
        EventData::HabitCheckedIn(d) => vec![(Habit, d.habit.as_uuid())],
        EventData::HabitSkipped(d) => vec![(Habit, d.habit.as_uuid())],
        EventData::HabitPaused(d) => vec![(Habit, d.habit.as_uuid())],
        EventData::HabitResumed(d) => vec![(Habit, d.habit.as_uuid())],
        EventData::ModeSwitched(d) => d
            .identity
            .map(|i| vec![(Identity, i.as_uuid())])
            .unwrap_or_default(),
        EventData::TimerStarted(d) => vec![target_ref(d.target)],
        EventData::TimerInterrupted(d) => vec![(Item, d.item.as_uuid())],
        EventData::ReviewCompleted(d) => d.focus.iter().map(|i| (Item, i.as_uuid())).collect(),
        EventData::RecommendationShown(d) => d
            .item
            .map(|i| vec![(Item, i.as_uuid())])
            .unwrap_or_default(),
        EventData::RecommendationAccepted(d) => vec![(Item, d.item.as_uuid())],
        EventData::RecommendationSkipped(d) => d
            .item
            .map(|i| vec![(Item, i.as_uuid())])
            .unwrap_or_default(),
        EventData::TimerStopped(d) => state
            .segments
            .get(&d.segment)
            .map(|s| vec![target_ref(s.target)])
            .unwrap_or_default(),
        EventData::IdentityTargetChanged(d) => vec![(Identity, d.identity.as_uuid())],
        EventData::RhythmSwitched(_) => Vec::new(),
        EventData::BreakStarted(_) => Vec::new(),
        EventData::BreakSnoozed(_) => Vec::new(),
        EventData::CommandUndone(_) => Vec::new(),
    }
}

fn target_ref(target: TimerTarget) -> (SubjectKind, Uuid) {
    match target {
        TimerTarget::Item(id) => (SubjectKind::Item, id.as_uuid()),
        TimerTarget::Habit(id) => (SubjectKind::Habit, id.as_uuid()),
    }
}

/// The segment that ended with `events`, labelled from post-command `state`.
fn stopped_view(
    state: &State,
    events: &[Event],
    now: &ZonedDateTime,
    settings: &Settings,
) -> Result<Option<StoppedView>, Error> {
    let Some(seg) = events.iter().find_map(|e| match &e.data {
        EventData::TimerStopped(d) => state.segments.get(&d.segment),
        _ => None,
    }) else {
        return Ok(None);
    };
    let today = crate::time::logical_day(now, settings)?;
    let view = query::View::new(state, settings, today, now);
    let ts = now.timestamp();
    let total_secs = match seg.target {
        TimerTarget::Item(id) => {
            let direct = crate::timer::item_seconds(state, ts);
            crate::timer::subtree_seconds(state, id, &direct)
        }
        TimerTarget::Habit(id) => {
            let by_day = crate::timer::habit_seconds(state, id, now.time_zone(), settings, ts)?;
            let day = crate::time::logical_day(
                &seg.start.to_zoned_datetime(now.time_zone().clone()),
                settings,
            )?;
            by_day.get(&day).copied().unwrap_or(0)
        }
    };
    let estimate = match seg.target {
        TimerTarget::Item(id) => state.items.get(&id).and_then(|i| i.estimate),
        TimerTarget::Habit(_) => None,
    };
    let total_minutes = total_secs / 60;
    Ok(Some(StoppedView {
        subject: view.target_subject(seg.target),
        minutes: crate::timer::seconds(seg, ts) / 60,
        total_minutes,
        estimate,
        over_estimate: estimate.is_some_and(|e| total_minutes >= 2 * i64::from(e.minutes)),
    }))
}

/// Builds labelled subjects for the given references, looking names up in `state`.
fn subjects_for(state: &State, refs: &[(SubjectKind, Uuid)]) -> Vec<Subject> {
    let mut seen = HashSet::new();
    let mut item_ids: Vec<Uuid> = state
        .items
        .values()
        .filter(|i| i.status != ItemStatus::Discarded)
        .map(|i| i.id.as_uuid())
        .collect();
    let habit_ids: Vec<Uuid> = state.habits.keys().map(|h| h.as_uuid()).collect();
    for (k, id) in refs {
        if *k == SubjectKind::Item && !item_ids.contains(id) {
            item_ids.push(*id);
        }
    }
    let item_len = ids::short_len(item_ids.iter());
    let habit_len = ids::short_len(habit_ids.iter());
    let mut out = Vec::new();
    for (kind, id) in refs {
        if !seen.insert(*id) {
            continue;
        }
        let (label, short) = match kind {
            SubjectKind::Item => match state.items.get(&ids::ItemId::from_uuid(*id)) {
                Some(i) => (i.title.clone(), Some(ids::short(id, item_len))),
                None => (id.to_string(), None),
            },
            SubjectKind::Habit => match state.habits.get(&ids::HabitId::from_uuid(*id)) {
                Some(h) => (h.name.clone(), Some(ids::short(id, habit_len))),
                None => (id.to_string(), None),
            },
            SubjectKind::Identity => match state.identities.get(&ids::IdentityId::from_uuid(*id)) {
                Some(i) => (i.name.clone(), None),
                None => (id.to_string(), None),
            },
        };
        out.push(Subject {
            kind: *kind,
            id: *id,
            short,
            label,
        });
    }
    out
}

impl Patina {
    /// Wraps a store.
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Loads settings from the data root.
    ///
    /// # Errors
    /// Storage or settings errors.
    pub fn settings(&self) -> Result<Settings, Error> {
        Ok(self.store.load_settings()?)
    }

    /// Executes a command and appends its events.
    ///
    /// Quick capture appends one line without replaying the log.
    ///
    /// # Errors
    /// Storage errors or a refused command.
    pub fn run(
        &self,
        cmd: Command,
        now: ZonedDateTime,
        source: Source,
    ) -> Result<CommandOutcome, Error> {
        let settings = self.store.load_settings()?;
        let ctx = Context {
            now,
            source,
            settings,
        };
        let device = self.store.device();
        let mut guard = self.store.lock()?;

        if let Command::Capture(c) = &cmd {
            let event = command::capture(device, &c.text, &ctx)?;
            guard.append(std::slice::from_ref(&event))?;
            let subjects = match &event.data {
                EventData::ItemCaptured(d) => vec![Subject {
                    kind: SubjectKind::Item,
                    id: d.item.as_uuid(),
                    short: None,
                    label: d.text.clone(),
                }],
                _ => Vec::new(),
            };
            return Ok(CommandOutcome {
                command: event.command,
                events: vec![event],
                subjects,
                needs_decision: Vec::new(),
                undone: Vec::new(),
                warnings: 0,
                stopped: None,
                left_off: None,
                card: None,
            });
        }

        let log = self.store.load()?;
        let mut state = project(&log.events);
        let executed = command::execute(&state, &log.events, device, &cmd, &ctx)?;
        let (events, card) = (executed.events, executed.card);
        guard.append(&events)?;
        let command_id = events
            .first()
            .map(|e| e.command)
            .ok_or(CommandError::NoChanges)?;
        let left_off = events.iter().find_map(|e| match &e.data {
            EventData::TimerStarted(d) => match d.target {
                TimerTarget::Item(id) => state.items.get(&id).and_then(|i| i.left_off.clone()),
                TimerTarget::Habit(_) => None,
            },
            _ => None,
        });

        let mut undone = Vec::new();
        let (label_state, refs, projection_warnings) = if matches!(cmd, Command::Undo) {
            let target = match events.first().map(|e| &e.data) {
                Some(EventData::CommandUndone(u)) => u.command,
                _ => command_id,
            };
            let refs: Vec<_> = log
                .events
                .iter()
                .filter(|e| e.command == target)
                .flat_map(|e| referenced(&state, &e.data))
                .collect();
            undone = log
                .events
                .iter()
                .filter(|e| e.command == target && e.category == Category::Domain)
                .map(|e| e.data.kind().to_string())
                .collect();
            let mut all = log.events.clone();
            all.extend(events.iter().cloned());
            let warnings = project(&all).warnings.len();
            (state, refs, warnings)
        } else {
            for e in &events {
                state.apply(e).map_err(CommandError::from)?;
            }
            let refs: Vec<_> = events
                .iter()
                .flat_map(|e| referenced(&state, &e.data))
                .collect();
            let warnings = state.warnings.len();
            (state, refs, warnings)
        };

        let subjects = subjects_for(&label_state, &refs);
        let stopped = if matches!(cmd, Command::Undo) {
            None
        } else {
            stopped_view(&label_state, &events, &ctx.now, &ctx.settings)?
        };
        let mut needs_decision = Vec::new();
        if matches!(cmd, Command::Abandon(_)) {
            let abandoned: HashSet<_> = events
                .iter()
                .filter_map(|e| match &e.data {
                    EventData::ItemAbandoned(a) => Some(a.item),
                    _ => None,
                })
                .collect();
            let dependents: Vec<(SubjectKind, Uuid)> = label_state
                .items
                .values()
                .filter(|i| {
                    i.status == ItemStatus::Todo
                        && i.depends_on.iter().any(|d| abandoned.contains(d))
                })
                .map(|i| (SubjectKind::Item, i.id.as_uuid()))
                .collect();
            needs_decision = subjects_for(&label_state, &dependents);
        }
        Ok(CommandOutcome {
            command: command_id,
            events,
            subjects,
            needs_decision,
            undone,
            warnings: log.warnings.len() + projection_warnings,
            stopped,
            left_off,
            card,
        })
    }

    /// Runs a read-only query.
    ///
    /// # Errors
    /// Storage errors, unresolved references, or date overflow.
    pub fn query(&self, q: Query, now: ZonedDateTime) -> Result<QueryOutput, Error> {
        let settings = self.store.load_settings()?;
        let log = self.store.load()?;
        let state = project(&log.events);
        query::run(&q, &state, &log.warnings, &now, &settings)
    }

    /// Like [`Patina::query`], but over the state redacted to the identities listed in
    /// `[mcp] visible_identities` (what an MCP client may read).
    ///
    /// # Errors
    /// Storage errors, unresolved references, or date overflow.
    pub fn query_visible(&self, q: Query, now: ZonedDateTime) -> Result<QueryOutput, Error> {
        let settings = self.store.load_settings()?;
        let log = self.store.load()?;
        let state = project(&log.events);
        let visible = visibility::visible_ids(&state, &settings.mcp.visible_identities);
        let state = visibility::redact(&state, &visible);
        query::run(&q, &state, &log.warnings, &now, &settings)
    }
}
