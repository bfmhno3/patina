//! Pure conversions between core views and the Slint data contract, and from form
//! input back to core commands. No I/O, no clock reads.

use jiff::civil::Date;
use jiff::{Timestamp, ZonedDateTime};
use patina_core::command::{self, AddItem, Triage, Update};
use patina_core::event::{Frequency, HabitKind, Repeat, RepeatUnit, Size};
use patina_core::query::{
    HabitView, ItemDetail, ItemView, NextView, ProjectProgress, ReviewView, RhythmView, TimerView,
    TodayView, TreeNode, ViewStatus,
};
use patina_core::recommend::{self, Card, Reason};
use patina_core::rhythm::BreakDue;
use patina_core::time::{self, Weekday};
use slint::{ModelRc, SharedString};

use crate::{
    BreakKindUi, CardData, CardKind, ConvRow, CountRow, DecisionRow, DetailData, FocusGroup,
    FrequencyUi, GroupRow, HabitKindUi, HabitRow, InboxRow, InterruptRow, ItemForm, ItemRow,
    NoteRow, PostponedRow, ProjectRow, ReasonData, ReasonKind, ReviewData, RhythmData,
    RhythmModeUi, TagRow, TimeRow, TimerData, TodayData, TrendRow, WaitRow, WeekPoint,
};

pub fn ss(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(slint::VecModel::from(v))
}

fn int(n: impl TryInto<i32>) -> i32 {
    n.try_into().unwrap_or(i32::MAX)
}

/// `HH:MM` in `now`'s zone.
pub fn local_hm(ts: Timestamp, now: &ZonedDateTime) -> String {
    ts.to_zoned_datetime(now.time_zone().clone())
        .strftime("%H:%M")
        .to_string()
}

/// `YYYY-MM-DD` in `now`'s zone.
pub fn local_day(ts: Timestamp, now: &ZonedDateTime) -> String {
    ts.to_zoned_datetime(now.time_zone().clone())
        .date()
        .to_string()
}

fn opt_date(d: Option<Date>) -> SharedString {
    d.map_or_else(SharedString::new, |d| ss(d.to_string()))
}

fn size_letter(s: Size) -> &'static str {
    match s {
        Size::S => "S",
        Size::M => "M",
        Size::L => "L",
    }
}

fn size_index(s: Size) -> i32 {
    match s {
        Size::S => 1,
        Size::M => 2,
        Size::L => 3,
    }
}

fn status_key(s: ViewStatus) -> &'static str {
    match s {
        ViewStatus::Inbox => "inbox",
        ViewStatus::Todo => "todo",
        ViewStatus::InProgress => "in_progress",
        ViewStatus::Blocked => "blocked",
        ViewStatus::Waiting => "waiting",
        ViewStatus::Done => "done",
        ViewStatus::Abandoned => "abandoned",
    }
}

