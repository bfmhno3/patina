//! Human-readable output (English, no color).

use std::fmt::Write as _;

use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use patina_core::command::{Command, NextAction};
use patina_core::engine::{CommandOutcome, Subject};
use patina_core::event::{
    EventData, Frequency, HabitKind, InterruptOutcome, Repeat, RepeatUnit, Size, TimerTarget,
};
use patina_core::ids;
use patina_core::query::{
    Banner, Card, HabitView, ItemDetail, ItemView, ModeView, NextView, QueryOutput, ReviewView,
    TimerView, TodayView, TreeNode, ViewStatus,
};
use patina_core::recommend::Reason;
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

/// `stopped <ref> <label> after <N>m (total <T>m)` when the command ended a segment.
fn stopped_line(o: &CommandOutcome, out: &mut String) {
    if let Some(st) = &o.stopped {
        let _ = writeln!(
            out,
            "stopped {} {} after {}m (total {}m)",
            sref(Some(&st.subject)),
            st.subject.label,
            st.minutes,
            st.total_minutes
        );
        if st.over_estimate
            && let Some(e) = st.estimate
        {
            let _ = writeln!(
                out,
                "  over twice the {} estimate ({}m); consider splitting: patina new \"<first step>\" --parent {}",
                size_name(e.size),
                e.minutes,
                sref(Some(&st.subject))
            );
        }
    }
}

fn size_name(s: Size) -> &'static str {
    match s {
        Size::S => "S",
        Size::M => "M",
        Size::L => "L",
    }
}

