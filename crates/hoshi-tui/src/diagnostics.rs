//! Small, process-shared rotating logs and crash capture. All text is redacted.
use crate::redact::Redactor;
use hoshi_core::{Clock, SystemClock};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static CONTEXT: OnceLock<Context> = OnceLock::new();
pub struct Context {
    pub dir: PathBuf,
    pub redactor: Redactor,
    pub server: bool,
    pub level: String,
}

pub fn install(dir: PathBuf, server: bool, level: String, config_dir: &Path) {
    if CONTEXT
        .set(Context {
            redactor: Redactor::for_dir(config_dir),
            dir,
            server,
            level,
        })
        .is_err()
    {
        return;
    }
    // ratatui installs its terminal-restoring hook around this hook at init.
    std::panic::set_hook(Box::new(|info| {
        let Some(context) = CONTEXT.get() else {
            return;
        };
        let message = if let Some(message) = info.payload().downcast_ref::<String>() {
            message.as_str()
        } else if let Some(message) = info.payload().downcast_ref::<&str>() {
            message
        } else {
            "non-text panic"
        };
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        match capture_crash(
            context,
            message,
            &location,
            &backtrace,
            SystemClock.now_unix(),
        ) {
            Ok(_) => eprintln!(
                "hoshi crashed. A crash report was saved. Run `hoshi report` to collect details for a bug report."
            ),
            Err(_) => eprintln!(
                "hoshi crashed. Crash capture failed; run `hoshi report` to collect available diagnostics."
            ),
        }
    }));
    log("info", "startup");
}

pub fn log(level: &str, message: &str) {
    let Some(context) = CONTEXT.get() else {
        return;
    };
    if let Err(error) = append(context, level, message, 1024 * 1024, 5) {
        eprintln!("hoshi: diagnostic log unavailable ({})", error.kind());
    }
}

fn rank(level: &str) -> u8 {
    match level {
        "error" => 0,
        "warn" => 1,
        "info" => 2,
        "debug" => 3,
        _ => 2,
    }
}

fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn append(
    context: &Context,
    level: &str,
    message: &str,
    max_bytes: u64,
    retained: usize,
) -> std::io::Result<()> {
    if rank(level) > rank(&context.level) {
        return Ok(());
    }
    let dir = context.dir.join("logs");
    std::fs::create_dir_all(&dir)?;
    let lock = private_file(&dir.join("rotation.lock"))?;
    lock.lock()?;
    let name = if context.server {
        "serve.log"
    } else {
        "hoshi.log"
    };
    let path = dir.join(name);
    let clean = context.redactor.redact(message).text;
    let clean: String = clean.chars().take(16_384).collect();
    let line = format!(
        "{} {level}: {}\n",
        SystemClock.now_unix(),
        clean.replace(['\n', '\r'], " ")
    );
    if path
        .metadata()
        .is_ok_and(|meta| meta.len().saturating_add(line.len() as u64) > max_bytes)
    {
        for index in (1..retained).rev() {
            let old = if index == 1 {
                path.clone()
            } else {
                dir.join(format!("{name}.{}", index - 1))
            };
            let new = dir.join(format!("{name}.{index}"));
            if old.exists() {
                if new.exists() {
                    std::fs::remove_file(&new)?;
                }
                std::fs::rename(old, new)?;
            }
        }
    }
    private_file(&path)?.write_all(line.as_bytes())
}

pub fn capture_crash(
    context: &Context,
    message: &str,
    location: &str,
    backtrace: &str,
    now: u64,
) -> std::io::Result<PathBuf> {
    let dir = context.dir.join("crashes");
    std::fs::create_dir_all(&dir)?;
    // Hash symbols without instruction addresses, so address randomization does
    // not change the signature for the same panic site.
    let frames: String = backtrace
        .lines()
        .filter(|line| !line.contains("0x"))
        .take(12)
        .collect::<Vec<_>>()
        .join("\n");
    let identity = context
        .redactor
        .redact(&format!("{message}\n{location}\n{frames}"))
        .text;
    let mut signature_hash = Sha256::new();
    signature_hash.update(identity.as_bytes());
    // Preserve distinct source sites even when text redaction hides file names.
    signature_hash.update(location.as_bytes());
    let signature = format!(
        "{signature_hash:x}",
        signature_hash = signature_hash.finalize()
    );
    let text = context.redactor.redact(&format!("hoshi {}\ncommit {}\nOS {} {}\nsignature {signature}\n{message}\n{location}\n{backtrace}\n", env!("CARGO_PKG_VERSION"), env!("HOSHI_COMMIT"), std::env::consts::OS, std::env::consts::ARCH)).text;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!("crash-{now}-{}-{nonce}.txt", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    #[cfg(unix)]
    std::fs::File::open(&dir)?.sync_all()?;
    Ok(path)
}

/// Acknowledgement is durable; a later process only offers newly created crashes.
pub fn unseen_crash(dir: &Path) -> Option<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir.join("crashes"))
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("crash-") && name.ends_with(".txt"))
        .collect();
    names.sort();
    let latest = names.pop()?;
    let acknowledged = std::fs::read_to_string(dir.join("crash-notice")).unwrap_or_default();
    (acknowledged != latest).then_some(latest)
}

