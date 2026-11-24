use jiff::ZonedDateTime;
use patina_core::command::*;
use patina_core::engine::Patina;
use patina_core::event::{Frequency, HabitKind, Source};
use patina_core::ids::DeviceId;
use patina_core::query::{Query, QueryOutput, TodayView};
use patina_core::store::Store;
use uuid::Uuid;

fn at(hhmm: &str) -> ZonedDateTime {
    format!("2026-11-24T{hhmm}:00+08:00[Asia/Shanghai]")
        .parse()
        .unwrap()
}

fn patina(dir: &tempfile::TempDir) -> Patina {
    let p = Patina::new(Store::new(
        dir.path().to_path_buf(),
        DeviceId::from_uuid(Uuid::from_u128(0xdead_beef)),
    ));
    p.run(Command::Init, at("08:00"), Source::Cli).unwrap();
    p
}

fn add(p: &Patina, title: &str, identity: &str, deadline: Option<&str>) {
    p.run(
        Command::AddItem(Box::new(AddItem {
            title: title.into(),
            identity: Some(identity.into()),
            parent: None,
            tags: vec![],
            defer: None,
            scheduled: None,
            deadline: deadline.map(|d| d.parse().unwrap()),
            waiting_for: None,
            follow_up: None,
            important: false,
            size: None,
            repeat: None,
        })),
        at("08:01"),
        Source::Cli,
    )
    .unwrap();
}

fn today(p: &Patina, now: &str, identity: &str) -> TodayView {
    let q = Query::Today {
        identity: Some(identity.into()),
        all: false,
    };
    match p.query(q, at(now)).unwrap() {
        QueryOutput::Today(t) => *t,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn banner_shows_other_identities_deadlines_but_not_plain_items() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "plain life task", "life", None);
    add(&p, "pay rent", "life", Some("2026-11-24"));
    add(&p, "later life deadline", "life", Some("2026-12-24"));
    add(&p, "work task", "work", None);
    let t = today(&p, "09:00", "work");
    let titles: Vec<&str> = t
        .banner
        .deadlines
        .iter()
        .map(|i| i.title.as_str())
        .collect();
    assert_eq!(titles, ["pay rent"]);
    assert!(t.actionable.iter().all(|i| i.title == "work task"));
}

#[test]
fn unfinished_habits_of_other_identities_enter_the_banner_from_evening_hour() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    p.run(
        Command::AddHabit(AddHabit {
            name: "stretch".into(),
            identity: "life".into(),
            kind: HabitKind::Check,
            frequency: Frequency::Daily,
            cue: None,
        }),
        at("08:02"),
        Source::Cli,
    )
    .unwrap();
    assert!(today(&p, "19:59", "work").banner.habits.is_empty());
    let t = today(&p, "20:00", "work");
    assert_eq!(t.banner.habits.len(), 1);
    assert!(t.habits.is_empty());
    p.run(
        Command::CheckIn(CheckIn {
            habit: "stretch".into(),
            day: None,
            count: None,
            note: None,
        }),
        at("20:05"),
        Source::Cli,
    )
    .unwrap();
    assert!(today(&p, "20:10", "work").banner.habits.is_empty());
}