fn repeat_text(r: &Repeat) -> String {
    match r {
        Repeat::Every { every, unit } => {
            let u = match unit {
                RepeatUnit::Day => 'd',
                RepeatUnit::Week => 'w',
                RepeatUnit::Month => 'm',
            };
            format!("every {every}{u}")
        }
        Repeat::AfterCompletion { days } => format!("after {days}d"),
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
            stopped_line(o, &mut out);
            let next = o.events.iter().find_map(|e| match &e.data {
                EventData::ItemCreated(d) if d.repeat_of.is_some() => Some(d),
                _ => None,
            });
            if let Some(n) = next {
                let sub = o.subjects.iter().find(|s| s.id == n.item.as_uuid());
                let _ = write!(out, "next: {} {}", sref(sub), label(sub));
                if let Some(d) = n.scheduled {
                    let _ = write!(out, " sched {d}");
                }
                out.push('\n');
            }
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
            stopped_line(o, &mut out);
        }
        Command::StartTimer(_) => started_lines(o, &mut out),
        Command::Next(c) => match &c.action {
            NextAction::Start => started_lines(o, &mut out),
            _ => {
                if let Some(v) = &o.card {
                    out.push_str(&card(v, now));
                }
            }
        },
        Command::StopTimer(_) => stopped_line(o, &mut out),
        Command::Stuck(_) => {
            stopped_line(o, &mut out);
            let blocker = o.events.iter().find_map(|e| match &e.data {
                EventData::ItemCreated(d) => Some(d.item.as_uuid()),
                _ => None,
            });
            let sub = blocker.and_then(|id| o.subjects.iter().find(|s| s.id == id));
            let _ = writeln!(out, "waiting on {} {}", sref(sub), label(sub));
        }
        Command::Interrupt(_) => {
            stopped_line(o, &mut out);
            let interrupted = o.events.iter().find_map(|e| match &e.data {
                EventData::TimerInterrupted(d) => Some((d.outcome, d.item.as_uuid())),
                _ => None,
            });
            match interrupted {
                Some((InterruptOutcome::Now, id)) => {
                    let sub = o.subjects.iter().find(|s| s.id == id);
                    let _ = writeln!(out, "interrupted: now timing {} {}", sref(sub), label(sub));
                }
                _ => out.push_str("interrupted: captured to inbox\n"),
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
        Command::CompleteReview(_) => {
            out.push_str("review recorded\n");
            for sub in &o.subjects {
                let _ = writeln!(out, "  focus: {} {}", sref(Some(sub)), sub.label);
            }
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
    if i.important {
        s.push_str(" !");
    }
    if let Some(e) = &i.estimate {
        let _ = write!(s, " [{}]", size_name(e.size));
    }
    if let Some(r) = &i.repeat {
        let _ = write!(s, " {}", repeat_text(r));
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
    } else if let HabitKind::Duration { minutes } = h.kind {
        format!("{}/{minutes} min", h.minutes.unwrap_or(0))
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

/// `[stopped ...]` then `started timer on <ref> <label>` and the hand-over note.
fn started_lines(o: &CommandOutcome, out: &mut String) {
    stopped_line(o, out);
    let started = o.events.iter().find_map(|e| match &e.data {
        EventData::TimerStarted(d) => Some(match d.target {
            TimerTarget::Item(i) => i.as_uuid(),
            TimerTarget::Habit(h) => h.as_uuid(),
        }),
        _ => None,
    });
    let sub = started.and_then(|id| o.subjects.iter().find(|s| s.id == id));
    let _ = writeln!(out, "started timer on {} {}", sref(sub), label(sub));
    if let Some(l) = &o.left_off {
        let _ = writeln!(out, "left off: {l}");
    }
}

fn plural(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn reason_text(r: &Reason) -> String {
    match r {
        Reason::Deadline { days } if *days < 0 => {
            format!("overdue by {} day{}", -days, plural(-days))
        }
        Reason::Deadline { days: 0 } => "due today".into(),
        Reason::Deadline { days } => format!("due in {days} day{}", plural(*days)),
        Reason::Scheduled { days_late: 0 } => "planned for today".into(),
        Reason::Scheduled { days_late } => {
            format!("planned {days_late} day{} ago", plural(*days_late))
        }
        Reason::Unlocks { count } => format!("unlocks {count}"),
        Reason::Downstream { count } => format!("{count} downstream"),
        Reason::Focus => "weekly focus".into(),
        Reason::Important => "important".into(),
        Reason::Small { size } => format!("small ({})", size_name(*size)),
        Reason::RecentlySkipped { count } => format!("skipped {count}x recently"),
        Reason::FirstStep => "first step".into(),
    }
}

fn card(v: &NextView, now: &ZonedDateTime) -> String {
    let mut out = String::new();
    if let Some(scope) = &v.scope {
        let _ = writeln!(out, "[{scope}]");
    }
    banner(&v.banner, &mut out);
    match &v.card {
        Card::Item { item, reasons, .. } => {
            let _ = writeln!(out, "next: {} {}", item.short, item.title);
            if !reasons.is_empty() {
                let why: Vec<String> = reasons.iter().map(reason_text).collect();
                let _ = writeln!(out, "  why: {}", why.join("; "));
            }
            out.push_str(
                "  start: patina next start | skip: patina next skip | too big: patina next split \"<first 5 minutes>\"\n",
            );
        }
        Card::BreakDown { item, .. } => {
            let _ = writeln!(out, "break it down: {} {}", item.short, item.title);
            out.push_str(
                "  what can you do in the first 5 minutes? patina next split \"<step>\"\n",
            );
        }
        Card::Review { .. } => out.push_str("weekly review due: patina review\n"),
        Card::Triage { inbox } => {
            let _ = writeln!(out, "inbox has {inbox} entries: patina inbox");
        }
        Card::Empty { elsewhere } => {
            match &v.scope {
                Some(scope) => {
                    let _ = writeln!(out, "nothing actionable in {scope}");
                }
                None => out.push_str("nothing actionable\n"),
            }
            if !elsewhere.is_empty() {
                let parts: Vec<String> = elsewhere
                    .iter()
                    .map(|e| format!("{} {}", e.identity, e.actionable))
                    .collect();
                let _ = writeln!(out, "elsewhere: {}", parts.join(", "));
            }
        }
    }
    out.push_str(&hints(&v.mode, v.timer.as_ref(), now));
    out
}

fn review(r: &ReviewView, now: &ZonedDateTime) -> String {
    let mut out = String::new();
    match r.since {
        Some(s) => {
            let day = s.to_zoned_datetime(now.time_zone().clone()).date();
            let _ = writeln!(out, "Since {day}");
        }
        None => out.push_str("Since the beginning\n"),
    }
    if !r.completed.is_empty() {
        out.push_str("Done\n");
        for g in &r.completed {
            let _ = writeln!(out, "  [{}]", g.identity);
            for i in &g.items {
                let _ = writeln!(out, "    {}", item_line(i, false));
            }
        }
    }
    if !r.projects.is_empty() {
        out.push_str("Projects\n");
        for p in &r.projects {
            let _ = writeln!(
                out,
                "  {} {}: +{} ({} done, {} open)",
                p.project.short, p.project.title, p.completed, p.done, p.open
            );
        }
    }
    if !r.postponed.is_empty() {
        out.push_str("Postponed\n");
        for p in &r.postponed {
            let _ = writeln!(
                out,
                "  {} (postponed {}x)",
                item_line(&p.item, true),
                p.count
            );
        }
        out.push_str(
            "  decide: patina new ... --parent <ref> | patina abandon <ref> --reason ... | patina edit <ref> --scheduled <date>\n",
        );
    }
    if !r.waiting.is_empty() {
        out.push_str("Waiting\n");
        for w in &r.waiting {
            let _ = writeln!(out, "  {} ({} days)", item_line(&w.item, true), w.days);
        }
    }
    if !r.inbox.is_empty() {
        out.push_str("Inbox\n");
        for e in &r.inbox {
            let _ = writeln!(out, "  {}  {}", e.short, e.text);
        }
    }
    if !r.focus.is_empty() {
        out.push_str("Current focus\n");
        for i in &r.focus {
            let _ = writeln!(out, "  {}", item_line(i, true));
        }
    }
    out.push_str("finish with: patina review done --conclusion \"...\" [--focus <ref>]...\n");
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
    if i.important {
        out.push_str("important: yes\n");
    }
    if let Some(e) = &i.estimate {
        let _ = write!(out, "estimate: {} ({}m)", size_name(e.size), e.minutes);
        if let Some(c) = d.children_estimate_minutes {
            let _ = write!(out, ", children sum {c}m");
        }
        out.push('\n');
    }
    if i.tracked_minutes > 0 {
        let _ = writeln!(out, "tracked: {}m", i.tracked_minutes);
    }
    if let Some(r) = &i.repeat {
        let _ = writeln!(out, "repeat: {}", repeat_text(r));
    }
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
    if let Some(l) = &d.left_off {
        let _ = writeln!(out, "left_off: {l}");
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
        QueryOutput::Review(r) => return review(r, now),
        QueryOutput::Next(n) => return card(n, now),
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
