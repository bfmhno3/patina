use jiff::ZonedDateTime;
use patina_core::command::*;
use patina_core::engine::{Error, Patina};
use patina_core::event::Source;
use patina_core::ids::DeviceId;
use patina_core::query::{Query, QueryOutput, ReviewView};
use patina_core::store::Store;
use uuid::Uuid;

fn at(day: &str, hhmm: &str) -> ZonedDateTime {
    format!("2026-11-{day}T{hhmm}:00+08:00[Asia/Shanghai]")
        .parse()
        .unwrap()
}

fn patina(dir: &tempfile::TempDir) -> Patina {
    let p = Patina::new(Store::new(
        dir.path().to_path_buf(),
        DeviceId::from_uuid(Uuid::from_u128(0xdead_beef)),
    ));
    p.run(Command::Init, at("16", "08:00"), Source::Cli)
        .unwrap();
    p
}

fn add(p: &Patina, title: &str, identity: &str) {
    p.run(
        Command::AddItem(Box::new(AddItem {
            title: title.into(),
            identity: Some(identity.into()),
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
        at("16", "08:01"),
        Source::Cli,
    )
    .unwrap();
}

fn done(p: &Patina, day: &str, hhmm: &str, title: &str) {
    p.run(
        Command::Complete(Complete {
            item: title.into(),
            note: None,
        }),
        at(day, hhmm),
        Source::Cli,
    )
    .unwrap();
}

fn review(p: &Patina, day: &str, hhmm: &str) -> ReviewView {
    match p.query(Query::Review, at(day, hhmm)).unwrap() {
        QueryOutput::Review(r) => *r,
        other => panic!("unexpected {other:?}"),
    }
}

fn finish(p: &Patina, day: &str, hhmm: &str, focus: &[&str]) -> Result<(), Error> {
    p.run(
        Command::CompleteReview(CompleteReview {
            conclusion: Some("steady".into()),
            focus: focus.iter().map(|s| s.to_string()).collect(),
            minimal: false,
        }),
        at(day, hhmm),
        Source::Cli,
    )
    .map(|_| ())
}

#[test]
fn window_starts_at_the_previous_review_and_due_resets() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    for t in ["a", "b", "c"] {
        add(&p, t, "work");
    }
    done(&p, "22", "09:00", "a"); // Sunday
    assert!(review(&p, "22", "09:30").due);
    finish(&p, "22", "10:00", &["c"]).unwrap();
    let r = review(&p, "22", "10:05");
    assert!(!r.due);
    assert_eq!(r.focus.len(), 1);
    done(&p, "23", "09:00", "b");
    let r = review(&p, "24", "09:00");
    let titles: Vec<&str> = r
        .completed
        .iter()
        .flat_map(|g| g.items.iter().map(|i| i.title.as_str()))
        .collect();
    assert_eq!(titles, ["b"]);
    assert!(r.since.is_some());
    assert!(!r.due);
    // Next Sunday it is due again.
    assert!(review(&p, "29", "09:00").due);
}

#[test]
fn focus_allows_one_open_item_per_identity() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "w1", "work");
    add(&p, "w2", "work");
    add(&p, "l1", "life");
    done(&p, "16", "09:00", "l1");
    let err = finish(&p, "22", "10:00", &["w1", "w2"]).unwrap_err();
    assert!(matches!(
        err,
        Error::Command(CommandError::DuplicateFocus { .. })
    ));
    let err = finish(&p, "22", "10:00", &["l1"]).unwrap_err();
    assert!(matches!(
        err,
        Error::Command(CommandError::Rejected(
            patina_core::state::Rejection::WrongStatus { .. }
        ))
    ));
    add(&p, "l2", "life");
    finish(&p, "22", "10:00", &["w1", "l2"]).unwrap();
    assert_eq!(review(&p, "22", "10:01").focus.len(), 2);
}

