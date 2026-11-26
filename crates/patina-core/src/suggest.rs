//! Identity-switch suggestions: what the owner usually does at this time of day. Pure.

use std::collections::BTreeMap;

use jiff::civil::Weekday;
use jiff::{SignedDuration, Span, Timestamp, ZonedDateTime};

use crate::error::TimeError;
use crate::ids::IdentityId;
use crate::mode;
use crate::state::State;
use crate::time::Settings;

/// A derived suggestion to switch identity mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSuggestion {
    /// The identity usually active at this time.
    pub identity: IdentityId,
    /// Sampled days on which it was active.
    pub matches: u32,
    /// Sampled days.
    pub samples: u32,
    /// The samples are weekend days.
    pub weekend: bool,
}

fn is_weekend(t: &ZonedDateTime) -> bool {
    matches!(t.weekday(), Weekday::Saturday | Weekday::Sunday)
}

/// The identity the owner is usually in at this time of day, when it differs from now.
///
/// Looks at the same weekday class (weekdays or weekends) and wall-clock time on each of
/// the last `lookback_days` days since the first recorded mode switch. Nothing is
/// suggested while a timer runs, right after a manual switch, after a dismissal, or when
/// two identities tie.
///
/// # Errors
/// [`TimeError`] on date arithmetic overflow.
pub fn mode_suggestion(
    state: &State,
    now: &ZonedDateTime,
    settings: &Settings,
) -> Result<Option<ModeSuggestion>, TimeError> {
    let cfg = &settings.suggest;
    let (Some(first), Some(last)) = (state.modes.first(), state.modes.last()) else {
        return Ok(None);
    };
    if !cfg.enabled || state.running.is_some() {
        return Ok(None);
    }
    let quiet_since: Timestamp = now
        .timestamp()
        .checked_sub(SignedDuration::from_mins(i64::from(cfg.snooze_minutes)))
        .map_err(TimeError::from)?;
    if last.at > quiet_since {
        return Ok(None);
    }

    let weekend = is_weekend(now);
    let mut samples = 0u32;
    let mut counts: BTreeMap<IdentityId, u32> = BTreeMap::new();
    for k in 1..=i64::from(cfg.lookback_days) {
        let t = now.checked_sub(Span::new().days(k))?;
        if is_weekend(&t) != weekend || t.timestamp() < first.at {
            continue;
        }
        samples += 1;
        if let Some(id) = mode::mode_at(state, t.timestamp()) {
            *counts.entry(id).or_insert(0) += 1;
        }
    }

    let mut ranked: Vec<(IdentityId, u32)> = counts.into_iter().collect();
    ranked.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let Some(&(identity, matches)) = ranked.first() else {
        return Ok(None);
    };
    if ranked.get(1).is_some_and(|(_, n)| *n == matches) {
        return Ok(None);
    }
    let dismissed = state
        .suggestion_dismissals
        .get(&identity)
        .is_some_and(|at| *at >= quiet_since);
    let current = mode::current(state, now.timestamp(), settings).effective();
    let suggest = matches >= cfg.min_days
        && f64::from(matches) / f64::from(samples) >= cfg.share
        && Some(identity) != current
        && state.identities.contains_key(&identity)
        && !dismissed;
    Ok(suggest.then_some(ModeSuggestion {
        identity,
        matches,
        samples,
        weekend,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use crate::testutil::TestLog;

    fn ts(day: &str, hhmm: &str) -> Timestamp {
        format!("{day}T{hhmm}:00+08:00").parse().unwrap()
    }

    fn zoned(day: &str, hhmm: &str) -> ZonedDateTime {
        format!("{day}T{hhmm}:00+08:00[Asia/Shanghai]")
            .parse()
            .unwrap()
    }

    fn switch(log: &mut TestLog, identity: Option<IdentityId>, day: &str, hhmm: &str) {
        let at = ts(day, hhmm);
        log.push_at(EventData::ModeSwitched(ModeSwitched { identity }), at, at);
    }

    /// Work from 09:00 to 18:00 on six of the eight weekdays from 2026-11-12 to 2026-11-23.
    fn routine() -> (TestLog, IdentityId) {
        let mut log = TestLog::new();
        let work = log.identity("work");
        for day in [
            "2026-11-12",
            "2026-11-13",
            "2026-11-16",
            "2026-11-18",
            "2026-11-20",
            "2026-11-23",
        ] {
            switch(&mut log, Some(work), day, "09:00");
            switch(&mut log, None, day, "18:00");
        }
        (log, work)
    }

    #[test]
    fn suggests_the_usual_identity_for_this_time() {
        let (log, work) = routine();
        let s = mode_suggestion(
            &log.state(),
            &zoned("2026-11-24", "09:05"),
            &Settings::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (s.identity, s.matches, s.samples, s.weekend),
            (work, 6, 8, false)
        );
    }

    #[test]
    fn dismissal_silences_it_until_the_snooze_passes() {
        let (mut log, work) = routine();
        let at = ts("2026-11-24", "09:00");
        log.push_at(
            EventData::SuggestionDismissed(SuggestionDismissed { identity: work }),
            at,
            at,
        );
        let settings = Settings::default();
        let st = log.state();
        assert!(
            mode_suggestion(&st, &zoned("2026-11-24", "09:05"), &settings)
                .unwrap()
                .is_none()
        );
        assert!(
            mode_suggestion(&st, &zoned("2026-11-24", "11:05"), &settings)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_recent_manual_switch_suppresses_it() {
        let (mut log, _) = routine();
        switch(&mut log, None, "2026-11-24", "09:00");
        assert!(
            mode_suggestion(
                &log.state(),
                &zoned("2026-11-24", "09:05"),
                &Settings::default()
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn weekends_have_too_few_samples() {
        let (log, _) = routine();
        assert!(
            mode_suggestion(
                &log.state(),
                &zoned("2026-11-28", "09:05"),
                &Settings::default()
            )
            .unwrap()
            .is_none()
        );
    }
}
