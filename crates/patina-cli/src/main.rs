//! `patina`: the command-line adapter over `patina-core`.
#![forbid(unsafe_code)]

mod env;
mod render;

use std::process::ExitCode;

use anyhow::Result;
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use jiff::ZonedDateTime;
use jiff::civil::Date;
use patina_core::command::{self, Command, Update};
use patina_core::engine::Patina;
use patina_core::event::{AbandonReason, Frequency, HabitKind, Repeat, RepeatUnit, Size};
use patina_core::query::{Query, QueryOutput};
use patina_core::store::Store;
use patina_core::time::{Weekday, logical_day};

use crate::env::{Env, parse_date, parse_time, parse_update};

/// Identity-driven personal task system.
#[derive(Debug, Parser)]
#[command(name = "patina", version, about)]
struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Create the default identities (work, life, study).
    Init,
    /// Capture a thought into the inbox.
    Add {
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Create a task directly.
    New(NewArgs),
    /// Turn an inbox entry into a task.
    Triage(TriageArgs),
    /// Discard an inbox entry.
    Discard { item: String },
    /// Merge an inbox entry into another item's notes.
    Merge {
        item: String,
        #[arg(long)]
        into: String,
    },
    /// Edit an item.
    Edit(EditArgs),
    /// Add a note to an item.
    Note {
        item: String,
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Start timing an item or habit (or just mark an item started with --no-timer).
    Start(StartArgs),
    /// Stop the running timer.
    Stop {
        /// When you actually stopped (HH:MM or YYYY-MM-DD HH:MM).
        #[arg(long)]
        at: Option<String>,
    },
    /// Mark the item you are timing (or --item) as waiting on something external.
    Stuck {
        /// What you are waiting for.
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        #[arg(long)]
        item: Option<String>,
        #[arg(long)]
        follow_up: Option<String>,
    },
    /// Park a new matter that interrupts the running timer.
    Interrupt {
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        /// Handle it right now: switch the timer to it.
        #[arg(long)]
        now: bool,
        #[arg(short, long, requires = "now")]
        identity: Option<String>,
        /// Where you stopped on the interrupted item.
        #[arg(long, requires = "now")]
        left_off: Option<String>,
    },
    /// Complete an item.
    Done {
        item: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Abandon an item and its open descendants.
    Abandon {
        item: String,
        #[arg(long, value_enum)]
        reason: Reason,
        #[arg(long)]
        note: Option<String>,
    },
    /// Reparent an item.
    Move(MoveArgs),
    /// Make an item depend on a sibling.
    Link {
        item: String,
        /// The prerequisite.
        #[arg(long)]
        on: String,
    },
    /// Remove a dependency.
    Unlink {
        item: String,
        /// The prerequisite.
        #[arg(long)]
        on: String,
    },
    /// Habits.
    Habit {
        #[command(subcommand)]
        cmd: HabitCmd,
    },
    /// Identities.
    Identity {
        #[command(subcommand)]
        cmd: IdentityCmd,
    },
    /// What to do today.
    Today {
        #[arg(short, long)]
        identity: Option<String>,
        /// Ignore the identity mode.
        #[arg(long, conflicts_with = "identity")]
        all: bool,
    },
    /// Show or switch the identity mode.
    Mode {
        /// An identity name or `away`; omit to show the current mode.
        target: Option<String>,
        /// When the switch happened (HH:MM or YYYY-MM-DD HH:MM), to correct a forgotten switch.
        #[arg(long, requires = "target")]
        at: Option<String>,
    },
    /// Show or set the break rhythm (fixed, flowtime, off).
    Rhythm {
        /// The mode to switch to; omit to show the rhythm.
        mode: Option<String>,
    },
    /// Start a break (stops the running timer), or snooze the reminder.
    Break {
        #[command(subcommand)]
        cmd: Option<BreakCmd>,
    },
    /// Show the weekly review, or record it with `review done`.
    Review {
        #[command(subcommand)]
        cmd: Option<ReviewCmd>,
    },
    /// List untriaged captures.
    Inbox,
    /// Show the next thing to do, or act on it with `start`, `skip`, or `split`.
    Next {
        #[command(subcommand)]
        cmd: Option<NextCmd>,
        #[arg(short, long)]
        identity: Option<String>,
        /// Ignore the identity mode.
        #[arg(long, conflicts_with = "identity")]
        all: bool,
    },
    /// List actionable items.
    List {
        #[arg(short, long)]
        identity: Option<String>,
    },
    /// Show the hierarchy.
    Tree {
        root: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Show one item.
    Show { item: String },
    /// One-line status.
    Status,
    /// Report problems in the event log.
    Doctor,
    /// Undo this device's latest command.
    Undo,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Reason {
    NoLongerImportant,
    Superseded,
    TooHard,
    Other,
}

impl From<Reason> for AbandonReason {
    fn from(r: Reason) -> Self {
        match r {
            Reason::NoLongerImportant => AbandonReason::NoLongerImportant,
            Reason::Superseded => AbandonReason::Superseded,
            Reason::TooHard => AbandonReason::TooHard,
            Reason::Other => AbandonReason::Other,
        }
    }
}

#[derive(Debug, Args)]
struct NewArgs {
    title: String,
    #[arg(short, long)]
    identity: Option<String>,
    #[arg(short, long)]
    parent: Option<String>,
    #[arg(short = 't', long = "tag")]
    tags: Vec<String>,
    #[arg(long)]
    defer: Option<String>,
    #[arg(long)]
    scheduled: Option<String>,
    #[arg(long)]
    deadline: Option<String>,
    #[arg(long)]
    waiting_for: Option<String>,
    #[arg(long)]
    follow_up: Option<String>,
    #[command(flatten)]
    extras: Extras,
}

/// Importance, size estimate, and recurrence for new items.
#[derive(Debug, Args)]
struct Extras {
    /// Mark as important.
    #[arg(long)]
    important: bool,
    /// Size estimate.
    #[arg(long, value_enum)]
    size: Option<SizeArg>,
    /// Repeat every N days, weeks, or months (e.g. 3d, 2w, 1m).
    #[arg(long, value_parser = parse_every, conflicts_with = "after")]
    every: Option<Repeat>,
    /// Repeat this many days after each completion.
    #[arg(long)]
    after: Option<u32>,
}

impl Extras {
    fn repeat(&self) -> Option<Repeat> {
        self.every
            .or(self.after.map(|days| Repeat::AfterCompletion { days }))
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SizeArg {
    S,
    M,
    L,
}

impl From<SizeArg> for Size {
    fn from(s: SizeArg) -> Self {
        match s {
            SizeArg::S => Size::S,
            SizeArg::M => Size::M,
            SizeArg::L => Size::L,
        }
    }
}

fn parse_every(s: &str) -> Result<Repeat, String> {
    let bad = || format!("invalid interval {s:?} (use <N>d, <N>w, or <N>m, e.g. 2w)");
    let s = s.trim();
    let unit = match s.chars().last() {
        Some('d') => RepeatUnit::Day,
        Some('w') => RepeatUnit::Week,
        Some('m') => RepeatUnit::Month,
        _ => return Err(bad()),
    };
    let every: u32 = s[..s.len() - 1].parse().map_err(|_| bad())?;
    if every == 0 {
        return Err(bad());
    }
    Ok(Repeat::Every { every, unit })
}

fn parse_size_update(s: &str) -> Result<Update<Size>, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "s" => Ok(Update::Set(Size::S)),
        "m" => Ok(Update::Set(Size::M)),
        "l" => Ok(Update::Set(Size::L)),
        "none" => Ok(Update::Clear),
        _ => Err(format!("invalid size {s:?} (use s, m, l, or none)")),
    }
}

#[derive(Debug, Args)]
struct TriageArgs {
    item: String,
    #[arg(short, long)]
    identity: Option<String>,
    #[arg(long)]
    title: Option<String>,
    #[arg(short, long)]
    parent: Option<String>,
    #[arg(short = 't', long = "tag")]
    tags: Vec<String>,
    #[arg(long)]
    defer: Option<String>,
    #[arg(long)]
    scheduled: Option<String>,
    #[arg(long)]
    deadline: Option<String>,
    #[arg(long)]
    waiting_for: Option<String>,
    #[arg(long)]
    follow_up: Option<String>,
    #[command(flatten)]
    extras: Extras,
}

#[derive(Debug, Args)]
struct EditArgs {
    item: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(short, long)]
    identity: Option<String>,
    #[arg(short = 't', long = "tag")]
    tags: Vec<String>,
    #[arg(long)]
    untag: Vec<String>,
    /// A date or `none`.
    #[arg(long)]
    defer: Option<String>,
    /// A date or `none`.
    #[arg(long)]
    scheduled: Option<String>,
    /// A date or `none`.
    #[arg(long)]
    deadline: Option<String>,
    /// A date or `none`.
    #[arg(long)]
    follow_up: Option<String>,
    #[arg(long, conflicts_with = "not_waiting")]
    waiting_for: Option<String>,
    #[arg(long)]
    not_waiting: bool,
    #[arg(long, conflicts_with = "not_important")]
    important: bool,
    #[arg(long)]
    not_important: bool,
    /// s, m, l, or `none`.
    #[arg(long, value_parser = parse_size_update)]
    size: Option<Update<Size>>,
    /// Repeat every N days, weeks, or months (e.g. 3d, 2w, 1m).
    #[arg(long, value_parser = parse_every, conflicts_with_all = ["after", "no_repeat"])]
    every: Option<Repeat>,
    /// Repeat this many days after each completion.
    #[arg(long, conflicts_with_all = ["every", "no_repeat"])]
    after: Option<u32>,
    /// Stop repeating.
    #[arg(long)]
    no_repeat: bool,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("what").required(true).args(["item", "habit"])))]
struct StartArgs {
    /// The item to time.
    item: Option<String>,
    /// A duration habit to time instead.
    #[arg(long)]
    habit: Option<String>,
    /// When you actually started (HH:MM or YYYY-MM-DD HH:MM).
    #[arg(long)]
    at: Option<String>,
    /// Only mark the item started; no timer.
    #[arg(long, conflicts_with_all = ["habit", "at"])]
    no_timer: bool,
}
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("dest").required(true).args(["parent", "top"])))]
struct MoveArgs {
    item: String,
    #[arg(long)]
    parent: Option<String>,
    #[arg(long)]
    top: bool,
}

