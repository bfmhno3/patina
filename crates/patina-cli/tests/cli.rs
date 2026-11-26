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

#[test]
fn next_card_start_then_forgotten_timer_then_review() {
    let s = Sandbox::new();
    let at = |day: &str, hm: &str| format!("2026-11-{day}T{hm}:00+08:00[Asia/Shanghai]");
    s.ok(&["init"]);
    s.ok(&["mode", "work"]);
    s.ok(&["new", "write parser", "-i", "work", "--size", "l"]);
    s.ok(&[
        "new",
        "reply mail",
        "-i",
        "work",
        "--size",
        "s",
        "--deadline",
        "tomorrow",
    ]);
    let card = s.ok(&["next"]);
    assert!(
        card.contains("next: ") && card.contains(" reply mail\n"),
        "{card}"
    );
    assert!(card.contains("  why: due in 1 day; small (S)\n"), "{card}");
    assert!(
        card.contains("patina next start | skip: patina next skip"),
        "{card}"
    );
    let started = s.ok(&["next", "start"]);
    assert!(started.starts_with("started timer on "), "{started}");
    let status = s.status();
    assert!(
        status.starts_with("mode:work timer:") && status.contains("+0m inbox:0"),
        "{status}"
    );
    let today = s.ok_at(&at("24", "23:50"), &["today"]);
    assert!(today.contains("hint: timer on "), "{today}");
    let stopped = s.ok_at(&at("25", "08:00"), &["stop", "--at", "09:40"]);
    assert!(stopped.contains("reply mail after 40m"), "{stopped}");
    let review = s.ok_at(&at("25", "08:01"), &["review"]);
    assert!(review.starts_with("Since the beginning\n"), "{review}");
    assert!(
        review.contains("finish with: patina review done"),
        "{review}"
    );
    // Proposal-level sources read the card without recording it.
    let out = s.run_as(Some("agent:test"), &["next"]);
    assert!(out.status.success());
}

#[test]
fn stuck_confirms_and_shows_the_next_card() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&["new", "draft", "-i", "work"]);
    s.ok(&["new", "email", "-i", "work", "--important"]);
    s.ok(&["start", "draft"]);
    let out = s.ok(&["stuck", "mentor", "feedback"]);
    assert!(out.contains("stopped "), "{out}");
    assert!(
        out.contains("waiting on ") && out.contains(" mentor feedback\n"),
        "{out}"
    );
    assert!(out.contains(" email\n  why: important\n"), "{out}");
}

#[test]
fn identity_target_shows_in_list() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    assert_eq!(
        s.ok(&["identity", "target", "work", "7.5"]),
        "target work: 7.5h/week\n"
    );
    assert!(
        s.ok(&["identity", "list"])
            .contains("work  0 open, 0 habits  target 7.5h/week")
    );
    let out = s.run_as(None, &["identity", "target", "work", "x"]);
    assert!(!out.status.success());
}

#[test]
fn minimal_review_is_recorded_in_the_log() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&["review", "done", "--minimal"]);
    let events = std::fs::read_dir(s.dir.path().join("events")).unwrap();
    let text: String = events
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    let line = text
        .lines()
        .find(|l| l.contains("review.completed"))
        .unwrap();
    assert!(line.contains("\"minimal\":true"), "{line}");
}

#[test]
fn graph_lists_siblings_by_layer_with_prerequisites() {
    let s = Sandbox::new();
    s.ok(&["init"]);
    s.ok(&["new", "P", "-i", "work"]);
    s.ok(&["new", "a", "-p", "P"]);
    s.ok(&["new", "b", "-p", "P"]);
    s.ok(&["link", "b", "--on", "a"]);
    let out = s.ok(&["graph", "P"]);
    let l0 = out.find("layer 0").expect(&out);
    let a = out.find("  a\n").expect(&out);
    let l1 = out.find("layer 1").expect(&out);
    let b = out.find("  b  <- ").expect(&out);
    assert!(l0 < a && a < l1 && l1 < b, "{out}");
}
