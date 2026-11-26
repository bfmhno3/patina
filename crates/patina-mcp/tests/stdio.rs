use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
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

/// A running `patina-mcp` over stdio.
struct Server {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Value>,
    next_id: i64,
}

impl Server {
    fn start(dir: &tempfile::TempDir) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_patina-mcp"))
            .env("PATINA_DIR", dir.path())
            .env("PATINA_DEVICE_ID", Uuid::from_u128(DEVICE).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
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
        let mut server = Self {
            child,
            stdin,
            rx,
            next_id: 0,
        };
        server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        writeln!(
            server.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .unwrap();
        server
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        self.stdin.flush().unwrap();
        loop {
            let v = self
                .rx
                .recv_timeout(Duration::from_secs(20))
                .expect("no reply");
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    fn tool_names(&mut self) -> Vec<String> {
        let tools = self.request("tools/list", json!({}));
        let mut names: Vec<String> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        names.sort_unstable();
        names
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn seeded() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let device = DeviceId::from_uuid(Uuid::from_u128(DEVICE));
    let p = Patina::new(Store::new(dir.path().to_path_buf(), device));
    let now = jiff::ZonedDateTime::now();
    p.run(command::Command::Init, now.clone(), Source::Cli)
        .unwrap();
    p.run(item("secret plan", "work"), now.clone(), Source::Cli)
        .unwrap();
    p.run(item("buy milk", "life"), now, Source::Cli).unwrap();
    dir
}

#[test]
fn lists_tools_hides_work_items_and_takes_proposals() {
    let dir = seeded();
    let mut s = Server::start(&dir);
    assert_eq!(
        s.tool_names(),
        [
            "capture",
            "frontier",
            "graph",
            "habits",
            "identities",
            "next",
            "proposals",
            "propose",
            "review",
            "show",
            "today",
            "tree"
        ]
    );
    let today = s.call("today", json!({"all": true}));
    let text = today["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("buy milk"), "{text}");
    assert!(!text.contains("secret plan"), "{text}");

    let proposed = s.call(
        "propose",
        json!({"summary": "add a step", "ops": [
            {"op": "add_item", "key": "a", "item": {"title": "sip", "parent": "buy milk"}}
        ]}),
    );
    assert_ne!(proposed["result"]["isError"], json!(true), "{proposed}");
    let listed = s.call("proposals", json!({}));
    let text = listed["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("add a step"), "{text}");

    let denied = s.call("run", json!({"command": {"command": "undo"}}));
    assert!(denied["error"].is_object(), "{denied}");
}

#[test]
fn read_only_level_lists_only_the_read_tools() {
    let dir = seeded();
    std::fs::write(
        dir.path().join("config.toml"),
        "[mcp]\ndefault_level = \"read_only\"\n",
    )
    .unwrap();
    let mut s = Server::start(&dir);
    assert_eq!(
        s.tool_names(),
        [
            "frontier",
            "graph",
            "habits",
            "identities",
            "next",
            "review",
            "show",
            "today",
            "tree"
        ]
    );
}
