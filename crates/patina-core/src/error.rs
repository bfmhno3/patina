//! Errors shared across modules.
//!
//! Every error enum serializes adjacently tagged:
//! `{"kind":"rejected","detail":{"kind":"cycle","detail":{"path":[...]}}}`.

use serde::{Serialize, Serializer};

/// Serializes a foreign error as its `Display` string.
pub(crate) fn ser_display<T: std::fmt::Display, S: Serializer>(
    value: &T,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.collect_str(value)
}

/// Date arithmetic failure.
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum TimeError {
    /// Underlying jiff error.
    #[error("date arithmetic failed: {0}")]
    Jiff(
        #[from]
        #[serde(serialize_with = "ser_display")]
        jiff::Error,
    ),
}

/// Invalid settings values.
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum SettingsError {
    /// `day_start_hour` above 23.
    #[error("day_start_hour must be 0..=23, got {0}")]
    DayStartHour(u8),
    /// `habit_half_life_days` not a positive finite number.
    #[error("habit_half_life_days must be a positive number, got {0}")]
    HalfLife(f64),
    /// `evening_hour` above 23.
    #[error("evening_hour must be 0..=23, got {0}")]
    EveningHour(u8),
    /// A `[rhythm]` value is zero.
    #[error("rhythm.{0} must be positive")]
    Rhythm(&'static str),
    /// A habit stage threshold is out of range.
    #[error("{0} is out of range")]
    HabitStage(&'static str),
    /// A `[suggest]` value is out of range.
    #[error("suggest.{0} is out of range")]
    Suggest(&'static str),
}
