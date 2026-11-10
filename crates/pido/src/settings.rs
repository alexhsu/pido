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
    /// The project directory of the last session, to prefill the picker.
    pub last_project: Option<String>,
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
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(error) = std::fs::write(&path, text) {
                    log::warn!("settings: cannot write {}: {error}", path.display());
                }
            }
            Err(error) => log::warn!("settings: cannot serialize: {error}"),
        }
    }
}

/// Load, mutate, and persist in one call: `save_setting(|s| s.theme = ...)`.
pub fn save_setting(apply: impl FnOnce(&mut Settings)) {
    let mut settings = Settings::load();
    apply(&mut settings);
    settings.save();
}
