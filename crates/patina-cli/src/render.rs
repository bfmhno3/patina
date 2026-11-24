//! Human-readable output (English, no color).

use std::fmt::Write as _;

use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use patina_core::command::Command;
use patina_core::engine::{CommandOutcome, Subject};
use patina_core::event::{EventData, Frequency, HabitKind};
use patina_core::ids;
use patina_core::query::{
    Banner, HabitView, ItemDetail, ItemView, ModeView, QueryOutput, TimerView, TodayView, TreeNode,
    ViewStatus,
};
use patina_core::time::Weekday;

fn sref(s: Option<&Subject>) -> &str {
    s.and_then(|s| s.short.as_deref()).unwrap_or("-")
}

fn label(s: Option<&Subject>) -> &str {
    s.map_or("", |s| s.label.as_str())
}

fn weekday_name(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

/// Confirmation text for a command.
pub fn outcome(cmd: &Command, o: &CommandOutcome, now: &ZonedDateTime) -> String {
    let first = o.subjects.first();
    let (s, l) = (sref(first), label(first));
    let mut out = String::new();
    match cmd {
        Command::Init => {
            let names: Vec<&str> = o.subjects.iter().map(|s| s.label.as_str()).collect();
            let _ = writeln!(out, "initialized identities: {}", names.join(", "));
        }
        Command::Capture(_) => out.push_str("captured\n"),
        Command::AddItem(_) => {
            let _ = writeln!(out, "created {s} {l}");
        }
        Command::Triage(_) => {
            let _ = writeln!(out, "triaged {s} {l}");
        }
        Command::Discard(_) => {
            let _ = writeln!(out, "discarded {s}");
        }
        Command::Merge(_) => {
            let _ = writeln!(out, "merged into {s} {l}");
        }
        Command::Edit(_) => {
            let _ = writeln!(out, "edited {s} {l}");
        }
        Command::Note(_) => {
            let _ = writeln!(out, "noted {s}");
        }
        Command::Start(_) => {
            let _ = writeln!(out, "started {s} {l}");
        }
        Command::Complete(_) => {
            let _ = writeln!(out, "done {s} {l}");
        }
        Command::Abandon(_) => {
            for sub in &o.subjects {
                let _ = writeln!(out, "abandoned {} {}", sref(Some(sub)), sub.label);
            }
            if !o.needs_decision.is_empty() {
                out.push_str("needs decision:\n");
                for sub in &o.needs_decision {
                    let _ = writeln!(out, "  {} {}", sref(Some(sub)), sub.label);
                }
                out.push_str(
                    "  resolve with 'patina unlink <ref> --on <ref>' or 'patina abandon <ref>'\n",
                );
            }
        }
        Command::Move(_) => {
            let _ = writeln!(out, "moved {s}");
        }
        Command::Link(_) => {
            let _ = writeln!(out, "{s} now depends on {}", sref(o.subjects.get(1)));
        }
        Command::Unlink(_) => {
            let _ = writeln!(out, "{s} no longer depends on {}", sref(o.subjects.get(1)));
        }
        Command::AddHabit(_) => {
            let _ = writeln!(out, "created habit {s} {l}");
        }
        Command::EditHabit(_) => {
            let _ = writeln!(out, "edited habit {s} {l}");
        }
        Command::CheckIn(_) => {
            for e in &o.events {
                if let EventData::HabitCheckedIn(d) = &e.data {
                    let back = if d.backfilled { " (backfilled)" } else { "" };
                    let _ = writeln!(out, "checked {l} for {}{back}", d.day);
                }
            }
        }
        Command::SkipHabit(_) => {
            for e in &o.events {
                if let EventData::HabitSkipped(d) = &e.data {
                    let _ = writeln!(out, "skipped {l} on {}", d.day);
                }
            }
        }
        Command::PauseHabit(_) => {
            for e in &o.events {
                if let EventData::HabitPaused(d) = &e.data {
                    let _ = writeln!(out, "paused {l} from {}", d.from);
                }
            }
        }
        Command::ResumeHabit(_) => {
            for e in &o.events {
                if let EventData::HabitResumed(d) = &e.data {
                    let _ = writeln!(out, "resumed {l} from {}", d.from);
                }
            }
        }
        Command::AddIdentity(_) => {
            let _ = writeln!(out, "added identity {l}");
        }
        Command::RenameIdentity(_) => {
            let _ = writeln!(out, "renamed identity to {l}");
        }
        Command::Undo => {
            let _ = writeln!(out, "undid: {}", o.undone.join(", "));
            for sub in &o.subjects {
                let _ = writeln!(out, "  {} {}", sref(Some(sub)), sub.label);
            }
        }
        Command::SwitchMode(c) => {
            let name = o.subjects.first().map_or("away", |s| s.label.as_str());
            let _ = write!(out, "mode: {name}");
            if let Some(at) = c.at {
                let _ = write!(out, " from {}", stamp(at, now));
            }
            out.push('\n');
        }
    }
    out
}

fn item_line(i: &ItemView, show_identity: bool) -> String {
    let mut s = format!("{}  ", i.short);
    for p in &i.path {
        s.push_str(p);
        s.push_str(" > ");
    }
    s.push_str(&i.title);
    for t in &i.tags {
        let _ = write!(s, " #{t}");
    }
    if show_identity && let Some(id) = &i.identity {
        let _ = write!(s, " [{id}]");
    }
    if let Some(d) = i.deadline {
        let _ = write!(s, " due {d}");
    }
    if let Some(d) = i.scheduled {
        let _ = write!(s, " sched {d}");
    }
    if let Some(w) = &i.waiting_for {
        let _ = write!(s, " waiting: {w}");
    }
    if let Some(d) = i.follow_up {
        let _ = write!(s, " follow-up {d}");
    }
    if !i.blocked_by.is_empty() {
        let shorts: Vec<String> = i
            .blocked_by
            .iter()
            .map(|b| ids::short(b, i.short.len()))
            .collect();
        let _ = write!(s, " (blocked by {})", shorts.join(", "));
    }
    if i.wrap_up {
        s.push_str(" (ready to wrap up)");
    }
    s
}

fn frequency_text(f: &Frequency) -> String {
    match f {
        Frequency::Daily => "daily".into(),
        Frequency::Weekly { times } => format!("{times}x/week"),
        Frequency::Weekdays { days } => days
            .iter()
            .map(|d| weekday_name(*d))
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn habit_line(h: &HabitView) -> String {
    let state = if let (Some(v), Some(t)) = (h.week_value, h.week_target) {
        format!("{v}/{t} this week")
    } else if h.value >= 1.0 {
        "done".to_string()
    } else if let HabitKind::Count { target } = h.kind {
        format!("{}/{target}", h.count.unwrap_or(0))
    } else {
        "todo".to_string()
    };
    format!(
        "{}  {}  [{}]  {state}  strength {}%",
        h.short,
        h.name,
        h.identity,
        (h.strength * 100.0).round()
    )
}

/// `HH:MM` for a time on `now`'s date, else `YYYY-MM-DD HH:MM` (both accepted by `--at`).
fn stamp(ts: Timestamp, now: &ZonedDateTime) -> String {
    let z = ts.to_zoned_datetime(now.time_zone().clone());
    let fmt = if z.date() == now.date() {
        "%H:%M"
    } else {
        "%Y-%m-%d %H:%M"
    };
    z.strftime(fmt).to_string()
}

/// Hints about a forgotten mode or a forgotten timer.
fn hints(mode: &ModeView, timer: Option<&TimerView>, now: &ZonedDateTime) -> String {
    let mut out = String::new();
    if mode.auto_away
        && let (Some(name), Some(last)) = (&mode.identity, mode.last_activity)
    {
        let t = stamp(last, now);
        let _ = writeln!(
            out,
            "hint: no activity since {t}; '{name}' mode may have been left on. Correct with 'patina mode away --at {t}' or continue with 'patina mode {name}'."
        );
    }
    if let Some(timer) = timer
        && timer.idle
        && let Some(last) = mode.last_activity
    {
        let short = timer.target.short.as_deref().unwrap_or("-");
        let _ = writeln!(
            out,
            "hint: timer on {short} {} started {}, no activity since {}; if you stopped earlier: 'patina stop --at {}'.",
            timer.target.label,
            stamp(timer.started_at, now),
            stamp(last, now),
            stamp(last, now),
        );
    }
    out
}

fn banner(b: &Banner, out: &mut String) -> bool {
    if b.deadlines.is_empty() && b.follow_ups.is_empty() && b.habits.is_empty() {
        return false;
    }
    out.push_str("Elsewhere\n");
    for i in &b.deadlines {
        let _ = writeln!(out, "  {}", item_line(i, true));
    }
    for i in &b.follow_ups {
        let _ = writeln!(out, "  {}", item_line(i, true));
    }
    for h in &b.habits {
        let _ = writeln!(out, "  {}", habit_line(h));
    }
    true
}

fn today(t: &TodayView, now: &ZonedDateTime) -> String {
    let mut out = String::new();
    let mut sections = 0;
    if let Some(scope) = &t.scope {
        let _ = writeln!(out, "[{scope}]");
    }
    if banner(&t.banner, &mut out) {
        sections += 1;
    }
    if !t.habits.is_empty() {
        sections += 1;
        out.push_str("Habits\n");
        for h in &t.habits {
            let _ = writeln!(out, "  {}", habit_line(h));
        }
    }
    let mut section = |title: &str, items: &[ItemView]| {
        if items.is_empty() {
            return;
        }
        sections += 1;
        let _ = writeln!(out, "{title}");
        for i in items {
            let _ = writeln!(out, "  {}", item_line(i, true));
        }
    };
    section("Follow up", &t.follow_ups);
    section("Deadlines", &t.deadlines);
    section("Scheduled", &t.scheduled);
    if !t.needs_decision.is_empty() {
        sections += 1;
        out.push_str("Needs decision\n");
        for n in &t.needs_decision {
            let _ = writeln!(out, "  {}", item_line(&n.item, true));
            let names: Vec<String> = n
                .abandoned
                .iter()
                .map(|a| format!("{} {}", a.short, a.title))
                .collect();
            let _ = writeln!(out, "    waiting on abandoned: {}", names.join(", "));
        }
    }
    let printed: Vec<_> = t
        .deadlines
        .iter()
        .chain(&t.scheduled)
        .map(|i| i.id)
        .collect();
    let actionable: Vec<&ItemView> = t
        .actionable
        .iter()
        .filter(|i| !printed.contains(&i.id))
        .collect();
    if !actionable.is_empty() {
        sections += 1;
        out.push_str("Actionable\n");
        let mut groups: Vec<(String, Vec<&ItemView>)> = Vec::new();
        for i in actionable {
            let name = i.identity.clone().unwrap_or_default();
            match groups.iter_mut().find(|(n, _)| *n == name) {
                Some((_, v)) => v.push(i),
                None => groups.push((name, vec![i])),
            }
        }
        for (name, items) in groups {
            let _ = writeln!(out, "  [{name}]");
            for i in items {
                let _ = writeln!(out, "    {}", item_line(i, false));
            }
        }
    }
    if sections == 0 && t.inbox == 0 {
        out.push_str("nothing to do today\n");
    }
    if t.inbox > 0 {
        let _ = writeln!(out, "inbox: {}", t.inbox);
    }
    out.push_str(&hints(&t.mode, t.timer.as_ref(), now));
    out
}

fn tree(nodes: &[TreeNode], depth: usize, out: &mut String) {
    for n in nodes {
        let marker = match n.item.status {
            ViewStatus::Todo | ViewStatus::Inbox => "[ ]",
            ViewStatus::InProgress => "[>]",
            ViewStatus::Blocked => "[!]",
            ViewStatus::Waiting => "[~]",
            ViewStatus::Done => "[x]",
            ViewStatus::Abandoned => "[-]",
        };
        let mut line = format!(
            "{}{marker} {}  {}",
            "  ".repeat(depth),
            n.item.short,
            n.item.title
        );
        if let Some(d) = n.item.deadline {
            let _ = write!(line, " due {d}");
        }
        if let Some(w) = &n.item.waiting_for {
            let _ = write!(line, " waiting: {w}");
        }
        if n.item.wrap_up {
            line.push_str(" (ready to wrap up)");
        }
        out.push_str(&line);
        out.push('\n');
        tree(&n.children, depth + 1, out);
    }
}

fn show(d: &ItemDetail, tz: &TimeZone) -> String {
    let i = &d.item;
    let mut out = String::new();
    let _ = writeln!(out, "title: {}", i.title);
    let _ = writeln!(out, "ref: {}", i.short);
    let _ = writeln!(out, "id: {}", i.id);
    if let Some(id) = &i.identity {
        let _ = writeln!(out, "identity: {id}");
    }
    let _ = writeln!(out, "status: {}", status_key(i.status));
    if !i.tags.is_empty() {
        let _ = writeln!(out, "tags: {}", i.tags.join(", "));
    }
    for (k, v) in [
        ("defer", i.defer),
        ("scheduled", i.scheduled),
        ("deadline", i.deadline),
        ("follow_up", i.follow_up),
    ] {
        if let Some(v) = v {
            let _ = writeln!(out, "{k}: {v}");
        }
    }
    if let Some(w) = &i.waiting_for {
        let _ = writeln!(out, "waiting_for: {w}");
    }
    if !i.path.is_empty() {
        let _ = writeln!(out, "path: {}", i.path.join(" > "));
    }
    if let Some(a) = &d.abandon {
        let _ = writeln!(out, "abandoned: {:?}", a.reason);
    }
    for (title, items) in [
        ("Prerequisites", &d.prerequisites),
        ("Dependents", &d.dependents),
        ("Children", &d.children),
    ] {
        if !items.is_empty() {
            let _ = writeln!(out, "{title}");
            for it in items {
                let _ = writeln!(out, "  {}", item_line(it, false));
            }
        }
    }
    if !d.notes.is_empty() {
        out.push_str("Notes\n");
        for n in &d.notes {
            let at =
                n.at.to_zoned_datetime(tz.clone())
                    .strftime("%Y-%m-%d %H:%M");
            let _ = writeln!(out, "  {at}  {}", n.text);
        }
    }
    out
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

/// Text for a query result.
pub fn query(o: &QueryOutput, now: &ZonedDateTime) -> String {
    let tz = now.time_zone();
    let mut out = String::new();
    match o {
        QueryOutput::Today(t) => return today(t, now),
        QueryOutput::Mode(m) => {
            let name = m.mode.identity.as_deref().unwrap_or("away");
            if m.mode.auto_away {
                let _ = write!(out, "mode: away (idle, was {name})");
            } else {
                let _ = write!(out, "mode: {name}");
            }
            if let Some(since) = m.mode.since {
                let _ = write!(out, " since {}", stamp(since, now));
            }
            out.push('\n');
            if let Some(t) = &m.timer {
                let short = t.target.short.as_deref().unwrap_or("-");
                let _ = writeln!(out, "timer: {short} {} {}m", t.target.label, t.minutes);
            }
            out.push_str(&hints(&m.mode, m.timer.as_ref(), now));
        }
        QueryOutput::Inbox(entries) => {
            if entries.is_empty() {
                out.push_str("inbox is empty\n");
            }
            for e in entries {
                let _ = writeln!(out, "{}  {}", e.short, e.text);
            }
        }
        QueryOutput::List(items) => {
            for i in items {
                let _ = writeln!(out, "{}", item_line(i, true));
            }
        }
        QueryOutput::Tree(nodes) => tree(nodes, 0, &mut out),
        QueryOutput::Show(d) => return show(d, tz),
        QueryOutput::Habits(habits) => {
            for h in habits {
                let paused = if h.paused { "  paused" } else { "" };
                let _ = writeln!(
                    out,
                    "{}  {}{paused}",
                    habit_line(h),
                    frequency_text(&h.frequency)
                );
            }
        }
        QueryOutput::Identities(ids) => {
            for i in ids {
                let _ = writeln!(
                    out,
                    "{}  {} open, {} habits",
                    i.name, i.open_items, i.habits
                );
            }
        }
        QueryOutput::Status(s) => {
            let name = s.mode.identity.as_deref().unwrap_or("away");
            let q = if s.mode.auto_away { "?" } else { "" };
            let _ = write!(out, "mode:{name}{q}");
            if let Some(t) = &s.timer {
                let short = t.target.short.as_deref().unwrap_or("-");
                let q = if t.idle { "?" } else { "" };
                let _ = write!(out, " timer:{short}+{}m{q}", t.minutes);
            }
            let _ = write!(
                out,
                " inbox:{} habits:{} due:{} follow-ups:{}",
                s.inbox, s.habits_left, s.due, s.follow_ups
            );
            if s.warnings > 0 {
                let _ = write!(out, " warnings:{}", s.warnings);
            }
            out.push('\n');
        }
        QueryOutput::Doctor(d) => {
            for w in &d.load {
                let _ = writeln!(out, "{}:{}: {}", w.file.display(), w.line, w.problem);
            }
            for w in &d.projection {
                let _ = writeln!(out, "event {}: {}", w.event, w.rejection);
            }
            if out.is_empty() {
                out.push_str("log ok\n");
            }
        }
    }
    out
}
