//! Local report collection is read-only. Publication uses a new file after consent.
use crate::redact::Redactor;
use hoshi_core::{Clock, SystemClock};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct Report {
    pub markdown: String,
    files: Vec<(String, Vec<u8>)>,
    pub includes_save: bool,
}

pub fn build(
    save: &Path,
    include_save: bool,
    log_lines: usize,
    balance: Option<&Path>,
    probe_server: bool,
    config_dir: Option<&Path>,
) -> Result<Report, String> {
    let dir = save
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let config_dir = config_dir.unwrap_or(dir);
    let redactor = Redactor::for_dir(config_dir);
    let mut files = Vec::new();
    let mut counts = std::collections::BTreeMap::new();
    let mut text = format!(
        "# hoshi support report\n\n## Build\n\nVersion: {}\nCommit: {}\nChannel: {}\nInstall method: {}\nCLI contract: {}\nSave schema supported: {}\nUpdate status: {}\n\n",
        crate::update::running_version(),
        env!("HOSHI_COMMIT"),
        crate::update::channel(),
        install_method(),
        crate::cli::SCHEMA_VERSION,
        hoshi_storage::SCHEMA_VERSION,
        update_status(dir)
    );
    let size = ratatui::crossterm::terminal::size()
        .map(|(w, h)| format!("{w} x {h}"))
        .unwrap_or_else(|_| "unavailable".into());
    let depth = if std::env::var_os("NO_COLOR").is_some() {
        "none"
    } else if std::env::var("COLORTERM")
        .is_ok_and(|value| value.contains("truecolor") || value == "24bit")
    {
        "truecolor"
    } else if std::env::var("TERM").is_ok_and(|value| value.contains("256color")) {
        "256"
    } else {
        "16"
    };
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "unknown".into());
    text.push_str(&format!("## Environment\n\nOS: {}\nArchitecture: {}\nTerminal: {size}\nColour depth: {depth}\nNO_COLOR: {}\nUnicode: {}\nImage protocol: {}\nLocale: {locale}\n\n", std::env::consts::OS, std::env::consts::ARCH, std::env::var_os("NO_COLOR").is_some(), locale != "C" && locale != "POSIX", std::env::var("HOSHI_PORTRAIT_PROTOCOL").unwrap_or_else(|_| "auto".into())));
    let config = read_text(&config_dir.join("config.toml"))?;
    let clean_config = redactor.redact_config(&config);
    for (rule, count) in clean_config.counts {
        *counts.entry(rule).or_insert(0) += count;
    }
    files.push(("config.txt".into(), clean_config.text.as_bytes().to_vec()));
    text.push_str(&format!(
        "## Config\n\n```toml\n{}\n```\n\nEnvironment variables set (values excluded):\n",
        clean_config.text
    ));
    let mut names: Vec<_> = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| key.starts_with("HOSHI_"))
        .collect();
    names.sort();
    for name in names {
        text.push_str(&format!("- {name}: set, value excluded\n"));
    }
    text.push_str("\n## Save\n\nPath: <save>\n");
    if save.is_file() {
        let size = save.metadata().map_err(|error| error.to_string())?.len();
        match hoshi_storage::inspect_diagnostics(save, 0, false)
            .and_then(|diagnostics| diagnostics.glance)
        {
            Ok(glance) => {
                text.push_str(&format!("Size: {size} bytes\nSchema: {}\nLast tick: {}\nPlanets: {}\nFleet strength: {}\nGenerator version: {}\nSpecies: {}\nSeasonal: {}\nRivals: {}\n", glance.schema, glance.last_tick_unix, glance.planets, glance.fleet_strength, glance.generator_version, glance.species_id.as_deref().unwrap_or("none"), glance.seasonal, glance.rivals_enabled));
            }
            Err(error) => text.push_str(&format!("Summary unavailable: {error}\n")),
        }
        match hoshi_storage::inspect_diagnostics(save, log_lines, include_save) {
            Ok(diagnostics) => {
                if let Some(bytes) = diagnostics.sqlite_snapshot {
                    text.push_str(&format!(
                        "SHA-256 (attached snapshot): {}\n",
                        digest(&bytes)
                    ));
                    files.push(("save.sqlite".into(), bytes));
                } else {
                    text.push_str(&format!("SHA-256 (save file): {}\n", hash_file(save)?));
                }
                text.push_str("\n## Event tail\n\n");
                for event in diagnostics.events {
                    text.push_str(&format!(
                        "{} {} {}: {}\n",
                        event.at, event.kind, event.action, event.outcome
                    ));
                }
            }
            Err(error) if include_save => return Err(format!("cannot include save: {error}")),
            Err(error) => {
                text.push_str(&format!("Event tail unavailable: {error}\n"));
                if let Ok(hash) = hash_file(save) {
                    text.push_str(&format!("SHA-256 (save file): {hash}\n"));
                }
            }
        }
    } else {
        if include_save {
            return Err("cannot include save: no readable SQLite save selected".into());
        }
        text.push_str("No readable save selected.\n");
    }
    text.push_str("\n## Logs\n\n");
    for name in ["hoshi.log", "serve.log"] {
        let mut combined = String::new();
        for suffix in (1..5).rev() {
            combined.push_str(&read_text(
                &dir.join("logs").join(format!("{name}.{suffix}")),
            )?);
        }
        combined.push_str(&read_text(&dir.join("logs").join(name))?);
        let lines: Vec<_> = combined.lines().rev().take(log_lines.min(10_000)).collect();
        let tail = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
        let clean = redactor.redact(&tail);
        for (rule, count) in clean.counts {
            *counts.entry(rule).or_insert(0) += count;
        }
        let clean = clean.text;
        text.push_str(&format!("### {name}\n\n```text\n{clean}\n```\n\n"));
        files.push((name.into(), clean.into_bytes()));
    }
    text.push_str("## Crash files from the last 7 days\n\n");
    let now = SystemClock.now_unix();
    let mut crashes = match std::fs::read_dir(dir.join("crashes")) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.to_string()),
    };
    crashes.sort_by_key(|entry| entry.file_name());
    for (index, entry) in crashes.into_iter().rev().take(100).enumerate() {
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|time| time.as_secs())
            .unwrap_or(0);
        if now.saturating_sub(modified) > 7 * 86_400 {
            continue;
        }
        let clean = redactor.redact(&read_text(&entry.path())?);
        for (rule, count) in clean.counts {
            *counts.entry(rule).or_insert(0) += count;
        }
        let clean = clean.text;
        text.push_str(&format!("```text\n{clean}\n```\n\n"));
        files.push((format!("crashes/crash-{index}.txt"), clean.into_bytes()));
    }
    let balance_bytes = if let Some(path) = balance {
        std::fs::read(path).map_err(|error| error.to_string())?
    } else {
        include_bytes!("../../hoshi-core/data/balance.toml").to_vec()
    };
    text.push_str(&format!(
        "## Balance\n\nBundled default: {}\nSHA-256: {}\n\n## Server\n\nContracts supported: v1\n",
        balance.is_none(),
        digest(&balance_bytes)
    ));
    if probe_server {
        text.push_str(&configured_health(config_dir));
    }
    if include_save {
        text.push_str(
            "\nWARNING: The archive includes the save, its galaxy seed, and world state.\n",
        );
    } else {
        text.push_str("\nSave contents and galaxy seed are excluded.\n");
    }
    let mut redacted = redactor.redact(&text);
    redacted.text.push_str("\n## Redaction rules fired\n\n");
    for (rule, count) in redacted.counts {
        *counts.entry(rule).or_insert(0) += count;
    }
    for (name, count) in counts {
        redacted.text.push_str(&format!("- {name}: {count}\n"));
    }
    Ok(Report {
        markdown: redacted.text,
        files,
        includes_save: include_save,
    })
}

