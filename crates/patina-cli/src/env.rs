//! Process environment: data root, device id, clock, actor, date arguments.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use jiff::civil::Date;
use jiff::{Timestamp, ZonedDateTime};
use patina_core::command::Update;
use patina_core::event::Source;
use patina_core::ids::DeviceId;
use patina_core::{store, time};

/// Resolved environment for one invocation.
pub struct Env {
    pub root: PathBuf,
    pub device: DeviceId,
    pub now: ZonedDateTime,
    pub source: Source,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Env {
    pub fn load() -> Result<Self> {
        let root = store::default_root()?;
        let now = match var("PATINA_NOW") {
            Some(v) => v
                .parse::<ZonedDateTime>()
                .context("PATINA_NOW is not an RFC 9557 time")?,
            None => ZonedDateTime::now(),
        };
        let source = match var("PATINA_ACTOR") {
            None => Source::Cli,
            Some(v) => match v.strip_prefix("agent:") {
                Some(name) if !name.is_empty() => Source::Agent {
                    name: name.to_string(),
                },
                _ => bail!("PATINA_ACTOR must be agent:<name>"),
            },
        };
        Ok(Self {
            root,
            device: store::device_id()?,
            now,
            source,
        })
    }
}

/// Parses `YYYY-MM-DD`, `today`, `tomorrow`, or `+N` relative to `today`.
pub fn parse_date(s: &str, today: Date) -> Result<Date> {
    time::parse_date(s, today).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid date {:?} (use YYYY-MM-DD, today, tomorrow, or +N)",
            s.trim()
        )
    })
}

/// Like [`parse_date`] but `none` clears the field.
pub fn parse_update(s: &str, today: Date) -> Result<Update<Date>> {
    if s.trim() == "none" {
        Ok(Update::Clear)
    } else {
        Ok(Update::Set(parse_date(s, today)?))
    }
}

/// Parses a past wall-clock time in `now`'s zone (see [`time::parse_time`]).
pub fn parse_time(s: &str, now: &ZonedDateTime) -> Result<Timestamp> {
    time::parse_time(s, now).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid time {:?} (use HH:MM or YYYY-MM-DD HH:MM)",
            s.trim()
        )
    })
}
