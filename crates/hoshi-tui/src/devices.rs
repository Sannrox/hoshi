//! Durable device sessions for browsers paired with `hoshi serve` (ADR 0016).
//!
//! One record per device in `<save>.devices.json` beside the save, mode
//! `0600`, replaced by atomic rename. Only `sha256(secret)` is stored; the
//! secret itself is returned once, by [`Devices::create`]. Never in the save.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// A session lives this long after its last use.
pub const LIFETIME_SECS: u64 = 30 * 24 * 60 * 60;
/// Use extends a session at most this often, so requests do not rewrite the file.
const TOUCH_SECS: u64 = 60 * 60;
const MAX_LABEL: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Mobile,
    Tablet,
    Desktop,
    Bot,
    Unknown,
}

/// One paired browser. Metadata is for display and carries no authorization weight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub label: String,
    pub device_type: DeviceType,
    pub os: String,
    pub browser: String,
    pub ip: String,
    pub created_at: u64,
    pub last_used_at: u64,
    pub expires_at: u64,
    secret_sha256: String,
}

/// Display metadata read from the request (User-Agent, peer address), never
/// from anything the client says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub device_type: DeviceType,
    pub os: String,
    pub browser: String,
    pub ip: String,
}

impl Client {
    pub fn from_request(user_agent: Option<&str>, peer: Option<IpAddr>) -> Self {
        let ua = user_agent.unwrap_or("");
        let has = |needle: &str| ua.contains(needle);
        let lower = ua.to_ascii_lowercase();
        let device_type = if ["bot", "crawler", "spider", "curl/", "wget/"]
            .iter()
            .any(|needle| lower.contains(needle))
        {
            DeviceType::Bot
        } else if has("iPad") || has("Tablet") || (has("Android") && !has("Mobile")) {
            DeviceType::Tablet
        } else if has("Mobi") || has("iPhone") || has("Android") {
            DeviceType::Mobile
        } else if has("Windows") || has("Macintosh") || has("X11") || has("CrOS") {
            DeviceType::Desktop
        } else {
            DeviceType::Unknown
        };
        let os = if has("iPhone") || has("iPad") || has("iPod") {
            "iOS"
        } else if has("Android") {
            "Android"
        } else if has("Windows") {
            "Windows"
        } else if has("Macintosh") {
            "macOS"
        } else if has("CrOS") {
            "ChromeOS"
        } else if has("Linux") || has("X11") {
            "Linux"
        } else {
            ""
        };
        let browser = if has("Edg/") {
            "Edge"
        } else if has("OPR/") {
            "Opera"
        } else if has("Firefox/") || has("FxiOS/") {
            "Firefox"
        } else if has("Chrome/") || has("CriOS/") {
            "Chrome"
        } else if has("Safari/") {
            "Safari"
        } else {
            ""
        };
        Self {
            device_type,
            os: os.into(),
            browser: browser.into(),
            ip: peer.map(|ip| ip.to_string()).unwrap_or_default(),
        }
    }
}

/// The device sessions of one save, loaded from and written through to its file.
#[derive(Debug, Clone)]
pub struct Devices {
    save: PathBuf,
    path: PathBuf,
    list: Vec<Device>,
    /// The file lags `list` after a failed write; the next use retries it.
    unsaved: bool,
}

