//! Read-only queries and their serializable views.

use std::collections::HashSet;

use jiff::Timestamp;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::dag;
use crate::engine::Error;
use crate::event::{AbandonReason, Frequency, HabitKind};
use crate::habit;
use crate::ids::{self, IdentityId};
use crate::resolve;
use crate::state::{Habit, Item, ItemStatus, Note, State, Warning};
use crate::store::LoadWarning;
use crate::time::{Settings, logical_day};

/// A read request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "snake_case")]
pub enum Query {
    /// What to do today.
    Today {
        /// Restrict to one identity.
        identity: Option<String>,
    },
    /// Untriaged captures.
    Inbox,
    /// The actionable frontier.
    List {
        /// Restrict to one identity.
        identity: Option<String>,
    },
    /// Hierarchy.
    Tree {
        /// Subtree root; none for all top-level items.
        root: Option<String>,
        /// Include settled items.
        #[serde(default)]
        all: bool,
    },
    /// One item in detail.
    Show {
        /// Item reference.
        item: String,
    },
    /// All habits.
    Habits,
    /// All identities.
    Identities,
    /// One-line status.
    Status,
    /// Log problems.
    Doctor,
}

/// Status shown to users; derived from stored status plus the graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewStatus {
    /// Not triaged.
    Inbox,
    /// Open, nothing started.
    Todo,
    /// Started or partly done.
    InProgress,
    /// Unmet prerequisites.
    Blocked,
    /// Waiting on something external.
    Waiting,
    /// Done.
    Done,
    /// Abandoned.
    Abandoned,
}

/// An inbox entry.
#[derive(Clone, Debug, Serialize)]
pub struct InboxEntry {
    /// Full id.
    pub id: Uuid,
    /// Short reference.
    pub short: String,
    /// Captured text.
    pub text: String,
    /// Capture time.
    pub captured_at: Timestamp,
}

/// An item as shown in lists.
#[derive(Clone, Debug, Serialize)]
pub struct ItemView {
    /// Full id.
    pub id: Uuid,
    /// Short reference.
    pub short: String,
    /// Title.
    pub title: String,
    /// Identity name.
    pub identity: Option<String>,
    /// Display status.
    pub status: ViewStatus,
    /// Ancestor titles, root first.
    pub path: Vec<String>,
    /// Tags.
    pub tags: Vec<String>,
    /// Hidden until.
    pub defer: Option<Date>,
    /// Planned day.
    pub scheduled: Option<Date>,
    /// Due day.
    pub deadline: Option<Date>,
    /// External wait.
    pub waiting_for: Option<String>,
    /// Follow-up day.
    pub follow_up: Option<Date>,
    /// Unmet prerequisites.
    pub blocked_by: Vec<Uuid>,
    /// All children settled.
    pub wrap_up: bool,
    /// Number of children.
    pub children: usize,
    /// Number of unsettled children.
    pub open_children: usize,
    /// Creation time.
    pub created_at: Timestamp,
}

/// An item with an abandoned prerequisite.
#[derive(Clone, Debug, Serialize)]
pub struct NeedsDecision {
    /// The dependent item.
    pub item: ItemView,
    /// Its abandoned prerequisites.
    pub abandoned: Vec<ItemView>,
}

/// A habit as shown in lists.
#[derive(Clone, Debug, Serialize)]
pub struct HabitView {
    /// Full id.
    pub id: Uuid,
    /// Short reference.
    pub short: String,
    /// Name.
    pub name: String,
    /// Identity name.
    pub identity: String,
    /// Kind.
    pub kind: HabitKind,
    /// Frequency.
    pub frequency: Frequency,
    /// Cue.
    pub cue: Option<String>,
    /// Paused today.
    pub paused: bool,
    /// Strength 0..=1.
    pub strength: f64,
    /// Still expected today.
    pub due: bool,
    /// Today's completion 0..=1.
    pub value: f64,
    /// Today's count.
    pub count: Option<u32>,
    /// Weekly: progress this week.
    pub week_value: Option<f64>,
    /// Weekly: target this week.
    pub week_target: Option<u8>,
}

