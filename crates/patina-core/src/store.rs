//! File storage: one append-only JSONL file per device. The only module doing I/O.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use crate::error::{SettingsError, ser_display};
use crate::event::{Event, UpgradeError};
use crate::ids::DeviceId;
use crate::time::Settings;

/// Storage failure.
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum StoreError {
    /// File system error.
    #[error("{}: {source}", path.display())]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Cause.
        #[serde(serialize_with = "ser_display")]
        source: std::io::Error,
    },
    /// `config.toml` does not parse.
    #[error("{}: {message}", path.display())]
    Config {
        /// Config path.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
    /// Invalid settings values.
    #[error(transparent)]
    Settings(#[from] SettingsError),
    /// No data or state directory can be determined on this platform.
    #[error("cannot determine a data directory")]
    NoDataDir,
    /// A device id file or variable does not hold a UUID.
    #[error("{}: {message}", path.display())]
    DeviceId {
        /// File or variable involved.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
    /// Event serialization failure.
    #[error("cannot serialize event: {0}")]
    Serialize(
        #[from]
        #[serde(serialize_with = "ser_display")]
        serde_json::Error,
    ),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The data root: `PATINA_DIR` when set and non-empty, else `<user data dir>/patina`.
///
/// # Errors
/// Returns [`StoreError::NoDataDir`] when the platform has no data directory.
pub fn default_root() -> Result<PathBuf, StoreError> {
    match std::env::var("PATINA_DIR").ok().filter(|v| !v.is_empty()) {
        Some(d) => Ok(PathBuf::from(d)),
        None => Ok(dirs::data_dir()
            .ok_or(StoreError::NoDataDir)?
            .join("patina")),
    }
}

/// Reads `dir/device-id`, creating it with a fresh UUIDv7 when missing.
///
/// # Errors
/// Fails on I/O errors or when the file does not hold a UUID.
pub fn device_id_in(dir: &Path) -> Result<DeviceId, StoreError> {
    let path = dir.join("device-id");
    match fs::read_to_string(&path) {
        Ok(text) => {
            let id = Uuid::parse_str(text.trim()).map_err(|e| StoreError::DeviceId {
                path: path.clone(),
                message: format!("not a UUID ({e})"),
            })?;
            Ok(DeviceId::from_uuid(id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = Uuid::now_v7();
            fs::create_dir_all(dir).map_err(io(dir))?;
            fs::write(&path, format!("{}\n", id.hyphenated())).map_err(io(&path))?;
            Ok(DeviceId::from_uuid(id))
        }
        Err(e) => Err(io(&path)(e)),
    }
}

/// This device's id: `PATINA_DEVICE_ID`, else the `device-id` file in the state directory.
///
/// # Errors
/// Fails when the variable or file is not a UUID, on I/O errors, or when the platform
/// has no state directory.
pub fn device_id() -> Result<DeviceId, StoreError> {
    if let Some(v) = std::env::var("PATINA_DEVICE_ID")
        .ok()
        .filter(|v| !v.is_empty())
    {
        let id = Uuid::parse_str(v.trim()).map_err(|e| StoreError::DeviceId {
            path: "PATINA_DEVICE_ID".into(),
            message: format!("not a UUID ({e})"),
        })?;
        return Ok(DeviceId::from_uuid(id));
    }
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .ok_or(StoreError::NoDataDir)?;
    device_id_in(&base.join("patina"))
}

/// Handle to a data root for one device.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
    device: DeviceId,
}

impl Store {
    /// Creates a handle; performs no I/O.
    pub fn new(root: PathBuf, device: DeviceId) -> Self {
        Self { root, device }
    }

    /// This device's id.
    pub fn device(&self) -> DeviceId {
        self.device
    }

    fn events_dir(&self) -> PathBuf {
        self.root.join("events")
    }

    /// Reads `<root>/config.toml`; a missing file yields defaults.
    ///
    /// # Errors
    /// Fails on I/O errors, unparsable TOML, or invalid values.
    pub fn load_settings(&self) -> Result<Settings, StoreError> {
        let path = self.root.join("config.toml");
        let settings = match fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<Settings>(&text).map_err(|e| StoreError::Config {
                path: path.clone(),
                message: e.to_string(),
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => return Err(io(&path)(e)),
        };
        settings.validate()?;
        Ok(settings)
    }

    /// Takes the exclusive write lock on this device's file, creating it if needed.
    ///
    /// # Errors
    /// Fails on I/O errors.
    pub fn lock(&self) -> Result<WriteGuard, StoreError> {
        let dir = self.events_dir();
        fs::create_dir_all(&dir).map_err(io(&dir))?;
        let path = dir.join(format!("{}.jsonl", self.device.as_uuid()));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)
            .map_err(io(&path))?;
        file.lock().map_err(io(&path))?;
        Ok(WriteGuard { file, path })
    }

    /// Reads every device file and merges events by `recorded_at`.
    ///
    /// Bad lines become warnings and are skipped.
    ///
    /// # Errors
    /// Fails on I/O errors.
    pub fn load(&self) -> Result<Log, StoreError> {
        let dir = self.events_dir();
        let mut files = Vec::new();
        match fs::read_dir(&dir) {
            Ok(rd) => {
                for entry in rd {
                    let path = entry.map_err(io(&dir))?.path();
                    if path.extension().is_some_and(|e| e == "jsonl") {
                        files.push(path);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(&dir)(e)),
        }
        files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        let mut events = Vec::new();
        let mut warnings = Vec::new();
        for path in files {
            let bytes = fs::read(&path).map_err(io(&path))?;
            let text = String::from_utf8_lossy(&bytes);
            for (n, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                match Event::from_line(line) {
                    Ok(e) => events.push(e),
                    Err(err) => warnings.push(LoadWarning {
                        file: path.clone(),
                        line: n + 1,
                        problem: err.into(),
                    }),
                }
            }
        }
        events.sort_by_key(|e| e.recorded_at);
        Ok(Log { events, warnings })
    }
}

/// Exclusive lock on this device's file; released on drop.
#[derive(Debug)]
pub struct WriteGuard {
    file: File,
    path: PathBuf,
}

impl WriteGuard {
    /// Appends events as lines in one write, then syncs.
    ///
    /// # Errors
    /// Fails on serialization or I/O errors.
    pub fn append(&mut self, events: &[Event]) -> Result<(), StoreError> {
        let mut buf = String::new();
        for e in events {
            buf.push_str(&e.to_line()?);
            buf.push('\n');
        }
        let path = &self.path;
        let len = self.file.metadata().map_err(io(path))?.len();
        if len > 0 {
            self.file.seek(SeekFrom::Start(len - 1)).map_err(io(path))?;
            let mut last = [0u8; 1];
            self.file.read_exact(&mut last).map_err(io(path))?;
            if last[0] != b'\n' {
                buf.insert(0, '\n');
            }
        }
        self.file.write_all(buf.as_bytes()).map_err(io(path))?;
        self.file.sync_data().map_err(io(path))?;
        Ok(())
    }
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// All events plus load problems.
#[derive(Debug)]
pub struct Log {
    /// Events in merged order.
    pub events: Vec<Event>,
    /// Lines that could not be read.
    pub warnings: Vec<LoadWarning>,
}

/// A skipped line.
#[derive(Clone, Debug, Serialize)]
pub struct LoadWarning {
    /// File containing the line.
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// What went wrong.
    pub problem: LoadProblem,
}

/// Why a line was skipped.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum LoadProblem {
    /// Not valid JSON or wrong shape.
    Malformed {
        /// Parser message.
        message: String,
    },
    /// Unknown event type.
    UnknownType {
        /// Type key.
        kind: String,
    },
    /// Newer schema version.
    UnsupportedVersion {
        /// Type key.
        kind: String,
        /// Stored version.
        version: u32,
    },
}

impl From<UpgradeError> for LoadProblem {
    fn from(e: UpgradeError) -> Self {
        match e {
            UpgradeError::Malformed { message } => LoadProblem::Malformed { message },
            UpgradeError::UnknownType { kind } => LoadProblem::UnknownType { kind },
            UpgradeError::UnsupportedVersion { kind, version } => {
                LoadProblem::UnsupportedVersion { kind, version }
            }
        }
    }
}

impl std::fmt::Display for LoadProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadProblem::Malformed { message } => write!(f, "malformed line: {message}"),
            LoadProblem::UnknownType { kind } => write!(f, "unknown event type {kind}"),
            LoadProblem::UnsupportedVersion { kind, version } => {
                write!(f, "unsupported version {version} of {kind}")
            }
        }
    }
}
