//! `hoshi serve`: remote view of one local save. No game HTTP yet.

use hoshi_core::{
    BalanceConfig, Clock, SaveError, SaveLock, SystemClock, lock_save, open_storage,
    resolve_save_path,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Loopback bind used when config and flags omit one.
pub const DEFAULT_BIND: &str = "127.0.0.1:8787";

/// Contract versions this process advertises. Game routes arrive in a follow-up.
pub const CONTRACT_VERSIONS: &[&str] = &["v1"];

#[derive(Debug)]
pub struct ServeError {
    pub message: String,
    pub exit: i32,
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServeError {}

impl From<SaveError> for ServeError {
    fn from(value: SaveError) -> Self {
        let exit = match value {
            SaveError::Locked => crate::cli::EXIT_LOCKED,
            _ => crate::cli::EXIT_INTERNAL,
        };
        Self {
            message: value.to_string(),
            exit,
        }
    }
}

impl From<std::io::Error> for ServeError {
    fn from(value: std::io::Error) -> Self {
        Self {
            message: value.to_string(),
            exit: crate::cli::EXIT_INTERNAL,
        }
    }
}

/// Resolved bind and save for one serve process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeOptions {
    pub bind: SocketAddr,
    pub save: PathBuf,
}

/// Listener plus the exclusive save lock.
pub struct Server {
    listener: TcpListener,
    _lock: SaveLock,
}

impl Server {
    pub fn open(opts: ServeOptions) -> Result<Self, ServeError> {
        let lock = lock_save(&opts.save)?;
        let mut store = open_storage(&opts.save)?;
        store
            .load_or_new(SystemClock.now_unix(), BalanceConfig::default())
            .map_err(|e| ServeError {
                message: e.to_string(),
                exit: crate::cli::EXIT_INTERNAL,
            })?;
        let listener = TcpListener::bind(opts.bind)?;
        Ok(Self {
            listener,
            _lock: lock,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ServeError> {
        Ok(self.listener.local_addr()?)
    }

    /// Serve until the listener fails. Tests call [`serve_one`] instead.
    pub fn run(self) -> Result<(), ServeError> {
        loop {
            let (stream, _) = self.listener.accept()?;
            let _ = serve_one(stream);
        }
    }
}

/// JSON body for `GET /healthz`.
pub fn healthz_body() -> Value {
    json!({
        "ok": true,
        "contract_versions": CONTRACT_VERSIONS,
    })
}

pub fn parse_bind(raw: &str) -> Result<SocketAddr, ServeError> {
    raw.parse().map_err(|_| ServeError {
        message: format!("invalid bind address {raw}"),
        exit: crate::cli::EXIT_INVALID,
    })
}

/// Bind from CLI, then config, then the loopback default.
pub fn resolve_bind(cli_bind: Option<&str>, config_bind: &str) -> Result<SocketAddr, ServeError> {
    if let Some(raw) = cli_bind {
        return parse_bind(raw);
    }
    if !config_bind.is_empty() {
        return parse_bind(config_bind);
    }
    parse_bind(DEFAULT_BIND)
}

pub fn resolve_save(cli_save: Option<&str>, config_save: &str) -> PathBuf {
    if let Some(path) = cli_save {
        return resolve_save_path(
            Some(Path::new(path)),
            std::env::var_os("HOSHI_SAVE").as_deref(),
        );
    }
    if !config_save.is_empty() {
        return PathBuf::from(config_save);
    }
    resolve_save_path(None, std::env::var_os("HOSHI_SAVE").as_deref())
}

pub fn first_positional(args: &[String]) -> Option<&str> {
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--save" || a == "--balance" || a == "--bind" || a == "--config" {
            i += 2;
            continue;
        }
        if a.starts_with("--save=") || a.starts_with("--bind=") || a.starts_with("--config=") {
            i += 1;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(a);
    }
    None
}

type ServeArgs = (Option<String>, Option<String>, Option<PathBuf>);

pub fn parse_serve_args(args: &[String]) -> Result<ServeArgs, ServeError> {
    let mut bind = None;
    let mut save = None;
    let mut config_dir = None;
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "serve" {
            i += 1;
            continue;
        }
        if a == "--bind" {
            let value = args.get(i + 1).ok_or_else(|| ServeError {
                message: "missing --bind address".into(),
                exit: crate::cli::EXIT_INVALID,
            })?;
            bind = Some(value.clone());
            i += 2;
            continue;
        }
        if let Some(value) = a.strip_prefix("--bind=") {
            bind = Some(value.to_string());
            i += 1;
            continue;
        }
        if a == "--save" {
            let value = args.get(i + 1).ok_or_else(|| ServeError {
                message: "missing --save path".into(),
                exit: crate::cli::EXIT_INVALID,
            })?;
            save = Some(value.clone());
            i += 2;
            continue;
        }
        if let Some(value) = a.strip_prefix("--save=") {
            save = Some(value.to_string());
            i += 1;
            continue;
        }
        if a == "--config" {
            let value = args.get(i + 1).ok_or_else(|| ServeError {
                message: "missing --config directory".into(),
                exit: crate::cli::EXIT_INVALID,
            })?;
            config_dir = Some(PathBuf::from(value));
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            return Err(ServeError {
                message: format!("unknown serve flag {a}"),
                exit: crate::cli::EXIT_INVALID,
            });
        }
        i += 1;
    }
    Ok((bind, save, config_dir))
}

fn serve_one(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n") {
            break;
        }
        if buf.len() > 8192 {
            return Ok(());
        }
    }
    let req = String::from_utf8_lossy(&buf);
    let path = req
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let (status, reason, body) = if path == "/healthz" {
        (
            200,
            "OK",
            serde_json::to_vec(&healthz_body()).unwrap_or_else(|_| b"{\"ok\":true}".to_vec()),
        )
    } else {
        (
            404,
            "Not Found",
            serde_json::to_vec(&json!({"ok": false, "error": "not found"}))
                .unwrap_or_else(|_| b"{\"ok\":false}".to_vec()),
        )
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&body)?;
    Ok(())
}

