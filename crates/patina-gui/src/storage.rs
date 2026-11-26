//! Choosing the data root on Android: the shared `Documents/Patina` folder when the app may
//! write there (so a sync tool can see it), the private app directory otherwise.

use std::fs;
use std::path::{Path, PathBuf};

use patina_core::ids::DeviceId;

/// Picks the data root and whether it is the shared folder.
///
/// The shared folder is used only when it can be created and written. This device's event
/// file and `config.toml` are copied over once from `internal`, never over an existing
/// file. Any failure falls back to `internal`. The device id is not part of the root: it
/// stays in `internal`, so it is never synced.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn choose_root(internal: &Path, shared: &Path, device: DeviceId) -> (PathBuf, bool) {
    match prepare(internal, shared, device) {
        Ok(()) => (shared.to_path_buf(), true),
        Err(_) => (internal.to_path_buf(), false),
    }
}

fn prepare(internal: &Path, shared: &Path, device: DeviceId) -> std::io::Result<()> {
    fs::create_dir_all(shared.join("events"))?;
    let probe = shared.join(".patina-probe");
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)?;

    let log = format!("{}.jsonl", device.as_uuid());
    for (from, to) in [
        (
            internal.join("events").join(&log),
            shared.join("events").join(&log),
        ),
        (internal.join("config.toml"), shared.join("config.toml")),
    ] {
        if from.exists() && !to.exists() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn existing_data_is_copied_once_and_never_overwritten() {
        let internal = tempfile::tempdir().unwrap();
        let shared = tempfile::tempdir().unwrap();
        let device = DeviceId::from_uuid(Uuid::from_u128(5));
        let log = format!("{}.jsonl", device.as_uuid());
        fs::create_dir_all(internal.path().join("events")).unwrap();
        fs::write(internal.path().join("events").join(&log), "old\n").unwrap();
        fs::write(internal.path().join("config.toml"), "day_start_hour = 5\n").unwrap();

        let (root, shared_ok) = choose_root(internal.path(), shared.path(), device);
        assert!(shared_ok);
        assert_eq!(root, shared.path());
        let copied = shared.path().join("events").join(&log);
        assert_eq!(fs::read_to_string(&copied).unwrap(), "old\n");
        assert!(shared.path().join("config.toml").exists());

        fs::write(&copied, "old\nnew\n").unwrap();
        choose_root(internal.path(), shared.path(), device);
        assert_eq!(fs::read_to_string(&copied).unwrap(), "old\nnew\n");
    }

    #[test]
    fn an_unwritable_shared_folder_falls_back_to_the_internal_one() {
        let internal = tempfile::tempdir().unwrap();
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let shared = blocker.path().join("Patina");
        let (root, shared_ok) = choose_root(
            internal.path(),
            &shared,
            DeviceId::from_uuid(Uuid::from_u128(5)),
        );
        assert!(!shared_ok);
        assert_eq!(root, internal.path());
    }
}