impl Devices {
    /// Load the sessions for `save`. A missing file is empty; an unreadable or
    /// malformed one is an error, so a later write never drops sessions it lost.
    pub fn open(save: &Path) -> std::io::Result<Self> {
        let path = store_path(save);
        let list = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err),
        };
        Ok(Self {
            save: save.to_path_buf(),
            path,
            list,
            unsaved: false,
        })
    }

    pub fn save(&self) -> &Path {
        &self.save
    }

    /// Store a new session and return it with its secret, which exists nowhere
    /// else afterwards. Nothing changes in memory unless the file was written.
    pub fn create(
        &mut self,
        label: Option<&str>,
        client: Client,
        now: u64,
    ) -> std::io::Result<(Device, String)> {
        let secret = crate::b64url::encode(&random::<32>()?);
        let label = label
            .map(|text| {
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_LABEL)
                    .collect::<String>()
            })
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| default_label(&client));
        let device = Device {
            id: crate::b64url::encode(&random::<9>()?),
            label,
            device_type: client.device_type,
            os: client.os,
            browser: client.browser,
            ip: client.ip,
            created_at: now,
            last_used_at: now,
            expires_at: now + LIFETIME_SECS,
            secret_sha256: digest(&secret),
        };
        let mut next: Vec<Device> = self
            .list
            .iter()
            .filter(|known| known.expires_at > now)
            .cloned()
            .collect();
        next.push(device.clone());
        store(&self.path, &next)?;
        self.list = next;
        self.unsaved = false;
        Ok((device, secret))
    }

    /// Live sessions for the operator: metadata only, never the secret or its hash.
    pub fn listing(&self, now: u64) -> Vec<serde_json::Value> {
        self.list
            .iter()
            .filter(|device| device.expires_at > now)
            .map(|device| {
                serde_json::json!({
                    "id": device.id,
                    "label": device.label,
                    "device_type": device.device_type,
                    "os": device.os,
                    "browser": device.browser,
                    "ip": device.ip,
                    "created_at": device.created_at,
                    "last_used_at": device.last_used_at,
                    "expires_at": device.expires_at,
                })
            })
            .collect()
    }

    /// Ids of live sessions (push delivery skips the others).
    pub fn live_ids(&self, now: u64) -> Vec<String> {
        self.list
            .iter()
            .filter(|device| device.expires_at > now)
            .map(|device| device.id.clone())
            .collect()
    }

    /// Whether `id` is still a live session (open sockets check this).
    pub fn holds(&self, id: &str, now: u64) -> bool {
        self.list
            .iter()
            .any(|device| device.id == id && device.expires_at > now)
    }

    /// End one session. `Ok(false)` when there is no such session. Nothing
    /// changes in memory unless the file was written, so a revoke that
    /// returned `Ok` survives a restart.
    pub fn revoke(&mut self, id: &str) -> std::io::Result<bool> {
        if !self.list.iter().any(|device| device.id == id) {
            return Ok(false);
        }
        let next: Vec<Device> = self
            .list
            .iter()
            .filter(|device| device.id != id)
            .cloned()
            .collect();
        store(&self.path, &next)?;
        self.list = next;
        self.unsaved = false;
        Ok(true)
    }

    /// Id of the live session holding `secret`, extended by this use. Expired
    /// sessions are refused and pruned. A failed write keeps the session
    /// usable and is retried on the next call.
    pub fn authenticate(&mut self, secret: &str, now: u64) -> Option<String> {
        let presented = digest(secret);
        let before = self.list.len();
        self.list.retain(|device| device.expires_at > now);
        let mut dirty = self.unsaved || self.list.len() != before;
        let found = self.list.iter_mut().find(|device| {
            crate::serve::eq_secret(device.secret_sha256.as_bytes(), presented.as_bytes())
        });
        let id = found.map(|device| {
            if now.saturating_sub(device.last_used_at) >= TOUCH_SECS {
                device.last_used_at = now;
                device.expires_at = now + LIFETIME_SECS;
                dirty = true;
            }
            device.id.clone()
        });
        if dirty {
            self.unsaved = store(&self.path, &self.list).is_err();
            if self.unsaved {
                crate::diagnostics::log("warn", "device sessions could not be written");
            }
        }
        id
    }
}

/// Sessions file for a save: `<save file>.devices.json` beside it.
pub fn store_path(save: &Path) -> PathBuf {
    let mut name = save.file_name().unwrap_or_default().to_os_string();
    name.push(".devices.json");
    save.with_file_name(name)
}

fn default_label(client: &Client) -> String {
    match (client.browser.as_str(), client.os.as_str()) {
        ("", "") => "Browser".into(),
        (browser, "") => browser.into(),
        ("", os) => os.into(),
        (browser, os) => format!("{browser} on {os}"),
    }
}

fn digest(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn random<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(buf)
}