#[test]
fn project_progress_counts_leaves_completed_in_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "thesis", "study");
    for t in ["intro", "method", "results"] {
        p.run(
            Command::AddItem(Box::new(AddItem {
                title: t.into(),
                identity: None,
                parent: Some("thesis".into()),
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
            at("16", "08:02"),
            Source::Cli,
        )
        .unwrap();
    }
    done(&p, "17", "09:00", "intro");
    finish(&p, "22", "10:00", &[]).unwrap();
    done(&p, "23", "09:00", "method");
    let r = review(&p, "24", "09:00");
    assert_eq!(r.projects.len(), 1);
    let pr = &r.projects[0];
    assert_eq!(
        (pr.project.title.as_str(), pr.completed, pr.done, pr.open),
        ("thesis", 1, 2, 1)
    );
}

fn run(p: &Patina, day: &str, hhmm: &str, cmd: Command) {
    p.run(cmd, at(day, hhmm), Source::Cli).unwrap();
}

fn child(p: &Patina, title: &str, parent: &str, day: &str, hhmm: &str) {
    run(
        p,
        day,
        hhmm,
        Command::AddItem(Box::new(AddItem {
            title: title.into(),
            identity: None,
            parent: Some(parent.into()),
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
    );
}

#[test]
fn review_statistics_cover_time_interruptions_notes_and_projects() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "proj", "work");
    child(&p, "p1", "proj", "16", "08:02");
    finish(&p, "18", "08:00", &[]).unwrap();

    run(
        &p,
        "19",
        "08:20",
        Command::SetIdentityTarget(SetIdentityTarget {
            identity: "work".into(),
            weekly_minutes: Some(600),
        }),
    );
    add(&p, "w", "work");
    add(&p, "l", "life");
    child(&p, "p2", "proj", "19", "08:25");
    run(
        &p,
        "19",
        "08:30",
        Command::AddHabit(AddHabit {
            name: "read".into(),
            identity: "study".into(),
            kind: patina_core::event::HabitKind::Check,
            frequency: patina_core::event::Frequency::Daily,
            cue: None,
        }),
    );
    let mode = |id: &str| {
        Command::SwitchMode(SwitchMode {
            identity: Some(id.into()),
            at: None,
        })
    };
    let start = |item: &str| {
        Command::StartTimer(StartTimer {
            target: TimerRef::Item(item.into()),
            at: None,
        })
    };
    run(&p, "19", "09:00", mode("work"));
    run(&p, "19", "10:00", start("w"));
    run(
        &p,
        "19",
        "10:30",
        Command::Interrupt(Interrupt {
            text: "call".into(),
            now: true,
            identity: Some("work".into()),
            left_off: None,
        }),
    );
    run(
        &p,
        "19",
        "11:30",
        Command::Complete(Complete {
            item: "call".into(),
            note: Some("nailed it".into()),
        }),
    );
    run(
        &p,
        "19",
        "14:30",
        Command::Capture(Capture {
            text: "idea".into(),
        }),
    );
    run(
        &p,
        "19",
        "14:31",
        Command::CheckIn(CheckIn {
            habit: "read".into(),
            day: None,
            count: None,
            note: Some("two chapters".into()),
        }),
    );
    run(&p, "19", "15:00", mode("life"));
    run(&p, "19", "15:10", start("l"));
    run(
        &p,
        "19",
        "15:40",
        Command::StopTimer(StopTimer { at: None }),
    );

    let r = review(&p, "25", "08:00");
    let work = r.time.iter().find(|t| t.identity == "work").unwrap();
    assert_eq!(work.tracked_minutes, 90);
    assert_eq!(work.target_minutes, Some(600));
    assert_eq!(work.deviation_minutes, Some(-510));
    // 09:00-15:00 in work mode minus the 13:30-14:30 stretch with no activity or timer.
    // Nothing happened between 11:30 and 14:30: the last hour of that gap is idle.
    assert_eq!(work.mode_minutes, 360 - 60);
    let life = r.time.iter().find(|t| t.identity == "life").unwrap();
    assert_eq!(life.tracked_minutes, 30);
    assert_eq!(life.target_minutes, None);
    assert_eq!(r.interruption_sources.len(), 1);
    assert_eq!(r.interruption_sources[0].identity.as_deref(), Some("work"));
    assert_eq!(r.interruption_sources[0].count, 1);
    let kinds: Vec<_> = r
        .notes
        .iter()
        .map(|n| (n.subject.as_str(), n.kind))
        .collect();
    assert!(kinds.contains(&("call", patina_core::query::NoteKind::Completion)));
    assert!(kinds.contains(&("read", patina_core::query::NoteKind::CheckIn)));
    let proj = r
        .open_projects
        .iter()
        .find(|pr| pr.project.title == "proj")
        .unwrap();
    assert_eq!(proj.open_change, 1);
}