fn parse_weekday(s: &str) -> Result<Weekday, String> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "mon" => Weekday::Mon,
        "tue" => Weekday::Tue,
        "wed" => Weekday::Wed,
        "thu" => Weekday::Thu,
        "fri" => Weekday::Fri,
        "sat" => Weekday::Sat,
        "sun" => Weekday::Sun,
        _ => {
            return Err(format!(
                "unknown weekday {s:?} (use mon, tue, wed, thu, fri, sat, sun)"
            ));
        }
    })
}

#[derive(Debug, Subcommand)]
enum HabitCmd {
    /// Create a habit.
    #[command(group(ArgGroup::new("freq").multiple(false).args(["daily", "weekly", "days"])))]
    Add {
        name: String,
        #[arg(short, long)]
        identity: String,
        /// Count habit with this daily target.
        #[arg(long, conflicts_with = "minutes")]
        count: Option<u32>,
        /// Duration habit: tracked minutes per day.
        #[arg(long)]
        minutes: Option<u32>,
        #[arg(long)]
        daily: bool,
        /// Times per week.
        #[arg(long)]
        weekly: Option<u8>,
        /// Comma-separated weekdays.
        #[arg(long, value_delimiter = ',', value_parser = parse_weekday)]
        days: Vec<Weekday>,
        #[arg(long)]
        cue: Option<String>,
    },
    /// Edit a habit.
    Edit {
        habit: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(short, long)]
        identity: Option<String>,
        #[arg(long, conflicts_with = "no_cue")]
        cue: Option<String>,
        #[arg(long)]
        no_cue: bool,
    },
    /// Check in.
    Check {
        habit: String,
        #[arg(long)]
        day: Option<String>,
        #[arg(long)]
        count: Option<u32>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Skip a day.
    Skip {
        habit: String,
        #[arg(long)]
        day: Option<String>,
    },
    /// Pause a habit.
    Pause {
        habit: String,
        #[arg(long)]
        from: Option<String>,
    },
    /// Resume a habit.
    Resume {
        habit: String,
        #[arg(long)]
        from: Option<String>,
    },
    /// Graduate a habit: nothing is due and its strength freezes.
    Graduate {
        habit: String,
        #[arg(long)]
        from: Option<String>,
    },
    /// Reinstate a graduated habit.
    Reinstate {
        habit: String,
        #[arg(long)]
        from: Option<String>,
    },
    /// List habits.
    List,
}