pub fn item_row(i: &ItemView, depth: usize) -> ItemRow {
    ItemRow {
        id: ss(i.id.to_string()),
        short: ss(&i.short),
        title: ss(&i.title),
        path: ss(i.path.join(" / ")),
        identity: ss(i.identity.as_deref().unwrap_or("")),
        status: ss(status_key(i.status)),
        defer: opt_date(i.defer),
        scheduled: opt_date(i.scheduled),
        deadline: opt_date(i.deadline),
        follow_up: opt_date(i.follow_up),
        waiting_for: ss(i.waiting_for.as_deref().unwrap_or("")),
        tags: ss(i
            .tags
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ")),
        tracked_minutes: int(i.tracked_minutes),
        estimate_minutes: i.estimate.map_or(0, |e| int(e.minutes)),
        open_children: int(i.open_children),
        children: int(i.children),
        depth: int(depth),
        estimate: ss(i.estimate.map_or("", |e| size_letter(e.size))),
        important: i.important,
        over_estimate: i.over_estimate,
        wrap_up: i.wrap_up,
        repeat: i.repeat.is_some(),
    }
}

pub fn item_rows(items: &[ItemView]) -> ModelRc<ItemRow> {
    model(items.iter().map(|i| item_row(i, 0)).collect())
}

fn weekday_key(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

pub fn habit_row(h: &HabitView) -> HabitRow {
    let (kind, target) = match h.kind {
        HabitKind::Check => (HabitKindUi::Check, 0),
        HabitKind::Count { target } => (HabitKindUi::Count, int(target)),
        HabitKind::Duration { minutes } => (HabitKindUi::Duration, int(minutes)),
    };
    let (frequency, times, days) = match &h.frequency {
        Frequency::Daily => (FrequencyUi::Daily, 0, Vec::new()),
        Frequency::Weekly { times } => (FrequencyUi::Weekly, i32::from(*times), Vec::new()),
        Frequency::Weekdays { days } => (
            FrequencyUi::Weekdays,
            0,
            days.iter().map(|d| ss(weekday_key(*d))).collect(),
        ),
    };
    HabitRow {
        id: ss(h.id.to_string()),
        name: ss(&h.name),
        identity: ss(&h.identity),
        cue: ss(h.cue.as_deref().unwrap_or("")),
        kind,
        target,
        frequency,
        times,
        days: model(days),
        paused: h.paused,
        due: h.due,
        strength: h.strength as f32,
        value: h.value as f32,
        week_value: h.week_value.unwrap_or(0.0) as f32,
        count: h.count.map_or(0, int),
        minutes: h.minutes.map_or(0, int),
        week_target: h.week_target.map_or(0, i32::from),
    }
}

pub fn habit_rows(habits: &[HabitView]) -> ModelRc<HabitRow> {
    model(habits.iter().map(habit_row).collect())
}

fn reason(r: &Reason) -> ReasonData {
    let plain = |kind| ReasonData {
        kind,
        value: 0,
        size: SharedString::new(),
    };
    match r {
        Reason::Deadline { days } => ReasonData {
            value: int(*days),
            ..plain(ReasonKind::Deadline)
        },
        Reason::Scheduled { days_late } => ReasonData {
            value: int(*days_late),
            ..plain(ReasonKind::Scheduled)
        },
        Reason::Unlocks { count } => ReasonData {
            value: int(*count),
            ..plain(ReasonKind::Unlocks)
        },
        Reason::Downstream { count } => ReasonData {
            value: int(*count),
            ..plain(ReasonKind::Downstream)
        },
        Reason::Focus => plain(ReasonKind::Focus),
        Reason::Important => plain(ReasonKind::Important),
        Reason::Small { size } => ReasonData {
            size: ss(size_letter(*size)),
            ..plain(ReasonKind::Small)
        },
        Reason::RecentlySkipped { count } => ReasonData {
            value: int(*count),
            ..plain(ReasonKind::RecentlySkipped)
        },
        Reason::FirstStep => plain(ReasonKind::FirstStep),
    }
}

/// Identity of a card for "was this card already recorded as shown".
pub fn card_key(nv: &NextView) -> (recommend::CardKind, Option<uuid::Uuid>, Option<String>) {
    let item = match &nv.card {
        Card::Item { item, .. } | Card::BreakDown { item, .. } => Some(item.id),
        _ => None,
    };
    (nv.card.kind(), item, nv.scope.clone())
}

pub fn card(nv: &NextView, now: &ZonedDateTime) -> CardData {
    let mut out = CardData {
        kind: CardKind::Empty,
        item: ItemRow::default(),
        reasons: model(Vec::new()),
        inbox: 0,
        since: SharedString::new(),
        elsewhere: model(Vec::new()),
    };
    match &nv.card {
        Card::Item { item, reasons, .. } | Card::BreakDown { item, reasons, .. } => {
            out.kind = if matches!(nv.card, Card::Item { .. }) {
                CardKind::Item
            } else {
                CardKind::BreakDown
            };
            out.item = item_row(item, 0);
            out.reasons = model(reasons.iter().map(reason).collect());
        }
        Card::Review { since } => {
            out.kind = CardKind::Review;
            out.since = ss(since.map_or_else(String::new, |s| local_day(s, now)));
        }
        Card::Triage { inbox } => {
            out.kind = CardKind::Triage;
            out.inbox = int(*inbox);
        }
        Card::Empty { elsewhere } => {
            out.elsewhere = model(
                elsewhere
                    .iter()
                    .map(|e| CountRow {
                        label: ss(&e.identity),
                        count: int(e.actionable),
                    })
                    .collect(),
            );
        }
    }
    out
}

pub fn timer_data(t: Option<&TimerView>, now: &ZonedDateTime) -> TimerData {
    match t {
        None => TimerData::default(),
        Some(t) => TimerData {
            running: true,
            label: ss(&t.target.label),
            target_id: ss(t.target.id.to_string()),
            is_habit: matches!(t.target.kind, patina_core::engine::SubjectKind::Habit),
            minutes: int(t.minutes),
            started: ss(local_hm(t.started_at, now)),
        },
    }
}

fn break_kind(k: patina_core::event::BreakKind) -> BreakKindUi {
    use patina_core::event::BreakKind;
    match k {
        BreakKind::Short => BreakKindUi::Short,
        BreakKind::Long => BreakKindUi::Long,
        BreakKind::Flow => BreakKindUi::Flow,
    }
}

pub fn break_kind_of(d: &BreakDue) -> BreakKindUi {
    break_kind(d.kind)
}

pub fn rhythm_mode(m: patina_core::event::RhythmMode) -> RhythmModeUi {
    use patina_core::event::RhythmMode;
    match m {
        RhythmMode::Fixed => RhythmModeUi::Fixed,
        RhythmMode::Flowtime => RhythmModeUi::Flowtime,
        RhythmMode::Off => RhythmModeUi::Off,
    }
}

pub fn rhythm_mode_core(m: RhythmModeUi) -> patina_core::event::RhythmMode {
    use patina_core::event::RhythmMode;
    match m {
        RhythmModeUi::Fixed => RhythmMode::Fixed,
        RhythmModeUi::Flowtime => RhythmMode::Flowtime,
        RhythmModeUi::Off => RhythmMode::Off,
    }
}

pub fn rhythm_data(r: &RhythmView, now: &ZonedDateTime) -> RhythmData {
    let mut out = RhythmData {
        mode: rhythm_mode(r.mode),
        focus_minutes: int(r.focus_minutes),
        round: int(r.round),
        ..RhythmData::default()
    };
    if let Some(d) = &r.due {
        out.due = true;
        out.due_kind = break_kind_of(d);
        out.due_minutes = int(d.minutes);
    }
    if let Some(b) = &r.on_break {
        out.on_break = true;
        out.break_kind = break_kind(b.kind);
        out.break_ends = ss(local_hm(b.ends_at, now));
    }
    if let Some(s) = &r.resume {
        out.resume_label = ss(&s.label);
        out.resume_id = ss(s.id.to_string());
        out.resume_is_habit = matches!(s.kind, patina_core::engine::SubjectKind::Habit);
    }
    out
}

pub fn today_data(t: &TodayView, has_identities: bool) -> TodayData {
    let banner_items: Vec<ItemView> = t
        .banner
        .deadlines
        .iter()
        .chain(&t.banner.follow_ups)
        .cloned()
        .collect();
    TodayData {
        today: ss(t.today.to_string()),
        scope_label: ss(t.scope.as_deref().unwrap_or("")),
        habits: habit_rows(&t.habits),
        follow_ups: item_rows(&t.follow_ups),
        deadlines: item_rows(&t.deadlines),
        scheduled: item_rows(&t.scheduled),
        actionable: item_rows(&t.actionable),
        needs_decision: model(
            t.needs_decision
                .iter()
                .map(|n| DecisionRow {
                    item: item_row(&n.item, 0),
                    abandoned: item_rows(&n.abandoned),
                })
                .collect(),
        ),
        inbox: int(t.inbox),
        warnings: int(t.warnings),
        review_due: t.review_due,
        has_identities,
        banner_items: item_rows(&banner_items),
        banner_habits: habit_rows(&t.banner.habits),
    }
}

/// Depth-first rows of a tree.
pub fn flatten_tree(nodes: &[TreeNode]) -> Vec<ItemRow> {
    fn walk(nodes: &[TreeNode], depth: usize, out: &mut Vec<ItemRow>) {
        for n in nodes {
            out.push(item_row(&n.item, depth));
            walk(&n.children, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, &mut out);
    out
}

fn repeat_parts(r: Option<&Repeat>) -> (i32, i32, i32) {
    match r {
        None => (0, 1, 0),
        Some(Repeat::Every { every, unit }) => (
            1,
            int(*every),
            match unit {
                RepeatUnit::Day => 0,
                RepeatUnit::Week => 1,
                RepeatUnit::Month => 2,
            },
        ),
        Some(Repeat::AfterCompletion { days }) => (2, int(*days), 0),
    }
}

pub fn detail_data(d: &ItemDetail, now: &ZonedDateTime) -> DetailData {
    let i = &d.item;
    let (repeat_kind, repeat_n, repeat_unit) = repeat_parts(i.repeat.as_ref());
    DetailData {
        item: item_row(i, 0),
        parent_id: ss(i.parent.map_or_else(String::new, |p| p.to_string())),
        tags_raw: ss(i.tags.join(" ")),
        waiting_for: ss(i.waiting_for.as_deref().unwrap_or("")),
        follow_up: opt_date(i.follow_up),
        defer: opt_date(i.defer),
        settled: ss(d.settled_at.map_or_else(String::new, |t| local_day(t, now))),
        completion_note: ss(d.completion_note.as_deref().unwrap_or("")),
        abandon_reason: ss(d.abandon.as_ref().map_or("", |a| abandon_key(a.reason))),
        abandon_note: ss(d
            .abandon
            .as_ref()
            .and_then(|a| a.note.as_deref())
            .unwrap_or("")),
        left_off: ss(d.left_off.as_deref().unwrap_or("")),
        size: i.estimate.map_or(0, |e| size_index(e.size)),
        repeat_kind,
        repeat_n,
        repeat_unit,
        children_estimate_minutes: d.children_estimate_minutes.map_or(-1, int),
        is_external: i.waiting_for.is_some(),
        notes: model(
            d.notes
                .iter()
                .map(|n| NoteRow {
                    day: ss(local_day(n.at, now)),
                    kind: SharedString::new(),
                    subject: SharedString::new(),
                    text: ss(&n.text),
                })
                .collect(),
        ),
        prerequisites: item_rows(&d.prerequisites),
        dependents: item_rows(&d.dependents),
        children: item_rows(&d.children),
    }
}

/// The index the abandon dialog uses for `r` (see [`abandon_reason`]), as text.
fn abandon_key(r: patina_core::event::AbandonReason) -> &'static str {
    use patina_core::event::AbandonReason;
    match r {
        AbandonReason::NoLongerImportant => "0",
        AbandonReason::Superseded => "1",
        AbandonReason::TooHard => "2",
        AbandonReason::Other => "3",
    }
}

pub fn abandon_reason(i: i32) -> patina_core::event::AbandonReason {
    use patina_core::event::AbandonReason;
    match i {
        0 => AbandonReason::NoLongerImportant,
        1 => AbandonReason::Superseded,
        2 => AbandonReason::TooHard,
        _ => AbandonReason::Other,
    }
}

pub fn inbox_row(e: &patina_core::query::InboxEntry, now: &ZonedDateTime) -> InboxRow {
    InboxRow {
        id: ss(e.id.to_string()),
        text: ss(&e.text),
        captured: ss(local_day(e.captured_at, now)),
        identity_hint: ss(e.identity_hint.as_deref().unwrap_or("")),
    }
}

fn project_row(p: &ProjectProgress) -> ProjectRow {
    ProjectRow {
        item: item_row(&p.project, 0),
        completed: int(p.completed),
        done: int(p.done),
        open: int(p.open),
        open_change: int(p.open_change),
    }
}

/// The review page data; `tree` is the open item tree, used for the focus pickers.
pub fn review_data(
    r: &ReviewView,
    tree: &[TreeNode],
    identities: &[String],
    now: &ZonedDateTime,
) -> ReviewData {
    let focus_groups = identities
        .iter()
        .map(|name| {
            let mut titles = vec![ss("—")];
            let mut ids = vec![SharedString::new()];
            for n in tree
                .iter()
                .filter(|n| n.item.identity.as_deref() == Some(name))
            {
                titles.push(ss(&n.item.title));
                ids.push(ss(n.item.id.to_string()));
            }
            FocusGroup {
                identity: ss(name),
                titles: model(titles),
                ids: model(ids),
            }
        })
        .collect();
    ReviewData {
        since: ss(r.since.map_or_else(String::new, |s| local_day(s, now))),
        due: r.due,
        completed: model(
            r.completed
                .iter()
                .map(|g| GroupRow {
                    label: ss(&g.identity),
                    items: item_rows(&g.items),
                })
                .collect(),
        ),
        projects: model(r.projects.iter().map(project_row).collect()),
        open_projects: model(r.open_projects.iter().map(project_row).collect()),
        time: model(
            r.time
                .iter()
                .map(|t| TimeRow {
                    identity: ss(&t.identity),
                    tracked: int(t.tracked_minutes),
                    target: t.target_minutes.map_or(-1, int),
                    deviation: t.deviation_minutes.map_or(0, int),
                    mode: int(t.mode_minutes),
                })
                .collect(),
        ),
        interruptions: int(r.interruptions.len()),
        sources: model(
            r.interruption_sources
                .iter()
                .map(|s| CountRow {
                    label: ss(s.identity.as_deref().unwrap_or("")),
                    count: int(s.count),
                })
                .collect(),
        ),
        interrupt_items: model(
            r.interruptions
                .iter()
                .map(|i| InterruptRow {
                    day: ss(local_day(i.at, now)),
                    title: ss(&i.title),
                    identity: ss(i.identity.as_deref().unwrap_or("")),
                    handled_now: matches!(i.outcome, patina_core::event::InterruptOutcome::Now),
                })
                .collect(),
        ),
        habits: model(
            r.habits
                .iter()
                .map(|h| TrendRow {
                    habit: habit_row(&h.habit),
                    start: h.start_strength as f32,
                    change: h.change as f32,
                    curve: model(h.curve.iter().map(|p| p.strength as f32).collect()),
                })
                .collect(),
        ),
        calib_error: r.calibration.error.unwrap_or(0.0) as f32,
        calib_samples: int(r.calibration.samples),
        calib_weeks: model(
            r.calibration
                .weeks
                .iter()
                .map(|w| WeekPoint {
                    label: ss(w.week_start.strftime("%m-%d").to_string()),
                    error: w.error.unwrap_or(0.0) as f32,
                    samples: int(w.samples),
                })
                .collect(),
        ),
        calib_tags: model(
            r.calibration
                .by_tag
                .iter()
                .map(|t| TagRow {
                    tag: ss(&t.tag),
                    error: t.error as f32,
                    samples: int(t.samples),
                })
                .collect(),
        ),
        conversions: model(
            r.calibration
                .conversions
                .iter()
                .map(|c| ConvRow {
                    tag: ss(c.tag.as_deref().unwrap_or("")),
                    size: ss(size_letter(c.size)),
                    minutes: int(c.minutes),
                    default_minutes: int(c.default_minutes),
                    samples: int(c.samples),
                })
                .collect(),
        ),
        postponed: model(
            r.postponed
                .iter()
                .map(|p| PostponedRow {
                    item: item_row(&p.item, 0),
                    count: int(p.count),
                })
                .collect(),
        ),
        waiting: model(
            r.waiting
                .iter()
                .map(|w| WaitRow {
                    item: item_row(&w.item, 0),
                    days: int(w.days),
                })
                .collect(),
        ),
        inbox: model(r.inbox.iter().map(|e| inbox_row(e, now)).collect()),
        notes: model(
            r.notes
                .iter()
                .map(|n| NoteRow {
                    day: ss(n.day.to_string()),
                    kind: ss(match n.kind {
                        patina_core::query::NoteKind::Completion => "completion",
                        patina_core::query::NoteKind::CheckIn => "check_in",
                    }),
                    subject: ss(&n.subject),
                    text: ss(&n.text),
                })
                .collect(),
        ),
        focus_current: item_rows(&r.focus),
        focus_groups: model(focus_groups),
    }
}

const BAD_DATE: &str = "use YYYY-MM-DD, today, tomorrow, or +N";

/// Parses a date field; empty is none.
pub fn date_field(s: &str, today: Date) -> Result<Option<Date>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    time::parse_date(s, today)
        .map(Some)
        .ok_or_else(|| format!("invalid date {s:?} ({BAD_DATE})"))
}

/// Words split on whitespace and commas, without a leading `#`.
pub fn parse_tags(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || c == ',')
        .map(|t| t.trim().trim_start_matches('#'))
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

pub fn opt_text(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Optional size from the form (0 is none, 1..=3 is S, M, L).
pub fn size_of(i: i32) -> Option<Size> {
    match i {
        1 => Some(Size::S),
        2 => Some(Size::M),
        3 => Some(Size::L),
        _ => None,
    }
}

fn repeat_of(f: &ItemForm) -> Result<Option<Repeat>, String> {
    let n = u32::try_from(f.repeat_n).unwrap_or(0);
    match f.repeat_kind {
        1 => {
            if n == 0 {
                return Err("repeat intervals must be positive".into());
            }
            let unit = match f.repeat_unit {
                1 => RepeatUnit::Week,
                2 => RepeatUnit::Month,
                _ => RepeatUnit::Day,
            };
            Ok(Some(Repeat::Every { every: n, unit }))
        }
        2 => {
            if n == 0 {
                return Err("repeat intervals must be positive".into());
            }
            Ok(Some(Repeat::AfterCompletion { days: n }))
        }
        _ => Ok(None),
    }
}

/// The date fields of an item form: defer, scheduled, deadline, follow-up.
pub type FormDates = (Option<Date>, Option<Date>, Option<Date>, Option<Date>);

/// Parses the four date fields of `f`.
pub fn item_form_dates(f: &ItemForm, today: Date) -> Result<FormDates, String> {
    Ok((
        date_field(&f.defer, today)?,
        date_field(&f.scheduled, today)?,
        date_field(&f.deadline, today)?,
        date_field(&f.follow_up, today)?,
    ))
}

pub fn add_item_command(f: &ItemForm, today: Date) -> Result<AddItem, String> {
    let (defer, scheduled, deadline, follow_up) = item_form_dates(f, today)?;
    Ok(AddItem {
        title: f.title.to_string(),
        identity: opt_text(&f.identity),
        parent: opt_text(&f.parent_id),
        tags: parse_tags(&f.tags),
        defer,
        scheduled,
        deadline,
        waiting_for: opt_text(&f.waiting_for),
        follow_up,
        important: f.important,
        size: size_of(f.size),
        repeat: repeat_of(f)?,
    })
}

pub fn triage_command(id: &str, f: &ItemForm, today: Date) -> Result<Triage, String> {
    let (defer, scheduled, deadline, follow_up) = item_form_dates(f, today)?;
    Ok(Triage {
        item: id.to_string(),
        identity: opt_text(&f.identity),
        title: opt_text(&f.title),
        parent: opt_text(&f.parent_id),
        tags: parse_tags(&f.tags),
        defer,
        scheduled,
        deadline,
        waiting_for: opt_text(&f.waiting_for),
        follow_up,
        important: f.important,
        size: size_of(f.size),
        repeat: repeat_of(f)?,
    })
}

/// An edit that changes nothing.
pub fn blank_edit(item: &str) -> command::Edit {
    command::Edit {
        item: item.to_string(),
        title: None,
        identity: None,
        add_tags: Vec::new(),
        remove_tags: Vec::new(),
        defer: None,
        scheduled: None,
        deadline: None,
        follow_up: None,
        waiting_for: None,
        important: None,
        size: None,
        repeat: None,
    }
}

fn date_update(
    field: &str,
    current: Option<Date>,
    today: Date,
) -> Result<Option<Update<Date>>, String> {
    match date_field(field, today)? {
        None => Ok(current.map(|_| Update::Clear)),
        Some(d) => Ok((Some(d) != current).then_some(Update::Set(d))),
    }
}

/// The edit that turns `d`'s item into what `f` describes; unchanged fields stay `None`.
pub fn edit_command(d: &ItemDetail, f: &ItemForm, today: Date) -> Result<command::Edit, String> {
    let cur = &d.item;
    let mut e = blank_edit(&cur.id.to_string());
    let title = f.title.trim();
    e.title = (!title.is_empty() && title != cur.title).then(|| title.to_string());
    e.identity = opt_text(&f.identity).filter(|i| Some(i.as_str()) != cur.identity.as_deref());
    let wanted = parse_tags(&f.tags);
    e.add_tags = wanted
        .iter()
        .filter(|t| !cur.tags.contains(t))
        .cloned()
        .collect();
    e.remove_tags = cur
        .tags
        .iter()
        .filter(|t| !wanted.contains(t))
        .cloned()
        .collect();
    e.defer = date_update(&f.defer, cur.defer, today)?;
    e.scheduled = date_update(&f.scheduled, cur.scheduled, today)?;
    e.deadline = date_update(&f.deadline, cur.deadline, today)?;
    e.follow_up = date_update(&f.follow_up, cur.follow_up, today)?;
    e.waiting_for = match (opt_text(&f.waiting_for), cur.waiting_for.as_deref()) {
        (None, Some(_)) => Some(Update::Clear),
        (Some(w), cur) if Some(w.as_str()) != cur => Some(Update::Set(w)),
        _ => None,
    };
    e.important = (f.important != cur.important).then_some(f.important);
    let cur_size = cur.estimate.map(|s| s.size);
    e.size = match size_of(f.size) {
        None => cur_size.map(|_| Update::Clear),
        Some(s) => (Some(s) != cur_size).then_some(Update::Set(s)),
    };
    let repeat = repeat_of(f)?;
    e.repeat = match repeat {
        None => cur.repeat.map(|_| Update::Clear),
        Some(r) => (Some(r) != cur.repeat).then_some(Update::Set(r)),
    };
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> ItemForm {
        ItemForm {
            repeat_n: 1,
            ..ItemForm::default()
        }
    }

    #[test]
    fn tags_split_on_spaces_and_commas() {
        assert_eq!(parse_tags("#a, b  #c,,"), ["a", "b", "c"]);
    }

    #[test]
    fn bad_dates_name_the_input() {
        let today: Date = "2026-11-24".parse().unwrap();
        let mut f = form();
        f.deadline = ss("someday");
        let err = item_form_dates(&f, today).unwrap_err();
        assert_eq!(
            err,
            "invalid date \"someday\" (use YYYY-MM-DD, today, tomorrow, or +N)"
        );
        f.deadline = ss("+2");
        assert_eq!(
            item_form_dates(&f, today).unwrap().2.unwrap().to_string(),
            "2026-11-26"
        );
    }

    #[test]
    fn repeat_needs_a_positive_interval() {
        let mut f = form();
        f.repeat_kind = 1;
        f.repeat_n = 0;
        assert!(repeat_of(&f).is_err());
        f.repeat_n = 2;
        f.repeat_unit = 1;
        assert_eq!(
            repeat_of(&f).unwrap(),
            Some(Repeat::Every {
                every: 2,
                unit: RepeatUnit::Week
            })
        );
    }
}