/// The today view.
#[derive(Clone, Debug, Serialize)]
pub struct TodayView {
    /// The logical day.
    pub today: Date,
    /// Habits due or done today.
    pub habits: Vec<HabitView>,
    /// External items to chase.
    pub follow_ups: Vec<ItemView>,
    /// Items due.
    pub deadlines: Vec<ItemView>,
    /// Items planned for today or earlier.
    pub scheduled: Vec<ItemView>,
    /// Items whose prerequisite was abandoned.
    pub needs_decision: Vec<NeedsDecision>,
    /// The frontier.
    pub actionable: Vec<ItemView>,
    /// Inbox count.
    pub inbox: usize,
    /// Log problems.
    pub warnings: usize,
}

/// A node in the tree view.
#[derive(Clone, Debug, Serialize)]
pub struct TreeNode {
    /// The item.
    pub item: ItemView,
    /// Children in order.
    pub children: Vec<TreeNode>,
}

/// Why an item was abandoned.
#[derive(Clone, Debug, Serialize)]
pub struct AbandonView {
    /// Reason key.
    pub reason: AbandonReason,
    /// Note.
    pub note: Option<String>,
}

/// One item in detail.
#[derive(Clone, Debug, Serialize)]
pub struct ItemDetail {
    /// The item.
    pub item: ItemView,
    /// Notes.
    pub notes: Vec<Note>,
    /// Items it depends on.
    pub prerequisites: Vec<ItemView>,
    /// Items depending on it.
    pub dependents: Vec<ItemView>,
    /// Children.
    pub children: Vec<ItemView>,
    /// Completion/abandon time.
    pub settled_at: Option<Timestamp>,
    /// Note on completion.
    pub completion_note: Option<String>,
    /// Abandon info.
    pub abandon: Option<AbandonView>,
}

/// An identity with counts.
#[derive(Clone, Debug, Serialize)]
pub struct IdentityView {
    /// Full id.
    pub id: Uuid,
    /// Name.
    pub name: String,
    /// Open items.
    pub open_items: usize,
    /// Habits.
    pub habits: usize,
}

/// The one-line status.
#[derive(Clone, Debug, Serialize)]
pub struct StatusLine {
    /// Inbox entries.
    pub inbox: usize,
    /// Habits still due and incomplete today.
    pub habits_left: usize,
    /// Items due.
    pub due: usize,
    /// Follow-ups due.
    pub follow_ups: usize,
    /// Log problems.
    pub warnings: usize,
}

/// Log problems.
#[derive(Clone, Debug, Serialize)]
pub struct DoctorView {
    /// Lines that could not be read.
    pub load: Vec<LoadWarning>,
    /// Events that could not be applied.
    pub projection: Vec<Warning>,
}

/// Output of any query.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum QueryOutput {
    /// [`Query::Today`]
    Today(Box<TodayView>),
    /// [`Query::Inbox`]
    Inbox(Vec<InboxEntry>),
    /// [`Query::List`]
    List(Vec<ItemView>),
    /// [`Query::Tree`]
    Tree(Vec<TreeNode>),
    /// [`Query::Show`]
    Show(Box<ItemDetail>),
    /// [`Query::Habits`]
    Habits(Vec<HabitView>),
    /// [`Query::Identities`]
    Identities(Vec<IdentityView>),
    /// [`Query::Status`]
    Status(StatusLine),
    /// [`Query::Doctor`]
    Doctor(DoctorView),
}

struct View<'a> {
    state: &'a State,
    settings: &'a Settings,
    today: Date,
    item_len: usize,
    habit_len: usize,
}

