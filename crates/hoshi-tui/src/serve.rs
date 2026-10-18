//! `hoshi serve`: remote view of one local save, including HTTP `/v1` actions.

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

/// Contract versions this process advertises.
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
    save: PathBuf,
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
            save: opts.save,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ServeError> {
        Ok(self.listener.local_addr()?)
    }

    /// Serve until the listener fails. Tests call [`serve_one`] instead.
    pub fn run(self) -> Result<(), ServeError> {
        loop {
            let (stream, _) = self.listener.accept()?;
            let _ = serve_one(&self.save, stream);
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

fn error_body(code: &str, message: &str) -> Value {
    json!({
        "schema_version": crate::cli::SCHEMA_VERSION,
        "ok": false,
        "error": { "code": code, "message": message },
    })
}

fn status_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Error",
    }
}

fn http_status(exit: i32) -> u16 {
    match exit {
        crate::cli::EXIT_OK => 200,
        crate::cli::EXIT_INVALID => 400,
        crate::cli::EXIT_REFUSED => 409,
        _ => 500,
    }
}

fn require_str<'a>(body: &'a Value, key: &str) -> Result<&'a str, String> {
    body.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))
}

/// Map one `/v1` request onto the same CLI words the harness already uses.
fn v1_parts(method: &str, path: &str, body: &Value) -> Result<Vec<String>, (u16, Value)> {
    let known = matches!(
        path,
        "/v1/state" | "/v1/build" | "/v1/research" | "/v1/cancel" | "/v1/fleet/send"
    );
    if path.starts_with("/v1/tick") || path == "/v1/tick" {
        return Err((404, error_body("invalid", "not found")));
    }
    if !known {
        return Err((404, error_body("invalid", "not found")));
    }
    let want_get = path == "/v1/state";
    if want_get && method != "GET" {
        return Err((405, error_body("invalid", "method not allowed")));
    }
    if !want_get && method != "POST" {
        return Err((405, error_body("invalid", "method not allowed")));
    }
    let parts = match path {
        "/v1/state" => vec!["state".into()],
        "/v1/build" => vec![
            "build".into(),
            require_str(body, "planet")
                .map_err(|m| (400, error_body("invalid", &m)))?
                .into(),
            require_str(body, "building")
                .map_err(|m| (400, error_body("invalid", &m)))?
                .into(),
        ],
        "/v1/research" => vec![
            "research".into(),
            require_str(body, "tech")
                .map_err(|m| (400, error_body("invalid", &m)))?
                .into(),
        ],
        "/v1/cancel" => vec![
            "cancel".into(),
            require_str(body, "item")
                .map_err(|m| (400, error_body("invalid", &m)))?
                .into(),
        ],
        "/v1/fleet/send" => {
            let mut parts = vec![
                "fleet".into(),
                "send".into(),
                require_str(body, "from")
                    .map_err(|m| (400, error_body("invalid", &m)))?
                    .into(),
                require_str(body, "to")
                    .map_err(|m| (400, error_body("invalid", &m)))?
                    .into(),
                require_str(body, "mission")
                    .map_err(|m| (400, error_body("invalid", &m)))?
                    .into(),
            ];
            let ships = body
                .get("ships")
                .and_then(Value::as_array)
                .ok_or_else(|| (400, error_body("invalid", "missing ships")))?;
            if ships.is_empty() {
                return Err((400, error_body("invalid", "missing ships")));
            }
            for spec in ships {
                let spec = spec
                    .as_str()
                    .ok_or_else(|| (400, error_body("invalid", "invalid ship spec")))?;
                parts.push(spec.into());
            }
            parts
        }
        _ => return Err((404, error_body("invalid", "not found"))),
    };
    Ok(parts)
}

/// Dispatch one HTTP request against the locked save.
pub fn handle(save: &Path, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    let path = path.split('?').next().unwrap_or(path);
    if path == "/healthz" {
        if method != "GET" {
            return (405, error_body("invalid", "method not allowed"));
        }
        return (200, healthz_body());
    }
    let json_body = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(value) => value,
            Err(_) => return (400, error_body("invalid", "invalid json")),
        }
    };
    let parts = match v1_parts(method, path, &json_body) {
        Ok(parts) => parts,
        Err(err) => return err,
    };
    let words: Vec<&str> = parts.iter().map(String::as_str).collect();
    let out = crate::cli::run_unlocked(save, &words);
    let value: Value = serde_json::from_str(&out.json)
        .unwrap_or_else(|_| error_body("internal", "malformed command result"));
    (http_status(out.exit), value)
}

fn header_end(buf: &[u8]) -> Option<(usize, usize)> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)))
}

