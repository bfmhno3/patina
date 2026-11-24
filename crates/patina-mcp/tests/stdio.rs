use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use patina_core::command::{self, AddItem};
use patina_core::engine::Patina;
use patina_core::event::Source;
use patina_core::ids::DeviceId;
use patina_core::store::Store;
use serde_json::{Value, json};
use uuid::Uuid;

const DEVICE: u128 = 0x019a_b3a1_0000_7000_8000_0000_0000_00aa;

fn item(title: &str, identity: &str) -> command::Command {
    command::Command::AddItem(Box::new(AddItem {
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
    }))
}

#[test]
fn lists_tools_and_hides_work_items() {
    let dir = tempfile::tempdir().unwrap();
    let device = DeviceId::from_uuid(Uuid::from_u128(DEVICE));
    let p = Patina::new(Store::new(dir.path().to_path_buf(), device));
    let now = jiff::ZonedDateTime::now();
    p.run(command::Command::Init, now.clone(), Source::Cli)
        .unwrap();
    p.run(item("secret plan", "work"), now.clone(), Source::Cli)
        .unwrap();
    p.run(item("buy milk", "life"), now, Source::Cli).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_patina-mcp"))
        .env("PATINA_DIR", dir.path())
        .env("PATINA_DEVICE_ID", Uuid::from_u128(DEVICE).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let response = |id: i64| -> Value {
        loop {
            let v = rx.recv_timeout(Duration::from_secs(20)).expect("no reply");
            if v["id"] == json!(id) {
                return v;
            }
        }
    };

    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }}),
    );
    response(1);
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let tools = response(2);
    let mut names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "frontier",
            "habits",
            "identities",
            "next",
            "review",
            "show",
            "today",
            "tree"
        ]
    );
    send(
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "today", "arguments": {"all": true}
        }}),
    );
    let today = response(3);
    let text = today["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("buy milk"), "{text}");
    assert!(!text.contains("secret plan"), "{text}");
    drop(send);
    child.kill().unwrap();
    child.wait().unwrap();
}
