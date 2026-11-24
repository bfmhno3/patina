//! Logical day, week, and settings.

use serde::{Deserialize, Serialize};

use crate::error::{SettingsError, TimeError};

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
    /// Local hour from which other identities' unfinished habits enter the banner.
    pub evening_hour: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            day_start_hour: 4,
            week_start: Weekday::Mon,
            habit_half_life_days: 14.0,
            away_after_minutes: 120,
            evening_hour: 20,
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
        Ok(())
    }
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
    fn week_start_is_monday() {
        let s = Settings::default();
        let d = jiff::civil::date(2026, 11, 29); // Sunday
        assert_eq!(week_start_of(d, &s).unwrap().to_string(), "2026-11-23");
    }
}