fn store(path: &Path, list: &[Device]) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(list).map_err(std::io::Error::other)?;
    atomicwrites::AtomicFile::new(path, atomicwrites::OverwriteBehavior::AllowOverwrite)
        .write(|file| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&text)?;
            file.sync_all()
        })
        .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static UNIQUE: AtomicU64 = AtomicU64::new(0);

    fn unique_save() -> PathBuf {
        let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "hoshi-devices-{}-{nanos}-{n}.sqlite",
            std::process::id()
        ))
    }

    const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";

    fn phone() -> Client {
        Client::from_request(Some(IPHONE), Some("192.0.2.7".parse().unwrap()))
    }

    #[test]
    fn metadata_comes_from_user_agent_and_peer() {
        assert_eq!(
            phone(),
            Client {
                device_type: DeviceType::Mobile,
                os: "iOS".into(),
                browser: "Safari".into(),
                ip: "192.0.2.7".into(),
            }
        );
        let tablet = Client::from_request(
            Some(
                "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
            ),
            None,
        );
        assert_eq!(
            (
                tablet.device_type,
                tablet.os.as_str(),
                tablet.browser.as_str()
            ),
            (DeviceType::Tablet, "Android", "Chrome")
        );
        let desktop = Client::from_request(
            Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36 Edg/126.0",
            ),
            None,
        );
        assert_eq!(
            (desktop.device_type, desktop.browser.as_str()),
            (DeviceType::Desktop, "Edge")
        );
        assert_eq!(
            Client::from_request(Some("curl/8.6.0"), None).device_type,
            DeviceType::Bot
        );
        assert_eq!(
            Client::from_request(None, None).device_type,
            DeviceType::Unknown
        );
    }

    #[test]
    fn sessions_survive_reload_and_store_no_secret() {
        let save = unique_save();
        std::fs::write(&save, b"save bytes").unwrap();
        let mut devices = Devices::open(&save).unwrap();
        let (device, secret) = devices.create(Some("Kitchen"), phone(), 1_000).unwrap();
        assert_eq!(device.label, "Kitchen");
        assert_eq!(device.expires_at, 1_000 + LIFETIME_SECS);
        let file = std::fs::read_to_string(store_path(&save)).unwrap();
        assert!(!file.contains(&secret), "plaintext secret on disk");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store_path(&save))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(std::fs::read(&save).unwrap(), b"save bytes");

        let mut reopened = Devices::open(&save).unwrap();
        assert_eq!(reopened.authenticate(&secret, 2_000), Some(device.id));
        assert_eq!(reopened.authenticate("not-a-session", 2_000), None);
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(store_path(&save));
    }

    #[test]
    fn use_extends_and_expiry_refuses_and_prunes() {
        let save = unique_save();
        let mut devices = Devices::open(&save).unwrap();
        let (_, secret) = devices.create(None, phone(), 0).unwrap();
        let (_, other) = devices.create(None, phone(), 0).unwrap();
        let later = LIFETIME_SECS - 1;
        assert!(devices.authenticate(&secret, later).is_some());
        let reopened = Devices::open(&save).unwrap();
        assert_eq!(reopened.list[0].expires_at, later + LIFETIME_SECS);

        let past = LIFETIME_SECS + 1;
        assert_eq!(devices.authenticate(&other, past), None);
        assert!(devices.authenticate(&secret, past).is_some());
        assert_eq!(Devices::open(&save).unwrap().list.len(), 1);
        let _ = std::fs::remove_file(store_path(&save));
    }

    #[test]
    #[cfg(unix)]
    fn failed_write_keeps_the_previous_file_and_memory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = unique_save().with_extension("d");
        std::fs::create_dir(&dir).unwrap();
        let save = dir.join("home.sqlite");
        let mut devices = Devices::open(&save).unwrap();
        let (_, secret) = devices.create(None, phone(), 1_000).unwrap();
        // No temp file can be created, so the rename never happens.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let failed = devices.create(None, phone(), 1_001);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(failed.is_err());
        assert_eq!(devices.list.len(), 1);
        // A refresh that cannot be written is retried once storage recovers.
        let later = 1_000 + 2 * TOUCH_SECS;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        assert!(devices.authenticate(&secret, later).is_some());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(devices.authenticate(&secret, later + 1).is_some());
        assert_eq!(
            Devices::open(&save).unwrap().list[0].expires_at,
            later + LIFETIME_SECS
        );
        let mut reopened = Devices::open(&save).unwrap();
        assert_eq!(reopened.list.len(), 1);
        assert!(reopened.authenticate(&secret, 2_000).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revoke_survives_reload_and_listing_hides_secrets() {
        let save = unique_save();
        let mut devices = Devices::open(&save).unwrap();
        let (kept, kept_secret) = devices.create(None, phone(), 1_000).unwrap();
        let (gone, gone_secret) = devices.create(None, phone(), 1_000).unwrap();
        let listing = serde_json::to_string(&devices.listing(1_000)).unwrap();
        assert!(listing.contains(&kept.id) && listing.contains("Safari on iOS"));
        assert!(!listing.contains(&kept_secret) && !listing.contains(&digest(&kept_secret)));
        assert!(!listing.contains("secret"), "{listing}");

        assert!(devices.revoke(&gone.id).unwrap());
        assert!(!devices.revoke(&gone.id).unwrap());
        assert!(!devices.holds(&gone.id, 1_000));
        let mut reopened = Devices::open(&save).unwrap();
        assert_eq!(reopened.authenticate(&gone_secret, 2_000), None);
        assert_eq!(reopened.authenticate(&kept_secret, 2_000), Some(kept.id));
        let _ = std::fs::remove_file(store_path(&save));
    }

    #[test]
    fn malformed_file_is_an_error_not_an_empty_list() {
        let save = unique_save();
        std::fs::write(store_path(&save), b"{").unwrap();
        assert!(Devices::open(&save).is_err());
        let _ = std::fs::remove_file(store_path(&save));
    }
}
