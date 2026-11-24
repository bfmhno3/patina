//! Identity mode: which identity is active, derived from switch events and idleness. Pure.

use jiff::{SignedDuration, Timestamp};

use crate::ids::IdentityId;
use crate::state::State;
use crate::time::Settings;

/// The identity in effect at time `t` (the last switch at or before `t`); `None` is away.
pub fn mode_at(state: &State, t: Timestamp) -> Option<IdentityId> {
    state
        .modes
        .iter()
        .rev()
        .find(|m| m.at <= t)
        .and_then(|m| m.identity)
}

/// The mode right now, including idleness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Current {
    /// Explicitly chosen identity; `None` is away.
    pub identity: Option<IdentityId>,
    /// When the explicit mode was set.
    pub since: Option<Timestamp>,
    /// No activity for longer than `away_after_minutes`.
    pub idle: bool,
    /// Idle with a mode set and nothing timing: the mode is treated as away.
    pub auto_away: bool,
    /// Latest recorded activity.
    pub last_activity: Option<Timestamp>,
}

impl Current {
    /// The identity used to scope views: away when `auto_away`.
    pub fn effective(&self) -> Option<IdentityId> {
        if self.auto_away { None } else { self.identity }
    }
}

/// Computes the current mode at `now`.
pub fn current(state: &State, now: Timestamp, settings: &Settings) -> Current {
    let last = state.modes.last();
    let identity = last.and_then(|m| m.identity);
    let last_activity = state.activity.iter().max().copied();
    let limit = SignedDuration::from_mins(i64::from(settings.away_after_minutes));
    let idle = last_activity.is_some_and(|a| now.duration_since(a) > limit);
    Current {
        identity,
        since: last.map(|m| m.at),
        idle,
        auto_away: idle && identity.is_some(),
        last_activity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::testutil::TestLog;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn switch(log: &mut TestLog, identity: Option<IdentityId>, at: &str) {
        log.push_at(
            EventData::ModeSwitched(ModeSwitched { identity }),
            ts(at),
            ts(at),
        );
    }

    #[test]
    fn backdated_switch_lands_in_effective_order() {
        let mut log = TestLog::new();
        let work = log.identity("work");
        let life = log.identity("life");
        switch(&mut log, Some(work), "2026-11-24T01:00:00Z");
        switch(&mut log, Some(life), "2026-11-24T05:00:00Z");
        // Recorded last, effective between the two: "I switched to away at 03:00".
        log.push_at(
            EventData::ModeSwitched(ModeSwitched { identity: None }),
            ts("2026-11-24T03:00:00Z"),
            ts("2026-11-24T06:00:00Z"),
        );
        let s = log.state();
        assert_eq!(mode_at(&s, ts("2026-11-24T02:00:00Z")), Some(work));
        assert_eq!(mode_at(&s, ts("2026-11-24T04:00:00Z")), None);
        assert_eq!(mode_at(&s, ts("2026-11-24T05:30:00Z")), Some(life));
        assert_eq!(mode_at(&s, ts("2026-11-24T00:30:00Z")), None);
    }

    #[test]
    fn auto_away_after_the_idle_limit_only() {
        let mut log = TestLog::new();
        let work = log.identity("work");
        log.push(EventData::ModeSwitched(ModeSwitched {
            identity: Some(work),
        }));
        let s = log.state();
        let settings = Settings::default();
        let last = s.activity.iter().max().copied().unwrap();
        let at = |mins: i64| current(&s, last + SignedDuration::from_mins(mins), &settings);
        let before = at(119);
        assert!(!before.idle && !before.auto_away);
        assert_eq!(before.effective(), Some(work));
        let after = at(121);
        assert!(after.idle && after.auto_away);
        assert_eq!(after.effective(), None);
        assert_eq!(after.identity, Some(work));
    }
}
