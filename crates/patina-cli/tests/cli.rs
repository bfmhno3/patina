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
        self.run_at(actor, NOW, args)
    }

    fn run_at(&self, actor: Option<&str>, now: &str, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_patina"));
        cmd.args(args)
            .env("PATINA_DIR", self.dir.path())
            .env("PATINA_DEVICE_ID", DEVICE)
            .env("PATINA_NOW", now)
            .env_remove("PATINA_ACTOR");
        if let Some(a) = actor {
            cmd.env("PATINA_ACTOR", a);
        }
        cmd.output().unwrap()
    }

    fn ok_at(&self, now: &str, args: &[&str]) -> String {
        let out = self.run_at(None, now, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
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
    assert_eq!(
        s.status(),
        "mode:away inbox:1 habits:0 due:0 follow-ups:0\n"
    );
    s.ok(&["triage", "buy milk", "-i", "life", "--deadline", "today"]);
    assert_eq!(
        s.status(),
        "mode:away inbox:0 habits:0 due:1 follow-ups:0\n"
    );
    s.ok(&["done", "buy milk"]);
    assert_eq!(
        s.status(),
        "mode:away inbox:0 habits:0 due:0 follow-ups:0\n"
    );
}

#[test]
fn mode_scopes_today_and_flags_a_forgotten_switch() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&["new", "ship report", "-i", "work"]);
    s.ok(&["new", "pay rent", "-i", "life", "--deadline", "today"]);
    s.ok(&["mode", "work"]);
    assert!(s.status().starts_with("mode:work inbox:0"));
    let today = s.ok(&["today"]);
    assert!(today.starts_with("[work]\n"), "{today}");
    assert!(today.contains("Elsewhere"), "{today}");
    assert!(today.contains("pay rent"));
    assert!(today.contains("ship report"));
    let all = s.ok(&["today", "--all"]);
    assert!(!all.contains("Elsewhere"), "{all}");
    // Three hours later the mode counts as forgotten.
    let later = "2026-11-24T12:00:00+08:00[Asia/Shanghai]";
    let status = s.ok_at(later, &["status"]);
    assert!(status.starts_with("mode:work? "), "{status}");
    let today = s.ok_at(later, &["today"]);
    assert!(
        today.contains("hint: no activity since 09:00; 'work' mode may have been left on."),
        "{today}"
    );
    s.ok_at(later, &["mode", "away", "--at", "09:30"]);
    assert_eq!(s.ok_at(later, &["mode"]), "mode: away since 09:30\n");
}

#[test]
fn triage_takes_the_mode_at_capture_time() {
    let s = Sandbox::new();
    let at = |h: &str| format!("2026-11-24T{h}:00+08:00[Asia/Shanghai]");
    s.ok_at(&at("08:00"), &["init"]);
    s.ok_at(&at("08:10"), &["mode", "study"]);
    s.ok_at(&at("08:20"), &["add", "read chapter"]);
    s.ok_at(&at("08:30"), &["mode", "work"]);
    s.ok_at(&at("08:40"), &["triage", "read chapter"]);
    assert!(
        s.ok_at(&at("08:50"), &["show", "read chapter"])
            .contains("identity: study")
    );
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
    assert!(s.status().contains(" inbox:2 "));
    s.ok(&["undo"]);
    assert!(s.status().contains(" inbox:1 "));
}