impl<'a> View<'a> {
    fn new(state: &'a State, settings: &'a Settings, today: Date) -> Self {
        let item_len = ids::short_len(
            state
                .items
                .values()
                .filter(|i| i.status != ItemStatus::Discarded)
                .map(|i| i.id.as_uuid())
                .collect::<Vec<_>>()
                .iter(),
        );
        let habit_len = ids::short_len(
            state
                .habits
                .keys()
                .map(|h| h.as_uuid())
                .collect::<Vec<_>>()
                .iter(),
        );
        Self {
            state,
            settings,
            today,
            item_len,
            habit_len,
        }
    }

    fn identity_name(&self, id: Option<IdentityId>) -> Option<String> {
        id.and_then(|i| self.state.identities.get(&i))
            .map(|i| i.name.clone())
    }

    fn view_status(&self, item: &Item) -> ViewStatus {
        match item.status {
            ItemStatus::Inbox | ItemStatus::Discarded => ViewStatus::Inbox,
            ItemStatus::Done => ViewStatus::Done,
            ItemStatus::Abandoned => ViewStatus::Abandoned,
            ItemStatus::Todo => {
                if item.is_external() {
                    ViewStatus::Waiting
                } else if !dag::unmet_prerequisites(self.state, item.id).is_empty() {
                    ViewStatus::Blocked
                } else if dag::is_in_progress(self.state, item.id) {
                    ViewStatus::InProgress
                } else {
                    ViewStatus::Todo
                }
            }
        }
    }

    fn item(&self, item: &Item) -> ItemView {
        let path = dag::ancestors(self.state, item.id)
            .into_iter()
            .rev()
            .filter_map(|a| self.state.items.get(&a))
            .map(|a| a.title.clone())
            .collect();
        ItemView {
            id: item.id.as_uuid(),
            short: ids::short(&item.id.as_uuid(), self.item_len),
            title: item.title.clone(),
            identity: self.identity_name(item.identity),
            status: self.view_status(item),
            path,
            tags: item.tags.iter().cloned().collect(),
            defer: item.defer,
            scheduled: item.scheduled,
            deadline: item.deadline,
            waiting_for: item.waiting_for.clone(),
            follow_up: item.follow_up,
            blocked_by: dag::unmet_prerequisites(self.state, item.id)
                .into_iter()
                .map(|i| i.as_uuid())
                .collect(),
            wrap_up: dag::is_wrap_up(self.state, item.id),
            children: item.children.len(),
            open_children: item
                .children
                .iter()
                .filter(|c| self.state.items.get(c).is_some_and(|c| !c.is_settled()))
                .count(),
            created_at: item.created_at,
        }
    }

    fn items(&self, ids: impl IntoIterator<Item = ids::ItemId>) -> Vec<ItemView> {
        ids.into_iter()
            .filter_map(|i| self.state.items.get(&i))
            .map(|i| self.item(i))
            .collect()
    }

    fn habit(&self, h: &Habit) -> Result<HabitView, Error> {
        let st = habit::today_status(h, self.today, self.settings)?;
        Ok(HabitView {
            id: h.id.as_uuid(),
            short: ids::short(&h.id.as_uuid(), self.habit_len),
            name: h.name.clone(),
            identity: self.identity_name(Some(h.identity)).unwrap_or_default(),
            kind: h.kind,
            frequency: h.frequency.clone(),
            cue: h.cue.clone(),
            paused: habit::is_paused(h, self.today),
            strength: habit::strength(h, self.today, self.settings)?,
            due: st.due,
            value: st.value,
            count: st.count,
            week_value: st.week_value,
            week_target: st.week_target,
        })
    }

    fn visible(&self, item: &Item) -> bool {
        item.status == ItemStatus::Todo && !dag::hidden_by_defer(self.state, item.id, self.today)
    }