#[derive(Debug, Subcommand)]
enum NextCmd {
    /// Start timing the shown item.
    Start,
    /// Skip the shown card and show another.
    Skip,
    /// The shown item is too big: add its first step and show that.
    Split {
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum BreakCmd {
    /// Postpone the pending break reminder.
    Snooze,
}

#[derive(Debug, Subcommand)]
enum ReviewCmd {
    /// Record the review.
    Done {
        /// What you conclude from this week.
        #[arg(long)]
        conclusion: Option<String>,
        /// A focus item for the coming week (at most one per identity).
        #[arg(long)]
        focus: Vec<String>,
        /// Record the minimal (steps 1, 5, 7) review.
        #[arg(long)]
        minimal: bool,
    },
}

#[derive(Debug, Subcommand)]
enum IdentityCmd {
    /// Add an identity.
    Add { name: String },
    /// Rename an identity.
    Rename { name: String, new: String },
    /// Set or clear the weekly time target (hours per week, or `none`).
    Target { name: String, hours: String },
    /// List identities.
    List,
}

enum Action {
    Run(Command),
    Ask(Query),
}

fn opt_date(s: &Option<String>, today: Date) -> Result<Option<Date>> {
    s.as_deref().map(|s| parse_date(s, today)).transpose()
}

fn opt_update(s: &Option<String>, today: Date) -> Result<Option<Update<Date>>> {
    s.as_deref().map(|s| parse_update(s, today)).transpose()
}

fn action(cmd: Cmd, today: Date, now: &ZonedDateTime, proposal_level: bool) -> Result<Action> {
    use command as c;
    Ok(match cmd {
        Cmd::Init => Action::Run(Command::Init),
        Cmd::Add { text } => Action::Run(Command::Capture(c::Capture {
            text: text.join(" "),
        })),
        Cmd::New(a) => Action::Run(Command::AddItem(Box::new(c::AddItem {
            title: a.title,
            identity: a.identity,
            parent: a.parent,
            tags: a.tags,
            defer: opt_date(&a.defer, today)?,
            scheduled: opt_date(&a.scheduled, today)?,
            deadline: opt_date(&a.deadline, today)?,
            waiting_for: a.waiting_for,
            follow_up: opt_date(&a.follow_up, today)?,
            important: a.extras.important,
            size: a.extras.size.map(Size::from),
            repeat: a.extras.repeat(),
        }))),
        Cmd::Triage(a) => Action::Run(Command::Triage(Box::new(c::Triage {
            item: a.item,
            identity: a.identity,
            title: a.title,
            parent: a.parent,
            tags: a.tags,
            defer: opt_date(&a.defer, today)?,
            scheduled: opt_date(&a.scheduled, today)?,
            deadline: opt_date(&a.deadline, today)?,
            waiting_for: a.waiting_for,
            follow_up: opt_date(&a.follow_up, today)?,
            important: a.extras.important,
            size: a.extras.size.map(Size::from),
            repeat: a.extras.repeat(),
        }))),
        Cmd::Discard { item } => Action::Run(Command::Discard(c::Discard { item })),
        Cmd::Merge { item, into } => Action::Run(Command::Merge(c::Merge { item, into })),
        Cmd::Edit(a) => Action::Run(Command::Edit(Box::new(c::Edit {
            item: a.item,
            title: a.title,
            identity: a.identity,
            add_tags: a.tags,
            remove_tags: a.untag,
            defer: opt_update(&a.defer, today)?,
            scheduled: opt_update(&a.scheduled, today)?,
            deadline: opt_update(&a.deadline, today)?,
            follow_up: opt_update(&a.follow_up, today)?,
            waiting_for: if a.not_waiting {
                Some(Update::Clear)
            } else {
                a.waiting_for.map(Update::Set)
            },
            important: if a.important {
                Some(true)
            } else if a.not_important {
                Some(false)
            } else {
                None
            },
            size: a.size,
            repeat: if a.no_repeat {
                Some(Update::Clear)
            } else {
                a.every
                    .or(a.after.map(|days| Repeat::AfterCompletion { days }))
                    .map(Update::Set)
            },
        }))),
        Cmd::Note { item, text } => Action::Run(Command::Note(c::Note {
            item,
            text: text.join(" "),
        })),
        Cmd::Start(a) => {
            let at = a.at.as_deref().map(|s| parse_time(s, now)).transpose()?;
            match (a.item, a.habit) {
                (Some(item), None) if a.no_timer => Action::Run(Command::Start(c::Start { item })),
                (Some(item), None) => Action::Run(Command::StartTimer(c::StartTimer {
                    target: c::TimerRef::Item(item),
                    at,
                })),
                (None, Some(habit)) => Action::Run(Command::StartTimer(c::StartTimer {
                    target: c::TimerRef::Habit(habit),
                    at,
                })),
                _ => anyhow::bail!("give either an item or --habit"),
            }
        }
        Cmd::Stop { at } => Action::Run(Command::StopTimer(c::StopTimer {
            at: at.as_deref().map(|s| parse_time(s, now)).transpose()?,
        })),
        Cmd::Stuck {
            text,
            item,
            follow_up,
        } => Action::Run(Command::Stuck(c::Stuck {
            item,
            waiting_for: text.join(" "),
            follow_up: opt_date(&follow_up, today)?,
        })),
        Cmd::Interrupt {
            text,
            now: handle_now,
            identity,
            left_off,
        } => Action::Run(Command::Interrupt(c::Interrupt {
            text: text.join(" "),
            now: handle_now,
            identity,
            left_off,
        })),
        Cmd::Done { item, note } => Action::Run(Command::Complete(c::Complete { item, note })),
        Cmd::Abandon { item, reason, note } => Action::Run(Command::Abandon(c::Abandon {
            item,
            reason: reason.into(),
            note,
        })),
        Cmd::Move(a) => Action::Run(Command::Move(c::Move {
            item: a.item,
            parent: a.parent,
        })),
        Cmd::Link { item, on } => Action::Run(Command::Link(c::Link {
            item,
            depends_on: on,
        })),
        Cmd::Unlink { item, on } => Action::Run(Command::Unlink(c::Unlink {
            item,
            depends_on: on,
        })),
        Cmd::Habit { cmd } => match cmd {
            HabitCmd::Add {
                name,
                identity,
                count,
                minutes,
                daily: _,
                weekly,
                days,
                cue,
            } => {
                let frequency = if let Some(times) = weekly {
                    Frequency::Weekly { times }
                } else if !days.is_empty() {
                    Frequency::Weekdays { days }
                } else {
                    Frequency::Daily
                };
                let kind = match (count, minutes) {
                    (Some(target), _) => HabitKind::Count { target },
                    (None, Some(minutes)) => HabitKind::Duration { minutes },
                    (None, None) => HabitKind::Check,
                };
                Action::Run(Command::AddHabit(c::AddHabit {
                    name,
                    identity,
                    kind,
                    frequency,
                    cue,
                }))
            }
            HabitCmd::Edit {
                habit,
                name,
                identity,
                cue,
                no_cue,
            } => Action::Run(Command::EditHabit(c::EditHabit {
                habit,
                name,
                identity,
                cue: if no_cue {
                    Some(Update::Clear)
                } else {
                    cue.map(Update::Set)
                },
            })),
            HabitCmd::Check {
                habit,
                day,
                count,
                note,
            } => Action::Run(Command::CheckIn(c::CheckIn {
                habit,
                day: opt_date(&day, today)?,
                count,
                note,
            })),
            HabitCmd::Skip { habit, day } => Action::Run(Command::SkipHabit(c::SkipHabit {
                habit,
                day: opt_date(&day, today)?,
            })),
            HabitCmd::Pause { habit, from } => Action::Run(Command::PauseHabit(c::PauseHabit {
                habit,
                from: opt_date(&from, today)?,
            })),
            HabitCmd::Resume { habit, from } => Action::Run(Command::ResumeHabit(c::ResumeHabit {
                habit,
                from: opt_date(&from, today)?,
            })),
            HabitCmd::Graduate { habit, from } => {
                Action::Run(Command::GraduateHabit(c::GraduateHabit {
                    habit,
                    from: opt_date(&from, today)?,
                }))
            }
            HabitCmd::Reinstate { habit, from } => {
                Action::Run(Command::ReinstateHabit(c::ReinstateHabit {
                    habit,
                    from: opt_date(&from, today)?,
                }))
            }
            HabitCmd::List => Action::Ask(Query::Habits),
        },
        Cmd::Identity { cmd } => match cmd {
            IdentityCmd::Add { name } => Action::Run(Command::AddIdentity(c::AddIdentity { name })),
            IdentityCmd::Rename { name, new } => {
                Action::Run(Command::RenameIdentity(c::RenameIdentity {
                    identity: name,
                    name: new,
                }))
            }
            IdentityCmd::Target { name, hours } => {
                let weekly_minutes = patina_core::time::parse_hours(&hours).ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid hours {hours:?} (use a number like 20 or 7.5, or none)"
                    )
                })?;
                Action::Run(Command::SetIdentityTarget(c::SetIdentityTarget {
                    identity: name,
                    weekly_minutes,
                }))
            }
            IdentityCmd::List => Action::Ask(Query::Identities),
        },
        Cmd::Today { identity, all } => Action::Ask(Query::Today { identity, all }),
        Cmd::Rhythm { mode: None } => Action::Ask(Query::Rhythm),
        Cmd::Rhythm { mode: Some(m) } => {
            use patina_core::event::RhythmMode;
            let mode = match m.trim().to_ascii_lowercase().as_str() {
                "fixed" => RhythmMode::Fixed,
                "flowtime" => RhythmMode::Flowtime,
                "off" => RhythmMode::Off,
                _ => anyhow::bail!("invalid rhythm {m:?} (use fixed, flowtime, or off)"),
            };
            Action::Run(Command::SetRhythm(c::SetRhythm { mode }))
        }
        Cmd::Break { cmd: None } => Action::Run(Command::StartBreak),
        Cmd::Break {
            cmd: Some(BreakCmd::Snooze),
        } => Action::Run(Command::SnoozeBreak),
        Cmd::Mode { target, at } => match target {
            None => Action::Ask(Query::Mode),
            Some(t) => Action::Run(Command::SwitchMode(c::SwitchMode {
                identity: if t.eq_ignore_ascii_case("away") {
                    None
                } else {
                    Some(t)
                },
                at: at.as_deref().map(|s| parse_time(s, now)).transpose()?,
            })),
        },
        Cmd::Next { cmd, identity, all } => match cmd {
            // Proposal-level sources only read: they must not record shown cards.
            None if proposal_level => Action::Ask(Query::Next { identity, all }),
            None => Action::Run(Command::Next(c::NextCmd {
                action: c::NextAction::Show { identity, all },
            })),
            Some(NextCmd::Start) => Action::Run(Command::Next(c::NextCmd {
                action: c::NextAction::Start,
            })),
            Some(NextCmd::Skip) => Action::Run(Command::Next(c::NextCmd {
                action: c::NextAction::Skip,
            })),
            Some(NextCmd::Split { text }) => Action::Run(Command::Next(c::NextCmd {
                action: c::NextAction::Split {
                    text: text.join(" "),
                },
            })),
        },
        Cmd::Review { cmd } => match cmd {
            None => Action::Ask(Query::Review),
            Some(ReviewCmd::Done {
                conclusion,
                focus,
                minimal,
            }) => Action::Run(Command::CompleteReview(c::CompleteReview {
                conclusion,
                focus,
                minimal,
            })),
        },
        Cmd::Inbox => Action::Ask(Query::Inbox),
        Cmd::List { identity } => Action::Ask(Query::List { identity }),
        Cmd::Tree { root, all } => Action::Ask(Query::Tree { root, all }),
        Cmd::Show { item } => Action::Ask(Query::Show { item }),
        Cmd::Status => Action::Ask(Query::Status),
        Cmd::Doctor => Action::Ask(Query::Doctor),
        Cmd::Undo => Action::Run(Command::Undo),
    })
}

