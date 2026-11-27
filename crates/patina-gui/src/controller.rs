//! The controller: runs core commands and queries, and projects the results into the
//! `App` global. All domain rules stay in `patina-core`; this only translates.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use jiff::ZonedDateTime;
use jiff::civil::Date;
use patina_core::command::{self, Command, TimerRef, Update};
use patina_core::engine::{CommandOutcome, Patina, SubjectKind};
use patina_core::event::{HabitKind, Source};
use patina_core::ids::DeviceId;
use patina_core::query::{ItemDetail, ItemView, Query, QueryOutput, TreeNode};
use patina_core::recommend::{self, NextView};
use patina_core::store::Store;
use patina_core::time::{self, parse_hours};
use slint::{ComponentHandle as _, Model as _, SharedString};
use uuid::Uuid;

use crate::convert::{self, GuiError, ss};
use crate::language::{self, Language};
use crate::{
    App, ItemForm, Labels, MainWindow, NoticeData, NoticeKind, PickerPurpose, PickerRow,
    PromptData, PromptKind, SettingsData, StorageUi,
};

/// Which items the Today page shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    /// The current identity mode.
    Mode,
    /// Everything.
    All,
    /// One identity.
    Identity(String),
}

type CardKey = (recommend::CardKind, Option<Uuid>, Option<String>);

/// What to resume after a break.
struct Resume {
    id: String,
    is_habit: bool,
}

