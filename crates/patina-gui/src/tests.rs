//! End-to-end checks of the controller and wiring against a headless window.

use std::cell::RefCell;
use std::rc::Rc;

use patina_core::ids::DeviceId;
use patina_core::store::Store;
use slint::{ComponentHandle as _, Model as _};
use uuid::Uuid;

use crate::controller::Controller;
use crate::{App, ItemForm, MainWindow, NoticeKind, wire};

struct Harness {
    window: MainWindow,
    ctl: Rc<RefCell<Controller>>,
    dir: tempfile::TempDir,
}

fn harness() -> Harness {
    i_slint_backend_testing::init_no_event_loop();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(
        dir.path().to_path_buf(),
        DeviceId::from_uuid(Uuid::from_u128(7)),
    );
    let window = MainWindow::new().unwrap();
    let ctl = Rc::new(RefCell::new(Controller::new(
        store,
        dir.path().to_path_buf(),
        window.as_weak(),
    )));
    wire::connect(&window, ctl.clone());
    ctl.borrow_mut().refresh();
    Harness { window, ctl, dir }
}

fn form(title: &str, identity: &str) -> ItemForm {
    ItemForm {
        title: title.into(),
        identity: identity.into(),
        repeat_n: 1,
        ..ItemForm::default()
    }
}

impl Harness {
    fn app(&self) -> App<'_> {
        self.window.global::<App<'_>>()
    }
}

#[test]
fn daily_loop_from_onboarding_to_review() {
    let h = harness();
    let app = h.app();
    assert!(!app.get_today().has_identities);
    app.invoke_init_identities();
    assert_eq!(app.get_identities().row_count(), 3);
    assert!(app.get_today().has_identities);

    // A bad date is reported and nothing is created.
    let mut f = form("Pay rent", "life");
    f.deadline = "someday".into();
    assert_eq!(
        app.invoke_add_item(f.clone()),
        "invalid date \"someday\" (use YYYY-MM-DD, today, tomorrow, or +N)"
    );
    f.deadline = "today".into();
    assert_eq!(app.invoke_add_item(f), "");
    app.invoke_set_scope(1, "".into());
    assert_eq!(app.get_card().item.title, "Pay rent");
    assert_eq!(app.get_today().deadlines.row_count(), 1);

    // Capture then triage.
    assert_eq!(app.invoke_capture("call bank".into()), "");
    assert_eq!(app.get_today().inbox, 1);
    app.invoke_navigate(1);
    assert_eq!(app.get_inbox().row_count(), 1);
    let entry = app.get_inbox().row_data(0).unwrap();
    assert_eq!(app.invoke_triage(entry.id, form("call bank", "life")), "");
    assert_eq!(app.get_inbox().row_count(), 0);

    // Timer: start from the Today card, stop again.
    app.invoke_navigate(0);
    app.invoke_card_start();
    assert!(app.get_timer().running);
    assert_eq!(app.get_timer().label, "Pay rent");
    app.invoke_stop_timer();
    assert!(!app.get_timer().running);
    assert_eq!(app.get_notice().kind, NoticeKind::Saved);

    // Completing with a note, then undoing it from the notice.
    let id = app.get_card().item.id;
    assert_eq!(app.invoke_complete(id.clone(), "paid".into()), "");
    app.invoke_notice_action(app.get_notice());
    app.invoke_open_item(id);
    assert!(app.get_detail_open());
    assert_eq!(app.get_detail().item.status, "in_progress");
    app.invoke_close_detail();

    // The review records and clears the Today badge.
    app.invoke_navigate(4);
    assert_eq!(app.get_review().focus_groups.row_count(), 3);
    assert_eq!(
        app.invoke_review_finish("steady".into(), slint::ModelRc::default(), true),
        ""
    );
    assert!(!app.get_today().review_due);
    drop(h.ctl);
    drop(h.dir);
}

