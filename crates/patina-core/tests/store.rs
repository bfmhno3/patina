use patina_core::event::*;
use patina_core::ids::*;
use patina_core::store::{LoadProblem, Store};
use uuid::Uuid;

fn event(n: u128, device: u128, at: &str) -> Event {
    let ts: jiff::Timestamp = at.parse().unwrap();
    Event {
        id: EventId::from_uuid(Uuid::from_u128(n)),
        device: DeviceId::from_uuid(Uuid::from_u128(device)),
        effective_at: ts,
        recorded_at: ts,
        source: Source::Cli,
        command: CommandId::from_uuid(Uuid::from_u128(n)),
        category: Category::Domain,
        data: EventData::ItemCaptured(ItemCaptured {
            item: ItemId::from_uuid(Uuid::from_u128(n)),
            text: format!("t{n}"),
        }),
    }
}

fn store(dir: &tempfile::TempDir, device: u128) -> Store {
    Store::new(
        dir.path().to_path_buf(),
        DeviceId::from_uuid(Uuid::from_u128(device)),
    )
}

#[test]
fn append_and_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir, 1);
    let a = [
        event(1, 1, "2026-11-24T01:00:00Z"),
        event(2, 1, "2026-11-24T01:00:00Z"),
    ];
    let b = [event(3, 1, "2026-11-24T02:00:00Z")];
    {
        let mut g = s.lock().unwrap();
        g.append(&a).unwrap();
    }
    s.lock().unwrap().append(&b).unwrap();
    let log = s.load().unwrap();
    assert!(log.warnings.is_empty());
    let expect: Vec<_> = a.iter().chain(b.iter()).cloned().collect();
    assert_eq!(log.events, expect);
}

#[test]
fn truncated_line_warns_and_next_append_repairs() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir, 1);
    s.lock()
        .unwrap()
        .append(&[event(1, 1, "2026-11-24T01:00:00Z")])
        .unwrap();
    let path = dir
        .path()
        .join("events")
        .join(format!("{}.jsonl", Uuid::from_u128(1)));
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{\"id\":\"trunc");
    std::fs::write(&path, text).unwrap();
    let log = s.load().unwrap();
    assert_eq!(log.events.len(), 1);
    assert_eq!(log.warnings.len(), 1);
    assert_eq!(log.warnings[0].line, 2);
    s.lock()
        .unwrap()
        .append(&[event(2, 1, "2026-11-24T02:00:00Z")])
        .unwrap();
    let log = s.load().unwrap();
    assert_eq!(log.events.len(), 2);
    assert_eq!(log.warnings.len(), 1);
}

#[test]
fn newer_version_is_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir, 1);
    s.lock()
        .unwrap()
        .append(&[event(1, 1, "2026-11-24T01:00:00Z")])
        .unwrap();
    let path = dir
        .path()
        .join("events")
        .join(format!("{}.jsonl", Uuid::from_u128(1)));
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"version\":1", "\"version\":2");
    std::fs::write(&path, text).unwrap();
    let log = s.load().unwrap();
    assert!(log.events.is_empty());
    assert!(matches!(
        log.warnings[0].problem,
        LoadProblem::UnsupportedVersion { version: 2, .. }
    ));
}

#[test]
fn devices_interleave_by_recorded_at() {
    let dir = tempfile::tempdir().unwrap();
    store(&dir, 1)
        .lock()
        .unwrap()
        .append(&[
            event(1, 1, "2026-11-24T01:00:00Z"),
            event(3, 1, "2026-11-24T03:00:00Z"),
        ])
        .unwrap();
    store(&dir, 2)
        .lock()
        .unwrap()
        .append(&[event(2, 2, "2026-11-24T02:00:00Z")])
        .unwrap();
    let ids: Vec<_> = store(&dir, 1)
        .load()
        .unwrap()
        .events
        .iter()
        .map(|e| e.id.as_uuid().as_u128())
        .collect();
    assert_eq!(ids, [1, 2, 3]);
}

#[test]
fn sync_conflict_copies_are_ignored_with_one_warning() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir, 1);
    s.lock()
        .unwrap()
        .append(&[event(1, 1, "2026-11-24T01:00:00Z")])
        .unwrap();
    let events = dir.path().join("events");
    let log_file = events.join(format!("{}.jsonl", Uuid::from_u128(1)));
    let copy = events.join(format!(
        "{}.sync-conflict-20261124-010000-ABCDEFG.jsonl",
        Uuid::from_u128(1)
    ));
    std::fs::copy(&log_file, &copy).unwrap();
    let log = s.load().unwrap();
    assert_eq!(log.events.len(), 1);
    assert_eq!(log.warnings.len(), 1);
    assert!(matches!(log.warnings[0].problem, LoadProblem::ForeignFile));
    assert_eq!(log.warnings[0].file, copy);
}

#[test]
fn the_same_event_in_two_device_files_loads_once() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir, 1);
    s.lock()
        .unwrap()
        .append(&[event(1, 1, "2026-11-24T01:00:00Z")])
        .unwrap();
    let events = dir.path().join("events");
    std::fs::copy(
        events.join(format!("{}.jsonl", Uuid::from_u128(1))),
        events.join(format!("{}.jsonl", Uuid::from_u128(2))),
    )
    .unwrap();
    let log = s.load().unwrap();
    assert_eq!(log.events.len(), 1);
    assert_eq!(log.warnings.len(), 1);
    assert!(matches!(
        log.warnings[0].problem,
        LoadProblem::DuplicateEvent { .. }
    ));
}
