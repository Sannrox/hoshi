//! Managed installations publish one durable pointer after all verification succeeds.
use crate::update::{self, Release, UpdateError};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Clone)]
pub struct Installed {
    pub active: String,
    pub previous: Option<String>,
}

pub struct Managed {
    pub root: PathBuf,
    _lock: std::fs::File,
    key: [u8; 32],
}

fn invalid(message: impl Into<String>) -> UpdateError {
    UpdateError::Metadata(message.into())
}

fn release_id(version: &str) -> Result<&str, UpdateError> {
    semver::Version::parse(version.trim_start_matches('v'))
        .map_err(|_| invalid("invalid version"))?;
    if version.contains(['/', '\\']) {
        return Err(invalid("invalid version path"));
    }
    Ok(version)
}

impl Managed {
    pub fn open(root: PathBuf, key: [u8; 32]) -> Result<Self, UpdateError> {
        std::fs::create_dir_all(&root)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("update.lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| invalid("another update is already running"))?;
        Ok(Self {
            root,
            _lock: lock,
            key,
        })
    }

    fn publication_lock(&self) -> Result<std::fs::File, UpdateError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("launch.lock"))?;
        file.lock()?;
        Ok(file)
    }

    pub fn installed(&self) -> Result<Installed, UpdateError> {
        let text = std::fs::read_to_string(self.root.join("current"))?;
        let mut lines = text.lines();
        let installed = Installed {
            active: lines
                .next()
                .ok_or_else(|| invalid("empty current pointer"))?
                .into(),
            previous: lines
                .next()
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        };
        if lines.next().is_some() {
            return Err(invalid("invalid current pointer"));
        }
        release_id(&installed.active)?;
        if let Some(previous) = &installed.previous {
            release_id(previous)?;
        }
        Ok(installed)
    }

    fn publish(&self, installed: &Installed) -> Result<(), UpdateError> {
        let text = format!(
            "{}\n{}\n",
            installed.active,
            installed.previous.as_deref().unwrap_or("")
        );
        update::atomic_write(&self.root.join("current"), text.as_bytes())
    }

    pub fn release(&self, id: &str) -> Result<Release, UpdateError> {
        let bytes = std::fs::read(
            self.root
                .join("releases")
                .join(release_id(id)?)
                .join("latest.json"),
        )?;
        let envelope: update::Envelope =
            serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
        envelope.verify(&self.key)
    }

    /// Staging may be abandoned on crash. Active/previous directories are never overwritten.
    pub fn install(
        &self,
        envelope: &update::Envelope,
        asset_source: &str,
    ) -> Result<(), UpdateError> {
        let release = envelope.verify(&self.key)?;
        release_id(&release.version)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| invalid("system clock predates epoch"))?
            .as_nanos();
        let separator = if release.version.contains('+') {
            '.'
        } else {
            '+'
        };
        let id = format!(
            "{}{separator}install.{nonce}.{}",
            release.version,
            std::process::id()
        );
        let installed = if self.root.join("current").exists() {
            Some(self.installed()?)
        } else {
            None
        };
        let staging = self.root.join("staging");
        if staging.exists() {
            std::fs::remove_dir_all(&staging)?;
        }
        std::fs::create_dir_all(&staging)?;
        let binary = staging.join(binary_name());
        download(asset_source, &binary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .open(&binary)?
            .sync_all()?;
        verify_binary(&binary, &release)?;
        let bytes = serde_json::to_vec(envelope).map_err(|error| invalid(error.to_string()))?;
        update::atomic_write(&staging.join("latest.json"), &bytes)?;
        let destination = self.root.join("releases").join(&id);
        std::fs::create_dir_all(self.root.join("releases"))?;
        if destination.exists() {
            return Err(invalid("release generation already exists"));
        }
        let _publication = self.publication_lock()?;
        std::fs::rename(&staging, &destination)?;
        sync_dir(&self.root.join("releases"))?;
        self.publish(&Installed {
            active: id,
            previous: installed.map(|old| old.active),
        })?;
        self.prune()
    }

    pub fn rollback(&self) -> Result<Release, UpdateError> {
        let installed = self.installed()?;
        let previous = installed
            .previous
            .ok_or_else(|| invalid("no previous release"))?;
        let release = self.release(&previous)?;
        verify_binary(
            &self
                .root
                .join("releases")
                .join(&previous)
                .join(binary_name()),
            &release,
        )?;
        let _publication = self.publication_lock()?;
        self.publish(&Installed {
            active: previous,
            previous: Some(installed.active),
        })?;
        Ok(release)
    }

    fn prune(&self) -> Result<(), UpdateError> {
        let installed = self.installed()?;
        for entry in std::fs::read_dir(self.root.join("releases"))? {
            let entry = entry?;
            let name = entry.file_name();
            if name != installed.active.as_str()
                && installed.previous.as_deref() != name.to_str()
                && entry.file_type()?.is_dir()
                && let Err(error) = std::fs::remove_dir_all(entry.path())
            {
                #[cfg(windows)]
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
                ) {
                    eprintln!(
                        "hoshi: old release is still in use; cleanup will retry on a later update"
                    );
                    continue;
                }
                return Err(error.into());
            }
        }
        Ok(())
    }
}