#[test]
fn habits_and_settings_round_trip() {
    let h = harness();
    let app = h.app();
    app.invoke_init_identities();
    assert_eq!(
        app.invoke_add_identity("work".into()).as_str(),
        "identity name \"work\" is already taken"
    );
    assert_eq!(app.invoke_set_target("work".into(), "7.5".into()), "");
    assert_eq!(
        app.invoke_set_target("work".into(), "x".into()),
        "invalid hours \"x\" (use a number like 20 or 7.5, or none)"
    );
    app.invoke_open_settings();
    let settings = app.get_settings();
    let work = settings.identities.row_data(0).unwrap();
    assert_eq!((work.name.as_str(), work.target.as_str()), ("work", "7.5"));
    assert_eq!(settings.focus_minutes, 25);
    app.invoke_close_settings();

    let habit = crate::HabitForm {
        name: "read".into(),
        identity: "study".into(),
        cue: "".into(),
        kind: 0,
        target: 1,
        frequency: 0,
        times: 3,
        days: slint::ModelRc::default(),
    };
    assert_eq!(app.invoke_add_habit(habit), "");
    app.invoke_navigate(3);
    let row = app.get_habits().row_data(0).unwrap();
    assert_eq!(row.name, "read");
    assert_eq!(app.invoke_check_in(row.id.clone(), 0, -1, "ok".into()), "");
    assert_eq!(
        app.invoke_check_in(row.id, 0, -1, "".into()),
        "already checked in on ".to_string() + &app.get_today().today
    );
}

#[test]
fn break_reminder_prompts_once_and_starting_the_break_stops_the_timer() {
    use patina_core::command::{Command, StartTimer, TimerRef};
    use patina_core::engine::Patina;
    use patina_core::event::Source;

    let h = harness();
    std::fs::write(
        h.dir.path().join("config.toml"),
        "[rhythm]\nfocus_minutes = 1\n",
    )
    .unwrap();
    let app = h.app();
    app.invoke_init_identities();
    assert_eq!(app.invoke_add_item(form("Write parser", "work")), "");

    // Start a timer two minutes in the past, as the CLI's `start --at` would.
    let patina = Patina::new(Store::new(
        h.dir.path().to_path_buf(),
        DeviceId::from_uuid(Uuid::from_u128(7)),
    ));
    let now = jiff::ZonedDateTime::now();
    let start = Command::StartTimer(StartTimer {
        target: TimerRef::Item("Write parser".into()),
        at: Some(now.timestamp() - jiff::SignedDuration::from_mins(2)),
    });
    patina.run(start, now, Source::Cli).unwrap();
    h.ctl.borrow_mut().refresh();
    assert_eq!(app.get_prompt().kind, crate::PromptKind::BreakDue);
    let seq = app.get_prompt().seq;
    assert!(app.get_rhythm().due);

    // A second refresh does not prompt again.
    h.ctl.borrow_mut().refresh();
    assert_eq!(app.get_prompt().seq, seq);

    app.invoke_start_break();
    assert!(!app.get_timer().running);
    assert!(app.get_rhythm().on_break);
    assert_eq!(app.get_rhythm().resume_label, "Write parser");
    app.invoke_resume();
    assert!(app.get_timer().running);
}

#[test]
fn graduating_a_habit_moves_it_to_the_graduated_list() {
    let h = harness();
    let app = h.app();
    app.invoke_init_identities();
    let habit = crate::HabitForm {
        name: "read".into(),
        identity: "study".into(),
        cue: "".into(),
        kind: 0,
        target: 1,
        frequency: 0,
        times: 3,
        days: slint::ModelRc::default(),
    };
    assert_eq!(app.invoke_add_habit(habit), "");
    app.invoke_navigate(3);
    let row = app.get_habits().row_data(0).unwrap();
    app.invoke_graduate_habit(row.id.clone());
    assert_eq!(app.get_habits().row_count(), 0);
    let graduated = app.get_graduated_habits().row_data(0).unwrap();
    assert_eq!(graduated.name, "read");
    assert_eq!(graduated.stage, "graduated");
}
