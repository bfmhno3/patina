use patina_core::command::*;
use patina_core::engine::Patina;
use patina_core::event::Source;
use patina_core::ids::DeviceId;
use patina_core::query::{Query, QueryOutput};
use patina_core::store::Store;
use uuid::Uuid;

const NOW: &str = "2026-11-24T09:00:00+08:00[Asia/Shanghai]";

fn run(p: &Patina, cmd: Command) {
    p.run(cmd, NOW.parse().unwrap(), Source::Cli).unwrap();
}

fn add(title: &str, parent: Option<&str>, identity: Option<&str>) -> Command {
    Command::AddItem(Box::new(AddItem {
        title: title.into(),
        identity: identity.map(String::from),
        parent: parent.map(String::from),
        tags: vec![],
        defer: None,
        scheduled: None,
        deadline: None,
        waiting_for: None,
        follow_up: None,
    }))
}

fn list(p: &Patina) -> Vec<patina_core::query::ItemView> {
    match p
        .query(Query::List { identity: None }, NOW.parse().unwrap())
        .unwrap()
    {
        QueryOutput::List(v) => v,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn frontier_follows_dependencies_and_wrap_up() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(
        dir.path().to_path_buf(),
        DeviceId::from_uuid(Uuid::from_u128(0xdead_beef)),
    );
    let p = Patina::new(store);
    run(&p, Command::Init);
    run(&p, add("P", None, Some("work")));
    run(&p, add("A", Some("P"), None));
    run(&p, add("B", Some("P"), None));
    run(
        &p,
        Command::Link(Link {
            item: "B".into(),
            depends_on: "A".into(),
        }),
    );
    let titles = |v: Vec<patina_core::query::ItemView>| -> Vec<String> {
        v.into_iter().map(|i| i.title).collect()
    };
    assert_eq!(titles(list(&p)), ["A"]);
    run(
        &p,
        Command::Complete(Complete {
            item: "A".into(),
            note: None,
        }),
    );
    assert_eq!(titles(list(&p)), ["B"]);
    run(
        &p,
        Command::Complete(Complete {
            item: "B".into(),
            note: None,
        }),
    );
    let l = list(&p);
    assert_eq!(titles(l.clone()), ["P"]);
    assert!(l[0].wrap_up);
    match p
        .query(Query::Today { identity: None }, NOW.parse().unwrap())
        .unwrap()
    {
        QueryOutput::Today(t) => assert_eq!(t.actionable.len(), 1),
        other => panic!("unexpected {other:?}"),
    }
}