fn verify_binary(binary: &Path, release: &Release) -> Result<(), UpdateError> {
    let asset = release
        .assets
        .get(target())
        .ok_or_else(|| invalid("release has no binary for this target"))?;
    let mut file = std::fs::File::open(binary)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let actual = format!("{:x}", hash.finalize());
    if !actual.eq_ignore_ascii_case(&asset.sha256) {
        return Err(invalid("asset checksum mismatch"));
    }
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success()
        || String::from_utf8_lossy(&output.stdout).trim_end() != release.version_output
    {
        return Err(invalid("downloaded binary reports an unexpected version"));
    }
    Ok(())
}

pub fn root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let releases = executable.parent()?.parent()?;
    if releases.file_name()? == "releases" {
        Some(releases.parent()?.to_owned())
    } else {
        None
    }
}

pub fn installed_version() -> Option<String> {
    let root = root()?;
    let text = std::fs::read_to_string(root.join("current")).ok()?;
    let active = text.lines().next()?;
    release_id(active).ok()?;
    let bytes = std::fs::read(root.join("releases").join(active).join("latest.json")).ok()?;
    let envelope: update::Envelope = serde_json::from_slice(&bytes).ok()?;
    Some(envelope.verify(&update::public_key().ok()?).ok()?.version)
}

pub fn binary_name() -> &'static str {
    if cfg!(windows) { "hoshi.exe" } else { "hoshi" }
}
pub fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => "unsupported",
    }
}

