//! User config beside the save directory. Not stored in a save.

use crate::keymap::Keymap;
use crate::notify;
use crate::portraits::ProtocolChoice;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct UserConfig {
    pub reduced_motion: bool,
    pub no_animation: bool,
    pub high_contrast: bool,
    pub bridge: bool,
    pub ai_advisor: bool,
    pub arena: bool,
    pub rivals: bool,
    pub demo: bool,
    pub governance: bool,
    pub chisei: bool,
    pub portraits: bool,
    pub portrait_protocol: ProtocolChoice,
    pub last_slot: Option<String>,
    pub away_threshold_secs: u64,
    pub tutorial: bool,
    pub reset_tutorial: bool,
    pub notify: bool,
    pub notify_build: bool,
    pub notify_raid: bool,
    pub notify_fleet: bool,
    pub notify_quiet_start: Option<u8>,
    pub notify_quiet_end: Option<u8>,
    pub commentary: bool,
    pub commentary_model_url: String,
    pub advisor_model_url: String,
    pub advisor_model_name: String,
    pub advisor_allow_lan: bool,
    pub cockpit_layout: String,
    pub cockpit_overview: String,
    pub cockpit_economy: String,
    pub cockpit_war: String,
    pub keys: Keymap,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            reduced_motion: false,
            no_animation: false,
            high_contrast: false,
            bridge: false,
            ai_advisor: false,
            arena: false,
            rivals: false,
            demo: false,
            governance: false,
            chisei: false,
            portraits: true,
            portrait_protocol: ProtocolChoice::Auto,
            last_slot: None,
            away_threshold_secs: 60,
            tutorial: true,
            reset_tutorial: false,
            notify: false,
            notify_build: true,
            notify_raid: true,
            notify_fleet: true,
            notify_quiet_start: None,
            notify_quiet_end: None,
            commentary: false,
            commentary_model_url: String::new(),
            advisor_model_url: String::new(),
            advisor_model_name: String::new(),
            advisor_allow_lan: false,
            cockpit_layout: "overview".into(),
            cockpit_overview: crate::cockpit::OVERVIEW_PANELS.into(),
            cockpit_economy: crate::cockpit::ECONOMY_PANELS.into(),
            cockpit_war: crate::cockpit::WAR_PANELS.into(),
            keys: Keymap::defaults(),
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
        let mut key_overrides: Vec<(String, String)> = Vec::new();
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
                "no_animation" => cfg.no_animation = value == "true",
                "high_contrast" => cfg.high_contrast = value == "true",
                "bridge" => cfg.bridge = value == "true",
                "ai_advisor" => cfg.ai_advisor = value == "true",
                "arena" => cfg.arena = value == "true",
                "rivals" => cfg.rivals = value == "true",
                "demo" => cfg.demo = value == "true",
                "governance" => cfg.governance = value == "true",
                "chisei" => cfg.chisei = value == "true",
                "portraits" => cfg.portraits = value == "true",
                "portrait_protocol" => {
                    if let Some(choice) = ProtocolChoice::from_key(value) {
                        cfg.portrait_protocol = choice;
                    }
                }
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
                "tutorial" => cfg.tutorial = value != "false",
                "reset_tutorial" => cfg.reset_tutorial = value == "true",
                "notify" => cfg.notify = value == "true",
                "notify_build" => cfg.notify_build = value == "true",
                "notify_raid" => cfg.notify_raid = value == "true",
                "notify_fleet" => cfg.notify_fleet = value == "true",
                "notify_quiet_start" => cfg.notify_quiet_start = parse_hour(value),
                "notify_quiet_end" => cfg.notify_quiet_end = parse_hour(value),
                "commentary" => cfg.commentary = value == "true",
                "commentary_model_url" => {
                    cfg.commentary_model_url = value.trim_matches('"').to_string();
                }
                "advisor_model_url" => {
                    cfg.advisor_model_url = value.trim_matches('"').to_string();
                }
                "advisor_model_name" => {
                    cfg.advisor_model_name = value.trim_matches('"').to_string();
                }
                "advisor_allow_lan" => cfg.advisor_allow_lan = value == "true",
                "cockpit_layout" => {
                    if crate::cockpit::CockpitLayout::from_key(value).is_some() {
                        cfg.cockpit_layout = value.to_string();
                    }
                }
                "cockpit_overview" => cfg.cockpit_overview = value.to_string(),
                "cockpit_economy" => cfg.cockpit_economy = value.to_string(),
                "cockpit_war" => cfg.cockpit_war = value.to_string(),
                key if key.starts_with("keys.") => {
                    key_overrides.push((key[5..].to_string(), value.to_string()));
                }
                _ => {}
            }
        }
        if !key_overrides.is_empty() {
            let pairs: Vec<(&str, &str)> = key_overrides
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            if let Ok(keys) = Keymap::from_overrides(pairs) {
                cfg.keys = keys;
            }
        }
        cfg
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let last = self.last_slot.as_deref().unwrap_or("");
        let quiet = match (self.notify_quiet_start, self.notify_quiet_end) {
            (Some(start), Some(end)) => {
                format!("notify_quiet_start = {start}\nnotify_quiet_end = {end}\n")
            }
            _ => String::new(),
        };
        let text = format!(
            "reduced_motion = {}\nno_animation = {}\nhigh_contrast = {}\nbridge = {}\nai_advisor = {}\narena = {}\nrivals = {}\ndemo = {}\ngovernance = {}\nchisei = {}\nportraits = {}\nportrait_protocol = \"{}\"\nlast_slot = \"{last}\"\naway_threshold_secs = {}\ntutorial = {}\nreset_tutorial = {}\nnotify = {}\nnotify_build = {}\nnotify_raid = {}\nnotify_fleet = {}\ncommentary = {}\ncommentary_model_url = \"{}\"\nadvisor_model_url = \"{}\"\nadvisor_model_name = \"{}\"\nadvisor_allow_lan = {}\ncockpit_layout = \"{}\"\ncockpit_overview = \"{}\"\ncockpit_economy = \"{}\"\ncockpit_war = \"{}\"\n{quiet}{}\n",
            self.reduced_motion,
            self.no_animation,
            self.high_contrast,
            self.bridge,
            self.ai_advisor,
            self.arena,
            self.rivals,
            self.demo,
            self.governance,
            self.chisei,
            self.portraits,
            self.portrait_protocol.as_key(),
            self.away_threshold_secs,
            self.tutorial,
            self.reset_tutorial,
            self.notify,
            self.notify_build,
            self.notify_raid,
            self.notify_fleet,
            self.commentary,
            self.commentary_model_url.replace('"', ""),
            self.advisor_model_url.replace('"', ""),
            self.advisor_model_name.replace('"', ""),
            self.advisor_allow_lan,
            self.cockpit_layout.replace('"', ""),
            self.cockpit_overview.replace('"', ""),
            self.cockpit_economy.replace('"', ""),
            self.cockpit_war.replace('"', ""),
            self.keys.to_config_lines().trim_end(),
        );
        std::fs::write(Self::path(dir), text)
    }

    pub fn notify_prefs(&self) -> notify::Prefs {
        notify::Prefs {
            enabled: self.notify,
            build: self.notify_build,
            raid: self.notify_raid,
            fleet: self.notify_fleet,
            quiet_start: self.notify_quiet_start,
            quiet_end: self.notify_quiet_end,
            bell: !self.reduced_motion,
        }
    }
}

