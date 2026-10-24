//! Explicit, confirmed update entry point. Never called from automatic checks.
use crate::{
    managed_update::{self, Managed},
    update::{self, UpdateError},
};
use hoshi_core::{Clock, SystemClock};
use std::{
    io::{self, Write},
    path::PathBuf,
    process::Command,
};

fn invalid(message: impl Into<String>) -> UpdateError {
    UpdateError::Metadata(message.into())
}

pub fn run(args: &[String]) -> Result<(), UpdateError> {
    let mut yes = false;
    let mut force = false;
    let mut rollback = false;
    let mut channel = update::channel().to_owned();
    let mut version = None;
    let mut install_dir = None;
    let mut save_dir = hoshi_storage::default_save_dir();
    let mut endpoint = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--yes" => yes = true,
            "--force" => force = true,
            "--rollback" => rollback = true,
            "--channel" => {
                channel = iter
                    .next()
                    .ok_or_else(|| invalid("--channel needs stable or nightly"))?
                    .clone()
            }
            "--version" => {
                version = Some(
                    iter.next()
                        .ok_or_else(|| invalid("--version needs a version"))?
                        .clone(),
                )
            }
            "--install-dir" => {
                install_dir = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| invalid("--install-dir needs a directory"))?,
                ))
            }
            "--save-dir" => {
                save_dir = PathBuf::from(
                    iter.next()
                        .ok_or_else(|| invalid("--save-dir needs a directory"))?,
                )
            }
            "--endpoint" => {
                endpoint = Some(
                    iter.next()
                        .ok_or_else(|| invalid("--endpoint needs a URL"))?
                        .clone(),
                )
            }
            _ => return Err(invalid(format!("unknown update option {arg}"))),
        }
    }
    if !matches!(channel.as_str(), "stable" | "nightly") {
        return Err(invalid("channel must be stable or nightly"));
    }
    if rollback && (version.is_some() || force || install_dir.is_some()) {
        return Err(invalid(
            "--rollback cannot be combined with a version, --force or --install-dir",
        ));
    }
    if let Some(version) = &version {
        semver::Version::parse(version.trim_start_matches('v'))
            .map_err(|_| invalid("invalid pinned version"))?;
    }
    let bootstrap = install_dir.is_some();
    let root = install_dir.or_else(managed_update::root);
    if rollback {
        let managed = Managed::open(
            root.ok_or_else(|| invalid("rollback requires a managed install"))?,
            update::public_key()?,
        )?;
        let previous = managed
            .installed()?
            .previous
            .ok_or_else(|| invalid("no previous release"))?;
        let release = managed.release(&previous)?;
        managed_update::check_saves(&save_dir, release.save_schema)?;
        confirm(yes, &format!("Roll back to {}?", release.version))?;
        let _save_gate = hoshi_storage::lock_save_migrations(&save_dir)
            .map_err(|error| invalid(error.to_string()))?;
        managed_update::check_saves(&save_dir, release.save_schema)?;
        managed.rollback()?;
        println!("Restored hoshi {}", release.version);
        return Ok(());
    }
    let config = crate::config::UserConfig::load(&save_dir);
    let endpoint = endpoint.unwrap_or_else(|| {
        if !config.update_endpoint.is_empty() {
            return config.update_endpoint.clone();
        }
        if let Some(version) = &version {
            format!(
                "https://raw.githubusercontent.com/Sannrox/hoshi/update-metadata/versions/v{}/latest.json",
                version.trim_start_matches('v')
            )
        } else {
            update::metadata_endpoint(&channel)
        }
    });
    let key = update::public_key()?;
    let mut result = update::fetch_metadata(&endpoint, &user_agent());
    for _ in 0..2 {
        if result.is_ok() {
            break;
        }
        result = update::fetch_metadata(&endpoint, &user_agent());
    }
    let envelope = result?;
    let release = envelope.verify(&key)?;
    if release.channel != channel {
        return Err(invalid("metadata channel does not match requested channel"));
    }
    if let Some(want) = &version
        && release.version.trim_start_matches('v') != want.trim_start_matches('v')
    {
        return Err(invalid("metadata version does not match requested version"));
    }
    let current =
        managed_update::installed_version().unwrap_or_else(|| update::running_version().into());
    if !bootstrap
        && !force
        && !update::newer(&release.version, &current)
        && version.is_none()
        && channel == update::channel()
    {
        println!("already up to date (v{current})");
        return Ok(());
    }
    managed_update::check_saves(&save_dir, release.save_schema)?;
    println!("hoshi {current} -> {}", release.version);
    if let Some(note) = &release.note {
        println!("{note}");
    }
    let migrates = release.save_schema > hoshi_storage::SCHEMA_VERSION;
    if migrates {
        println!(
            "This release migrates saves to schema {}. Older builds may refuse them. A backup will be kept before installation.",
            release.save_schema
        );
    }
    if let Some(root) = root {
        let managed = Managed::open(root, key)?;
        if !bootstrap
            && !force
            && version.is_none()
            && channel == update::channel()
            && managed.root.join("current").exists()
        {
            let installed = managed.release(&managed.installed()?.active)?;
            if !update::newer(&release.version, &installed.version) {
                println!("already up to date (v{})", installed.version);
                return Ok(());
            }
        }
        confirm(yes, "Install this release?")?;
        let _save_gate = hoshi_storage::lock_save_migrations(&save_dir)
            .map_err(|error| invalid(error.to_string()))?;
        let saves = managed_update::check_saves(&save_dir, release.save_schema)?;
        managed_update::backup_saves(&save_dir, &saves, SystemClock.now_unix())?;
        let asset = release
            .assets
            .get(managed_update::target())
            .ok_or_else(|| invalid("no release for this target"))?;
        let source = if let Some(path) = endpoint.strip_prefix("file://") {
            format!(
                "file://{}",
                PathBuf::from(path)
                    .parent()
                    .ok_or_else(|| invalid("invalid local endpoint"))?
                    .join(&asset.name)
                    .display()
            )
        } else {
            let tag = format!("v{}", release.version.trim_start_matches('v'));
            format!(
                "https://github.com/Sannrox/hoshi/releases/download/{tag}/{}",
                asset.name
            )
        };
        managed.install(&envelope, &source)?;
        launcher(&managed)?;
        println!(
            "Installed hoshi {}. Launcher: {}",
            release.version,
            managed.root.join("bin/hoshi").display()
        );
        return Ok(());
    }
    let executable = std::env::current_exe()?;
    let path = executable.to_string_lossy();
    let tag = format!("v{}", release.version.trim_start_matches('v'));
    let command = if path.contains("/Cellar/") || path.contains("/homebrew/") {
        vec!["brew", "upgrade", "hoshi"]
    } else if executable
        .parent()
        .is_some_and(|parent| parent.ends_with(".cargo/bin"))
    {
        vec![
            "cargo",
            "install",
            "--git",
            "https://github.com/Sannrox/hoshi",
            "--locked",
            "--tag",
            tag.as_str(),
            "hoshi",
        ]
    } else if path == "/usr/bin/hoshi" {
        println!(
            "Run: curl -fsSL https://raw.githubusercontent.com/Sannrox/hoshi/main/scripts/install.sh | sh"
        );
        return Err(invalid(
            "package installation requires administrator action",
        ));
    } else {
        println!("Running binary: {}", executable.display());
        println!("For a managed install: hoshi update --install-dir <data-dir>/hoshi");
        return Err(invalid(
            "unknown installation method; use the original package manager",
        ));
    };
    if force || version.is_some() || channel != update::channel() {
        return Err(invalid(
            "pinning, channel changes and --force require a managed installation; use your package manager directly",
        ));
    }
    confirm(yes, &format!("Run {}?", command.join(" ")))?;
    let _save_gate = hoshi_storage::lock_save_migrations(&save_dir)
        .map_err(|error| invalid(error.to_string()))?;
    let saves = managed_update::check_saves(&save_dir, release.save_schema)?;
    managed_update::backup_saves(&save_dir, &saves, SystemClock.now_unix())?;
    if command[0] == "brew" {
        let info = Command::new("brew")
            .args(["info", "--json=v2", "hoshi"])
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .output()?;
        let value: serde_json::Value = serde_json::from_slice(&info.stdout)
            .map_err(|_| invalid("cannot inspect package-manager target"))?;
        if !info.status.success()
            || value["formulae"][0]["versions"]["stable"].as_str()
                != Some(release.version.trim_start_matches('v'))
        {
            return Err(invalid(
                "package-manager target differs from the verified release; refresh it and retry",
            ));
        }
    }
    let mut process = Command::new(command[0]);
    process.args(&command[1..]);
    if command[0] == "brew" {
        // Use the inspected local formula; do not refresh the tap during mutation.
        process.env("HOMEBREW_NO_AUTO_UPDATE", "1");
    }
    if command[0] == "cargo" {
        process.env("HOSHI_RELEASE_VERSION", &release.version);
        process.env_remove("HOSHI_BUILD");
        let prefix = format!("hoshi {} (", release.version);
        if let Some(build) = release
            .version_output
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(')'))
        {
            process.env("HOSHI_BUILD", build);
        }
        process.env(
            "HOSHI_RELEASE_PUBLIC_KEY",
            key.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        );
    }
    if !process.status()?.success() {
        println!("Run manually: {}", command.join(" "));
        return Err(invalid("package manager failed"));
    }
    let installed_binary = if command[0] == "brew" {
        let prefix = Command::new("brew").args(["--prefix", "hoshi"]).output()?;
        if !prefix.status.success() {
            return Err(invalid(
                "cannot locate updated package; check brew --prefix hoshi",
            ));
        }
        PathBuf::from(String::from_utf8_lossy(&prefix.stdout).trim()).join("bin/hoshi")
    } else {
        executable
    };
    let actual = Command::new(installed_binary).arg("--version").output()?;
    if !actual.status.success()
        || String::from_utf8_lossy(&actual.stdout).trim_end() != release.version_output
    {
        return Err(invalid(
            "package manager installed a different build; check hoshi --version before opening saves",
        ));
    }
    Ok(())
}

