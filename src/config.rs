//! User settings stored in `config.toml` in the platform's config directory
//! (`%APPDATA%\peekr` on Windows, `$XDG_CONFIG_HOME/peekr` or `~/.config/peekr` on Linux).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::platform;
use crate::shortcut::Shortcut;

const FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
// Missing fields fall back to defaults, so older config files keep working.
#[serde(default)]
pub struct Config {
    /// Global shortcut that starts a capture.
    pub hotkey: Shortcut,
}

impl Config {
    /// Loads the config; a missing or unreadable file means default settings.
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                log::warn!("cannot read {}: {e}; using default settings", path.display());
                return Self::default();
            }
        };

        toml::from_str(&text).unwrap_or_else(|e| {
            log::warn!("invalid {}: {e}; using default settings", path.display());
            Self::default()
        })
    }

    pub fn save(&self) -> Result<()> {
        let path = path().context("no config directory on this system")?;
        fs::create_dir_all(path.parent().context("config path has no parent")?)?;

        let text = format!("# peekr settings\n\n{}", toml::to_string(self)?);
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;

        log::info!("saved settings to {}", path.display());
        Ok(())
    }
}

fn path() -> Option<PathBuf> {
    Some(platform::config_dir()?.join("peekr").join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let config = Config {
            hotkey: "Ctrl+Alt+F9".parse().expect("valid shortcut"),
        };

        let text = toml::to_string(&config).expect("serializable");
        let parsed: Config = toml::from_str(&text).expect("deserializable");

        assert_eq!(parsed, config);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let parsed: Config = toml::from_str("").expect("empty config is valid");

        assert_eq!(parsed, Config::default());
    }
}