    fn follow_ups(&self, filter: Option<IdentityId>) -> Vec<&Item> {
        let mut v: Vec<&Item> = self
            .state
            .items
            .values()
            .filter(|i| self.visible(i) && i.is_external())
            .filter(|i| i.follow_up.is_some_and(|d| d <= self.today))
            .filter(|i| filter.is_none() || i.identity == filter)
            .collect();
        v.sort_by_key(|i| (i.follow_up, i.created_at));
        v
    }

    fn actionable(&self, filter: Option<IdentityId>) -> Vec<&Item> {
        let mut v: Vec<&Item> = dag::frontier(self.state, self.today)
            .into_iter()
            .filter_map(|i| self.state.items.get(&i))
            .filter(|i| filter.is_none() || i.identity == filter)
            .collect();
        v.sort_by_key(|i| {
            (
                i.deadline.is_none(),
                i.deadline,
                i.scheduled.is_none(),
                i.scheduled,
                i.created_at,
            )
        });
        v
    }
}

fn tree_node(v: &View<'_>, item: &Item, all: bool, seen: &mut HashSet<ids::ItemId>) -> TreeNode {
    seen.insert(item.id);
    let mut children = Vec::new();
    for c in item.children.iter().filter_map(|c| v.state.items.get(c)) {
        if (all || !c.is_settled()) && !seen.contains(&c.id) {
            children.push(tree_node(v, c, all, seen));
        }
    }
    TreeNode {
        item: v.item(item),
        children,
    }
}