/// Owns the core facade and the small amount of UI-side state.
pub struct Controller {
    patina: Patina,
    root: PathBuf,
    device: DeviceId,
    storage: StorageUi,
    window: slint::Weak<MainWindow>,
    scope: Scope,
    card_key: Option<CardKey>,
    asked_timer: Option<Uuid>,
    asked_mode: Option<jiff::Timestamp>,
    was_due: bool,
    was_on_break: bool,
    tree_all: bool,
    detail: Option<String>,
    graph: Option<String>,
    /// Proposal parts the owner unticked, by proposal id.
    deselected: HashMap<String, BTreeSet<usize>>,
    today: Option<Date>,
    mode_name: Option<String>,
    resume: Option<Resume>,
    prompt_seq: i32,
    notice_seq: i32,
    /// Directory of the per-device GUI preferences (`gui.toml`).
    prefs_dir: PathBuf,
    /// The language preference; `None` follows the system.
    language: Option<Language>,
    /// The language shown, read by the date picker's formatter.
    locale: Rc<Cell<Language>>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Controller {
    /// Creates a controller for `store`; `window` receives the view models.
    pub fn new(
        store: Store,
        root: PathBuf,
        window: slint::Weak<MainWindow>,
        storage: StorageUi,
        prefs_dir: PathBuf,
    ) -> Self {
        let device = store.device();
        Self {
            patina: Patina::new(store),
            root,
            device,
            storage,
            window,
            scope: Scope::Mode,
            card_key: None,
            asked_timer: None,
            asked_mode: None,
            was_due: false,
            was_on_break: false,
            tree_all: false,
            detail: None,
            graph: None,
            deselected: HashMap::new(),
            today: None,
            mode_name: None,
            resume: None,
            prompt_seq: 0,
            notice_seq: 0,
            language: language::load(&prefs_dir),
            prefs_dir,
            locale: Rc::new(Cell::new(Language::En)),
        }
    }

    fn ask(&self, q: Query) -> Result<QueryOutput, String> {
        self.patina.query(q, ZonedDateTime::now()).map_err(err)
    }

    /// Words a GUI-side error in the current language through `Labels.gui_error`.
    fn error_text(&self, e: &GuiError) -> String {
        let (key, detail) = e.key();
        self.window.upgrade().map_or_else(
            || format!("{e:?}"), // no window, nobody reads it; never "" (that means success)
            |w| {
                w.global::<Labels<'_>>()
                    .invoke_gui_error(key.into(), detail.into())
                    .to_string()
            },
        )
    }

    /// The language shown, shared with the date picker's formatter.
    pub fn locale(&self) -> Rc<Cell<Language>> {
        self.locale.clone()
    }

    /// Shows the preferred (or system) language; call once the window exists.
    pub fn apply_language(&self) {
        let lang = language::resolve(self.language);
        self.locale.set(lang);
        language::select(lang);
    }

    pub fn set_language(&mut self, pref: Option<Language>) {
        self.language = pref;
        self.apply_language();
        if let Err(e) = language::save(&self.prefs_dir, pref) {
            let text = self.error_text(&GuiError::SaveLanguage(e.to_string()));
            self.set_notice(NoticeKind::Error, &text, "");
        }
        self.refresh();
    }

    fn scope_args(&self) -> (Option<String>, bool) {
        match &self.scope {
            Scope::Mode => (None, false),
            Scope::All => (None, true),
            Scope::Identity(n) => (Some(n.clone()), false),
        }
    }

    fn today_date(&self) -> Result<Date, String> {
        match self.today {
            Some(d) => Ok(d),
            None => {
                let settings = self.patina.settings().map_err(err)?;
                time::logical_day(&ZonedDateTime::now(), &settings).map_err(err)
            }
        }
    }

    // ---- notices ----

    fn set_notice(&mut self, kind: NoticeKind, text: &str, target_id: &str) {
        self.notice_seq += 1;
        if let Some(win) = self.window.upgrade() {
            win.global::<App<'_>>().set_notice(NoticeData {
                seq: self.notice_seq,
                kind,
                text: ss(text),
                target_id: ss(target_id),
            });
        }
    }

    // ---- commands ----

    /// Runs a command without refreshing or raising notices.
    fn run(&mut self, cmd: Command) -> Result<CommandOutcome, String> {
        self.patina
            .run(cmd, ZonedDateTime::now(), Source::Gui)
            .map_err(err)
    }

    /// Runs a command, raises the matching notice, and refreshes the window.
    fn exec(&mut self, cmd: Command) -> Result<CommandOutcome, String> {
        let quiet = matches!(
            cmd,
            Command::Next(_) | Command::SnoozeBreak | Command::DismissSuggestion(_)
        );
        let undo = matches!(cmd, Command::Undo);
        let result = self.run(cmd);
        match &result {
            Ok(out) => {
                if let Some(card) = &out.card {
                    self.card_key = Some(convert::card_key(card));
                }
                if let Some(text) = &out.left_off {
                    self.set_notice(NoticeKind::LeftOff, text, "");
                } else if let Some(st) = out.stopped.as_ref().filter(|s| s.over_estimate) {
                    self.set_notice(
                        NoticeKind::OverEstimate,
                        &st.subject.label,
                        &st.subject.id.to_string(),
                    );
                } else if !out.needs_decision.is_empty() {
                    self.set_notice(
                        NoticeKind::NeedsDecision,
                        &out.needs_decision.len().to_string(),
                        "",
                    );
                } else if undo {
                    self.set_notice(NoticeKind::Undone, "", "");
                } else if !quiet {
                    let label = out.subjects.first().map_or("", |s| s.label.as_str());
                    self.set_notice(NoticeKind::Saved, label, "");
                }
            }
            Err(e) => self.set_notice(NoticeKind::Error, e, ""),
        }
        self.refresh();
        result
    }

    /// Like [`Controller::exec`], reduced to what a dialog needs: "" or the error text.
    fn exec_text(&mut self, cmd: Command) -> SharedString {
        match self.exec(cmd) {
            Ok(_) => SharedString::new(),
            Err(e) => ss(e),
        }
    }

    /// Fire-and-forget: errors still raise a notice.
    fn exec_quiet(&mut self, cmd: Command) {
        let _ = self.exec(cmd);
    }

    /// Runs a command that was built from fallible input.
    fn exec_built(&mut self, cmd: Result<Command, String>) -> SharedString {
        match cmd {
            Ok(c) => self.exec_text(c),
            Err(e) => ss(e),
        }
    }

    // ---- navigation ----

    pub fn navigate(&mut self, page: i32) {
        if let Some(win) = self.window.upgrade() {
            let app = win.global::<App<'_>>();
            app.set_page(page);
            app.set_detail_open(false);
            app.set_graph_open(false);
            app.set_settings_open(false);
        }
        self.detail = None;
        self.graph = None;
        self.refresh();
    }

    pub fn open_item(&mut self, id: &str) {
        self.detail = Some(id.to_string());
        if let Some(win) = self.window.upgrade() {
            win.global::<App<'_>>().set_detail_open(true);
        }
        self.refresh();
    }

    pub fn close_detail(&mut self) {
        self.detail = None;
        if let Some(win) = self.window.upgrade() {
            win.global::<App<'_>>().set_detail_open(false);
        }
        self.refresh();
    }

    // ---- AI proposals and bulk undo ----

    fn proposals(&self) -> Result<Vec<patina_core::query::ProposalView>, String> {
        match self.ask(Query::Proposals {
            all: false,
            source: None,
        })? {
            QueryOutput::Proposals(v) => Ok(v),
            _ => Err("unexpected query result".into()),
        }
    }

    /// Ticks or unticks one part; unticking drops the parts that need it, ticking brings
    /// back the parts it needs.
    pub fn toggle_unit(&mut self, id: &str, index: i32) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let Ok(views) = self.proposals() else {
            return;
        };
        let Some(p) = views.iter().find(|p| p.id.to_string() == id) else {
            return;
        };
        let units: Vec<patina_core::proposal::Unit> = p
            .units
            .iter()
            .map(|u| patina_core::proposal::Unit {
                ops: Vec::new(),
                needs: u.needs.clone(),
            })
            .collect();
        let off = self.deselected.entry(id.to_string()).or_default();
        let checked = p
            .units
            .get(index)
            .is_some_and(|u| convert::unit_checked(u.state, Some(off), index));
        if checked {
            off.extend(patina_core::proposal::dependents(&units, index));
        } else {
            for u in patina_core::proposal::with_needs(&units, &BTreeSet::from([index])) {
                off.remove(&u);
            }
        }
        self.refresh();
    }

    pub fn accept_proposal(&mut self, id: &str) -> SharedString {
        let views = match self.proposals() {
            Ok(v) => v,
            Err(e) => return ss(e),
        };
        let off = self.deselected.get(id);
        let selected: Vec<usize> = views
            .iter()
            .find(|p| p.id.to_string() == id)
            .map(|p| {
                p.units
                    .iter()
                    .filter(|u| convert::unit_checked(u.state, off, u.index))
                    .map(|u| u.index)
                    .collect()
            })
            .unwrap_or_default();
        if selected.is_empty() {
            return ss(self.error_text(&GuiError::NothingSelected));
        }
        let result = self.exec_text(Command::AcceptProposal(command::AcceptProposal {
            proposal: id.to_string(),
            units: Some(selected),
        }));
        if result.is_empty() {
            self.deselected.remove(id);
        }
        result
    }

    pub fn reject_proposal(&mut self, id: &str) -> SharedString {
        self.exec_text(Command::RejectProposal(command::RejectProposal {
            proposal: id.to_string(),
            note: None,
        }))
    }

    pub fn undo_source(&mut self, source: &str) -> SharedString {
        self.exec_text(Command::UndoSource(command::UndoSource {
            source: source.to_string(),
            since: None,
        }))
    }

    pub fn open_graph(&mut self, id: &str) {
        self.graph = Some(id.to_string());
        self.detail = None;
        if let Some(win) = self.window.upgrade() {
            let app = win.global::<App<'_>>();
            app.set_graph_open(true);
            app.set_detail_open(false);
        }
        self.refresh();
    }

    pub fn close_graph(&mut self) {
        self.graph = None;
        if let Some(win) = self.window.upgrade() {
            win.global::<App<'_>>().set_graph_open(false);
        }
        self.refresh();
    }

    pub fn set_settings_open(&mut self, open: bool) {
        if let Some(win) = self.window.upgrade() {
            win.global::<App<'_>>().set_settings_open(open);
        }
        self.refresh();
    }

    pub fn set_scope(&mut self, kind: i32, name: &str) {
        self.scope = match kind {
            0 => Scope::Mode,
            1 => Scope::All,
            _ => Scope::Identity(name.to_string()),
        };
        self.card_key = None;
        self.refresh();
    }

    pub fn set_show_settled(&mut self, all: bool) {
        self.tree_all = all;
        self.refresh();
    }

    pub fn notice_action(&mut self, notice: &NoticeData) {
        match notice.kind {
            NoticeKind::Saved => self.exec_quiet(Command::Undo),
            NoticeKind::OverEstimate => self.open_item(notice.target_id.as_str()),
            NoticeKind::BreakOver => self.resume(),
            NoticeKind::NeedsDecision => self.navigate(0),
            _ => {}
        }
    }

    // ---- capture, mode, timer, rhythm ----

    pub fn capture(&mut self, text: &str) -> SharedString {
        self.exec_text(Command::Capture(command::Capture {
            text: text.to_string(),
        }))
    }

    pub fn switch_mode(&mut self, name: &str) {
        self.exec_quiet(Command::SwitchMode(command::SwitchMode {
            identity: convert::opt_text(name),
            at: None,
        }));
    }

    pub fn undo(&mut self) -> SharedString {
        self.exec_text(Command::Undo)
    }

    pub fn start_break(&mut self) {
        self.exec_quiet(Command::StartBreak);
    }

    pub fn snooze_break(&mut self) {
        self.exec_quiet(Command::SnoozeBreak);
    }

    pub fn set_rhythm(&mut self, mode: patina_core::event::RhythmMode) {
        self.exec_quiet(Command::SetRhythm(command::SetRhythm { mode }));
    }

    pub fn dismiss_suggestion(&mut self, identity: &str) {
        self.exec_quiet(Command::DismissSuggestion(command::DismissSuggestion {
            identity: identity.to_string(),
        }));
    }

    pub fn mark_started(&mut self, id: &str) {
        self.exec_quiet(Command::Start(command::Start {
            item: id.to_string(),
        }));
    }

    pub fn discard(&mut self, id: &str) {
        self.exec_quiet(Command::Discard(command::Discard {
            item: id.to_string(),
        }));
    }

    pub fn init_identities(&mut self) {
        self.exec_quiet(Command::Init);
    }

    pub fn add_identity(&mut self, name: &str) -> SharedString {
        self.exec_text(Command::AddIdentity(command::AddIdentity {
            name: name.to_string(),
        }))
    }

    pub fn rename_identity(&mut self, old: &str, new: &str) -> SharedString {
        self.exec_text(Command::RenameIdentity(command::RenameIdentity {
            identity: old.to_string(),
            name: new.to_string(),
        }))
    }

    pub fn card_action(&mut self, action: command::NextAction) -> SharedString {
        self.exec_text(Command::Next(command::NextCmd { action }))
    }

    pub fn start_timer(&mut self, id: &str, habit: bool) {
        let target = if habit {
            TimerRef::Habit(id.to_string())
        } else {
            TimerRef::Item(id.to_string())
        };
        self.exec_quiet(Command::StartTimer(command::StartTimer {
            target,
            at: None,
        }));
    }

    pub fn stop_timer(&mut self) {
        self.exec_quiet(Command::StopTimer(command::StopTimer { at: None }));
    }

    pub fn stuck(&mut self, item: &str, waiting: &str, follow_up: &str) -> SharedString {
        let cmd = self.today_date().and_then(|today| {
            Ok(Command::Stuck(command::Stuck {
                item: convert::opt_text(item),
                waiting_for: waiting.to_string(),
                follow_up: convert::date_field(follow_up, today)
                    .map_err(|e| self.error_text(&e))?,
            }))
        });
        self.exec_built(cmd)
    }

    pub fn interrupt(
        &mut self,
        text: &str,
        now: bool,
        identity: &str,
        left_off: &str,
    ) -> SharedString {
        self.exec_text(Command::Interrupt(command::Interrupt {
            text: text.to_string(),
            now,
            identity: convert::opt_text(identity),
            left_off: convert::opt_text(left_off),
        }))
    }

    pub fn resume(&mut self) {
        if let Some(r) = self.resume.take() {
            self.start_timer(&r.id, r.is_habit);
        }
    }

    fn past_time(&self, hhmm: &str) -> Result<jiff::Timestamp, String> {
        time::parse_time(hhmm, &ZonedDateTime::now())
            .ok_or_else(|| self.error_text(&GuiError::Time(hhmm.trim().to_string())))
    }

    pub fn correct_timer(&mut self, hhmm: &str) -> SharedString {
        let cmd = self
            .past_time(hhmm)
            .map(|at| Command::StopTimer(command::StopTimer { at: Some(at) }));
        self.exec_built(cmd)
    }

    pub fn correct_mode(&mut self, hhmm: &str) -> SharedString {
        let cmd = self.past_time(hhmm).map(|at| {
            Command::SwitchMode(command::SwitchMode {
                identity: None,
                at: Some(at),
            })
        });
        self.exec_built(cmd)
    }

    /// "Still here": re-affirms the current mode, which counts as activity.
    pub fn keep_mode(&mut self) {
        let identity = self.mode_name.clone();
        self.exec_quiet(Command::SwitchMode(command::SwitchMode {
            identity,
            at: None,
        }));
    }

    // ---- items ----

    pub fn complete(&mut self, id: &str, note: &str) -> SharedString {
        self.exec_text(Command::Complete(command::Complete {
            item: id.to_string(),
            note: convert::opt_text(note),
        }))
    }

    pub fn abandon(&mut self, id: &str, reason: i32, note: &str) -> SharedString {
        self.exec_text(Command::Abandon(command::Abandon {
            item: id.to_string(),
            reason: convert::abandon_reason(reason),
            note: convert::opt_text(note),
        }))
    }

    pub fn add_item(&mut self, form: &ItemForm) -> SharedString {
        let cmd = self
            .today_date()
            .and_then(|today| {
                convert::add_item_command(form, today).map_err(|e| self.error_text(&e))
            })
            .map(|c| Command::AddItem(Box::new(c)));
        self.exec_built(cmd)
    }

    pub fn triage(&mut self, id: &str, form: &ItemForm) -> SharedString {
        let cmd = self
            .today_date()
            .and_then(|today| {
                convert::triage_command(id, form, today).map_err(|e| self.error_text(&e))
            })
            .map(|c| Command::Triage(Box::new(c)));
        self.exec_built(cmd)
    }

    fn detail_of(&self, id: &str) -> Result<ItemDetail, String> {
        match self.ask(Query::Show {
            item: id.to_string(),
        })? {
            QueryOutput::Show(d) => Ok(*d),
            _ => Err("unexpected query result".into()),
        }
    }

    pub fn edit_item(&mut self, id: &str, form: &ItemForm) -> SharedString {
        let cmd = self.today_date().and_then(|today| {
            let detail = self.detail_of(id)?;
            convert::edit_command(&detail, form, today)
                .map_err(|e| self.error_text(&e))
                .map(|e| Command::Edit(Box::new(e)))
        });
        self.exec_built(cmd)
    }

    /// An edit of one date field.
    fn set_date_field(&mut self, id: &str, date: &str, follow_up: bool) -> SharedString {
        let cmd = self.today_date().and_then(|today| {
            let d = convert::date_field(date, today)
                .map_err(|e| self.error_text(&e))?
                .ok_or_else(|| self.error_text(&GuiError::DateRequired))?;
            let mut e = convert::blank_edit(id);
            if follow_up {
                e.follow_up = Some(Update::Set(d));
            } else {
                e.scheduled = Some(Update::Set(d));
            }
            Ok(Command::Edit(Box::new(e)))
        });
        self.exec_built(cmd)
    }

    pub fn set_scheduled(&mut self, id: &str, date: &str) -> SharedString {
        self.set_date_field(id, date, false)
    }

    pub fn set_follow_up(&mut self, id: &str, date: &str) -> SharedString {
        self.set_date_field(id, date, true)
    }

    pub fn add_note(&mut self, id: &str, text: &str) -> SharedString {
        self.exec_text(Command::Note(command::Note {
            item: id.to_string(),
            text: text.to_string(),
        }))
    }

    pub fn move_item(&mut self, id: &str, parent: &str) -> SharedString {
        self.exec_text(Command::Move(command::Move {
            item: id.to_string(),
            parent: convert::opt_text(parent),
        }))
    }

    pub fn link(&mut self, id: &str, depends_on: &str) -> SharedString {
        self.exec_text(Command::Link(command::Link {
            item: id.to_string(),
            depends_on: depends_on.to_string(),
        }))
    }

    pub fn unlink(&mut self, id: &str, depends_on: &str) {
        self.exec_quiet(Command::Unlink(command::Unlink {
            item: id.to_string(),
            depends_on: depends_on.to_string(),
        }));
    }

    /// Removes every edge from `id` to an abandoned prerequisite.
    pub fn unlink_abandoned(&mut self, id: &str) {
        let Ok(detail) = self.detail_of(id) else {
            return;
        };
        for p in detail
            .prerequisites
            .iter()
            .filter(|p| p.status == patina_core::query::ViewStatus::Abandoned)
        {
            self.unlink(id, &p.id.to_string());
        }
    }

    pub fn merge(&mut self, id: &str, into: &str) -> SharedString {
        self.exec_text(Command::Merge(command::Merge {
            item: id.to_string(),
            into: into.to_string(),
        }))
    }

    // ---- habits ----

    fn habit_day(&self, offset: i32) -> Result<Option<Date>, String> {
        if offset <= 0 {
            return Ok(None);
        }
        let today = self.today_date()?;
        today
            .checked_sub(jiff::Span::new().days(i64::from(offset)))
            .map(Some)
            .map_err(err)
    }

    pub fn check_in(&mut self, id: &str, offset: i32, count: i32, note: &str) -> SharedString {
        let cmd = self.habit_day(offset).map(|day| {
            Command::CheckIn(command::CheckIn {
                habit: id.to_string(),
                day,
                count: u32::try_from(count).ok(),
                note: convert::opt_text(note),
            })
        });
        self.exec_built(cmd)
    }

    pub fn skip_habit(&mut self, id: &str, offset: i32) -> SharedString {
        let cmd = self.habit_day(offset).map(|day| {
            Command::SkipHabit(command::SkipHabit {
                habit: id.to_string(),
                day,
            })
        });
        self.exec_built(cmd)
    }

    pub fn pause_habit(&mut self, id: &str, pause: bool) {
        let habit = id.to_string();
        let cmd = if pause {
            Command::PauseHabit(command::PauseHabit { habit, from: None })
        } else {
            Command::ResumeHabit(command::ResumeHabit { habit, from: None })
        };
        self.exec_quiet(cmd);
    }

    pub fn graduate_habit(&mut self, id: &str, graduate: bool) {
        let habit = id.to_string();
        let cmd = if graduate {
            Command::GraduateHabit(command::GraduateHabit { habit, from: None })
        } else {
            Command::ReinstateHabit(command::ReinstateHabit { habit, from: None })
        };
        self.exec_quiet(cmd);
    }

    pub fn add_habit(&mut self, f: &crate::HabitForm) -> SharedString {
        use patina_core::event::Frequency;
        use patina_core::time::Weekday;
        let n = u32::try_from(f.target).unwrap_or(0);
        let kind = match f.kind {
            1 => HabitKind::Count { target: n },
            2 => HabitKind::Duration { minutes: n },
            _ => HabitKind::Check,
        };
        let frequency = match f.frequency {
            1 => Frequency::Weekly {
                times: u8::try_from(f.times).unwrap_or(0),
            },
            2 => {
                let all = [
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Fri,
                    Weekday::Sat,
                    Weekday::Sun,
                ];
                Frequency::Weekdays {
                    days: f
                        .days
                        .iter()
                        .zip(all)
                        .filter_map(|(on, d)| on.then_some(d))
                        .collect(),
                }
            }
            _ => Frequency::Daily,
        };
        self.exec_text(Command::AddHabit(command::AddHabit {
            name: f.name.to_string(),
            identity: f.identity.to_string(),
            kind,
            frequency,
            cue: convert::opt_text(&f.cue),
        }))
    }

    pub fn edit_habit(&mut self, id: &str, name: &str, identity: &str, cue: &str) -> SharedString {
        self.exec_text(Command::EditHabit(command::EditHabit {
            habit: id.to_string(),
            name: Some(name.to_string()),
            identity: convert::opt_text(identity),
            cue: Some(match convert::opt_text(cue) {
                Some(c) => Update::Set(c),
                None => Update::Clear,
            }),
        }))
    }

    // ---- review and settings ----

    pub fn review_finish(
        &mut self,
        conclusion: &str,
        focus: Vec<String>,
        minimal: bool,
    ) -> SharedString {
        self.exec_text(Command::CompleteReview(command::CompleteReview {
            conclusion: convert::opt_text(conclusion),
            focus: focus.into_iter().filter(|f| !f.is_empty()).collect(),
            minimal,
        }))
    }

    pub fn set_target(&mut self, name: &str, hours: &str) -> SharedString {
        match parse_hours(hours) {
            Some(weekly_minutes) => {
                self.exec_text(Command::SetIdentityTarget(command::SetIdentityTarget {
                    identity: name.to_string(),
                    weekly_minutes,
                }))
            }
            None => ss(self.error_text(&GuiError::Hours(hours.trim().to_string()))),
        }
    }

    // ---- pickers ----

    /// Fills `App.picker_rows` with the candidates for `purpose`, filtered by `filter`.
    pub fn picker_query(&mut self, purpose: PickerPurpose, filter: &str, context: &str) {
        let rows = self
            .picker_rows(purpose, filter, context)
            .unwrap_or_default();
        if let Some(win) = self.window.upgrade() {
            win.global::<App<'_>>()
                .set_picker_rows(convert::model(rows));
        }
    }

    fn tree(&self, all: bool) -> Result<Vec<TreeNode>, String> {
        match self.ask(Query::Tree { root: None, all })? {
            QueryOutput::Tree(t) => Ok(t),
            _ => Err("unexpected query result".into()),
        }
    }

    fn picker_rows(
        &self,
        purpose: PickerPurpose,
        filter: &str,
        context: &str,
    ) -> Result<Vec<PickerRow>, String> {
        fn open(i: &ItemView) -> bool {
            i.waiting_for.is_none()
                && !matches!(
                    i.status,
                    patina_core::query::ViewStatus::Done
                        | patina_core::query::ViewStatus::Abandoned
                )
        }
        fn collect<'a>(
            nodes: &'a [TreeNode],
            skip: &str,
            drop_recurring: bool,
            out: &mut Vec<&'a ItemView>,
        ) {
            for n in nodes {
                if n.item.id.to_string() == skip {
                    continue;
                }
                if open(&n.item) && !(drop_recurring && n.item.repeat.is_some()) {
                    out.push(&n.item);
                }
                collect(&n.children, skip, drop_recurring, out);
            }
        }
        fn siblings<'a>(nodes: &'a [TreeNode], id: &str) -> Option<&'a [TreeNode]> {
            if nodes.iter().any(|n| n.item.id.to_string() == id) {
                return Some(nodes);
            }
            nodes.iter().find_map(|n| siblings(&n.children, id))
        }

        let tree;
        let mut items: Vec<&ItemView> = Vec::new();
        let prerequisites: Vec<Uuid>;
        match purpose {
            PickerPurpose::Parent | PickerPurpose::MergeInto => {
                tree = self.tree(false)?;
                collect(
                    &tree,
                    context,
                    matches!(purpose, PickerPurpose::Parent),
                    &mut items,
                );
            }
            PickerPurpose::Link => {
                tree = self.tree(true)?;
                prerequisites = self
                    .detail_of(context)
                    .map(|d| d.prerequisites.iter().map(|p| p.id).collect())
                    .unwrap_or_default();
                if let Some(sibs) = siblings(&tree, context) {
                    items = sibs
                        .iter()
                        .map(|n| &n.item)
                        .filter(|i| i.id.to_string() != context && !prerequisites.contains(&i.id))
                        .collect();
                }
            }
        }
        let needle = filter.trim().to_lowercase();
        Ok(items
            .into_iter()
            .map(|i| PickerRow {
                id: ss(i.id.to_string()),
                title: ss(&i.title),
                path: ss(i.path.join(" / ")),
            })
            .filter(|r| {
                needle.is_empty()
                    || r.title.to_lowercase().contains(&needle)
                    || r.path.to_lowercase().contains(&needle)
            })
            .take(50)
            .collect())
    }

    // ---- refresh ----

    /// Re-reads everything the window shows. Failures raise an error notice.
    pub fn refresh(&mut self) {
        let Some(win) = self.window.upgrade() else {
            return;
        };
        if let Err(e) = self.refresh_inner(&win) {
            self.set_notice(NoticeKind::Error, &e, "");
        }
    }

    fn refresh_inner(&mut self, win: &MainWindow) -> Result<(), String> {
        let app = win.global::<App<'_>>();
        let now = ZonedDateTime::now();
        let (identity, all) = self.scope_args();

        let QueryOutput::Today(today) = self.ask(Query::Today {
            identity: identity.clone(),
            all,
        })?
        else {
            return Err("unexpected query result".into());
        };
        let QueryOutput::Identities(identities) = self.ask(Query::Identities)? else {
            return Err("unexpected query result".into());
        };
        let names: Vec<String> = identities.iter().map(|i| i.name.clone()).collect();
        self.today = Some(today.today);
        self.mode_name = if today.mode.auto_away {
            None
        } else {
            today.mode.identity.clone()
        };
        self.resume = today.rhythm.resume.as_ref().map(|s| Resume {
            id: s.id.to_string(),
            is_habit: matches!(s.kind, SubjectKind::Habit),
        });

        app.set_identities(convert::model(names.iter().map(ss).collect()));
        app.set_identity_rows(convert::identity_rows(&identities));
        app.set_mode_label(ss(self.mode_name.as_deref().unwrap_or("")));
        app.set_mode_identity(ss(self.mode_name.as_deref().unwrap_or("")));
        app.set_timer(convert::timer_data(today.timer.as_ref(), &now));
        app.set_rhythm(convert::rhythm_data(&today.rhythm, &now));
        app.set_today(convert::today_data(&today, !identities.is_empty()));

        // Prompts are evaluated before the card is recorded: recording counts as
        // activity and would end the idleness being asked about.
        let rhythm = &today.rhythm;
        let mut prompt: Option<PromptData> = None;
        let last_activity = today.mode.last_activity;
        if let Some(t) = today
            .timer
            .as_ref()
            .filter(|t| t.idle && self.asked_timer != Some(t.segment))
        {
            self.asked_timer = Some(t.segment);
            prompt = Some(PromptData {
                kind: PromptKind::TimerIdle,
                label: ss(&t.target.label),
                since: ss(convert::local_hm(t.started_at, &now)),
                default_time: ss(convert::local_hm(
                    last_activity.unwrap_or(t.started_at),
                    &now,
                )),
                ..PromptData::default()
            });
        } else if today.mode.idle
            && today.mode.identity.is_some()
            && today.timer.is_none()
            && self.asked_mode != today.mode.since
        {
            self.asked_mode = today.mode.since;
            let since = today.mode.since.unwrap_or_else(|| now.timestamp());
            prompt = Some(PromptData {
                kind: PromptKind::ModeIdle,
                label: ss(today.mode.identity.as_deref().unwrap_or("")),
                since: ss(convert::local_hm(since, &now)),
                default_time: ss(convert::local_hm(last_activity.unwrap_or(since), &now)),
                ..PromptData::default()
            });
        } else if let Some(d) = rhythm.due.filter(|_| !self.was_due) {
            let focus = i32::try_from(rhythm.focus_minutes).unwrap_or(i32::MAX);
            let minutes = i32::try_from(d.minutes).unwrap_or(i32::MAX);
            crate::notify::desktop(
                "Patina",
                &win.global::<Labels<'_>>().invoke_break_due(focus, minutes),
            );
            prompt = Some(PromptData {
                kind: PromptKind::BreakDue,
                minutes,
                focus_minutes: focus,
                break_kind: convert::break_kind_of(&d),
                ..PromptData::default()
            });
        }
        self.was_due = rhythm.due.is_some();
        if let Some(mut p) = prompt {
            self.prompt_seq += 1;
            p.seq = self.prompt_seq;
            app.set_prompt(p);
        }
        if self.was_on_break && rhythm.on_break.is_none() && today.timer.is_none() {
            let text = win
                .global::<Labels<'_>>()
                .invoke_notice(NoticeKind::BreakOver, SharedString::new());
            crate::notify::desktop("Patina", &text);
            self.set_notice(NoticeKind::BreakOver, "", "");
        }
        self.was_on_break = rhythm.on_break.is_some();

        // The card, only while the Today page is visible.
        let on_today = app.get_page() == 0
            && (!app.get_detail_open() || app.get_two_pane())
            && !app.get_graph_open()
            && !app.get_settings_open();
        if on_today {
            let QueryOutput::Next(nv) = self.ask(Query::Next {
                identity: identity.clone(),
                all,
            })?
            else {
                return Err("unexpected query result".into());
            };
            let key = convert::card_key(&nv);
            let shown: NextView = if self.card_key.as_ref() == Some(&key) {
                *nv
            } else {
                let out = self.run(Command::Next(command::NextCmd {
                    action: command::NextAction::Show { identity, all },
                }))?;
                self.card_key = Some(key);
                out.card.map_or(*nv, |c| *c)
            };
            app.set_card(convert::card(&shown, &now));
        } else {
            self.card_key = None;
        }

        // The current page.
        match app.get_page() {
            1 => {
                let QueryOutput::Inbox(entries) = self.ask(Query::Inbox)? else {
                    return Err("unexpected query result".into());
                };
                app.set_inbox(convert::model(
                    entries
                        .iter()
                        .map(|e| convert::inbox_row(e, &now))
                        .collect(),
                ));
                let views = self.proposals()?;
                self.deselected
                    .retain(|id, _| views.iter().any(|p| p.id.to_string() == *id));
                app.set_proposals(convert::proposal_rows(&views, &self.deselected, &now));
            }
            2 => {
                let nodes = self.tree(self.tree_all)?;
                app.set_tree(convert::model(convert::flatten_tree(&nodes)));
            }
            3 => {
                let QueryOutput::Habits(habits) = self.ask(Query::Habits)? else {
                    return Err("unexpected query result".into());
                };
                let (graduated, active): (Vec<_>, Vec<_>) = habits
                    .into_iter()
                    .partition(|h| h.stage == patina_core::query::HabitStage::Graduated);
                app.set_habits(convert::habit_rows(&active));
                app.set_graduated_habits(convert::habit_rows(&graduated));
            }
            4 => {
                let QueryOutput::Review(review) = self.ask(Query::Review)? else {
                    return Err("unexpected query result".into());
                };
                let open = self.tree(false)?;
                app.set_review(convert::review_data(&review, &open, &names, &now));
            }
            _ => {}
        }

        if let Some(id) = self.detail.clone() {
            match self.detail_of(&id) {
                Ok(d) => app.set_detail(convert::detail_data(&d, &now)),
                Err(_) => {
                    self.detail = None;
                    app.set_detail_open(false);
                }
            }
        }

        if let Some(id) = self.graph.clone() {
            match self.ask(Query::Graph { item: id }) {
                Ok(QueryOutput::Graph(g)) => app.set_graph(convert::graph_data(&g)),
                _ => {
                    self.graph = None;
                    app.set_graph_open(false);
                }
            }
        }

        if app.get_settings_open() {
            let settings = self.patina.settings().map_err(err)?;
            let r = &settings.rhythm;
            let QueryOutput::Sources(sources) = self.ask(Query::Sources)? else {
                return Err("unexpected query result".into());
            };
            app.set_settings(SettingsData {
                language: convert::language_ui(self.language),
                sources: convert::source_rows(&sources, &now),
                storage: self.storage,
                root: ss(self.root.display().to_string()),
                device: ss(self.device.to_string()),
                version: ss(env!("CARGO_PKG_VERSION")),
                focus_minutes: i32::try_from(r.focus_minutes).unwrap_or(i32::MAX),
                short_break: i32::try_from(r.short_break_minutes).unwrap_or(i32::MAX),
                long_break: i32::try_from(r.long_break_minutes).unwrap_or(i32::MAX),
                rounds: i32::try_from(r.rounds_before_long).unwrap_or(i32::MAX),
                flow_ratio: i32::try_from(r.flow_ratio).unwrap_or(i32::MAX),
                snooze: i32::try_from(r.snooze_minutes).unwrap_or(i32::MAX),
            });
        }
        Ok(())
    }
}