pub fn acknowledge_crash(dir: &Path, crash: &str) -> std::io::Result<()> {
    atomicwrites::AtomicFile::new(
        dir.join("crash-notice"),
        atomicwrites::OverwriteBehavior::AllowOverwrite,
    )
    .write(|file| file.write_all(crash.as_bytes()))
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panic_hook_persists_redacted_crash_and_prints_report_hint() {
        const CHILD_DIR: &str = "HOSHI_TEST_CRASH_DIR";
        if let Some(dir) = std::env::var_os(CHILD_DIR) {
            let dir = PathBuf::from(dir);
            install(dir.clone(), false, "info".into(), &dir);
            if std::env::var_os("HOSHI_TEST_CRASH_PTY").is_some() {
                let _terminal = ratatui::init();
                panic!("Bearer panic-fixture-secret private.example.test");
            }
            panic!("Bearer panic-fixture-secret private.example.test");
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hoshi-panic-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "diagnostics::tests::panic_hook_persists_redacted_crash_and_prints_report_hint",
                "--nocapture",
            ])
            .env(CHILD_DIR, &dir)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Run `hoshi report`"));
        let crashes: Vec<_> = std::fs::read_dir(dir.join("crashes"))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(crashes.len(), 1);
        let crash = std::fs::read_to_string(crashes[0].path()).unwrap();
        assert!(crash.contains("signature ") && crash.contains("commit "));
        assert!(!crash.contains("panic-fixture-secret") && !crash.contains("private.example.test"));
        let notice = unseen_crash(&dir).unwrap();
        acknowledge_crash(&dir, &notice).unwrap();
        assert!(unseen_crash(&dir).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn panic_restores_real_terminal_before_printing_the_hint() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("hoshi-panic-pty-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let script = r#"
import os, pty, select, subprocess, sys, termios, time
master, slave = pty.openpty()
before = termios.tcgetattr(slave)
env = dict(os.environ, HOSHI_TEST_CRASH_DIR=sys.argv[2], HOSHI_TEST_CRASH_PTY='1', TERM='xterm-256color')
child = subprocess.Popen([sys.argv[1], '--exact', 'diagnostics::tests::panic_hook_persists_redacted_crash_and_prints_report_hint', '--nocapture'], stdin=slave, stdout=slave, stderr=slave, env=env)
output = bytearray()
deadline = time.monotonic() + 10
while child.poll() is None:
    if time.monotonic() > deadline:
        child.kill()
        child.wait()
        raise AssertionError('panic subprocess timed out')
    if select.select([master], [], [], .1)[0]:
        output.extend(os.read(master, 65536))
while select.select([master], [], [], .05)[0]:
    output.extend(os.read(master, 65536))
after = termios.tcgetattr(slave)
os.close(master)
os.close(slave)
assert child.returncode != 0, output
# The kernel may set PENDIN while restoring queued input on a PTY.
mask = ~getattr(termios, 'PENDIN', 0)
before[3] &= mask
after[3] &= mask
assert before == after, (before, after)
assert b'Run `hoshi report`' in output, output
assert b'\x1b[?1049l' in output, output
assert output.index(b'\x1b[?1049l') < output.index(b'Run `hoshi report`'), output
"#;
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(std::env::current_exe().unwrap())
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read_dir(dir.join("crashes")).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rotation_is_bounded_and_crash_identity_is_stable() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("hoshi-diagnostics-{}-{nonce}", std::process::id()));
        let context = Context {
            redactor: Redactor::for_dir(&dir),
            dir: dir.clone(),
            server: false,
            level: "debug".into(),
        };
        for _ in 0..20 {
            append(
                &context,
                "error",
                "Bearer secret-token private.example.test",
                60,
                3,
            )
            .unwrap();
        }
        let logs: Vec<_> = std::fs::read_dir(dir.join("logs"))
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_name() != "rotation.lock")
            .collect();
        assert!(logs.len() <= 3);
        for file in logs {
            let text = std::fs::read_to_string(file.path()).unwrap();
            assert!(!text.contains("secret-token") && !text.contains("private.example.test"));
        }
        let first = capture_crash(
            &context,
            "Bearer secret-token",
            "main.rs:12",
            "0x12345\nsymbol",
            10,
        )
        .unwrap();
        let second = capture_crash(
            &context,
            "Bearer secret-token",
            "main.rs:12",
            "0x99999\nsymbol",
            11,
        )
        .unwrap();
        let a = std::fs::read_to_string(first).unwrap();
        let b = std::fs::read_to_string(second).unwrap();
        assert_eq!(
            a.lines().find(|line| line.starts_with("signature")),
            b.lines().find(|line| line.starts_with("signature"))
        );
        assert!(!a.contains("secret-token"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
