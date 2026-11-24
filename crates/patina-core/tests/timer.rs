use jiff::ZonedDateTime;
use patina_core::command::*;
use patina_core::engine::{CommandOutcome, Error, Patina};
use patina_core::event::{EventData, Frequency, HabitKind, Source};
use patina_core::ids::DeviceId;
use patina_core::query::{ItemView, Query, QueryOutput, StatusLine};
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

fn run(p: &Patina, now: &str, cmd: Command) -> Result<CommandOutcome, Error> {
    p.run(cmd, at(now), Source::Cli)
}

fn add(p: &Patina, title: &str, parent: Option<&str>) {
    run(
        p,
        "08:01",
        Command::AddItem(Box::new(AddItem {
            title: title.into(),
            identity: Some("work".into()),
            parent: parent.map(String::from),
            tags: vec![],
            defer: None,
            scheduled: None,
            deadline: None,
            waiting_for: None,
            follow_up: None,
        })),
    )
    .unwrap();
}

fn start(item: &str, at: Option<&str>) -> Command {
    Command::StartTimer(StartTimer {
        target: TimerRef::Item(item.into()),
        at: at.map(|h| self::at(h).timestamp()),
    })
}

fn stop(at_: Option<&str>) -> Command {
    Command::StopTimer(StopTimer {
        at: at_.map(|h| at(h).timestamp()),
    })
}

fn status(p: &Patina, now: &str) -> StatusLine {
    match p.query(Query::Status, at(now)).unwrap() {
        QueryOutput::Status(s) => s,
        other => panic!("unexpected {other:?}"),
    }
}

fn item(p: &Patina, now: &str, name: &str) -> ItemView {
    match p.query(Query::Show { item: name.into() }, at(now)).unwrap() {
        QueryOutput::Show(d) => d.item,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn starting_another_timer_stops_the_running_one_at_the_same_instant() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "A", None);
    add(&p, "B", None);
    run(&p, "09:00", start("A", None)).unwrap();
    let out = run(&p, "09:30", start("B", None)).unwrap();
    let kinds: Vec<&str> = out.events.iter().map(|e| e.data.kind()).collect();
    assert_eq!(kinds, ["timer.stopped", "timer.started"]);
    assert_eq!(out.events[0].effective_at, out.events[1].effective_at);
    let stopped = out.stopped.unwrap();
    assert_eq!((stopped.minutes, stopped.total_minutes), (30, 30));
    assert_eq!(item(&p, "10:00", "A").tracked_minutes, 30);
    assert_eq!(item(&p, "10:00", "B").tracked_minutes, 30);
    let timer = status(&p, "10:00").timer.unwrap();
    assert_eq!(timer.target.label, "B");
    assert!(matches!(
        run(&p, "10:00", start("B", None)).unwrap_err(),
        Error::Command(CommandError::AlreadyTiming)
    ));
}

#[test]
fn stop_before_the_segment_start_is_refused_and_a_correction_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "A", None);
    run(&p, "09:00", start("A", None)).unwrap();
    assert!(run(&p, "12:00", stop(Some("08:30"))).is_err());
    let out = run(&p, "12:00", stop(Some("09:40"))).unwrap();
    assert_eq!(out.stopped.unwrap().minutes, 40);
    assert!(matches!(
        run(&p, "12:01", stop(None)).unwrap_err(),
        Error::Command(CommandError::NoRunningTimer)
    ));
    assert!(matches!(
        run(&p, "12:01", start("A", Some("13:00"))).unwrap_err(),
        Error::Command(CommandError::FutureTime { .. })
    ));
}

#[test]
fn completing_the_timed_item_stops_its_segment() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "A", None);
    run(&p, "09:00", start("A", None)).unwrap();
    let out = run(
        &p,
        "09:20",
        Command::Complete(Complete {
            item: "A".into(),
            note: None,
        }),
    )
    .unwrap();
    assert_eq!(out.stopped.unwrap().minutes, 20);
    assert!(status(&p, "09:21").timer.is_none());
}

#[test]
fn duration_habit_completes_from_tracked_segments() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    run(
        &p,
        "08:02",
        Command::AddHabit(AddHabit {
            name: "read".into(),
            identity: "study".into(),
            kind: HabitKind::Duration { minutes: 30 },
            frequency: Frequency::Daily,
            cue: None,
        }),
    )
    .unwrap();
    let habit = |h: &str| {
        Command::StartTimer(StartTimer {
            target: TimerRef::Habit(h.into()),
            at: None,
        })
    };
    let value = |now: &str| match p.query(Query::Habits, at(now)).unwrap() {
        QueryOutput::Habits(h) => (h[0].value, h[0].minutes),
        other => panic!("unexpected {other:?}"),
    };
    run(&p, "09:00", habit("read")).unwrap();
    run(&p, "09:15", stop(None)).unwrap();
    assert_eq!(value("09:16"), (0.5, Some(15)));
    assert_eq!(status(&p, "09:16").habits_left, 1);
    run(&p, "10:00", habit("read")).unwrap();
    run(&p, "10:15", stop(None)).unwrap();
    assert_eq!(value("10:16"), (1.0, Some(30)));
    assert_eq!(status(&p, "10:16").habits_left, 0);
}

#[test]
fn undo_of_a_stop_resumes_the_timer() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "A", None);
    run(&p, "09:00", start("A", None)).unwrap();
    run(&p, "09:10", stop(None)).unwrap();
    assert!(status(&p, "09:11").timer.is_none());
    let out = run(&p, "09:12", Command::Undo).unwrap();
    assert_eq!(out.undone, ["timer.stopped"]);
    assert_eq!(status(&p, "09:13").timer.unwrap().target.label, "A");
    assert!(
        out.events
            .iter()
            .all(|e| matches!(e.data, EventData::CommandUndone(_)))
    );
}
