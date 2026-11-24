//! Process environment: data root, device id, clock, actor, date arguments.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use jiff::civil::{Date, DateTime, Time};
use jiff::{Timestamp, ZonedDateTime};
use patina_core::command::Update;
use patina_core::event::Source;
use patina_core::ids::DeviceId;
use uuid::Uuid;

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

fn device_id() -> Result<DeviceId> {
    if let Some(v) = var("PATINA_DEVICE_ID") {
        let id = Uuid::parse_str(v.trim()).context("PATINA_DEVICE_ID is not a UUID")?;
        return Ok(DeviceId::from_uuid(id));
    }
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("cannot determine a state directory for the device id")?;
    let path = base.join("patina").join("device-id");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let id = Uuid::parse_str(text.trim())
                .with_context(|| format!("{} does not contain a UUID", path.display()))?;
            Ok(DeviceId::from_uuid(id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = Uuid::now_v7();
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("cannot create {}", dir.display()))?;
            }
            std::fs::write(&path, format!("{}\n", id.hyphenated()))
                .with_context(|| format!("cannot write {}", path.display()))?;
            Ok(DeviceId::from_uuid(id))
        }
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

impl Env {
    pub fn load() -> Result<Self> {
        let root = match var("PATINA_DIR") {
            Some(d) => PathBuf::from(d),
            None => dirs::data_dir()
                .context("cannot determine a data directory")?
                .join("patina"),
        };
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
            device: device_id()?,
            now,
            source,
        })
    }
}

/// Parses `YYYY-MM-DD`, `today`, `tomorrow`, or `+N` relative to `today`.
pub fn parse_date(s: &str, today: Date) -> Result<Date> {
    let s = s.trim();
    match s {
        "today" => Ok(today),
        "tomorrow" => Ok(today.tomorrow()?),
        _ => {
            if let Some(n) = s.strip_prefix('+') {
                let n: i64 = n.parse().with_context(|| format!("invalid date {s:?}"))?;
                Ok(today.checked_add(jiff::Span::new().days(n))?)
            } else {
                s.parse().with_context(|| {
                    format!("invalid date {s:?} (use YYYY-MM-DD, today, tomorrow, or +N)")
                })
            }
        }
    }
}

/// Like [`parse_date`] but `none` clears the field.
pub fn parse_update(s: &str, today: Date) -> Result<Update<Date>> {
    if s.trim() == "none" {
        Ok(Update::Clear)
    } else {
        Ok(Update::Set(parse_date(s, today)?))
    }
}

/// Parses a past wall-clock time in `now`'s zone.
///
/// `HH:MM` is that time today, or yesterday when today's would be in the future;
/// `YYYY-MM-DDTHH:MM` and `YYYY-MM-DD HH:MM` are explicit.
pub fn parse_time(s: &str, now: &ZonedDateTime) -> Result<Timestamp> {
    let s = s.trim();
    let invalid = || anyhow::anyhow!("invalid time {s:?} (use HH:MM or YYYY-MM-DD HH:MM)");
    let tz = now.time_zone().clone();
    if let Ok(time) = Time::strptime("%H:%M", s) {
        let at = now.date().to_datetime(time).to_zoned_datetime(tz)?;
        if at.timestamp() > now.timestamp() {
            return Ok(at.checked_sub(jiff::Span::new().days(1))?.timestamp());
        }
        return Ok(at.timestamp());
    }
    let dt = DateTime::strptime("%Y-%m-%dT%H:%M", s)
        .or_else(|_| DateTime::strptime("%Y-%m-%d %H:%M", s))
        .map_err(|_| invalid())?;
    Ok(dt.to_zoned_datetime(tz)?.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_of_day_rolls_back_past_now() {
        let now: ZonedDateTime = "2026-11-24T09:00:00+08:00[Asia/Shanghai]".parse().unwrap();
        let at = |s: &str| parse_time(s, &now).unwrap().to_string();
        assert_eq!(at("08:30"), "2026-11-24T00:30:00Z");
        assert_eq!(at("23:50"), "2026-11-23T15:50:00Z");
        assert_eq!(at("2026-11-20 10:00"), "2026-11-20T02:00:00Z");
        assert_eq!(at("2026-11-20T10:00"), "2026-11-20T02:00:00Z");
        assert!(parse_time("noon", &now).is_err());
    }
}