/// Start the serve loop. Prints one JSON line with the bound address, then blocks.
pub fn run(args: &[String]) -> Result<(), ServeError> {
    let (cli_bind, cli_save, config_dir) = parse_serve_args(args)?;
    let hint = resolve_save(cli_save.as_deref(), "");
    let dir = config_dir.unwrap_or_else(|| {
        if cli_save.is_some() {
            config_dir_for_save(&hint)
        } else {
            hoshi_core::default_save_dir()
        }
    });
    let cfg = crate::config::UserConfig::load(&dir);
    let save = resolve_save(cli_save.as_deref(), &cfg.serve_save);
    let bind = resolve_bind(cli_bind.as_deref(), &cfg.serve_bind)?;
    let server = Server::open(ServeOptions { bind, save })?;
    let addr = server.local_addr()?;
    println!(
        "{}",
        json!({
            "ok": true,
            "bind": addr.to_string(),
            "contract_versions": CONTRACT_VERSIONS,
        })
    );
    let _ = std::io::stdout().flush();
    server.run()
}

pub fn config_dir_for_save(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{lock_save, save_lock_path};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static UNIQUE: AtomicU64 = AtomicU64::new(0);

    fn get_path(addr: SocketAddr, path: &str) -> std::io::Result<(u16, String)> {
        let mut stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )?;
        let mut buf = String::new();
        stream.read_to_string(&mut buf)?;
        let status = buf
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = buf
            .split("\r\n\r\n")
            .nth(1)
            .or_else(|| buf.split("\n\n").nth(1))
            .unwrap_or("")
            .to_string();
        Ok((status, body))
    }

    fn unique_save() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "hoshi-serve-{}-{nanos}-{n}.sqlite",
            std::process::id()
        ))
    }

    fn ephemeral(save: PathBuf) -> Server {
        Server::open(ServeOptions {
            bind: parse_bind("127.0.0.1:0").unwrap(),
            save,
        })
        .expect("bind serve")
    }

    #[test]
    fn default_bind_is_loopback() {
        let addr = parse_bind(DEFAULT_BIND).unwrap();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 8787);
    }

    #[test]
    fn resolve_bind_prefers_cli_then_config() {
        let from_cli = resolve_bind(Some("127.0.0.1:0"), "127.0.0.1:9").unwrap();
        assert_eq!(from_cli.port(), 0);
        let from_cfg = resolve_bind(None, "127.0.0.1:0").unwrap();
        assert!(from_cfg.ip().is_loopback());
        let from_default = resolve_bind(None, "").unwrap();
        assert!(from_default.ip().is_loopback());
        assert_eq!(from_default.port(), 8787);
    }

    #[test]
    fn healthz_lists_contract_versions() {
        let save = unique_save();
        let server = ephemeral(save.clone());
        let addr = server.local_addr().unwrap();
        assert!(addr.ip().is_loopback());
        assert_ne!(addr.port(), 8787);
        let thread = std::thread::spawn(move || {
            let (stream, _) = server.listener.accept().unwrap();
            serve_one(stream).unwrap();
            server
        });
        let (status, body) = get_path(addr, "/healthz").unwrap();
        assert_eq!(status, 200, "{body}");
        let value: Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(
            value["contract_versions"],
            json!(CONTRACT_VERSIONS.to_vec())
        );
        drop(thread.join().unwrap());
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn second_writer_is_refused_while_serve_holds_the_lock() {
        let save = unique_save();
        let server = ephemeral(save.clone());
        let err = lock_save(&save).unwrap_err();
        assert!(matches!(err, SaveError::Locked), "{err:?}");
        drop(server);
        let holder = lock_save(&save).unwrap();
        drop(holder);
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn load_or_new_creates_a_save() {
        let save = unique_save();
        assert!(!save.exists());
        let server = ephemeral(save.clone());
        drop(server);
        let store = open_storage(&save).expect("open served save");
        let loaded = store
            .load(BalanceConfig::default())
            .expect("load served save");
        assert_eq!(loaded.home, hoshi_core::Coord::HOME);
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn first_positional_skips_flags() {
        let args = vec![
            "hoshi".into(),
            "--save".into(),
            "x.sqlite".into(),
            "serve".into(),
            "--bind".into(),
            "127.0.0.1:0".into(),
        ];
        assert_eq!(first_positional(&args), Some("serve"));
    }
}
