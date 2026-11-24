use jiff::ZonedDateTime;
use patina_core::command::*;
use patina_core::engine::{CommandOutcome, Error, Patina};
use patina_core::event::{EventData, Size, Source};
use patina_core::ids::DeviceId;
use patina_core::query::{Card, NextView, Query, QueryOutput};
use patina_core::recommend::{CardKind, Reason};
use patina_core::store::Store;
use uuid::Uuid;

fn at(day: &str, hhmm: &str) -> ZonedDateTime {
    format!("2026-11-{day}T{hhmm}:00+08:00[Asia/Shanghai]")
        .parse()
        .unwrap()
}

fn patina(dir: &tempfile::TempDir, day: &str) -> Patina {
    let p = Patina::new(Store::new(
        dir.path().to_path_buf(),
        DeviceId::from_uuid(Uuid::from_u128(0xdead_beef)),
    ));
    p.run(Command::Init, at(day, "08:00"), Source::Cli).unwrap();
    p
}

#[derive(Default)]
struct New<'a> {
    deadline: Option<&'a str>,
    important: bool,
    size: Option<Size>,
}

fn add(p: &Patina, day: &str, title: &str, o: New<'_>) {
    p.run(
        Command::AddItem(Box::new(AddItem {
            title: title.into(),
            identity: Some("work".into()),
            parent: None,
            tags: vec![],
            defer: None,
            scheduled: None,
            deadline: o.deadline.map(|d| d.parse().unwrap()),
            waiting_for: None,
            follow_up: None,
            important: o.important,
            size: o.size,
            repeat: None,
        })),
        at(day, "08:01"),
        Source::Cli,
    )
    .unwrap();
}

fn act(p: &Patina, day: &str, hhmm: &str, action: NextAction) -> Result<CommandOutcome, Error> {
    p.run(
        Command::Next(NextCmd { action }),
        at(day, hhmm),
        Source::Cli,
    )
}

fn show(p: &Patina, day: &str, hhmm: &str) -> NextView {
    let out = act(
        p,
        day,
        hhmm,
        NextAction::Show {
            identity: Some("work".into()),
            all: false,
        },
    )
    .unwrap();
    *out.card.unwrap()
}

fn item_card(v: &NextView) -> (&str, &[Reason]) {
    match &v.card {
        Card::Item { item, reasons, .. } => (&item.title, reasons),
        other => panic!("expected an item card, got {other:?}"),
    }
}

#[test]
fn deadline_beats_importance_until_skipped_and_the_skip_expires() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir, "24");
    add(
        &p,
        "24",
        "file taxes",
        New {
            deadline: Some("2026-11-24"),
            ..New::default()
        },
    );
    add(
        &p,
        "24",
        "side project",
        New {
            important: true,
            ..New::default()
        },
    );
    let v = show(&p, "24", "09:00");
    let (title, reasons) = item_card(&v);
    assert_eq!(title, "file taxes");
    assert_eq!(reasons, [Reason::Deadline { days: 0 }]);

    let skipped = act(&p, "24", "09:01", NextAction::Skip).unwrap();
    let v = *skipped.card.unwrap();
    let (title, reasons) = item_card(&v);
    assert_eq!(title, "side project");
    assert_eq!(reasons, [Reason::Important]);

    // Just inside the cooldown the skipped item is still penalized.
    let v = show(&p, "24", "13:00");
    assert_eq!(item_card(&v).0, "side project");
    // Four hours after the skip (strictly), the penalty is gone.
    let v = show(&p, "24", "13:01");
    assert_eq!(item_card(&v).0, "file taxes");
}

#[test]
fn shown_and_skipped_are_observations_that_survive_undo() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir, "24");
    let urgent = New {
        deadline: Some("2026-11-24"),
        ..New::default()
    };
    add(&p, "24", "task a", urgent);
    add(&p, "24", "task b", New::default());
    let out = act(
        &p,
        "24",
        "09:00",
        NextAction::Show {
            identity: None,
            all: true,
        },
    )
    .unwrap();
    assert!(
        out.events
            .iter()
            .all(|e| e.category == patina_core::event::Category::Observation)
    );
    // The observation-only command is never the undo target: undo hits the latest creation.
    let undone = p
        .run(Command::Undo, at("24", "09:05"), Source::Cli)
        .unwrap();
    assert_eq!(undone.undone, ["item.created"]);
}