fn download(source: &str, destination: &Path) -> Result<(), UpdateError> {
    if let Some(path) = source.strip_prefix("file://") {
        std::fs::copy(path, destination)?;
    } else {
        if !source.starts_with("https://") {
            return Err(invalid("binary downloads require HTTPS"));
        }
        let status = Command::new("curl")
            .args([
                "--disable",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--max-time",
                "300",
                "--output",
            ])
            .arg(destination)
            .args(["--url", source])
            .stdin(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(invalid("binary download failed"));
        }
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(destination)?
        .sync_all()?;
    Ok(())
}

fn sync_dir(path: &Path) -> Result<(), UpdateError> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Refuse incompatible downgrades before any pointer or package-manager change.
pub fn check_saves(dir: &Path, target_schema: u32) -> Result<Vec<PathBuf>, UpdateError> {
    let mut saves = Vec::new();
    if !dir.exists() {
        return Ok(saves);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if hoshi_core::BackendKind::from_path(&path) != hoshi_core::BackendKind::Sqlite {
            return Err(invalid(
                "back up and check optional-backend saves manually before updating",
            ));
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(invalid(
                "linked entries in the save directory require manual compatibility checks and backups",
            ));
        }
        if !kind.is_file() {
            continue;
        }
        let mut header = [0; 16];
        let mut file = std::fs::File::open(&path)?;
        if file.read(&mut header)? < 16 || &header != b"SQLite format 3\0" {
            continue;
        }
        let schema = hoshi_core::save_schema(&path).map_err(|error| invalid(error.to_string()))?;
        if schema > target_schema {
            return Err(invalid(format!(
                "save schema {schema} exceeds target schema {target_schema}; restore a matching backup first"
            )));
        }
        saves.push(path);
    }
    Ok(saves)
}

pub fn backup_saves(dir: &Path, saves: &[PathBuf], now: u64) -> Result<(), UpdateError> {
    let backups = dir.join("update-backups");
    std::fs::create_dir_all(&backups)?;
    sync_dir(dir)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(backups.join("backup.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| invalid("another save backup is already running"))?;
    let staging = backups.join("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir(&staging)?;
    for save in saves {
        let name = save
            .file_name()
            .ok_or_else(|| invalid("invalid save name"))?;
        hoshi_core::backup_save(save, &staging.join(name))
            .map_err(|error| invalid(error.to_string()))?;
    }
    sync_dir(&staging)?;
    let destination = backups.join(format!("{now}-{}", std::process::id()));
    if destination.exists() {
        return Err(invalid("backup already exists; retry later"));
    }
    std::fs::rename(&staging, destination)?;
    sync_dir(&backups)?;
    let mut retained = std::fs::read_dir(&backups)?.collect::<Result<Vec<_>, _>>()?;
    retained.retain(|entry| {
        entry.file_type().is_ok_and(|kind| kind.is_dir()) && entry.file_name() != "staging"
    });
    retained.sort_by_key(|entry| entry.file_name());
    let prune = retained.len().saturating_sub(3);
    for entry in retained.into_iter().take(prune) {
        std::fs::remove_dir_all(entry.path())?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::BTreeMap;

    fn fixture(dir: &Path, version: &str, key: &SigningKey) -> (update::Envelope, String) {
        let source = dir.join(format!("asset-{version}"));
        let bytes = format!("#!/bin/sh\nprintf 'hoshi {version}\\n'\n");
        std::fs::write(&source, &bytes).unwrap();
        let release = Release {
            version: version.into(),
            channel: "stable".into(),
            version_output: format!("hoshi {version}"),
            published_at: 100,
            note: None,
            save_schema: hoshi_core::SCHEMA_VERSION,
            cli_contract: 2,
            server_contracts: vec!["v1".into()],
            assets: BTreeMap::from([(
                target().into(),
                update::Asset {
                    name: format!("asset-{version}"),
                    sha256: format!("{:x}", Sha256::digest(bytes.as_bytes())),
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
            update::Envelope { payload, signature },
            format!("file://{}", source.display()),
        )
    }

    #[test]
    fn managed_update_verifies_before_swap_retains_previous_and_recovers_staging() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("hoshi-managed-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let root = dir.join("install");
        let key = SigningKey::from_bytes(&[9; 32]);
        let public = key.verifying_key().to_bytes();
        let managed = Managed::open(root.clone(), public).unwrap();
        assert!(
            Managed::open(root.clone(), public)
                .err()
                .unwrap()
                .to_string()
                .contains("another update")
        );
        let (first, first_source) = fixture(&dir, "0.2.0", &key);
        managed.install(&first, &first_source).unwrap();
        let before = std::fs::read(root.join("current")).unwrap();
        let (mut second, second_source) = fixture(&dir, "0.3.0", &key);
        second.signature = "00".repeat(64);
        assert!(managed.install(&second, &second_source).is_err());
        assert_eq!(std::fs::read(root.join("current")).unwrap(), before);
        let (second, second_source) = fixture(&dir, "0.3.0", &key);
        std::fs::write(second_source.strip_prefix("file://").unwrap(), "tampered").unwrap();
        assert!(managed.install(&second, &second_source).is_err());
        assert_eq!(std::fs::read(root.join("current")).unwrap(), before);
        // An abandoned download is harmless and is replaced on the next attempt.
        drop(managed);
        let managed = Managed::open(root.clone(), public).unwrap();
        assert_eq!(std::fs::read(root.join("current")).unwrap(), before);
        let (second, second_source) = fixture(&dir, "0.3.0", &key);
        managed.install(&second, &second_source).unwrap();
        assert_eq!(
            managed
                .release(&managed.installed().unwrap().active)
                .unwrap()
                .version,
            "0.3.0"
        );
        let pointer = std::fs::read(root.join("current")).unwrap();
        let previous = managed.installed().unwrap().previous.unwrap();
        let previous_binary = root.join("releases").join(previous).join(binary_name());
        let original = std::fs::read(&previous_binary).unwrap();
        std::fs::write(&previous_binary, "truncated").unwrap();
        assert!(managed.rollback().is_err());
        assert_eq!(std::fs::read(root.join("current")).unwrap(), pointer);
        std::fs::write(&previous_binary, original).unwrap();
        assert_eq!(managed.rollback().unwrap().version, "0.2.0");
        let (third, third_source) = fixture(&dir, "0.4.0", &key);
        managed.install(&third, &third_source).unwrap();
        assert_eq!(std::fs::read_dir(root.join("releases")).unwrap().count(), 2);
        assert_eq!(managed.rollback().unwrap().version, "0.2.0");
        drop(managed);
        let saves = dir.join("saves");
        std::fs::create_dir(&saves).unwrap();
        let custom = saves.join("empire.save");
        drop(hoshi_core::open_save(&custom).unwrap());
        let uppercase = saves.join("EMPIRE.SQLITE");
        std::fs::copy(&custom, &uppercase).unwrap();
        let discovered = check_saves(&saves, hoshi_core::SCHEMA_VERSION).unwrap();
        assert_eq!(discovered.len(), 2);
        let gate = hoshi_core::lock_save_migrations(&saves).unwrap();
        let new_save = saves.join("new.sqlite");
        assert!(matches!(
            hoshi_core::open_save(&new_save),
            Err(hoshi_core::SaveError::Locked)
        ));
        assert!(!new_save.exists());
        drop(hoshi_core::open_save(&custom).unwrap());
        drop(gate);
        drop(hoshi_core::open_save(&custom).unwrap());
        let linked = saves.join("linked.sqlite");
        std::os::unix::fs::symlink(&custom, &linked).unwrap();
        assert!(check_saves(&saves, hoshi_core::SCHEMA_VERSION).is_err());
        std::fs::remove_file(linked).unwrap();
        assert!(check_saves(&saves, hoshi_core::SCHEMA_VERSION - 1).is_err());
        backup_saves(&saves, &discovered, 100).unwrap();
        let completed = std::fs::read_dir(saves.join("update-backups"))
            .unwrap()
            .map(Result::unwrap)
            .find(|entry| entry.file_type().unwrap().is_dir())
            .unwrap();
        assert_eq!(
            hoshi_core::save_schema(&completed.path().join("empire.save")).unwrap(),
            hoshi_core::SCHEMA_VERSION
        );
        std::fs::write(saves.join("other.MIKURA"), "optional backend").unwrap();
        assert!(check_saves(&saves, hoshi_core::SCHEMA_VERSION).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