fn user_agent() -> String {
    format!(
        "hoshi/{} ({}; {})",
        update::running_version(),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn confirm(yes: bool, question: &str) -> Result<(), UpdateError> {
    if yes {
        return Ok(());
    }
    print!("{question} [y/N] ");
    io::stdout().flush()?;
    let mut response = String::new();
    io::stdin().read_line(&mut response)?;
    if response.trim().eq_ignore_ascii_case("y") || response.trim().eq_ignore_ascii_case("yes") {
        Ok(())
    } else {
        Err(invalid("update cancelled"))
    }
}

fn launcher(managed: &Managed) -> Result<(), UpdateError> {
    let root = &managed.root;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(root.join("bin"))?;
        let active_binary = root
            .join("releases")
            .join(managed.installed()?.active)
            .join("hoshi");
        atomicwrites::AtomicFile::new(
            root.join("bin/hoshi"),
            atomicwrites::OverwriteBehavior::AllowOverwrite,
        )
        .write(|file| -> std::io::Result<()> {
            file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
            let mut source = std::fs::File::open(&active_binary)?;
            std::io::copy(&mut source, file)?;
            Ok(())
        })
        .map_err(|error| UpdateError::Io(std::io::Error::other(error.to_string())))?;
    }
    #[cfg(windows)]
    {
        update::atomic_write(&root.join("bin/hoshi.cmd"), b"@echo off\r\n:launch\r\nset /p version=<\"%~dp0..\\current\"\r\n\"%~dp0..\\releases\\%version%\\hoshi.exe\" %*\r\nset status=%errorlevel%\r\nif not %status%==9009 exit /b %status%\r\nif exist \"%~dp0..\\releases\\%version%\\hoshi.exe\" exit /b %status%\r\nset /p next=<\"%~dp0..\\current\"\r\nif not \"%next%\"==\"%version%\" goto launch\r\nexit /b %status%\r\n")?;
    }
    Ok(())
}

/// The native launcher holds the installer lock only until exec resolves the
/// selected pathname. Close-on-exec releases it without changing signal state.
#[cfg(unix)]
pub fn launch_managed_if_needed() -> Result<(), UpdateError> {
    use std::os::unix::process::CommandExt;
    let executable = std::env::current_exe()?;
    let Some(bin) = executable
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "bin"))
    else {
        return Ok(());
    };
    let Some(root) = bin.parent().filter(|path| path.join("current").is_file()) else {
        return Ok(());
    };
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("launch.lock"))?;
    lock.lock_shared()?;
    let pointer = std::fs::read_to_string(root.join("current"))?;
    let generation = pointer
        .lines()
        .next()
        .ok_or_else(|| invalid("empty managed pointer"))?;
    semver::Version::parse(generation).map_err(|_| invalid("invalid managed pointer"))?;
    if generation.contains(['/', '\\']) {
        return Err(invalid("invalid managed pointer"));
    }
    let error = Command::new(root.join("releases").join(generation).join("hoshi"))
        .args(std::env::args_os().skip(1))
        .exec();
    Err(error.into())
}