// Only an explicitly configured numeric loopback address can be probed. No DNS
// resolution or credential-bearing request is part of local report collection.
fn configured_health(dir: &Path) -> String {
    let bind = crate::config::UserConfig::load(dir).serve_bind;
    if bind.is_empty() {
        return String::new();
    }
    let Ok(address) = bind.parse::<std::net::SocketAddr>() else {
        return "Configured health: not probed (numeric loopback address required).\n".into();
    };
    if !address.ip().is_loopback() {
        return "Configured health: not probed (loopback required).\n".into();
    }
    let timeout = std::time::Duration::from_millis(250);
    let response = (|| -> Result<serde_json::Value, String> {
        let mut stream =
            std::net::TcpStream::connect_timeout(&address, timeout).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        let deadline = std::time::Instant::now() + timeout;
        let mut buffer = [0; 2048];
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or("health response timed out")?;
            stream
                .set_read_timeout(Some(remaining))
                .map_err(|e| e.to_string())?;
            let count = stream.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            if bytes.len() + count > 16_384 {
                return Err("health response too large".into());
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let response = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        let (headers, body) = response
            .split_once("\r\n\r\n")
            .ok_or("invalid health response")?;
        if !headers
            .lines()
            .next()
            .is_some_and(|line| line.split_whitespace().nth(1) == Some("200"))
        {
            return Err("health endpoint returned an unsuccessful status".into());
        }
        serde_json::from_str(body).map_err(|e| e.to_string())
    })();
    match response {
        Ok(value) => {
            let version = value.get("version").and_then(|v| v.as_str());
            let mismatch = version.map(|v| v != env!("CARGO_PKG_VERSION"));
            format!(
                "Configured loopback health: {value}\nVersion mismatch: {}\n",
                mismatch
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "server did not report a version".into())
            )
        }
        Err(error) => format!("Configured loopback health unavailable: {error}\n"),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn read_text(path: &Path) -> Result<String, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
fn install_method() -> &'static str {
    let path = std::env::current_exe().unwrap_or_default();
    if path.components().any(|part| part.as_os_str() == "releases") {
        "managed"
    } else if path.components().any(|part| part.as_os_str() == "Cellar") {
        "package manager"
    } else if path.components().any(|part| part.as_os_str() == ".cargo") {
        "cargo"
    } else {
        "standalone/source"
    }
}
fn update_status(dir: &Path) -> String {
    // Read cached status only. Reporting never requests release metadata.
    let cached = std::fs::read(dir.join("update-state.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    if let Some(at) = cached
        .as_ref()
        .and_then(|value| value["checked_at"].as_u64())
    {
        format!("cached check at {at}; not refreshed")
    } else {
        "not checked".into()
    }
}

pub fn write(report: &Report, out: &Path, archive: bool) -> Result<(), String> {
    if report.includes_save && !archive {
        return Err("save inclusion requires archive format".into());
    }
    let parent = out
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    atomicwrites::AtomicFile::new(out, atomicwrites::OverwriteBehavior::DisallowOverwrite)
        .write(|file| -> std::io::Result<()> {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            if archive {
                let encoder =
                    flate2::write::GzEncoder::new(&mut *file, flate2::Compression::default());
                let mut tar = tar::Builder::new(encoder);
                for (name, bytes) in std::iter::once(("report.md", report.markdown.as_bytes()))
                    .chain(
                        report
                            .files
                            .iter()
                            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
                    )
                {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(bytes.len() as u64);
                    header.set_mode(0o600);
                    header.set_cksum();
                    tar.append_data(&mut header, name, bytes)?;
                }
                tar.into_inner()?.finish()?;
            } else {
                file.write_all(report.markdown.as_bytes())?;
            }
            file.sync_all()
        })
        .map_err(|error| error.to_string())
}

pub fn default_out(save: &Path, archive: bool) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    save.parent()
        .unwrap_or(Path::new("."))
        .join("reports")
        .join(format!(
            "hoshi-report-{}-{}-{nonce}.{}",
            SystemClock.now_unix(),
            std::process::id(),
            if archive { "tar.gz" } else { "md" }
        ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub archive: bool,
    pub include_save: bool,
    pub log_lines: usize,
    pub out: Option<PathBuf>,
    pub yes: bool,
    pub open_issue: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            archive: false,
            include_save: false,
            log_lines: 100,
            out: None,
            yes: false,
            open_issue: false,
        }
    }
}
impl Options {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.iter();
        let mut explicit_format = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--yes" => options.yes = true,
                "--include-save" => options.include_save = true,
                "--open-issue" => options.open_issue = true,
                "--format" => {
                    explicit_format = true;
                    options.archive = match args.next().map(String::as_str) {
                        Some("archive") => true,
                        Some("markdown") => false,
                        _ => return Err("--format needs markdown or archive".into()),
                    };
                }
                "--out" => {
                    options.out = Some(PathBuf::from(args.next().ok_or("--out needs a path")?))
                }
                "--log-lines" => {
                    options.log_lines = args
                        .next()
                        .ok_or("--log-lines needs a count")?
                        .parse::<usize>()
                        .ok()
                        .filter(|count| *count <= 10_000)
                        .ok_or("--log-lines must be in 0..=10000")?
                }
                _ => return Err(format!("unexpected report argument {arg}")),
            }
        }
        if options.include_save {
            if explicit_format && !options.archive {
                return Err("--include-save needs archive format".into());
            }
            options.archive = true;
        }
        Ok(options)
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut save = None;
    let mut balance = std::env::var_os("HOSHI_BALANCE").map(PathBuf::from);
    let mut report_args = Vec::new();
    let mut command_seen = false;
    let mut words = args.iter().skip(1);
    while let Some(word) = words.next() {
        match word.as_str() {
            "report" if !command_seen => command_seen = true,
            "--save" => save = Some(PathBuf::from(words.next().ok_or("--save needs a path")?)),
            "--balance" => {
                balance = Some(PathBuf::from(words.next().ok_or("--balance needs a path")?))
            }
            _ if word.starts_with("--save=") => save = Some(PathBuf::from(&word[7..])),
            _ if word.starts_with("--balance=") => balance = Some(PathBuf::from(&word[10..])),
            _ => report_args.push(word.clone()),
        }
    }
    let save = hoshi_storage::resolve_save_path(
        save.as_deref(),
        std::env::var_os("HOSHI_SAVE").as_deref(),
    );
    let options = Options::parse(&report_args)?;
    let report = build(
        &save,
        options.include_save,
        options.log_lines,
        balance.as_deref(),
        true,
        None,
    )?;
    print!("{}", report.markdown);
    if options.archive {
        println!("\nArchive files:\n- report.md");
        for (name, _) in &report.files {
            println!("- {name}");
        }
    }
    if !options.yes {
        print!("Write this local report? [y/N] ");
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
            return Err("report cancelled".into());
        }
    }
    let out = options
        .out
        .unwrap_or_else(|| default_out(&save, options.archive));
    write(&report, &out, options.archive)?;
    println!("Report saved: {}", out.display());
    if options.open_issue {
        println!(
            "https://github.com/Sannrox/hoshi/issues/new?title=Bug%20report&body=Attach%20the%20local%20support%20report%20after%20reviewing%20it."
        );
    }
    Ok(())
}
