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

#[test]
fn forgotten_timer_is_flagged_and_stop_at_corrects_it() {
    let s = Sandbox::new();
    let at = |day: &str, hm: &str| format!("2026-11-{day}T{hm}:00+08:00[Asia/Shanghai]");
    s.ok_at(&at("24", "08:50"), &["init"]);
    s.ok_at(&at("24", "08:51"), &["new", "write parser", "-i", "work"]);
    s.ok_at(&at("24", "08:52"), &["mode", "work"]);
    let started = s.ok_at(&at("24", "09:00"), &["start", "write parser"]);
    assert!(started.starts_with("started timer on "), "{started}");
    let status = s.ok_at(&at("24", "09:30"), &["status"]);
    assert!(
        status.starts_with("mode:work timer:") && status.contains("+30m inbox:0"),
        "{status}"
    );
    let today = s.ok_at(&at("24", "23:50"), &["today"]);
    assert!(
        today.contains("started 09:00, no activity since 09:00; if you stopped earlier: 'patina stop --at 09:00'"),
        "{today}"
    );
    let stopped = s.ok_at(&at("25", "08:00"), &["stop", "--at", "09:40"]);
    assert!(
        stopped.contains("write parser after 40m (total 40m)"),
        "{stopped}"
    );
    assert!(
        s.ok_at(&at("25", "08:01"), &["status"])
            .starts_with("mode:work inbox:0")
    );
}

#[test]
fn recurring_chore_and_over_estimate_hint() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&[
        "new",
        "pay rent",
        "-i",
        "life",
        "--every",
        "1m",
        "--deadline",
        "2026-11-30",
        "--size",
        "s",
        "--important",
    ]);
    let list = s.ok(&["list"]);
    assert!(
        list.contains("pay rent") && list.contains(" ! [S] every 1m"),
        "{list}"
    );
    s.ok(&["start", "pay rent", "--at", "08:10"]);
    let done = s.ok(&["done", "pay rent"]);
    assert!(done.contains("over twice the S estimate (15m)"), "{done}");
    assert!(
        done.contains("next: ") && done.contains(" pay rent"),
        "{done}"
    );
    assert!(s.ok(&["list"]).contains("due 2026-12-30"));
    let bad = s.run_as(None, &["new", "x", "-i", "work", "--every", "0d"]);
    assert!(!bad.status.success());
}

#[test]
fn weekly_review_shows_sections_and_records_focus() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&["new", "send invoice", "-i", "work"]);
    s.ok(&["new", "plan trip", "-i", "life"]);
    s.ok(&["add", "loose thought"]);
    s.ok(&["done", "send invoice"]);
    let r = s.ok(&["review"]);
    assert!(
        r.starts_with("Since the beginning\nDone\n  [work]\n"),
        "{r}"
    );
    assert!(r.contains("send invoice"), "{r}");
    assert!(r.contains("Inbox\n") && r.contains("loose thought"), "{r}");
    assert!(
        r.ends_with("finish with: patina review done --conclusion \"...\" [--focus <ref>]...\n"),
        "{r}"
    );
    let done = s.ok(&[
        "review",
        "done",
        "--conclusion",
        "ok week",
        "--focus",
        "plan trip",
    ]);
    assert!(done.starts_with("review recorded\n  focus: "), "{done}");
    let r = s.ok(&["review"]);
    assert!(r.starts_with("Since 2026-11-24\n"), "{r}");
    assert!(
        r.contains("Current focus\n") && r.contains("plan trip"),
        "{r}"
    );
    assert!(!r.contains("Done\n"), "{r}");
}
