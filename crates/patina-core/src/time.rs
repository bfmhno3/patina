//! Logical day, week, and settings.

use serde::{Deserialize, Serialize};

use crate::error::{SettingsError, TimeError};
use crate::event::Size;
use crate::visibility::McpSettings;

/// How many days back a habit may be backfilled (and skipped).
pub const BACKFILL_DAYS: i64 = 2;

/// Day of the week (`jiff::civil::Weekday` has no serde impl).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Weekday {
    const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];

    /// Converts from jiff's weekday.
    pub fn from_jiff(w: jiff::civil::Weekday) -> Self {
        Self::ALL[usize::from(w.to_monday_zero_offset().unsigned_abs()) % 7]
    }

    /// Converts to jiff's weekday.
    pub fn to_jiff(self) -> jiff::civil::Weekday {
        jiff::civil::Weekday::from_monday_zero_offset(self.offset() as i8)
            .unwrap_or(jiff::civil::Weekday::Monday)
    }

    fn offset(self) -> i64 {
        Self::ALL.iter().position(|w| *w == self).unwrap_or(0) as i64
    }
}

/// Minutes each size stands for (`[size_minutes]`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SizeMinutes {
    /// Minutes for S.
    pub s: u32,
    /// Minutes for M.
    pub m: u32,
    /// Minutes for L.
    pub l: u32,
}

impl Default for SizeMinutes {
    fn default() -> Self {
        Self {
            s: 15,
            m: 30,
            l: 60,
        }
    }
}

impl SizeMinutes {
    /// Minutes configured for `size`.
    pub fn minutes(&self, size: Size) -> u32 {
        match size {
            Size::S => self.s,
            Size::M => self.m,
            Size::L => self.l,
        }
    }
}

/// Scoring weights of the recommendation (`[recommend]`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Weights {
    /// Due today or overdue (decays with days left).
    pub deadline: f64,
    /// Planned for today or earlier.
    pub scheduled: f64,
    /// Per item it would unlock.
    pub unlock: f64,
    /// Per transitive dependent.
    pub downstream: f64,
    /// In the weekly focus.
    pub focus: f64,
    /// Marked important.
    pub important: f64,
    /// Size S bonus.
    pub size_s: f64,
    /// Size M bonus.
    pub size_m: f64,
    /// Per recent skip (negative).
    pub skip: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            deadline: 6.0,
            scheduled: 2.0,
            unlock: 1.0,
            downstream: 0.25,
            focus: 2.0,
            important: 3.0,
            size_s: 1.5,
            size_m: 0.5,
            skip: -4.0,
        }
    }
}

/// Break reminder parameters (`[rhythm]`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RhythmSettings {
    /// Minutes of focus before a fixed-mode break.
    pub focus_minutes: u32,
    /// Short break length.
    pub short_break_minutes: u32,
    /// Long break length.
    pub long_break_minutes: u32,
    /// Focus blocks before a long break.
    pub rounds_before_long: u32,
    /// Flowtime: focus minutes per break minute.
    pub flow_ratio: u32,
    /// Minutes a snooze postpones the reminder.
    pub snooze_minutes: u32,
}

impl Default for RhythmSettings {
    fn default() -> Self {
        Self {
            focus_minutes: 25,
            short_break_minutes: 5,
            long_break_minutes: 15,
            rounds_before_long: 4,
            flow_ratio: 5,
            snooze_minutes: 10,
        }
    }
}

/// Identity-switch suggestion parameters (`[suggest]`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuggestSettings {
    /// Whether suggestions are made at all.
    pub enabled: bool,
    /// How many past days to look at.
    pub lookback_days: u32,
    /// Minimum matching days.
    pub min_days: u32,
    /// Minimum share of sampled days that must match.
    pub share: f64,
    /// Minutes a dismissal or a manual switch silences the suggestion.
    pub snooze_minutes: u32,
}

impl Default for SuggestSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            lookback_days: 28,
            min_days: 5,
            share: 0.6,
            snooze_minutes: 120,
        }
    }
}