fn parse_hour(value: &str) -> Option<u8> {
    let n: u8 = value.parse().ok()?;
    (n <= 23).then_some(n)
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
        assert!(!loaded.arena);
        assert!(!loaded.rivals);
        assert!(!loaded.demo);
        assert!(!loaded.governance);
        assert!(!loaded.chisei);
        assert!(loaded.tutorial);
        assert!(!loaded.reset_tutorial);
        assert!(!loaded.notify);
        assert!(!loaded.commentary);
        assert!(loaded.commentary_model_url.is_empty());
        assert!(loaded.advisor_model_url.is_empty());
        assert!(loaded.advisor_model_name.is_empty());
        assert!(!loaded.advisor_allow_lan);
        assert_eq!(loaded.cockpit_layout, "overview");
        assert!(loaded.portraits);
        assert_eq!(loaded.portrait_protocol, ProtocolChoice::Auto);
        let mut cfg = loaded;
        cfg.reduced_motion = true;
        cfg.portraits = false;
        cfg.portrait_protocol = ProtocolChoice::Off;
        cfg.last_slot = Some("alpha".into());
        cfg.commentary = true;
        cfg.commentary_model_url = "http://127.0.0.1:8080/v1".into();
        cfg.advisor_model_url = "http://127.0.0.1:11434/v1".into();
        cfg.advisor_model_name = "local".into();
        cfg.advisor_allow_lan = true;
        cfg.arena = true;
        cfg.rivals = true;
        cfg.demo = true;
        cfg.governance = true;
        cfg.chisei = true;
        cfg.cockpit_layout = "war".into();
        cfg.cockpit_war = "map,fleets".into();
        cfg.save(&dir).unwrap();
        let again = UserConfig::load(&dir);
        assert!(again.reduced_motion);
        assert_eq!(again.last_slot.as_deref(), Some("alpha"));
        assert!(!again.bridge);
        assert!(again.arena);
        assert!(again.rivals);
        assert!(again.demo);
        assert!(again.governance);
        assert!(again.chisei);
        assert_eq!(again.cockpit_layout, "war");
        assert_eq!(again.cockpit_war, "map,fleets");
        assert!(!again.notify);
        assert!(again.commentary);
        assert_eq!(again.commentary_model_url, "http://127.0.0.1:8080/v1");
        assert_eq!(again.advisor_model_url, "http://127.0.0.1:11434/v1");
        assert_eq!(again.advisor_model_name, "local");
        assert!(again.advisor_allow_lan);
        assert!(!again.portraits);
        assert_eq!(again.portrait_protocol, ProtocolChoice::Off);
        assert_eq!(again.keys, Keymap::defaults());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_key_remaps_and_ignores_conflicts() {
        let dir = unique_dir();
        std::fs::write(
            UserConfig::path(&dir),
            "keys.quit = \"p\"\nkeys.help = \"?\"\n",
        )
        .unwrap();
        let cfg = UserConfig::load(&dir);
        assert!(cfg.keys.matches(
            crate::keymap::Binding::Quit,
            ratatui::crossterm::event::KeyCode::Char('p')
        ));
        std::fs::write(UserConfig::path(&dir), "keys.quit = \"j\"\n").unwrap();
        let rejected = UserConfig::load(&dir);
        assert_eq!(rejected.keys, Keymap::defaults());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_notify_toggles_and_quiet_hours() {
        let dir = unique_dir();
        std::fs::write(
            UserConfig::path(&dir),
            "notify = true\nnotify_raid = false\nnotify_quiet_start = 22\nnotify_quiet_end = 8\n",
        )
        .unwrap();
        let cfg = UserConfig::load(&dir);
        assert!(cfg.notify);
        assert!(cfg.notify_build);
        assert!(!cfg.notify_raid);
        assert_eq!(cfg.notify_quiet_start, Some(22));
        assert_eq!(cfg.notify_quiet_end, Some(8));
        let prefs = cfg.notify_prefs();
        assert!(prefs.enabled);
        assert!(!prefs.raid);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
