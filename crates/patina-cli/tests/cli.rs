use std::process::{Command, Output};

const DEVICE: &str = "019ab3a1-0000-7000-8000-000000000001";
const NOW: &str = "2026-11-24T09:00:00+08:00[Asia/Shanghai]";

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn run_as(&self, actor: Option<&str>, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_patina"));
        cmd.args(args)
            .env("PATINA_DIR", self.dir.path())
            .env("PATINA_DEVICE_ID", DEVICE)
            .env("PATINA_NOW", NOW)
            .env_remove("PATINA_ACTOR");
        if let Some(a) = actor {
            cmd.env("PATINA_ACTOR", a);
        }
        cmd.output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run_as(None, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn status(&self) -> String {
        self.ok(&["status"])
    }
}

#[test]
fn capture_triage_complete_updates_status() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&["add", "buy", "milk"]);
    assert_eq!(s.status(), "inbox:1 habits:0 due:0 follow-ups:0\n");
    s.ok(&["triage", "buy milk", "-i", "life", "--deadline", "today"]);
    assert_eq!(s.status(), "inbox:0 habits:0 due:1 follow-ups:0\n");
    s.ok(&["done", "buy milk"]);
    assert_eq!(s.status(), "inbox:0 habits:0 due:0 follow-ups:0\n");
}

#[test]
fn habits_check_in_and_backfill_window() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&["habit", "add", "read", "-i", "study"]);
    assert!(s.status().contains("habits:1"));
    s.ok(&["habit", "check", "read"]);
    assert!(s.status().contains("habits:0"));
    let out = s.run_as(None, &["habit", "check", "read", "--day", "2026-11-20"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("backfill"));
}

#[test]
fn agent_actor_is_capture_only() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    let out = s.run_as(Some("agent:test"), &["new", "x", "-i", "work"]);
    assert_eq!(out.status.code(), Some(1));
    let out = s.run_as(Some("agent:test"), &["add", "idea"]);
    assert!(out.status.success());
    let mut found = false;
    for entry in std::fs::read_dir(s.dir.path().join("events")).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        found |= text
            .lines()
            .any(|l| l.contains("\"source\":\"agent:test\""));
    }
    assert!(found);
}

#[test]
fn undo_removes_the_capture() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&["add", "one"]);
    s.ok(&["add", "two"]);
    assert!(s.status().starts_with("inbox:2"));
    s.ok(&["undo"]);
    assert!(s.status().starts_with("inbox:1"));
}