/// User settings from `<data root>/config.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Hour (local) at which the logical day starts.
    pub day_start_hour: u8,
    /// First day of the week.
    pub week_start: Weekday,
    /// Habit strength half-life in days.
    pub habit_half_life_days: f64,
    /// Minutes without activity after which identity mode counts as away.
    pub away_after_minutes: u32,
    /// Strength from which a habit counts as maintained.
    pub habit_maintain_strength: f64,
    /// Strength a habit must hold to be ready to graduate.
    pub habit_graduate_strength: f64,
    /// Days the graduation strength must hold.
    pub habit_graduate_days: u32,
    /// Local hour from which other identities' unfinished habits enter the banner.
    pub evening_hour: u8,
    /// Minutes per size.
    pub size_minutes: SizeMinutes,
    /// Day of the weekly review.
    pub review_day: Weekday,
    /// Postponements after which the review asks for a decision.
    pub postpone_threshold: u32,
    /// Days an external item waits before the review flags it.
    pub waiting_alert_days: u32,
    /// Inbox size from which triage is recommended.
    pub inbox_threshold: usize,
    /// Hours a skipped recommendation stays penalized.
    pub skip_cooldown_hours: u32,
    /// Recommendation weights.
    pub recommend: Weights,
    /// Break reminders.
    pub rhythm: RhythmSettings,
    /// What MCP clients may read.
    pub mcp: McpSettings,
    /// Identity-switch suggestions.
    pub suggest: SuggestSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            day_start_hour: 4,
            week_start: Weekday::Mon,
            habit_half_life_days: 14.0,
            away_after_minutes: 120,
            habit_maintain_strength: 0.8,
            habit_graduate_strength: 0.9,
            habit_graduate_days: 28,
            evening_hour: 20,
            size_minutes: SizeMinutes::default(),
            review_day: Weekday::Sun,
            postpone_threshold: 3,
            waiting_alert_days: 7,
            inbox_threshold: 10,
            skip_cooldown_hours: 4,
            recommend: Weights::default(),
            rhythm: RhythmSettings::default(),
            mcp: McpSettings::default(),
            suggest: SuggestSettings::default(),
        }
    }
}

impl Settings {
    /// Checks value ranges.
    ///
    /// # Errors
    /// Returns [`SettingsError`] for an out-of-range hour or a non-positive half-life.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.day_start_hour > 23 {
            return Err(SettingsError::DayStartHour(self.day_start_hour));
        }
        if !(self.habit_half_life_days.is_finite() && self.habit_half_life_days > 0.0) {
            return Err(SettingsError::HalfLife(self.habit_half_life_days));
        }
        if self.evening_hour > 23 {
            return Err(SettingsError::EveningHour(self.evening_hour));
        }
        for (name, v) in [
            ("habit_maintain_strength", self.habit_maintain_strength),
            ("habit_graduate_strength", self.habit_graduate_strength),
        ] {
            if !(v.is_finite() && v > 0.0 && v <= 1.0) {
                return Err(SettingsError::HabitStage(name));
            }
        }
        if self.habit_maintain_strength > self.habit_graduate_strength {
            return Err(SettingsError::HabitStage("habit_maintain_strength"));
        }
        if !(1..=365).contains(&self.habit_graduate_days) {
            return Err(SettingsError::HabitStage("habit_graduate_days"));
        }
        let r = &self.rhythm;
        for (name, value) in [
            ("focus_minutes", r.focus_minutes),
            ("short_break_minutes", r.short_break_minutes),
            ("long_break_minutes", r.long_break_minutes),
            ("rounds_before_long", r.rounds_before_long),
            ("flow_ratio", r.flow_ratio),
            ("snooze_minutes", r.snooze_minutes),
        ] {
            if value == 0 {
                return Err(SettingsError::Rhythm(name));
            }
        }
        let s = &self.suggest;
        if !(1..=366).contains(&s.lookback_days) {
            return Err(SettingsError::Suggest("lookback_days"));
        }
        if s.min_days == 0 {
            return Err(SettingsError::Suggest("min_days"));
        }
        if !(s.share.is_finite() && s.share > 0.0 && s.share <= 1.0) {
            return Err(SettingsError::Suggest("share"));
        }
        if s.snooze_minutes == 0 {
            return Err(SettingsError::Suggest("snooze_minutes"));
        }
        Ok(())
    }
}

/// Parses `YYYY-MM-DD`, `today`, `tomorrow`, or `+N` relative to `today`.
pub fn parse_date(s: &str, today: jiff::civil::Date) -> Option<jiff::civil::Date> {
    let s = s.trim();
    match s {
        "today" => Some(today),
        "tomorrow" => today.tomorrow().ok(),
        _ => match s.strip_prefix('+') {
            Some(n) => today
                .checked_add(jiff::Span::new().days(n.parse::<i64>().ok()?))
                .ok(),
            None => s.parse().ok(),
        },
    }
}

