//! Signed release metadata and restart-safe update state.

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_METADATA_BYTES: u64 = 65_536;
const CHECK_INTERVAL: u64 = 86_400;

#[derive(Debug)]
pub enum UpdateError {
    Io(std::io::Error),
    Metadata(String),
    Signature,
    Transport(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Metadata(error) => write!(f, "update metadata: {error}"),
            Self::Signature => write!(f, "release signature verification failed"),
            Self::Transport(error) => write!(f, "update request: {error}"),
        }
    }
}
impl std::error::Error for UpdateError {}
impl From<std::io::Error> for UpdateError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub channel: String,
    pub version_output: String,
    pub published_at: u64,
    pub note: Option<String>,
    pub save_schema: u32,
    pub cli_contract: u32,
    pub server_contracts: Vec<String>,
    pub assets: BTreeMap<String, Asset>,
}

/// The signature covers the exact UTF-8 payload bytes, avoiding JSON reserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub payload: String,
    pub signature: String,
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], UpdateError> {
    if value.len() != N * 2 {
        return Err(UpdateError::Metadata("invalid hex length".into()));
    }
    let mut bytes = [0; N];
    for (slot, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let text =
            std::str::from_utf8(pair).map_err(|_| UpdateError::Metadata("invalid hex".into()))?;
        *slot = u8::from_str_radix(text, 16)
            .map_err(|_| UpdateError::Metadata("invalid hex".into()))?;
    }
    Ok(bytes)
}

