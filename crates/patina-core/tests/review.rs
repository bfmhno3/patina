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