/// Runs a query against projected state.
///
/// # Errors
/// Unresolved references or date overflow.
pub fn run(
    q: &Query,
    state: &State,
    load_warnings: &[LoadWarning],
    now: &jiff::ZonedDateTime,
    settings: &Settings,
) -> Result<QueryOutput, Error> {
    let today = logical_day(now, settings)?;
    let v = View::new(state, settings, today);
    let warnings = load_warnings.len() + state.warnings.len();
    let filter = |name: &Option<String>| -> Result<Option<IdentityId>, Error> {
        Ok(name
            .as_deref()
            .map(|n| resolve::identity(state, n))
            .transpose()?)
    };
    let inbox_count = state
        .items
        .values()
        .filter(|i| i.status == ItemStatus::Inbox)
        .count();
    Ok(match q {
        Query::Today { identity } => {
            let f = filter(identity)?;
            let mut habits = Vec::new();
            for h in state
                .habits
                .values()
                .filter(|h| f.is_none() || Some(h.identity) == f)
            {
                let hv = v.habit(h)?;
                if hv.due || hv.value > 0.0 {
                    habits.push(hv);
                }
            }
            let in_scope = |i: &&Item| f.is_none() || i.identity == f;
            let mut deadlines: Vec<&Item> = state
                .items
                .values()
                .filter(|i| v.visible(i) && i.deadline.is_some_and(|d| d <= today))
                .filter(in_scope)
                .collect();
            deadlines.sort_by_key(|i| (i.deadline, i.created_at));
            let mut scheduled: Vec<&Item> = state
                .items
                .values()
                .filter(|i| v.visible(i) && !i.is_external())
                .filter(|i| i.scheduled.is_some_and(|d| d <= today))
                .filter(in_scope)
                .collect();
            scheduled.sort_by_key(|i| (i.scheduled, i.created_at));
            let needs_decision = state
                .items
                .values()
                .filter(|i| i.status == ItemStatus::Todo)
                .filter(in_scope)
                .filter_map(|i| {
                    let ab = dag::abandoned_prerequisites(state, i.id);
                    (!ab.is_empty()).then(|| NeedsDecision {
                        item: v.item(i),
                        abandoned: v.items(ab),
                    })
                })
                .collect();
            QueryOutput::Today(Box::new(TodayView {
                today,
                habits,
                follow_ups: v.follow_ups(f).into_iter().map(|i| v.item(i)).collect(),
                deadlines: deadlines.into_iter().map(|i| v.item(i)).collect(),
                scheduled: scheduled.into_iter().map(|i| v.item(i)).collect(),
                needs_decision,
                actionable: v.actionable(f).into_iter().map(|i| v.item(i)).collect(),
                inbox: inbox_count,
                warnings,
            }))
        }
        Query::Inbox => QueryOutput::Inbox(
            state
                .items
                .values()
                .filter(|i| i.status == ItemStatus::Inbox)
                .map(|i| InboxEntry {
                    id: i.id.as_uuid(),
                    short: ids::short(&i.id.as_uuid(), v.item_len),
                    text: i.title.clone(),
                    captured_at: i.created_at,
                })
                .collect(),
        ),
        Query::List { identity } => {
            let f = filter(identity)?;
            QueryOutput::List(v.actionable(f).into_iter().map(|i| v.item(i)).collect())
        }
        Query::Tree { root, all } => {
            let mut seen = HashSet::new();
            let nodes = match root {
                Some(r) => {
                    let id = resolve::item(state, r)?;
                    state
                        .items
                        .get(&id)
                        .map(|i| vec![tree_node(&v, i, *all, &mut seen)])
                        .unwrap_or_default()
                }
                None => state
                    .items
                    .values()
                    .filter(|i| i.parent.is_none())
                    .filter(|i| {
                        matches!(
                            i.status,
                            ItemStatus::Todo | ItemStatus::Done | ItemStatus::Abandoned
                        )
                    })
                    .filter(|i| *all || !i.is_settled())
                    .map(|i| tree_node(&v, i, *all, &mut seen))
                    .collect(),
            };
            QueryOutput::Tree(nodes)
        }
        Query::Show { item } => {
            let id = resolve::item(state, item)?;
            let i = state
                .items
                .get(&id)
                .ok_or(crate::state::Rejection::UnknownItem(id))
                .map_err(crate::command::CommandError::from)?;
            QueryOutput::Show(Box::new(ItemDetail {
                item: v.item(i),
                notes: i.notes.clone(),
                prerequisites: v.items(i.depends_on.iter().copied()),
                dependents: v.items(
                    state
                        .items
                        .values()
                        .filter(|o| o.depends_on.contains(&id))
                        .map(|o| o.id),
                ),
                children: v.items(i.children.iter().copied()),
                settled_at: i.settled_at,
                completion_note: i.completion_note.clone(),
                abandon: i.abandon.as_ref().map(|(reason, note)| AbandonView {
                    reason: *reason,
                    note: note.clone(),
                }),
            }))
        }
        Query::Habits => {
            let mut out = Vec::new();
            for h in state.habits.values() {
                out.push(v.habit(h)?);
            }
            QueryOutput::Habits(out)
        }
        Query::Identities => QueryOutput::Identities(
            state
                .identities
                .values()
                .map(|i| IdentityView {
                    id: i.id.as_uuid(),
                    name: i.name.clone(),
                    open_items: state
                        .items
                        .values()
                        .filter(|t| t.status == ItemStatus::Todo && t.identity == Some(i.id))
                        .count(),
                    habits: state.habits.values().filter(|h| h.identity == i.id).count(),
                })
                .collect(),
        ),
        Query::Status => {
            let mut habits_left = 0;
            for h in state.habits.values() {
                let st = habit::today_status(h, today, settings)?;
                if st.due && st.value < 1.0 {
                    habits_left += 1;
                }
            }
            QueryOutput::Status(StatusLine {
                inbox: inbox_count,
                habits_left,
                due: state
                    .items
                    .values()
                    .filter(|i| {
                        i.status == ItemStatus::Todo && i.deadline.is_some_and(|d| d <= today)
                    })
                    .count(),
                follow_ups: v.follow_ups(None).len(),
                warnings,
            })
        }
        Query::Doctor => QueryOutput::Doctor(DoctorView {
            load: load_warnings.to_vec(),
            projection: state.warnings.clone(),
        }),
    })
}