#[test]
fn review_card_outranks_items_and_skipping_it_moves_on() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir, "16");
    add(&p, "16", "task", New::default());
    // Sunday the 22nd: the review is due.
    let v = show(&p, "22", "09:00");
    assert!(matches!(v.card, Card::Review { since: None }));
    let skipped = act(&p, "22", "09:01", NextAction::Skip).unwrap();
    assert_eq!(item_card(&skipped.card.unwrap()).0, "task");
    // After the cooldown the review comes back.
    let v = show(&p, "22", "13:02");
    assert!(matches!(v.card, Card::Review { .. }));
}

#[test]
fn split_shows_the_new_first_step_and_start_times_it() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir, "24");
    add(
        &p,
        "24",
        "write thesis",
        New {
            size: Some(Size::L),
            ..New::default()
        },
    );
    let v = show(&p, "24", "09:00");
    assert!(matches!(v.card, Card::BreakDown { .. }), "{:?}", v.card);
    let out = act(
        &p,
        "24",
        "09:01",
        NextAction::Split {
            text: "outline chapter 1".into(),
        },
    )
    .unwrap();
    let v = *out.card.unwrap();
    let (title, reasons) = item_card(&v);
    assert_eq!(title, "outline chapter 1");
    assert_eq!(reasons, [Reason::FirstStep]);
    match &v.card {
        Card::Item { score, item, .. } => {
            assert_eq!(*score, None);
            assert_eq!(item.path, ["write thesis"]);
        }
        other => panic!("{other:?}"),
    }
    let started = act(&p, "24", "09:02", NextAction::Start).unwrap();
    assert!(
        started
            .events
            .iter()
            .any(|e| matches!(e.data, EventData::TimerStarted(_)))
    );
    assert!(
        started
            .events
            .iter()
            .any(|e| matches!(e.data, EventData::RecommendationAccepted(_)))
    );
    assert!(matches!(
        act(&p, "24", "09:03", NextAction::Start).unwrap_err(),
        Error::Command(CommandError::AlreadyTiming)
    ));
}

#[test]
fn start_and_skip_need_a_shown_card() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir, "24");
    add(&p, "24", "task", New::default());
    for action in [
        NextAction::Start,
        NextAction::Skip,
        NextAction::Split { text: "x".into() },
    ] {
        assert!(matches!(
            act(&p, "24", "09:00", action).unwrap_err(),
            Error::Command(CommandError::NoRecommendation)
        ));
    }
}

#[test]
fn empty_card_lists_other_identities_and_query_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir, "24");
    p.run(
        Command::AddItem(Box::new(AddItem {
            title: "read paper".into(),
            identity: Some("study".into()),
            parent: None,
            tags: vec![],
            defer: None,
            scheduled: None,
            deadline: None,
            waiting_for: None,
            follow_up: None,
            important: false,
            size: None,
            repeat: None,
        })),
        at("24", "08:01"),
        Source::Cli,
    )
    .unwrap();
    let v = show(&p, "24", "09:00");
    match &v.card {
        Card::Empty { elsewhere } => {
            assert_eq!(elsewhere.len(), 1);
            assert_eq!(
                (elsewhere[0].identity.as_str(), elsewhere[0].actionable),
                ("study", 1)
            );
        }
        other => panic!("{other:?}"),
    }
    // Empty cards are shown (recorded) but cannot be skipped.
    assert!(matches!(
        act(&p, "24", "09:01", NextAction::Skip).unwrap_err(),
        Error::Command(CommandError::NoRecommendation)
    ));
    let before = std::fs::read_dir(dir.path().join("events"))
        .unwrap()
        .map(|e| std::fs::metadata(e.unwrap().path()).unwrap().len())
        .sum::<u64>();
    let q = Query::Next {
        identity: Some("study".into()),
        all: false,
    };
    match p.query(q, at("24", "09:02")).unwrap() {
        QueryOutput::Next(n) => assert_eq!(item_card(&n).0, "read paper"),
        other => panic!("unexpected {other:?}"),
    }
    let after = std::fs::read_dir(dir.path().join("events"))
        .unwrap()
        .map(|e| std::fs::metadata(e.unwrap().path()).unwrap().len())
        .sum::<u64>();
    assert_eq!(before, after);
    assert_eq!(CardKind::Empty, v.card.kind());
}
