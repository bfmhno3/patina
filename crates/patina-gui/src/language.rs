//! UI language: the per-device preference in `gui.toml`, "follow system", and selecting
//! the bundled translation.

use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A bundled UI language; `key` is its folder under `translations/` and its `gui.toml` value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    En,
    ZhCn,
    ZhTw,
}

impl Language {
    pub fn key(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh_CN",
            Self::ZhTw => "zh_TW",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        [Self::En, Self::ZhCn, Self::ZhTw]
            .into_iter()
            .find(|l| l.key() == s)
    }

    /// Maps a system locale (`zh-Hant-TW`, `zh_TW.UTF-8`, `en-US`) to a bundled language.
    pub fn from_locale(locale: &str) -> Self {
        let lower = locale.to_lowercase();
        let base = lower.split(['.', '@']).next().unwrap_or_default();
        let mut parts = base.split(['-', '_']);
        if parts.next() != Some("zh") {
            return Self::En;
        }
        if parts.any(|p| matches!(p, "hant" | "tw" | "hk" | "mo")) {
            Self::ZhTw
        } else {
            Self::ZhCn
        }
    }
}

/// The language to show: `pref`, or the system's when `pref` is `None` ("follow system").
pub fn resolve(pref: Option<Language>) -> Language {
    pref.unwrap_or_else(|| {
        sys_locale::get_locale().map_or(Language::En, |l| Language::from_locale(&l))
    })
}

/// Selects `lang`'s bundled translation; every `@tr` binding re-evaluates.
pub fn select(lang: Language) {
    if let Err(e) = slint::select_bundled_translation(lang.key()) {
        eprintln!("cannot select language {}: {e}", lang.key());
    }
}

const FILE: &str = "gui.toml";

#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct Prefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
}

/// Reads the preference from `dir/gui.toml`; `None` means follow the system.
///
/// An unreadable or invalid file is reported on stderr and ignored: it never stops startup.
pub fn load(dir: &Path) -> Option<Language> {
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return None;
        }
    };
    let prefs: Prefs = match toml::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ignoring {}: {e}", path.display());
            return None;
        }
    };
    let key = prefs.language?;
    let lang = Language::from_key(&key);
    if lang.is_none() {
        eprintln!("ignoring {}: unknown language {key:?}", path.display());
    }
    lang
}

/// Writes the preference to `dir/gui.toml`, creating `dir`.
///
/// # Errors
///
/// Fails when the directory or file cannot be written.
pub fn save(dir: &Path, pref: Option<Language>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = toml::to_string(&Prefs {
        language: pref.map(|l| l.key().to_string()),
    })
    .map_err(std::io::Error::other)?;
    std::fs::write(dir.join(FILE), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_map_to_bundled_languages() {
        for (locale, want) in [
            ("zh-Hant-TW", Language::ZhTw),
            ("zh_TW.UTF-8", Language::ZhTw),
            ("zh-HK", Language::ZhTw),
            ("zh-CN", Language::ZhCn),
            ("zh-Hans-CN", Language::ZhCn),
            ("zh", Language::ZhCn),
            ("zh-SG", Language::ZhCn),
            ("en-US", Language::En),
            ("ja-JP", Language::En),
            ("C", Language::En),
        ] {
            assert_eq!(Language::from_locale(locale), want, "{locale}");
        }
    }

    #[test]
    fn the_preference_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("device");
        assert_eq!(load(&dir), None);
        save(&dir, Some(Language::ZhTw)).unwrap();
        assert_eq!(load(&dir), Some(Language::ZhTw));
        save(&dir, None).unwrap();
        assert_eq!(load(&dir), None);
    }

    #[test]
    fn a_bad_file_follows_the_system() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(FILE), "language = \"fr\"").unwrap();
        assert_eq!(load(tmp.path()), None);
        std::fs::write(tmp.path().join(FILE), "language = ").unwrap();
        assert_eq!(load(tmp.path()), None);
    }
}
