//! Persisted user preferences.
//!
//! Settings live in a small JSON file inside the platform configuration
//! directory. Loading never fails: a broken or missing file simply yields the
//! defaults.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;

/// User configurable options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Directory that contains the `ffmpeg` / `ffprobe` binaries.
    #[serde(default)]
    pub ffmpeg_dir: Option<PathBuf>,
    /// Length of a single `Left` / `Right` seek step in seconds.
    #[serde(default = "default_step")]
    pub step_seconds: f64,
    /// Length of a `Shift + Left` / `Shift + Right` seek step in seconds.
    #[serde(default = "default_fast_step")]
    pub fast_step_seconds: f64,
    /// Use the dark colour scheme.
    #[serde(default = "default_true")]
    pub dark_theme: bool,
    /// Snap segment boundaries to key frames by default.
    #[serde(default = "default_true")]
    pub snap_to_keyframes: bool,
    /// The language of the user interface.
    #[serde(default)]
    pub language: Lang,
}

fn default_step() -> f64 {
    1.0
}

fn default_fast_step() -> f64 {
    10.0
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ffmpeg_dir: None,
            step_seconds: default_step(),
            fast_step_seconds: default_fast_step(),
            dark_theme: true,
            snap_to_keyframes: true,
            language: Lang::default(),
        }
    }
}

impl Settings {
    /// Read the settings file, falling back to the defaults on any problem.
    ///
    /// When there is no settings file yet, the language of the operating
    /// system is used, so that the application greets the user in their own
    /// language.
    pub fn load() -> Self {
        let exists = Self::config_path()
            .map(|path| path.exists())
            .unwrap_or(false);
        match Self::read() {
            Ok(settings) => settings,
            Err(error) => {
                log::warn!("could not read the settings file: {error:#}");
                let mut settings = Self::default();
                if !exists {
                    settings.language = Lang::detect();
                }
                settings
            }
        }
    }

    /// Write the settings file.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path().context("no configuration directory available")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).context("could not encode the settings")?;
        std::fs::write(&path, json).with_context(|| format!("could not write {}", path.display()))
    }

    fn read() -> Result<Self> {
        let path = Self::config_path().context("no configuration directory available")?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        serde_json::from_str(&text).context("the settings file is corrupted")
    }

    /// Location of the settings file.
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("lossless-vidcut").join("settings.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let settings = Settings::default();
        assert!(settings.dark_theme);
        assert_eq!(settings.step_seconds, 1.0);
        assert_eq!(settings.fast_step_seconds, 10.0);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = Settings {
            ffmpeg_dir: Some(PathBuf::from("/opt/ffmpeg/bin")),
            step_seconds: 0.5,
            fast_step_seconds: 5.0,
            dark_theme: false,
            snap_to_keyframes: false,
            language: Lang::Russian,
        };
        let json = serde_json::to_string(&settings).expect("encode");
        let decoded: Settings = serde_json::from_str(&json).expect("decode");
        assert_eq!(settings, decoded);
    }

    #[test]
    fn a_file_without_a_language_stays_english() {
        let json = r#"{"step_seconds": 2.0}"#;
        let decoded: Settings = serde_json::from_str(json).expect("decode");
        assert_eq!(decoded.language, Lang::English);
    }
}
