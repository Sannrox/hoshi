//! User config beside the save directory. Not stored in a save.

use crate::keymap::Keymap;
use crate::notify;
use crate::portraits::ProtocolChoice;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct UserConfig {
    pub bug_report_target: String,
    pub log_level: String,
    pub serve_support_report: bool,
    pub serve_admin_token: String,
    pub update_check: bool,
    pub update_endpoint: String,
    pub collapse_changelog: bool,
    pub unknown_lines: Vec<String>,
    pub reduced_motion: bool,
    pub no_animation: bool,
    pub high_contrast: bool,
    pub bridge: bool,
    pub ai_advisor: bool,
    pub arena: bool,
    pub self_play: bool,
    pub cheats: bool,
    pub rivals: bool,
    pub season: bool,
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
    pub detail_panel: bool,
    /// Bind `host:port` for `hoshi serve`. Empty means the loopback default.
    pub serve_bind: String,
    /// Save path for `hoshi serve`. Empty means the default save slot.
    pub serve_save: String,
    /// Bearer secret for the process save. Empty rejects `/v1`. Not stored in the save.
    pub serve_token: String,
    /// Extra token file (`secret<whitespace>save-path` per line). Empty skips it.
    pub serve_token_file: String,
    /// Max `/v1` calls per token and action inside the window.
    pub serve_rate_limit: u32,
    /// Sliding window in seconds for serve rate limits.
    pub serve_rate_window_secs: u64,
    /// Player opt-in for out-of-session web push. Off by default.
    pub serve_push: bool,
    /// Operator VAPID private key. Runtime config, never the save.
    pub serve_vapid_private: String,
    /// Operator VAPID contact URI (`mailto:` or `https:`).
    pub serve_vapid_subject: String,
    /// Push subscription endpoint. Empty skips a live POST.
    pub serve_push_endpoint: String,
    /// Player opt-in for optional identity login. Off by default.
    pub serve_oidc: bool,
    /// Identity issuer URL. Runtime config, never the save.
    pub serve_oidc_issuer: String,
    /// Relying-party client id at the issuer.
    pub serve_oidc_client_id: String,
    /// Authorization endpoint. Empty keeps the flow off.
    pub serve_oidc_authorization: String,
    /// Token endpoint. Unused until a live exchange exists.
    pub serve_oidc_token: String,
    pub keys: Keymap,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            bug_report_target: "public".into(),
            log_level: "info".into(),
            serve_support_report: true,
            serve_admin_token: String::new(),
            update_check: true,
            update_endpoint: String::new(),
            collapse_changelog: false,
            unknown_lines: Vec::new(),
            reduced_motion: false,
            no_animation: false,
            high_contrast: false,
            bridge: false,
            ai_advisor: false,
            arena: false,
            self_play: false,
            cheats: false,
            rivals: false,
            season: true,
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
            detail_panel: true,
            serve_bind: String::new(),
            serve_save: String::new(),
            serve_token: String::new(),
            serve_token_file: String::new(),
            serve_rate_limit: crate::serve::DEFAULT_RATE_LIMIT,
            serve_rate_window_secs: crate::serve::DEFAULT_RATE_WINDOW_SECS,
            serve_push: false,
            serve_vapid_private: String::new(),
            serve_vapid_subject: String::new(),
            serve_push_endpoint: String::new(),
            serve_oidc: false,
            serve_oidc_issuer: String::new(),
            serve_oidc_client_id: String::new(),
            serve_oidc_authorization: String::new(),
            serve_oidc_token: String::new(),
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
        let parsed = toml::from_str::<toml::Table>(&text);
        cfg.bug_report_target = match parsed.as_ref() {
            Ok(value) => value
                .get("bug_report_target")
                .map(|value| value.as_str().unwrap_or("off").to_owned())
                .unwrap_or_else(|| "public".into()),
            Err(_) => "off".into(),
        };
        #[derive(serde::Deserialize)]
        struct Destination {
            bug_report_target: Option<toml::Spanned<String>>,
        }
        let destination_span = toml::from_str::<Destination>(&text)
            .ok()
            .and_then(|destination| destination.bug_report_target.map(|value| value.span()));
        let mut offset = 0;
        let mut key_overrides: Vec<(String, String)> = Vec::new();
        let mut extension_table = false;
        for raw in text.split_inclusive('\n') {
            let start = offset;
            offset += raw.len();
            if destination_span
                .as_ref()
                .is_some_and(|span| start < span.end && offset > span.start)
            {
                continue;
            }
            let raw = raw.trim_end_matches('\n');
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                extension_table = true;
            }
            if extension_table {
                cfg.unknown_lines.push(raw.to_owned());
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                cfg.unknown_lines.push(raw.to_owned());
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            match key {
                "log_level" => {
                    if matches!(value, "error" | "warn" | "info" | "debug") {
                        cfg.log_level = value.into();
                    }
                }
                "serve_support_report" => {
                    cfg.serve_support_report = value
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .parse::<bool>()
                        .unwrap_or(false)
                }
                "bug_report_target" => {}
                "serve_admin_token" => {
                    cfg.serve_admin_token = line
                        .split_once('=')
                        .and_then(|(_, raw)| serde_json::from_str(raw.trim()).ok())
                        .unwrap_or_else(|| value.into())
                }
                "update_check" => {
                    cfg.update_check = value
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .parse::<bool>()
                        .unwrap_or(false)
                }
                "update_endpoint" => {
                    cfg.update_endpoint = serde_json::from_str(
                        raw.split_once('=')
                            .map(|(_, value)| value.trim())
                            .unwrap_or(""),
                    )
                    .unwrap_or_else(|_| value.to_owned())
                }
                "collapse_changelog" => cfg.collapse_changelog = value == "true",
                "reduced_motion" => cfg.reduced_motion = value == "true",
                "no_animation" => cfg.no_animation = value == "true",
                "high_contrast" => cfg.high_contrast = value == "true",
                "bridge" => cfg.bridge = value == "true",
                "ai_advisor" => cfg.ai_advisor = value == "true",
                "arena" => cfg.arena = value == "true",
                "cheats" => cfg.cheats = value == "true",
                "self_play" => cfg.self_play = value == "true",
                "rivals" => cfg.rivals = value == "true",
                "season" => cfg.season = value == "true",
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
                "detail_panel" => cfg.detail_panel = value == "true",
                "serve_bind" => cfg.serve_bind = value.trim_matches('"').to_string(),
                "serve_save" => cfg.serve_save = value.trim_matches('"').to_string(),
                "serve_token" => cfg.serve_token = value.trim_matches('"').to_string(),
                "serve_token_file" => cfg.serve_token_file = value.trim_matches('"').to_string(),
                "serve_rate_limit" => {
                    if let Ok(n) = value.parse() {
                        cfg.serve_rate_limit = n;
                    }
                }
                "serve_rate_window_secs" => {
                    if let Ok(n) = value.parse() {
                        cfg.serve_rate_window_secs = n;
                    }
                }
                "serve_push" => cfg.serve_push = value == "true",
                "serve_vapid_private" => {
                    cfg.serve_vapid_private = value.trim_matches('"').to_string()
                }
                "serve_vapid_subject" => {
                    cfg.serve_vapid_subject = value.trim_matches('"').to_string()
                }
                "serve_push_endpoint" => {
                    cfg.serve_push_endpoint = value.trim_matches('"').to_string()
                }
                "serve_oidc" => cfg.serve_oidc = value == "true",
                "serve_oidc_issuer" => cfg.serve_oidc_issuer = value.trim_matches('"').to_string(),
                "serve_oidc_client_id" => {
                    cfg.serve_oidc_client_id = value.trim_matches('"').to_string()
                }
                "serve_oidc_authorization" => {
                    cfg.serve_oidc_authorization = value.trim_matches('"').to_string()
                }
                "serve_oidc_token" => cfg.serve_oidc_token = value.trim_matches('"').to_string(),
                key if key.starts_with("keys.") => {
                    key_overrides.push((key[5..].to_string(), value.to_string()));
                }
                _ => cfg.unknown_lines.push(raw.to_owned()),
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
        use fs2::FileExt;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("config.lock"))?;
        lock.lock_exclusive()?;
        let last = self.last_slot.as_deref().unwrap_or("");
        let quiet = match (self.notify_quiet_start, self.notify_quiet_end) {
            (Some(start), Some(end)) => {
                format!("notify_quiet_start = {start}\nnotify_quiet_end = {end}\n")
            }
            _ => String::new(),
        };
        let text = format!(
            "reduced_motion = {}\nno_animation = {}\nhigh_contrast = {}\nbridge = {}\nai_advisor = {}\narena = {}\nself_play = {}\ncheats = {}\nrivals = {}\nseason = {}\ndemo = {}\ngovernance = {}\nchisei = {}\nportraits = {}\nportrait_protocol = \"{}\"\nlast_slot = \"{last}\"\naway_threshold_secs = {}\ntutorial = {}\nreset_tutorial = {}\nnotify = {}\nnotify_build = {}\nnotify_raid = {}\nnotify_fleet = {}\ncommentary = {}\ncommentary_model_url = \"{}\"\nadvisor_model_url = \"{}\"\nadvisor_model_name = \"{}\"\nadvisor_allow_lan = {}\ncockpit_layout = \"{}\"\ncockpit_overview = \"{}\"\ncockpit_economy = \"{}\"\ncockpit_war = \"{}\"\ndetail_panel = {}\nserve_bind = \"{}\"\nserve_save = \"{}\"\nserve_token = \"{}\"\nserve_token_file = \"{}\"\nserve_rate_limit = {}\nserve_rate_window_secs = {}\nserve_push = {}\nserve_vapid_private = \"{}\"\nserve_vapid_subject = \"{}\"\nserve_push_endpoint = \"{}\"\nserve_oidc = {}\nserve_oidc_issuer = \"{}\"\nserve_oidc_client_id = \"{}\"\nserve_oidc_authorization = \"{}\"\nserve_oidc_token = \"{}\"\n{quiet}{}\n",
            self.reduced_motion,
            self.no_animation,
            self.high_contrast,
            self.bridge,
            self.ai_advisor,
            self.arena,
            self.self_play,
            self.cheats,
            self.rivals,
            self.season,
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
            self.detail_panel,
            self.serve_bind.replace('"', ""),
            self.serve_save.replace('"', ""),
            self.serve_token.replace('"', ""),
            self.serve_token_file.replace('"', ""),
            self.serve_rate_limit,
            self.serve_rate_window_secs,
            self.serve_push,
            self.serve_vapid_private.replace('"', ""),
            self.serve_vapid_subject.replace('"', ""),
            self.serve_push_endpoint.replace('"', ""),
            self.serve_oidc,
            self.serve_oidc_issuer.replace('"', ""),
            self.serve_oidc_client_id.replace('"', ""),
            self.serve_oidc_authorization.replace('"', ""),
            self.serve_oidc_token.replace('"', ""),
            self.keys.to_config_lines().trim_end(),
        );
        let text = format!(
            "log_level = {}\nserve_support_report = {}\nserve_admin_token = {}\n{text}",
            serde_json::to_string(&self.log_level)?,
            self.serve_support_report,
            serde_json::to_string(&self.serve_admin_token)?
        );
        let mut text = format!(
            "update_check = {}\nupdate_endpoint = {}\ncollapse_changelog = {}\n{text}",
            self.update_check,
            serde_json::to_string(&self.update_endpoint)?,
            self.collapse_changelog
        );
        // Re-read extensions so a concurrently installed newer build keeps its keys.
        let current = Self::load(dir);
        let unknown = if Self::path(dir).exists() {
            &current.unknown_lines
        } else {
            &self.unknown_lines
        };
        for line in unknown {
            text.push_str(line);
            text.push('\n');
        }
        let text = format!(
            "bug_report_target = {}\n{text}",
            toml::Value::String(self.bug_report_target.clone())
        );
        atomicwrites::AtomicFile::new(
            Self::path(dir),
            atomicwrites::OverwriteBehavior::AllowOverwrite,
        )
        .write(|file| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            std::io::Write::write_all(file, text.as_bytes())?;
            file.sync_all()
        })
        .map_err(|error| std::io::Error::other(error.to_string()))
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
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let suffix = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("hoshi-cfg-{}-{nanos}-{suffix}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bug_target_accepts_toml_strings_and_comments() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("hoshi-bug-config-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        for (raw, expected) in [
            ("\"public\" # public tracker", "public"),
            ("'off'", "off"),
            (
                "\"\"\"https://tracker.invalid\\\n  /new?body={body}\"\"\"",
                "https://tracker.invalid/new?body={body}",
            ),
            (
                "'https://tracker.invalid/new?title={title}' # custom",
                "https://tracker.invalid/new?title={title}",
            ),
        ] {
            std::fs::write(
                UserConfig::path(&dir),
                format!("bug_report_target = {raw}\n"),
            )
            .unwrap();
            let config = UserConfig::load(&dir);
            assert_eq!(config.bug_report_target, expected);
            config.save(&dir).unwrap();
            assert_eq!(UserConfig::load(&dir).bug_report_target, expected);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn round_trips_and_defaults_keep_bridge_and_ai_off() {
        let dir = unique_dir();
        let loaded = UserConfig::load(&dir);
        assert!(!loaded.bridge);
        assert!(!loaded.ai_advisor);
        assert!(!loaded.arena);
        assert!(!loaded.self_play);
        assert!(!loaded.rivals);
        assert!(loaded.season);
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
        assert!(loaded.detail_panel);
        assert!(loaded.portraits);
        assert_eq!(loaded.portrait_protocol, ProtocolChoice::Auto);
        std::fs::write(
            UserConfig::path(&dir),
            "future_setting = 7\n[future]\nbridge = true\n",
        )
        .unwrap();
        let loaded = UserConfig::load(&dir);
        assert!(!loaded.bridge);
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
        cfg.self_play = true;
        cfg.rivals = true;
        cfg.season = false;
        cfg.demo = true;
        cfg.governance = true;
        cfg.chisei = true;
        cfg.cockpit_layout = "war".into();
        cfg.cockpit_war = "map,fleets".into();
        cfg.detail_panel = false;
        cfg.serve_bind = "127.0.0.1:0".into();
        cfg.serve_save = "slot.sqlite".into();
        cfg.log_level = "debug".into();
        cfg.serve_support_report = false;
        cfg.serve_admin_token = "admin-\"quoted\"-fixture".into();
        cfg.save(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(UserConfig::path(&dir))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let saved = std::fs::read_to_string(UserConfig::path(&dir)).unwrap();
        assert!(saved.contains("future_setting = 7\n[future]\nbridge = true\n"));
        let again = UserConfig::load(&dir);
        assert!(again.reduced_motion);
        assert_eq!(again.last_slot.as_deref(), Some("alpha"));
        assert!(!again.bridge);
        assert!(again.arena);
        assert!(again.self_play);
        assert!(again.rivals);
        assert!(!again.season);
        assert!(again.demo);
        assert!(again.governance);
        assert!(again.chisei);
        assert_eq!(again.cockpit_layout, "war");
        assert_eq!(again.cockpit_war, "map,fleets");
        assert!(!again.detail_panel);
        assert_eq!(again.serve_bind, "127.0.0.1:0");
        assert_eq!(again.serve_save, "slot.sqlite");
        assert_eq!(again.log_level, "debug");
        assert!(!again.serve_support_report);
        assert_eq!(again.serve_admin_token, cfg.serve_admin_token);
        assert!(again.serve_token.is_empty());
        assert_eq!(again.serve_rate_limit, crate::serve::DEFAULT_RATE_LIMIT);
        assert!(!again.serve_push);
        assert!(again.serve_vapid_private.is_empty());
        assert!(!again.serve_oidc);
        assert!(again.serve_oidc_issuer.is_empty());
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
    fn loads_serve_token_and_rate_limits() {
        let dir = unique_dir();
        std::fs::write(
            UserConfig::path(&dir),
            "serve_token = \"abc\"\nserve_token_file = \"tokens.txt\"\nserve_rate_limit = 3\nserve_rate_window_secs = 9\n",
        )
        .unwrap();
        let cfg = UserConfig::load(&dir);
        assert_eq!(cfg.serve_token, "abc");
        assert_eq!(cfg.serve_token_file, "tokens.txt");
        assert_eq!(cfg.serve_rate_limit, 3);
        assert_eq!(cfg.serve_rate_window_secs, 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_serve_push_and_vapid() {
        let dir = unique_dir();
        std::fs::write(
            UserConfig::path(&dir),
            "serve_push = true\nserve_vapid_private = \"k\"\nserve_vapid_subject = \"mailto:hoshi@localhost\"\nserve_push_endpoint = \"http://127.0.0.1/push\"\n",
        )
        .unwrap();
        let cfg = UserConfig::load(&dir);
        assert!(cfg.serve_push);
        assert_eq!(cfg.serve_vapid_private, "k");
        assert_eq!(cfg.serve_vapid_subject, "mailto:hoshi@localhost");
        assert_eq!(cfg.serve_push_endpoint, "http://127.0.0.1/push");
        cfg.save(&dir).unwrap();
        let again = UserConfig::load(&dir);
        assert!(again.serve_push);
        assert_eq!(again.serve_vapid_private, "k");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_serve_oidc() {
        let dir = unique_dir();
        std::fs::write(
            UserConfig::path(&dir),
            "serve_oidc = true\nserve_oidc_issuer = \"http://127.0.0.1/iss\"\nserve_oidc_client_id = \"hoshi\"\nserve_oidc_authorization = \"http://127.0.0.1/auth\"\nserve_oidc_token = \"http://127.0.0.1/token\"\n",
        )
        .unwrap();
        let cfg = UserConfig::load(&dir);
        assert!(cfg.serve_oidc);
        assert_eq!(cfg.serve_oidc_issuer, "http://127.0.0.1/iss");
        assert_eq!(cfg.serve_oidc_client_id, "hoshi");
        assert_eq!(cfg.serve_oidc_authorization, "http://127.0.0.1/auth");
        assert_eq!(cfg.serve_oidc_token, "http://127.0.0.1/token");
        cfg.save(&dir).unwrap();
        let again = UserConfig::load(&dir);
        assert!(again.serve_oidc);
        assert_eq!(again.serve_oidc_client_id, "hoshi");
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
    fn update_opt_out_with_inline_comment_survives_save() {
        let dir = unique_dir();
        std::fs::write(
            UserConfig::path(&dir),
            "update_check = false # disable release requests\n",
        )
        .unwrap();
        let cfg = UserConfig::load(&dir);
        assert!(!cfg.update_check);
        cfg.save(&dir).unwrap();
        assert!(!UserConfig::load(&dir).update_check);
        std::fs::remove_dir_all(dir).unwrap();
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
