use jiff::ZonedDateTime;
use patina_core::command::*;
use patina_core::engine::{Error, Patina};
use patina_core::event::{EventData, Source};
use patina_core::ids::DeviceId;
use patina_core::proposal::ProposalOp;
use patina_core::query::{ProposalView, Query, QueryOutput, UnitState};
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

fn mcp() -> Source {
    Source::Mcp { client: "t".into() }
}

fn new_item(title: &str, identity: Option<&str>, parent: Option<&str>) -> AddItem {
    AddItem {
        title: title.into(),
        identity: identity.map(String::from),
        parent: parent.map(String::from),
        tags: vec![],
        defer: None,
        scheduled: None,
        deadline: None,
        waiting_for: None,
        follow_up: None,
        important: false,
        size: None,
        repeat: None,
    }
}

fn add(p: &Patina, title: &str, identity: &str, hhmm: &str) {
    p.run(
        Command::AddItem(Box::new(new_item(title, Some(identity), None))),
        at(hhmm),
        Source::Cli,
    )
    .unwrap();
}

fn retitle(item: &str, title: &str) -> Edit {
    Edit {
        item: item.into(),
        title: Some(title.into()),
        identity: None,
        add_tags: vec![],
        remove_tags: vec![],
        defer: None,
        scheduled: None,
        deadline: None,
        follow_up: None,
        waiting_for: None,
        important: None,
        size: None,
        repeat: None,
    }
}

fn proposals(p: &Patina, all: bool) -> Vec<ProposalView> {
    match p
        .query(Query::Proposals { all, source: None }, at("23:00"))
        .unwrap()
    {
        QueryOutput::Proposals(v) => v,
        other => panic!("unexpected {other:?}"),
    }
}

fn err(r: Result<impl std::fmt::Debug, Error>) -> String {
    r.unwrap_err().to_string()
}

#[test]
fn accepting_a_part_brings_what_it_needs_and_attributes_changes_to_the_proposer() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "P", "life", "08:01");
    let ops = vec![
        ProposalOp::AddItem {
            key: "a".into(),
            item: new_item("a", None, Some("P")),
        },
        ProposalOp::AddItem {
            key: "b".into(),
            item: new_item("b", None, Some("P")),
        },
        ProposalOp::Link(Link {
            item: "@b".into(),
            depends_on: "@a".into(),
        }),
    ];
    p.run(
        Command::Propose(Propose {
            summary: "split P".into(),
            ops,
        }),
        at("09:00"),
        mcp(),
    )
    .unwrap();
    let pending = proposals(&p, false);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].units.len(), 2);
    assert_eq!(pending[0].units[1].needs, vec![0]);
    assert!(pending[0].units.iter().all(|u| u.state == UnitState::Ready));

    let out = p
        .run(
            Command::AcceptProposal(AcceptProposal {
                proposal: pending[0].short.clone(),
                units: Some(vec![1]),
            }),
            at("09:10"),
            Source::Gui,
        )
        .unwrap();
    let created: Vec<_> = out
        .events
        .iter()
        .filter(|e| matches!(e.data, EventData::ItemCreated(_)))
        .collect();
    assert_eq!(created.len(), 2);
    assert!(created.iter().all(|e| e.source == mcp()));
    let accepted = out
        .events
        .iter()
        .find_map(|e| match &e.data {
            EventData::ProposalAccepted(a) => Some((e.source.clone(), a.accepted.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(accepted, (Source::Gui, vec![0, 1]));
    assert!(proposals(&p, false).is_empty());
    let all = proposals(&p, true);
    assert!(all[0].units.iter().all(|u| u.state == UnitState::Accepted));
}

#[test]
fn a_part_that_touches_an_item_changed_since_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "P", "life", "08:01");
    p.run(
        Command::Propose(Propose {
            summary: "rename P".into(),
            ops: vec![ProposalOp::Edit(Box::new(retitle("P", "Project")))],
        }),
        at("09:00"),
        mcp(),
    )
    .unwrap();
    p.run(
        Command::Edit(Box::new(retitle("P", "P by hand"))),
        at("09:05"),
        Source::Cli,
    )
    .unwrap();
    let pending = proposals(&p, false);
    assert_eq!(pending[0].units[0].state, UnitState::Stale);
    let message = err(p.run(
        Command::AcceptProposal(AcceptProposal {
            proposal: pending[0].short.clone(),
            units: None,
        }),
        at("09:10"),
        Source::Cli,
    ));
    assert!(message.contains("out of date"), "{message}");
}

#[test]
fn default_level_may_only_propose_and_work_items_stay_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let p = patina(&dir);
    add(&p, "P", "life", "08:01");
    add(&p, "W", "work", "08:02");
    let message = err(p.run(
        Command::Edit(Box::new(retitle("P", "x"))),
        at("09:00"),
        mcp(),
    ));
    assert!(message.contains("may only capture"), "{message}");
    let message = err(p.run(
        Command::Propose(Propose {
            summary: "note".into(),
            ops: vec![ProposalOp::Note(Note {
                item: "W".into(),
                text: "hi".into(),
            })],
        }),
        at("09:00"),
        mcp(),
    ));
    assert!(
        message.starts_with("proposal step 1: no match for"),
        "{message}"
    );
}

#[test]
fn trusted_clients_write_directly_and_the_owner_undoes_them_in_bulk() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[mcp.clients]\nt = \"trusted\"\n",
    )
    .unwrap();
    let p = patina(&dir);
    add(&p, "P", "life", "08:01");
    p.run(
        Command::Edit(Box::new(retitle("P", "P2"))),
        at("09:00"),
        mcp(),
    )
    .unwrap();
    assert!(
        err(p.run(Command::Undo, at("09:01"), mcp())).contains("only the owner"),
        "trusted clients cannot undo"
    );
    let sources = match p.query(Query::Sources, at("09:02")).unwrap() {
        QueryOutput::Sources(s) => s,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].commands, 1);
    p.run(
        Command::UndoSource(UndoSource {
            source: "mcp:t".into(),
            since: None,
        }),
        at("09:03"),
        Source::Cli,
    )
    .unwrap();
    match p
        .query(Query::Show { item: "P".into() }, at("09:04"))
        .unwrap()
    {
        QueryOutput::Show(d) => assert_eq!(d.item.title, "P"),
        other => panic!("unexpected {other:?}"),
    }
    assert!(
        err(p.run(
            Command::UndoSource(UndoSource {
                source: "cli".into(),
                since: None
            }),
            at("09:05"),
            Source::Cli
        ))
        .contains("only mcp:<client>")
    );
}

#[test]
fn read_only_clients_cannot_even_capture() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[mcp]\ndefault_level = \"read_only\"\n",
    )
    .unwrap();
    let p = patina(&dir);
    let message = err(p.run(
        Command::Capture(Capture { text: "x".into() }),
        at("09:00"),
        mcp(),
    ));
    assert!(message.contains("only read"), "{message}");
}