/// Parses a past wall-clock time in `now`'s zone.
///
/// `HH:MM` is that time today, or yesterday when today's would be in the future;
/// `YYYY-MM-DDTHH:MM` and `YYYY-MM-DD HH:MM` are explicit.
pub fn parse_time(s: &str, now: &jiff::ZonedDateTime) -> Option<jiff::Timestamp> {
    let s = s.trim();
    let tz = now.time_zone().clone();
    if let Ok(time) = jiff::civil::Time::strptime("%H:%M", s) {
        let at = now.date().to_datetime(time).to_zoned_datetime(tz).ok()?;
        if at.timestamp() > now.timestamp() {
            return Some(at.checked_sub(jiff::Span::new().days(1)).ok()?.timestamp());
        }
        return Some(at.timestamp());
    }
    let dt = jiff::civil::DateTime::strptime("%Y-%m-%dT%H:%M", s)
        .or_else(|_| jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M", s))
        .ok()?;
    Some(dt.to_zoned_datetime(tz).ok()?.timestamp())
}

/// Parses a weekly target in hours: `none` clears it, else `0 < h <= 168`.
///
/// Returns `None` for invalid input, `Some(None)` for no target.
pub fn parse_hours(s: &str) -> Option<Option<u32>> {
    let s = s.trim();
    if s == "none" {
        return Some(None);
    }
    let h: f64 = s.parse().ok()?;
    (h.is_finite() && h > 0.0 && h <= 168.0).then(|| Some((h * 60.0).round() as u32))
}

/// The logical day containing `now` (the day starts at `day_start_hour`).
///
/// # Errors
/// Returns [`TimeError`] on date arithmetic overflow.
pub fn logical_day(
    now: &jiff::ZonedDateTime,
    settings: &Settings,
) -> Result<jiff::civil::Date, TimeError> {
    let shifted = now.checked_sub(jiff::SignedDuration::from_hours(i64::from(
        settings.day_start_hour,
    )))?;
    Ok(shifted.date())
}

/// First day of the week containing `day`.
///
/// # Errors
/// Returns [`TimeError`] on date arithmetic overflow.
pub fn week_start_of(
    day: jiff::civil::Date,
    settings: &Settings,
) -> Result<jiff::civil::Date, TimeError> {
    let offset = (i64::from(day.weekday().to_monday_zero_offset()) - settings.week_start.offset())
        .rem_euclid(7);
    Ok(day.checked_sub(jiff::Span::new().days(offset))?)
}

/// The latest day on or before `day` that falls on weekday `w`.
///
/// # Errors
/// Returns [`TimeError`] on date arithmetic overflow.
pub fn last_on_or_before(
    day: jiff::civil::Date,
    w: Weekday,
) -> Result<jiff::civil::Date, TimeError> {
    let back = (i64::from(day.weekday().to_monday_zero_offset()) - w.offset()).rem_euclid(7);
    Ok(day.checked_sub(jiff::Span::new().days(back))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_day_before_cutoff_is_previous_day() {
        let s = Settings::default();
        let now: jiff::ZonedDateTime = "2026-11-24T03:30:00+08:00[Asia/Shanghai]".parse().unwrap();
        assert_eq!(logical_day(&now, &s).unwrap().to_string(), "2026-11-23");
        let now: jiff::ZonedDateTime = "2026-11-24T04:00:00+08:00[Asia/Shanghai]".parse().unwrap();
        assert_eq!(logical_day(&now, &s).unwrap().to_string(), "2026-11-24");
    }

    #[test]
    fn time_of_day_rolls_back_past_now() {
        let now: jiff::ZonedDateTime = "2026-11-24T09:00:00+08:00[Asia/Shanghai]".parse().unwrap();
        let at = |s: &str| parse_time(s, &now).unwrap().to_string();
        assert_eq!(at("08:30"), "2026-11-24T00:30:00Z");
        assert_eq!(at("23:50"), "2026-11-23T15:50:00Z");
        assert_eq!(at("2026-11-20 10:00"), "2026-11-20T02:00:00Z");
        assert_eq!(at("2026-11-20T10:00"), "2026-11-20T02:00:00Z");
        assert!(parse_time("noon", &now).is_none());
    }

    #[test]
    fn hours_parse_to_minutes() {
        assert_eq!(parse_hours("7.5"), Some(Some(450)));
        assert_eq!(parse_hours("none"), Some(None));
        assert_eq!(parse_hours("0"), None);
        assert_eq!(parse_hours("200"), None);
        assert_eq!(parse_hours("x"), None);
    }

    #[test]
    fn week_start_is_monday() {
        let s = Settings::default();
        let d = jiff::civil::date(2026, 11, 29); // Sunday
        assert_eq!(week_start_of(d, &s).unwrap().to_string(), "2026-11-23");
    }
}
