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
use patina_core::proposal::{ChangeView, value_text};
use patina_core::query::{
    Banner, Card, GraphView, HabitView, ItemDetail, ItemView, ModeView, NextView, QueryOutput,
    ReviewView, TimerView, TodayView, TreeNode, ViewStatus,
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
        Command::GraduateHabit(_) => {
            for e in &o.events {
                if let EventData::HabitGraduated(d) = &e.data {
                    let _ = writeln!(out, "graduated {l} from {}", d.from);
                }
            }
        }
        Command::ReinstateHabit(_) => {
            for e in &o.events {
                if let EventData::HabitReinstated(d) = &e.data {
                    let _ = writeln!(out, "reinstated {l} from {}", d.from);
                }
            }
        }
        Command::AddIdentity(_) => {
            let _ = writeln!(out, "added identity {l}");
        }
        Command::RenameIdentity(_) => {
            let _ = writeln!(out, "renamed identity to {l}");
        }
        Command::SetIdentityTarget(c) => match c.weekly_minutes {
            Some(m) => {
                let _ = writeln!(out, "target {l}: {}/week", hours_text(m));
            }
            None => {
                let _ = writeln!(out, "target {l}: none");
            }
        },
        Command::SetRhythm(c) => {
            let _ = writeln!(out, "rhythm: {}", rhythm_name(c.mode));
        }
        Command::StartBreak => {
            stopped_line(o, &mut out);
            for e in &o.events {
                if let EventData::BreakStarted(d) = &e.data {
                    let until =
                        e.effective_at + jiff::SignedDuration::from_mins(i64::from(d.minutes));
                    let _ = writeln!(
                        out,
                        "break: {} {}m until {}",
                        break_name(d.kind),
                        d.minutes,
                        stamp(until, now)
                    );
                }
            }
        }
        Command::SnoozeBreak => {
            for e in &o.events {
                if let EventData::BreakSnoozed(d) = &e.data {
                    let _ = writeln!(out, "snoozed {}m", d.minutes);
                }
            }
        }
        Command::Undo => {
            let _ = writeln!(out, "undid: {}", o.undone.join(", "));
            for sub in &o.subjects {
                let _ = writeln!(out, "  {} {}", sref(Some(sub)), sub.label);
            }
        }
        Command::UndoSource(c) => {
            let n = o
                .events
                .iter()
                .filter(|e| matches!(e.data, EventData::CommandUndone(_)))
                .count();
            let _ = writeln!(out, "undid {n} command(s) from {}", c.source);
        }
        Command::Propose(_) => {
            for e in &o.events {
                if let EventData::ProposalSubmitted(d) = &e.data {
                    let _ = writeln!(
                        out,
                        "proposed {}: {}",
                        ids::short(&d.proposal.as_uuid(), 6),
                        d.summary
                    );
                }
            }
        }
        Command::AcceptProposal(_) => {
            for e in &o.events {
                if let EventData::ProposalAccepted(d) = &e.data {
                    let parts = |v: &[u32]| {
                        v.iter()
                            .map(|u| (u + 1).to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    let _ = write!(out, "accepted part(s) {}", parts(&d.accepted));
                    if !d.stale.is_empty() {
                        let _ = write!(out, "; out of date: {}", parts(&d.stale));
                    }
                    out.push('\n');
                }
            }
        }
        Command::RejectProposal(_) => {
            for e in &o.events {
                if let EventData::ProposalRejected(d) = &e.data {
                    let _ = writeln!(out, "rejected {}", ids::short(&d.proposal.as_uuid(), 6));
                }
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
        Command::DismissSuggestion(_) => {
            let _ = writeln!(out, "dismissed suggestion for {l}");
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

fn rhythm_name(m: patina_core::event::RhythmMode) -> &'static str {
    use patina_core::event::RhythmMode;
    match m {
        RhythmMode::Fixed => "fixed",
        RhythmMode::Flowtime => "flowtime",
        RhythmMode::Off => "off",
    }
}

fn break_name(k: patina_core::event::BreakKind) -> &'static str {
    use patina_core::event::BreakKind;
    match k {
        BreakKind::Short => "short",
        BreakKind::Long => "long",
        BreakKind::Flow => "flow",
    }
}

/// Minutes as hours: `20h` when whole, else one decimal (`7.5h`).
fn hours_text(minutes: u32) -> String {
    if minutes.is_multiple_of(60) {
        format!("{}h", minutes / 60)
    } else {
        format!("{:.1}h", f64::from(minutes) / 60.0)
    }
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

/// `1h05`, `-0h30`.
fn hm(minutes: i64) -> String {
    let sign = if minutes < 0 { "-" } else { "" };
    let m = minutes.abs();
    format!("{sign}{}h{:02}", m / 60, m % 60)
}

fn signed(n: i64) -> String {
    if n > 0 {
        format!("+{n}")
    } else {
        n.to_string()
    }
}

fn review_stats(r: &ReviewView, now: &ZonedDateTime, out: &mut String) {
    if !r.open_projects.is_empty() {
        out.push_str("Open projects\n");
        for p in &r.open_projects {
            let _ = writeln!(
                out,
                "  {} {}: {} done, {} open ({} open)",
                p.project.short,
                p.project.title,
                p.done,
                p.open,
                signed(p.open_change)
            );
        }
    }
    if r.time
        .iter()
        .any(|t| t.tracked_minutes > 0 || t.target_minutes.is_some() || t.mode_minutes > 0)
    {
        out.push_str("Time\n");
        for t in r
            .time
            .iter()
            .filter(|t| t.tracked_minutes > 0 || t.target_minutes.is_some() || t.mode_minutes > 0)
        {
            let _ = write!(out, "  [{}] tracked {}", t.identity, hm(t.tracked_minutes));
            if let (Some(target), Some(dev)) = (t.target_minutes, t.deviation_minutes) {
                let plus = if dev > 0 { "+" } else { "" };
                let _ = write!(out, " / target {} ({plus}{})", hm(target), hm(dev));
            }
            let _ = writeln!(out, "  mode {}", hm(t.mode_minutes));
        }
    }
    if !r.interruptions.is_empty() {
        out.push_str("Interruptions\n");
        let sources: Vec<String> = r
            .interruption_sources
            .iter()
            .map(|s| format!("{} {}", s.identity.as_deref().unwrap_or("inbox"), s.count))
            .collect();
        let _ = writeln!(
            out,
            "  {} total: {}",
            r.interruptions.len(),
            sources.join(", ")
        );
        for i in &r.interruptions {
            let day = i.at.to_zoned_datetime(now.time_zone().clone()).date();
            let _ = writeln!(out, "    {day} {}", i.title);
        }
    }
    if !r.habits.is_empty() {
        out.push_str("Habits\n");
        for h in &r.habits {
            let _ = writeln!(
                out,
                "  {} [{}] {}% ({} points)",
                h.habit.name,
                h.habit.identity,
                (h.habit.strength * 100.0).round(),
                signed((h.change * 100.0).round() as i64)
            );
        }
    }
    if !r.graduated.is_empty() {
        out.push_str("Graduated (spot check)\n");
        for h in &r.graduated {
            let _ = writeln!(out, "  {} {} [{}]", h.short, h.name, h.identity);
        }
    }
    let c = &r.calibration;
    if c.samples > 0 || !c.conversions.is_empty() || c.weeks.iter().any(|w| w.samples > 0) {
        out.push_str("Calibration\n");
        match c.error {
            Some(e) => {
                let _ = writeln!(out, "  error {e:.2} over {}", c.samples);
            }
            None => out.push_str("  no timed estimates\n"),
        }
        if let Some(t) = c.by_tag.first() {
            let _ = writeln!(out, "  worst: #{} {:.2}", t.tag, t.error);
        }
        let weeks: Vec<String> = c
            .weeks
            .iter()
            .map(|w| {
                let e = w
                    .error
                    .map_or_else(|| "-".to_string(), |e| format!("{e:.2}"));
                format!("{} {e}", w.week_start)
            })
            .collect();
        let _ = writeln!(out, "  weeks: {}", weeks.join(" "));
        for k in &c.conversions {
            let tag = k
                .tag
                .as_ref()
                .map_or_else(|| "all".to_string(), |t| format!("#{t}"));
            let _ = writeln!(
                out,
                "  {tag} {} ~{}m (default {}m, {} samples)",
                size_name(k.size),
                k.minutes,
                k.default_minutes,
                k.samples
            );
        }
    }
    if !r.notes.is_empty() {
        out.push_str("Notes\n");
        for n in &r.notes {
            let _ = writeln!(out, "  {} {}: {}", n.day, n.subject, n.text);
        }
    }
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
    review_stats(r, now, &mut out);
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
    if let Some(s) = &t.suggestion {
        let _ = write!(
            out,
            "suggest: switch to {} (usual on {} at this time: {}/{} days)",
            s.identity,
            if s.weekend { "weekends" } else { "weekdays" },
            s.matches,
            s.samples
        );
        if let Some(m) = s.below_target_minutes {
            let _ = write!(out, ", {} below target this week", hm(m));
        }
        out.push('\n');
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
    if t.proposals > 0 {
        let _ = writeln!(
            out,
            "proposals: {} pending (patina proposal list)",
            t.proposals
        );
    }
    out.push_str(&hints(&t.mode, t.timer.as_ref(), now));
    out
}

/// The snake_case key of a unit-like serializable enum.
fn snake(v: &impl serde::Serialize) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn change_lines(c: &ChangeView, out: &mut String) {
    let fields = |fields: &[patina_core::proposal::FieldChange], out: &mut String| {
        for f in fields {
            let _ = writeln!(
                out,
                "      {}: {} -> {}",
                f.field,
                value_text(&f.old),
                value_text(&f.new)
            );
        }
    };
    match c {
        ChangeView::AddItem {
            title,
            parent,
            fields: f,
            ..
        } => {
            let _ = write!(out, "    add \"{title}\"");
            if let Some(p) = parent {
                let _ = write!(out, " under \"{p}\"");
            }
            out.push('\n');
            fields(f, out);
        }
        ChangeView::Edit { item, fields: f } => {
            let _ = writeln!(out, "    edit \"{item}\"");
            fields(f, out);
        }
        ChangeView::Note { item, text } => {
            let _ = writeln!(out, "    note on \"{item}\": {text}");
        }
        ChangeView::Link { item, depends_on } => {
            let _ = writeln!(out, "    \"{item}\" depends on \"{depends_on}\"");
        }
        ChangeView::Unlink { item, depends_on } => {
            let _ = writeln!(out, "    \"{item}\" no longer depends on \"{depends_on}\"");
        }
        ChangeView::Move { item, parent } => match parent {
            Some(p) => {
                let _ = writeln!(out, "    move \"{item}\" under \"{p}\"");
            }
            None => {
                let _ = writeln!(out, "    move \"{item}\" to top level");
            }
        },
    }
}

fn marker(s: ViewStatus) -> &'static str {
    match s {
        ViewStatus::Todo | ViewStatus::Inbox => "[ ]",
        ViewStatus::InProgress => "[>]",
        ViewStatus::Blocked => "[!]",
        ViewStatus::Waiting => "[~]",
        ViewStatus::Done => "[x]",
        ViewStatus::Abandoned => "[-]",
    }
}

fn graph(g: &GraphView) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} {}", g.root.short, g.root.title);
    if g.nodes.is_empty() {
        out.push_str("no sub-items\n");
        return out;
    }
    let last = g.nodes.iter().map(|n| n.layer).max().unwrap_or(0);
    for layer in 0..=last {
        let _ = writeln!(out, "layer {layer}");
        for n in g.nodes.iter().filter(|n| n.layer == layer) {
            let _ = write!(
                out,
                "  {} {}  {}",
                marker(n.item.status),
                n.item.short,
                n.item.title
            );
            let needs: Vec<&str> = g
                .edges
                .iter()
                .filter(|e| e.to == n.item.id)
                .filter_map(|e| g.nodes.iter().find(|p| p.item.id == e.from))
                .map(|p| p.item.short.as_str())
                .collect();
            if !needs.is_empty() {
                let _ = write!(out, "  <- {}", needs.join(", "));
            }
            out.push('\n');
        }
    }
    out
}

fn tree(nodes: &[TreeNode], depth: usize, out: &mut String) {
    for n in nodes {
        let marker = marker(n.item.status);
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
        QueryOutput::Rhythm(r) => {
            let _ = writeln!(
                out,
                "rhythm: {}  focus {}m  round {}",
                rhythm_name(r.mode),
                r.focus_minutes,
                r.round
            );
            if let Some(d) = &r.due {
                let _ = writeln!(out, "break due: {} {}m", break_name(d.kind), d.minutes);
            }
            if let Some(b) = &r.on_break {
                let _ = writeln!(out, "on break until {}", stamp(b.ends_at, now));
            }
        }
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
        QueryOutput::Graph(g) => return graph(g),
        QueryOutput::Proposals(list) => {
            if list.is_empty() {
                out.push_str("no proposals\n");
            }
            for p in list {
                let _ = writeln!(
                    out,
                    "{}  [{}]  {}  ({}, {})",
                    p.short,
                    snake(&p.status),
                    p.summary,
                    String::from(p.source.clone()),
                    stamp(p.submitted_at, now)
                );
                for u in &p.units {
                    let _ = write!(out, "  part {} [{}]", u.index + 1, snake(&u.state));
                    if !u.needs.is_empty() {
                        let needs: Vec<String> =
                            u.needs.iter().map(|n| (n + 1).to_string()).collect();
                        let _ = write!(out, " needs {}", needs.join(", "));
                    }
                    if let Some(problem) = &u.problem {
                        let _ = write!(out, ": {problem}");
                    }
                    out.push('\n');
                    for c in &u.changes {
                        change_lines(c, &mut out);
                    }
                }
            }
        }
        QueryOutput::Sources(list) => {
            if list.is_empty() {
                out.push_str("no AI changes\n");
            }
            for s in list {
                let _ = writeln!(
                    out,
                    "{}  {} command(s), last {}",
                    String::from(s.source.clone()),
                    s.commands,
                    stamp(s.last_at, now)
                );
            }
        }
        QueryOutput::Habits(habits) => {
            for h in habits {
                let ready = if h.graduation_ready {
                    "  ready to graduate"
                } else {
                    ""
                };
                let _ = writeln!(
                    out,
                    "{}  {}  {}{ready}",
                    habit_line(h),
                    frequency_text(&h.frequency),
                    h.stage.key()
                );
            }
        }
        QueryOutput::Identities(ids) => {
            for i in ids {
                let _ = write!(
                    out,
                    "{}  {} open, {} habits",
                    i.name, i.open_items, i.habits
                );
                if let Some(m) = i.weekly_target_minutes {
                    let _ = write!(out, "  target {}/week", hours_text(m));
                }
                out.push('\n');
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