fn content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        let line = line.trim();
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn serve_one(save: &Path, mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    let mut header_mark = None;
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if header_mark.is_none() {
            if let Some(mark) = header_end(&buf) {
                header_mark = Some(mark);
            } else if buf.len() > 8192 {
                return Ok(());
            }
        }
        if let Some((start, sep)) = header_mark {
            let header_len = start + sep;
            let want = content_length(std::str::from_utf8(&buf[..start]).unwrap_or(""))
                .unwrap_or(0)
                .min(65_536);
            if buf.len() >= header_len + want {
                break;
            }
        }
        if buf.len() > 65_536 + 8192 {
            return Ok(());
        }
    }
    let Some((start, sep)) = header_mark.or_else(|| header_end(&buf)) else {
        return Ok(());
    };
    let header_len = start + sep;
    let headers = String::from_utf8_lossy(&buf[..start]);
    let request_line = headers.lines().next().unwrap_or("");
    let mut words = request_line.split_whitespace();
    let method = words.next().unwrap_or("GET");
    let path = words.next().unwrap_or("/");
    let want = content_length(&headers).unwrap_or(0).min(65_536);
    let body = if header_len + want <= buf.len() {
        buf[header_len..header_len + want].to_vec()
    } else {
        buf[header_len.min(buf.len())..].to_vec()
    };
    let (status, value) = handle(save, method, path, &body);
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    let header = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status_phrase(status),
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

    fn post_json(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut buf = String::new();
        stream.read_to_string(&mut buf).unwrap();
        let status = buf
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let text = buf
            .split("\r\n\r\n")
            .nth(1)
            .or_else(|| buf.split("\n\n").nth(1))
            .unwrap_or("")
            .to_string();
        (status, text)
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
            serve_one(&server.save, stream).unwrap();
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
    fn tcp_post_build_reads_json_body() {
        let save = unique_save();
        seed_save(&save);
        let server = ephemeral(save.clone());
        let addr = server.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (stream, _) = server.listener.accept().unwrap();
            serve_one(&server.save, stream).unwrap();
            server
        });
        let (status, body) = post_json(
            addr,
            "/v1/build",
            r#"{"planet":"home","building":"metal_mine"}"#,
        );
        assert_eq!(status, 200, "{body}");
        let value: Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(value["message"], "queued build");
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

    fn seed_save(path: &Path) -> Value {
        let argv = vec![
            "hoshi".into(),
            format!("--save={}", path.display()),
            "--seed=1".into(),
            "state".into(),
        ];
        let out = crate::cli::run_cli(&argv);
        assert_eq!(out.exit, 0, "{}", out.json);
        serde_json::from_str(&out.json).unwrap()
    }

    fn cli_on(path: &Path, parts: &[&str]) -> (Value, i32) {
        let mut argv = vec!["hoshi".into(), format!("--save={}", path.display())];
        argv.extend(parts.iter().map(|s| (*s).to_string()));
        let out = crate::cli::run_cli(&argv);
        (serde_json::from_str(&out.json).unwrap(), out.exit)
    }

    fn collect_keys(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    out.push(key.clone());
                    collect_keys(child, out);
                }
            }
            Value::Array(items) => {
                for child in items {
                    collect_keys(child, out);
                }
            }
            _ => {}
        }
    }

    fn schema_required() -> Vec<String> {
        let schema: Value =
            serde_json::from_str(include_str!("../../../docs/schema/serve-v1.json")).unwrap();
        schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    fn assert_schema(value: &Value) {
        for key in schema_required() {
            assert!(value.get(&key).is_some(), "missing {key} in {value}");
        }
        assert_eq!(value["schema_version"], crate::cli::SCHEMA_VERSION);
        assert!(value["ok"].is_boolean());
    }

    #[test]
    fn v1_state_build_research_cancel_fleet_match_cli_and_core() {
        let save_cli = unique_save();
        let save_http = unique_save();
        seed_save(&save_cli);
        seed_save(&save_http);

        let sequence = [
            ("GET", "/v1/state", json!({})),
            (
                "POST",
                "/v1/build",
                json!({"planet": "home", "building": "metal_mine"}),
            ),
            ("POST", "/v1/cancel", json!({"item": "0"})),
            (
                "POST",
                "/v1/build",
                json!({"planet": "home", "building": "metal_mine"}),
            ),
            ("POST", "/v1/research", json!({"tech": "energy"})),
            (
                "POST",
                "/v1/fleet/send",
                json!({
                    "from": "home",
                    "to": "1:1:8",
                    "mission": "attack",
                    "ships": ["light_fighter:1"]
                }),
            ),
            ("GET", "/v1/state", json!({})),
        ];
        let cli_words = [
            &["state"][..],
            &["build", "home", "metal_mine"][..],
            &["cancel", "0"][..],
            &["build", "home", "metal_mine"][..],
            &["research", "energy"][..],
            &[
                "fleet",
                "send",
                "home",
                "1:1:8",
                "attack",
                "light_fighter:1",
            ][..],
            &["state"][..],
        ];

        let mut last_http = json!({});
        let mut last_cli = json!({});
        for ((method, path, body), words) in sequence.iter().zip(cli_words) {
            let raw = serde_json::to_vec(body).unwrap();
            let (status, http) = handle(&save_http, method, path, &raw);
            assert_schema(&http);
            assert!(
                status == 200 || status == 400 || status == 409,
                "{method} {path} {status} {http}"
            );
            let (cli, exit) = cli_on(&save_cli, words);
            assert_schema(&cli);
            assert_eq!(http, cli, "{method} {path}");
            assert_eq!(
                http_status(exit),
                status,
                "{method} {path} exit {exit} status {status}"
            );
            last_http = http;
            last_cli = cli;
        }

        let mut core = hoshi_core::GameState::with_seed(0, BalanceConfig::default(), 1);
        core.enqueue_build(hoshi_core::BuildingKind::MetalMine, 0)
            .unwrap();
        core.cancel_build(0, 0).unwrap();
        core.enqueue_build(hoshi_core::BuildingKind::MetalMine, 0)
            .unwrap();
        let _ = hoshi_core::propose(
            &core,
            &hoshi_core::Intent::Research {
                tech: "energy".into(),
            },
            0,
        )
        .and_then(|p| hoshi_core::apply(&mut core, &p, 0));
        let ships = {
            let mut fleet = hoshi_core::Fleet::default();
            fleet.add(hoshi_core::ShipKind::LightFighter, 1);
            fleet
        };
        let dest = hoshi_core::Coord::from_key("1:1:8").unwrap();
        let _ = core.launch_attack(ships, dest, 100, 0);

        assert_eq!(last_http, last_cli);
        assert_eq!(
            last_http["buildings"]["metal_mine"],
            json!(core.buildings.metal_mine)
        );
        assert!(
            (last_http["resources"]["iron"].as_f64().unwrap() - core.resources.iron).abs() < 1e-6
        );
        assert_eq!(
            last_http["research"]["levels"]["energy"],
            json!(core.tech_level("energy"))
        );

        let _ = std::fs::remove_file(&save_cli);
        let _ = std::fs::remove_file(&save_http);
        let _ = std::fs::remove_file(save_lock_path(&save_cli));
        let _ = std::fs::remove_file(save_lock_path(&save_http));
    }

    #[test]
    fn v1_fleet_ships_cannot_retarget_save_or_balance() {
        let save = unique_save();
        seed_save(&save);
        let other = unique_save();
        let body = json!({
            "from": "home",
            "to": "1:1:8",
            "mission": "attack",
            "ships": [format!("--save={}", other.display()), "light_fighter:1"]
        });
        let (status, value) = handle(
            &save,
            "POST",
            "/v1/fleet/send",
            &serde_json::to_vec(&body).unwrap(),
        );
        assert_eq!(status, 400, "{value}");
        assert_eq!(value["ok"], false);
        assert!(!other.exists(), "injected --save must not create a file");
        let body = json!({
            "from": "home",
            "to": "1:1:8",
            "mission": "attack",
            "ships": ["--balance=/tmp/cheat.toml", "light_fighter:1"]
        });
        let (status, value) = handle(
            &save,
            "POST",
            "/v1/fleet/send",
            &serde_json::to_vec(&body).unwrap(),
        );
        assert_eq!(status, 400, "{value}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn v1_fair_play_hides_truth_and_refuses_tick() {
        let save = unique_save();
        seed_save(&save);
        let (status, state) = handle(&save, "GET", "/v1/state", b"");
        assert_eq!(status, 200);
        assert_schema(&state);
        let mut keys = Vec::new();
        collect_keys(&state, &mut keys);
        for banned in [
            "rng",
            "rng_state",
            "deceived",
            "unrevealed",
            "pending_reports",
            "pirate_timer",
            "next_raid_at",
        ] {
            assert!(
                !keys.iter().any(|k| k == banned),
                "leaked {banned} in {keys:?}"
            );
        }
        if let Some(raid) = state.get("raid")
            && raid.is_object()
        {
            for key in raid.as_object().unwrap().keys() {
                assert_eq!(key, "lands_in_secs", "hidden raid field {key}");
            }
        }
        let (tick_get, body) = handle(&save, "GET", "/v1/tick", b"");
        assert_eq!(tick_get, 404, "{body}");
        let (tick_post, body) = handle(&save, "POST", "/v1/tick", br#"{"advance":"60"}"#);
        assert_eq!(tick_post, 404, "{body}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }
}
