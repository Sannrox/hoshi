//! User config beside the save directory. Not stored in a save.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct UserConfig {
    pub reduced_motion: bool,
    pub high_contrast: bool,
    pub bridge: bool,
    pub ai_advisor: bool,
    pub last_slot: Option<String>,
    pub away_threshold_secs: u64,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            reduced_motion: false,
            high_contrast: false,
            bridge: false,
            ai_advisor: false,
            last_slot: None,
            away_threshold_secs: 60,
        }
    }
}

impl UserConfig {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("config.toml")
    }

    pub fn load(dir: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(Self::path(dir)) else {
            return Self::default();
        };
        let mut cfg = Self::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            match key {
                "reduced_motion" => cfg.reduced_motion = value == "true",
                "high_contrast" => cfg.high_contrast = value == "true",
                "bridge" => cfg.bridge = value == "true",
                "ai_advisor" => cfg.ai_advisor = value == "true",
                "last_slot" => {
                    cfg.last_slot = if value.is_empty() {
                        None
                    } else {
                        Some(value.to_string())
                    }
                }
                "away_threshold_secs" => {
                    if let Ok(n) = value.parse() {
                        cfg.away_threshold_secs = n;
                    }
                }
                _ => {}
            }
        }
        cfg
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let last = self.last_slot.as_deref().unwrap_or("");
        let text = format!(
            "reduced_motion = {}\nhigh_contrast = {}\nbridge = {}\nai_advisor = {}\nlast_slot = \"{last}\"\naway_threshold_secs = {}\n",
            self.reduced_motion,
            self.high_contrast,
            self.bridge,
            self.ai_advisor,
            self.away_threshold_secs,
        );
        std::fs::write(Self::path(dir), text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("hoshi-cfg-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_and_defaults_keep_bridge_and_ai_off() {
        let dir = unique_dir();
        let loaded = UserConfig::load(&dir);
        assert!(!loaded.bridge);
        assert!(!loaded.ai_advisor);
        let mut cfg = loaded;
        cfg.reduced_motion = true;
        cfg.last_slot = Some("alpha".into());
        cfg.save(&dir).unwrap();
        let again = UserConfig::load(&dir);
        assert!(again.reduced_motion);
        assert_eq!(again.last_slot.as_deref(), Some("alpha"));
        assert!(!again.bridge);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
