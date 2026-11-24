//! Process environment: data root, device id, clock, actor, date arguments.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use jiff::ZonedDateTime;
use jiff::civil::Date;
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
