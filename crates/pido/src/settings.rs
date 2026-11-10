//! Persisted user settings: theme, locale, last project, window size.
//!
//! Stored as JSON under `<config_dir>/pido/settings.json`
//! (`%APPDATA%\pido` on Windows, `~/.config/pido` on Linux,
//! `~/Library/Application Support/pido` on macOS).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// `"dark"` or `"light"`.
    pub theme: Option<String>,
    /// `"zh-CN"` or `"en-US"`.
    pub locale: Option<String>,
    /// The project directory of the last session, to prefill the picker and
    /// reconnect at startup.
    pub last_project: Option<String>,
    /// The session file of the last active session; the app resumes it on
    /// startup instead of starting a fresh empty one.
    pub last_session: Option<String>,
    pub window: Option<WindowState>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("pido").join("settings.json"))
    }

    /// Load settings; missing or malformed files fall back to defaults
    /// (a broken settings file must never stop the app).
    pub fn load() -> Self {
        Self::path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Write settings; failures are logged, not fatal.
    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            log::warn!("settings: cannot create {}: {error}", parent.display());
            return;
        }
        let text = match serde_json::to_string_pretty(self) {
            Ok(text) => text,
            Err(error) => {
                log::warn!("settings: cannot serialize: {error}");
                return;
            }
        };
        // Write a sibling temp file and rename it into place. On Windows a
        // rename cannot replace an existing file, so remove the old one only
        // after the temp write has succeeded.
        let temp = path.with_extension("json.tmp");
        if let Err(error) = std::fs::write(&temp, text) {
            log::warn!("settings: cannot write {}: {error}", temp.display());
            return;
        }
        if let Err(error) = std::fs::rename(&temp, &path) {
            #[cfg(windows)]
            {
                let _ = std::fs::remove_file(&path);
                if std::fs::rename(&temp, &path).is_ok() {
                    return;
                }
            }
            log::warn!("settings: cannot replace {}: {error}", path.display());
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// Load, mutate, and persist in one call: `save_setting(|s| s.theme = ...)`.
pub fn save_setting(apply: impl FnOnce(&mut Settings)) {
    let mut settings = Settings::load();
    apply(&mut settings);
    settings.save();
}