fn warn(n: usize) {
    if n > 0 {
        eprintln!("warning: {n} log problem(s); run 'patina doctor'");
    }
}

fn run(cli: Cli) -> Result<()> {
    let env = Env::load()?;
    let patina = Patina::new(Store::new(env.root.clone(), env.device));
    let settings = patina.settings()?;
    let today = logical_day(&env.now, &settings)?;
    let action = action(cli.command, today, &env.now, env.source.is_proposal_level())?;
    match action {
        Action::Run(cmd) => {
            let outcome = patina.run(cmd.clone(), env.now.clone(), env.source.clone())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                print!("{}", render::outcome(&cmd, &outcome, &env.now));
            }
            warn(outcome.warnings);
            // "Stuck" immediately recommends the next thing (design: no dead end).
            if matches!(cmd, Command::Stuck(_)) && !cli.json && !env.source.is_proposal_level() {
                let show = Command::Next(command::NextCmd {
                    action: command::NextAction::Show {
                        identity: None,
                        all: false,
                    },
                });
                let next = patina.run(show.clone(), env.now.clone(), env.source.clone())?;
                print!("{}", render::outcome(&show, &next, &env.now));
            }
        }
        Action::Ask(q) => {
            let out = patina.query(q, env.now.clone())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                print!("{}", render::query(&out, &env.now));
            }
            if let QueryOutput::Today(t) = &out {
                warn(t.warnings);
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if json {
                let value = match e.downcast_ref::<patina_core::engine::Error>() {
                    Some(err) => serde_json::to_value(err).unwrap_or_default(),
                    None => serde_json::json!({"kind": "other", "detail": e.to_string()}),
                };
                println!("{}", serde_json::json!({ "error": value }));
            } else {
                eprintln!("error: {e}");
            }
            ExitCode::FAILURE
        }
    }
}
