use jiff::ZonedDateTime;
use jiff::civil::Date;
use patina_core::command::*;
use patina_core::engine::Patina;
use patina_core::event::{Frequency, HabitKind, Source};
use patina_core::ids::DeviceId;
use patina_core::query::{HabitStage, HabitView, Query, QueryOutput};
use patina_core::store::Store;
use uuid::Uuid;

fn at(day: Date, hhmm: &str) -> ZonedDateTime {
    format!("{day}T{hhmm}:00+08:00[Asia/Shanghai]")
        .parse()
        .unwrap()
}

fn day(s: &str) -> Date {
    s.parse().unwrap()
}

fn plus(d: Date, n: i64) -> Date {
    d.checked_add(jiff::Span::new().days(n)).unwrap()
}

fn habit(p: &Patina, now: ZonedDateTime) -> HabitView {
    match p.query(Query::Habits, now).unwrap() {
        QueryOutput::Habits(mut h) => h.remove(0),
        other => panic!("unexpected {other:?}"),
    }
}

fn today_habits(p: &Patina, now: ZonedDateTime) -> usize {
    let q = Query::Today {
        identity: None,
        all: true,
    };
    match p.query(q, now).unwrap() {
        QueryOutput::Today(t) => t.habits.len(),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn graduation_freezes_strength_and_reinstatement_resumes_it() {
    let dir = tempfile::tempdir().unwrap();
    let p = Patina::new(Store::new(
        dir.path().to_path_buf(),
        DeviceId::from_uuid(Uuid::from_u128(0xdead_beef)),
    ));
    let start = day("2026-06-01");
    p.run(Command::Init, at(start, "08:00"), Source::Cli)
        .unwrap();
    p.run(
        Command::AddHabit(AddHabit {
            name: "stretch".into(),
            identity: "life".into(),
            kind: HabitKind::Check,
            frequency: Frequency::Daily,
            cue: None,
        }),
        at(start, "08:02"),
        Source::Cli,
    )
    .unwrap();
    for i in 0..80 {
        p.run(
            Command::CheckIn(CheckIn {
                habit: "stretch".into(),
                day: None,
                count: None,
                note: None,
            }),
            at(plus(start, i), "09:00"),
            Source::Cli,
        )
        .unwrap();
    }
    let last = plus(start, 79);
    let h = habit(&p, at(last, "10:00"));
    assert_eq!(h.stage, HabitStage::Maintaining);
    assert!(h.graduation_ready);

    let grad = |name: &str| {
        Command::GraduateHabit(GraduateHabit {
            habit: name.into(),
            from: None,
        })
    };
    p.run(grad("stretch"), at(plus(last, 1), "09:00"), Source::Cli)
        .unwrap();
    let day1 = at(plus(last, 1), "10:00");
    assert_eq!(habit(&p, day1.clone()).stage, HabitStage::Graduated);
    assert_eq!(today_habits(&p, day1.clone()), 0);
    let frozen = habit(&p, day1).strength;

    let later = at(plus(last, 11), "10:00");
    assert_eq!(habit(&p, later.clone()).strength, frozen);
    let err = p
        .run(
            Command::CheckIn(CheckIn {
                habit: "stretch".into(),
                day: None,
                count: None,
                note: None,
            }),
            later.clone(),
            Source::Cli,
        )
        .unwrap_err();
    assert!(err.to_string().contains("habit has graduated on"), "{err}");

    p.run(
        Command::ReinstateHabit(ReinstateHabit {
            habit: "stretch".into(),
            from: None,
        }),
        at(plus(last, 11), "11:00"),
        Source::Cli,
    )
    .unwrap();
    let back = at(plus(last, 11), "12:00");
    assert!(habit(&p, back.clone()).due);
    assert_eq!(today_habits(&p, back), 1);
}