impl Envelope {
    pub fn verify(&self, public_key: &[u8; 32]) -> Result<Release, UpdateError> {
        if self.payload.len() as u64 > MAX_METADATA_BYTES {
            return Err(UpdateError::Metadata("payload too large".into()));
        }
        let key = VerifyingKey::from_bytes(public_key).map_err(|_| UpdateError::Signature)?;
        let signature = Signature::from_bytes(&decode_hex::<64>(&self.signature)?);
        key.verify_strict(self.payload.as_bytes(), &signature)
            .map_err(|_| UpdateError::Signature)?;
        let release: Release = serde_json::from_str(&self.payload)
            .map_err(|error| UpdateError::Metadata(error.to_string()))?;
        if !matches!(release.channel.as_str(), "stable" | "nightly")
            || semver::Version::parse(release.version.trim_start_matches('v')).is_err()
            || release.save_schema == 0
            || release.cli_contract == 0
            || release.server_contracts.is_empty()
        {
            return Err(UpdateError::Metadata("invalid release fields".into()));
        }
        for asset in release.assets.values() {
            if asset.name.is_empty()
                || asset.name == "."
                || asset.name == ".."
                || asset.name.contains(['/', '\\'])
            {
                return Err(UpdateError::Metadata("asset must be a file name".into()));
            }
            decode_hex::<32>(&asset.sha256)?;
        }
        Ok(release)
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), UpdateError> {
    let parent = path
        .parent()
        .ok_or_else(|| UpdateError::Metadata("missing state directory".into()))?;
    std::fs::create_dir_all(parent)?;
    atomicwrites::AtomicFile::new(path, atomicwrites::OverwriteBehavior::AllowOverwrite)
        .write(|file| file.write_all(bytes))
        .map_err(|error| UpdateError::Io(std::io::Error::other(error.to_string())))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckCache {
    pub checked_at: Option<u64>,
    pub checked_channel: Option<String>,
    pub envelope: Option<Envelope>,
    pub dismissed_version: Option<String>,
    pub last_seen_version: Option<String>,
    pub disclosure_seen: bool,
    pub warned_unknown_config: Option<String>,
}

impl CheckCache {
    pub fn load(dir: &Path) -> Result<Self, UpdateError> {
        match std::fs::read(dir.join("update-state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| UpdateError::Metadata(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn save(&self, dir: &Path) -> Result<(), UpdateError> {
        let bytes =
            serde_json::to_vec(self).map_err(|error| UpdateError::Metadata(error.to_string()))?;
        atomic_write(&dir.join("update-state.json"), &bytes)
    }
    /// Mutate the latest state under a short process-shared lock. Networking never
    /// holds this lock, so dismissals cannot be lost to an in-flight response.
    pub fn modify(dir: &Path, change: impl FnOnce(&mut Self)) -> Result<Self, UpdateError> {
        use fs2::FileExt;
        std::fs::create_dir_all(dir)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("update-state.lock"))?;
        lock.lock_exclusive()?;
        let mut cache = Self::load(dir)?;
        change(&mut cache);
        cache.save(dir)?;
        Ok(cache)
    }

    pub fn due(&self, now: u64) -> bool {
        self.checked_at
            .is_none_or(|at| now.saturating_sub(at) >= CHECK_INTERVAL)
    }
}

/// One bounded GET. Redirects and user curl configuration are disabled.
pub fn fetch_metadata(endpoint: &str, user_agent: &str) -> Result<Envelope, UpdateError> {
    let bytes = if let Some(path) = endpoint.strip_prefix("file://") {
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(MAX_METADATA_BYTES + 1).read_to_end(&mut bytes)?;
        bytes
    } else {
        if !endpoint.starts_with("https://") && !endpoint.starts_with("http://") {
            return Err(UpdateError::Metadata(
                "endpoint must be HTTP(S) or file://".into(),
            ));
        }
        let mut child = Command::new("curl")
            .args([
                "--disable",
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                "10",
                "--connect-timeout",
                "10",
                "--proto",
                "=https,http",
                "--user-agent",
                user_agent,
                "--header",
                "Accept:",
                "--url",
                endpoint,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut bytes = Vec::new();
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| UpdateError::Transport("no response stream".into()))?;
        let read = (&mut stdout)
            .take(MAX_METADATA_BYTES + 1)
            .read_to_end(&mut bytes);
        if read.is_err() || bytes.len() as u64 > MAX_METADATA_BYTES {
            let _ = child.kill();
        }
        let output = child.wait_with_output()?;
        read?;
        if !output.status.success() {
            return Err(UpdateError::Transport(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        bytes
    };
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(UpdateError::Metadata("response too large".into()));
    }
    serde_json::from_slice(&bytes).map_err(|error| UpdateError::Metadata(error.to_string()))
}

pub fn metadata_endpoint(channel: &str) -> String {
    format!("https://raw.githubusercontent.com/Sannrox/hoshi/update-metadata/{channel}/latest.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed_release() -> (Envelope, [u8; 32]) {
        let key = SigningKey::from_bytes(&[7; 32]);
        let release = Release {
            version: "0.2.0".into(),
            channel: "stable".into(),
            version_output: "hoshi 0.2.0".into(),
            published_at: 10,
            note: Some("Back up saves before updating.".into()),
            save_schema: 46,
            cli_contract: 2,
            server_contracts: vec!["v1".into()],
            assets: BTreeMap::from([(
                "aarch64-apple-darwin".into(),
                Asset {
                    name: "hoshi-aarch64-apple-darwin".into(),
                    sha256: "ab".repeat(32),
                },
            )]),
        };
        let payload = serde_json::to_string(&release).unwrap();
        let signature = key
            .sign(payload.as_bytes())
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        (
            Envelope { payload, signature },
            key.verifying_key().to_bytes(),
        )
    }

    #[test]
    fn background_check_returns_before_response_and_caches_one_get() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        let dir = std::env::temp_dir().join(format!(
            "hoshi-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/latest.json", listener.local_addr().unwrap());
        let (envelope, key) = signed_release();
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let (respond_tx, respond_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            request_tx
                .send(String::from_utf8(request).unwrap())
                .unwrap();
            respond_rx
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let settings = CheckSettings {
            enabled: true,
            offline: false,
            endpoint,
        };
        let session = CheckSession::start(dir.clone(), settings.clone(), key, 100);
        // The session is usable while the server is deliberately withholding its response.
        assert!(session.release().is_none());
        let request = request_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert!(request.starts_with("GET /latest.json HTTP/1.1\r\n"));
        assert!(request.contains(&format!(
            "User-Agent: hoshi/{} ({}; {})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
        assert_eq!(request.lines().filter(|line| line.contains(':')).count(), 2);
        CheckCache::modify(&dir, |cache| cache.dismissed_version = Some("0.2.0".into())).unwrap();
        // A concurrent process sees the durable attempt before the GET finishes.
        assert!(check_once(&dir, &settings, &key, 100).unwrap().is_none());
        respond_tx.send(()).unwrap();
        server.join().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while session.release().is_none() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(session.release().unwrap().version, "0.2.0");
        assert_eq!(
            CheckCache::load(&dir).unwrap().dismissed_version.as_deref(),
            Some("0.2.0")
        );
        // Listener has closed: a second GET would fail instead of returning the cache.
        assert_eq!(
            check_once(&dir, &settings, &key, 101)
                .unwrap()
                .unwrap()
                .version,
            "0.2.0"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn release_log_selects_only_versions_between_installed_builds() {
        let text = "# Changelog\n## Unreleased\nfuture\n## [0.4.0]\nlater\n## 0.3.0\nthree\n## 0.2.0\ntwo\n## 0.1.0\none\n";
        assert_eq!(
            changelog_since(text, "0.1.0", "0.3.0"),
            "## 0.3.0\nthree\n## 0.2.0\ntwo\n"
        );
        assert!(!newer("0.2.0-nightly", "0.2.0"));
    }

    #[test]
    fn release_signature_rejects_tampering_and_foreign_keys() {
        let (mut envelope, key) = signed_release();
        assert_eq!(envelope.verify(&key).unwrap().version, "0.2.0");
        assert!(
            envelope
                .verify(&SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes())
                .is_err()
        );
        envelope.payload = envelope.payload.replace("0.2.0", "0.3.0");
        assert!(envelope.verify(&key).is_err());
    }

    #[test]
    fn update_cache_preserves_release_and_user_choices_across_restart() {
        let dir = std::env::temp_dir().join(format!(
            "hoshi-update-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let (envelope, key) = signed_release();
        let cache = CheckCache {
            checked_at: Some(100),
            checked_channel: Some("stable".into()),
            envelope: Some(envelope),
            dismissed_version: Some("0.2.0".into()),
            last_seen_version: Some("0.1.0".into()),
            disclosure_seen: true,
            warned_unknown_config: None,
        };
        cache.save(&dir).unwrap();
        let loaded = CheckCache::load(&dir).unwrap();
        assert!(!loaded.due(100 + CHECK_INTERVAL - 1));
        assert!(loaded.due(100 + CHECK_INTERVAL));
        assert!(!loaded.due(99));
        assert_eq!(
            loaded.envelope.unwrap().verify(&key).unwrap().version,
            "0.2.0"
        );
        assert_eq!(loaded.dismissed_version.as_deref(), Some("0.2.0"));
        assert_eq!(loaded.last_seen_version.as_deref(), Some("0.1.0"));
        assert!(loaded.disclosure_seen);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[derive(Debug, Clone)]
pub struct CheckSettings {
    pub enabled: bool,
    pub offline: bool,
    pub endpoint: String,
}

/// Serialize attempts across processes and persist the attempt before networking.
/// A crash can postpone an optional check, but cannot create a request storm.
pub fn check_once(
    dir: &Path,
    settings: &CheckSettings,
    key: &[u8; 32],
    now: u64,
) -> Result<Option<Release>, UpdateError> {
    use fs2::FileExt;
    let cached = CheckCache::load(dir)?;
    let previous = cached
        .envelope
        .as_ref()
        .and_then(|envelope| envelope.verify(key).ok())
        .filter(|release| release.channel == channel());
    let channel_due = cached.checked_channel.as_deref() != Some(channel()) || cached.due(now);
    if !settings.enabled || settings.offline || !channel_due {
        return Ok(previous);
    }
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("update-check.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(previous);
    }
    let cache = CheckCache::load(dir)?;
    if cache.checked_channel.as_deref() == Some(channel()) && !cache.due(now) {
        return Ok(cache
            .envelope
            .as_ref()
            .and_then(|envelope| envelope.verify(key).ok()));
    }
    CheckCache::modify(dir, |cache| {
        cache.checked_at = Some(now);
        cache.checked_channel = Some(channel().into());
    })?;
    let user_agent = format!(
        "hoshi/{} ({}; {})",
        running_version(),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let envelope = fetch_metadata(&settings.endpoint, &user_agent)?;
    let release = envelope.verify(key)?;
    if release.channel != channel() {
        return Err(UpdateError::Metadata(
            "metadata channel does not match this build".into(),
        ));
    }
    if previous
        .as_ref()
        .is_none_or(|old| newer(&release.version, &old.version) || release.version == old.version)
    {
        CheckCache::modify(dir, |cache| cache.envelope = Some(envelope))?;
        Ok(Some(release))
    } else {
        Ok(previous)
    }
}

#[derive(Clone)]
pub struct CheckSession {
    result: std::sync::Arc<std::sync::Mutex<Option<Release>>>,
}

impl CheckSession {
    pub fn start(dir: PathBuf, settings: CheckSettings, key: [u8; 32], now: u64) -> Self {
        let cached = CheckCache::load(&dir)
            .ok()
            .and_then(|cache| cache.envelope)
            .and_then(|envelope| envelope.verify(&key).ok());
        let result = std::sync::Arc::new(std::sync::Mutex::new(cached));
        let worker_result = std::sync::Arc::clone(&result);
        if settings.enabled && !settings.offline {
            std::thread::spawn(move || match check_once(&dir, &settings, &key, now) {
                Ok(release) => {
                    if let Ok(mut slot) = worker_result.lock() {
                        *slot = release;
                    }
                }
                Err(error) => {
                    if std::env::var_os("HOSHI_DEBUG").is_some() {
                        eprintln!("hoshi: {error}");
                    }
                }
            });
        }
        Self { result }
    }

    pub fn release(&self) -> Option<Release> {
        self.result.lock().ok().and_then(|slot| slot.clone())
    }
}

pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

pub fn channel() -> &'static str {
    if option_env!("HOSHI_BUILD").is_some_and(|build| build.contains("nightly")) {
        "nightly"
    } else {
        "stable"
    }
}

pub fn public_key() -> Result<[u8; 32], UpdateError> {
    decode_hex(option_env!("HOSHI_RELEASE_PUBLIC_KEY").ok_or_else(|| {
        UpdateError::Metadata("this build has no release verification key".into())
    })?)
}

pub fn newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => false,
    }
}

pub fn changelog_since(text: &str, previous: &str, current: &str) -> String {
    let mut include = false;
    let mut result = String::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let version = heading
                .trim_start_matches('[')
                .split([']', ' '])
                .next()
                .unwrap_or("");
            include = newer(version, previous) && !newer(version, current);
        }
        if include {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

pub fn offline(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--offline")
        || std::env::var("HOSHI_OFFLINE").as_deref() == Ok("1")
}

pub fn settings(config: &crate::config::UserConfig, args: &[String]) -> CheckSettings {
    CheckSettings {
        enabled: config.update_check
            && std::env::var("HOSHI_SKIP_VERSION_CHECK").as_deref() != Ok("1"),
        offline: offline(args),
        endpoint: if config.update_endpoint.is_empty() {
            metadata_endpoint(channel())
        } else {
            config.update_endpoint.clone()
        },
    }
}

pub fn running_version() -> &'static str {
    option_env!("HOSHI_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub fn version_body(latest: Option<Release>) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "schema_version": crate::cli::SCHEMA_VERSION,
        "version": running_version(),
        "channel": channel(),
        "save_schema": hoshi_core::SCHEMA_VERSION,
        "cli_contract": crate::cli::SCHEMA_VERSION,
        "contract_versions": crate::serve::CONTRACT_VERSIONS,
        "installed_version": crate::managed_update::installed_version().filter(|version| version != running_version()),
        "latest": latest.filter(|release| release.channel == channel() && newer(&release.version, running_version())),
    })
}
