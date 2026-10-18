//! `hoshi serve`: remote view of one local save, including HTTP `/v1` actions.

use hoshi_core::{
    BalanceConfig, Clock, SaveError, SaveLock, SystemClock, lock_save, open_storage,
    resolve_save_path,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default allowed `/v1` calls per token and action inside [`DEFAULT_RATE_WINDOW_SECS`].
pub const DEFAULT_RATE_LIMIT: u32 = 60;
/// Sliding window for per-token, per-action limits.
pub const DEFAULT_RATE_WINDOW_SECS: u64 = 60;

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

/// One bearer secret bound to one save path. Never stored in the save file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedToken {
    pub secret: String,
    pub save: PathBuf,
}

/// Bearer gate and sliding-window limits for `/v1`.
#[derive(Debug, Clone)]
pub struct Auth {
    tokens: Vec<ScopedToken>,
    limit: u32,
    window_secs: u64,
    hits: HashMap<(usize, String), Vec<u64>>,
}

impl Auth {
    pub fn empty() -> Self {
        Self::with_tokens(Vec::new(), DEFAULT_RATE_LIMIT, DEFAULT_RATE_WINDOW_SECS)
    }

    #[cfg(test)]
    pub fn single(save: &Path, secret: &str) -> Self {
        if secret.is_empty() {
            return Self::empty();
        }
        Self::with_tokens(
            vec![ScopedToken {
                secret: secret.into(),
                save: save.to_path_buf(),
            }],
            DEFAULT_RATE_LIMIT,
            DEFAULT_RATE_WINDOW_SECS,
        )
    }

    pub fn with_tokens(tokens: Vec<ScopedToken>, limit: u32, window_secs: u64) -> Self {
        Self {
            tokens,
            limit: if limit == 0 {
                DEFAULT_RATE_LIMIT
            } else {
                limit
            },
            window_secs: if window_secs == 0 {
                DEFAULT_RATE_WINDOW_SECS
            } else {
                window_secs
            },
            hits: HashMap::new(),
        }
    }

    /// Load operator secrets from config and process environment. Never the save.
    pub fn load(
        save: &Path,
        config_token: &str,
        token_file: Option<&Path>,
        limit: u32,
        window_secs: u64,
    ) -> Self {
        let mut tokens = Vec::new();
        if !config_token.is_empty() {
            tokens.push(ScopedToken {
                secret: config_token.into(),
                save: save.to_path_buf(),
            });
        }
        if let Ok(env) = std::env::var("HOSHI_SERVE_TOKEN")
            && !env.is_empty()
        {
            tokens.push(ScopedToken {
                secret: env,
                save: save.to_path_buf(),
            });
        }
        let file = std::env::var_os("HOSHI_SERVE_TOKEN_FILE")
            .map(PathBuf::from)
            .or_else(|| token_file.map(Path::to_path_buf));
        if let Some(path) = file {
            tokens.extend(load_token_file(&path));
        }
        Self::with_tokens(tokens, limit, window_secs)
    }

    fn allow(
        &mut self,
        save: &Path,
        action: &str,
        authorization: Option<&str>,
        now: u64,
    ) -> Result<(), (u16, Value)> {
        let presented = match bearer(authorization) {
            Some(token) => token,
            None => return Err((401, error_body("unauthorized", "unauthorized"))),
        };
        let Some(idx) = self
            .tokens
            .iter()
            .position(|token| eq_secret(token.secret.as_bytes(), presented.as_bytes()))
        else {
            return Err((401, error_body("unauthorized", "unauthorized")));
        };
        if !same_save(&self.tokens[idx].save, save) {
            return Err((401, error_body("unauthorized", "unauthorized")));
        }
        let window = self.window_secs;
        let limit = self.limit;
        let hits = self.hits.entry((idx, action.to_string())).or_default();
        hits.retain(|at| now.saturating_sub(*at) < window);
        if hits.len() as u32 >= limit {
            return Err((429, error_body("rate_limited", "rate limited")));
        }
        hits.push(now);
        Ok(())
    }
}

fn load_token_file(path: &Path) -> Vec<ScopedToken> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((secret, save)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let secret = secret.trim();
        let save = save.trim();
        if secret.is_empty() || save.is_empty() {
            continue;
        }
        out.push(ScopedToken {
            secret: secret.into(),
            save: PathBuf::from(save),
        });
    }
    out
}

fn bearer(header: Option<&str>) -> Option<&str> {
    let raw = header?.trim();
    let (scheme, rest) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = rest.trim();
    (!token.is_empty()).then_some(token)
}

fn eq_secret(left: &[u8], right: &[u8]) -> bool {
    let mut acc = left.len() ^ right.len();
    let n = left.len().max(right.len());
    for i in 0..n {
        let a = left.get(i).copied().unwrap_or(0);
        let b = right.get(i).copied().unwrap_or(0);
        acc |= (a ^ b) as usize;
    }
    acc == 0
}

fn same_save(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => left == right,
    }
}

/// Listener plus the exclusive save lock.
pub struct Server {
    listener: TcpListener,
    _lock: SaveLock,
    save: PathBuf,
    auth: Auth,
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
            auth: Auth::empty(),
        })
    }

    pub fn with_auth(mut self, auth: Auth) -> Self {
        self.auth = auth;
        self
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ServeError> {
        Ok(self.listener.local_addr()?)
    }

    /// Serve until the listener fails. Tests call [`serve_one`] instead.
    pub fn run(mut self) -> Result<(), ServeError> {
        loop {
            let (stream, _) = self.listener.accept()?;
            let _ = serve_one(&self.save, stream, &mut self.auth);
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
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        429 => "Too Many Requests",
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
pub fn handle(
    save: &Path,
    method: &str,
    path: &str,
    body: &[u8],
    authorization: Option<&str>,
    auth: &mut Auth,
    now: u64,
) -> (u16, Value) {
    let path = path.split('?').next().unwrap_or(path);
    if path == "/healthz" {
        if method != "GET" {
            return (405, error_body("invalid", "method not allowed"));
        }
        return (200, healthz_body());
    }
    if path.starts_with("/v1")
        && let Err(err) = auth.allow(save, path, authorization, now)
    {
        return err;
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
    header_value(headers, "content-length")?.parse().ok()
}

fn header_value(headers: &str, want: &str) -> Option<String> {
    for line in headers.lines() {
        let line = line.trim();
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case(want) {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn serve_one(save: &Path, mut stream: TcpStream, auth: &mut Auth) -> std::io::Result<()> {
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
    let authorization = header_value(&headers, "authorization");
    let now = SystemClock.now_unix();
    let (status, value) = handle(
        save,
        method,
        path,
        &body,
        authorization.as_deref(),
        auth,
        now,
    );
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    let extra = match status {
        401 => "WWW-Authenticate: Bearer\r\n".to_string(),
        429 => format!("Retry-After: {}\r\n", auth.window_secs),
        _ => String::new(),
    };
    let header = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
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
    let token_file = if cfg.serve_token_file.is_empty() {
        None
    } else {
        let path = PathBuf::from(&cfg.serve_token_file);
        Some(if path.is_absolute() {
            path
        } else {
            dir.join(path)
        })
    };
    let auth = Auth::load(
        &save,
        &cfg.serve_token,
        token_file.as_deref(),
        cfg.serve_rate_limit,
        cfg.serve_rate_window_secs,
    );
    let server = Server::open(ServeOptions { bind, save })?.with_auth(auth);
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

    const TEST_TOKEN: &str = "test-token";

    fn handle_v1(save: &Path, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
        let mut auth = Auth::single(save, TEST_TOKEN);
        handle(
            save,
            method,
            path,
            body,
            Some("Bearer test-token"),
            &mut auth,
            1_000,
        )
    }

    fn get_path(addr: SocketAddr, path: &str) -> std::io::Result<(u16, String)> {
        get_path_auth(addr, path, None)
    }

    fn get_path_auth(
        addr: SocketAddr,
        path: &str,
        authorization: Option<&str>,
    ) -> std::io::Result<(u16, String)> {
        let mut stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let auth_line = match authorization {
            Some(value) => format!("Authorization: {value}\r\n"),
            None => String::new(),
        };
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\n{auth_line}Connection: close\r\n\r\n"
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
        let auth = Auth::single(&save, TEST_TOKEN);
        Server::open(ServeOptions {
            bind: parse_bind("127.0.0.1:0").unwrap(),
            save,
        })
        .expect("bind serve")
        .with_auth(auth)
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
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TEST_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
            let mut server = server;
            let (stream, _) = server.listener.accept().unwrap();
            serve_one(&server.save, stream, &mut server.auth).unwrap();
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
            let mut server = server;
            let (stream, _) = server.listener.accept().unwrap();
            serve_one(&server.save, stream, &mut server.auth).unwrap();
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

    fn schema_doc() -> Value {
        serde_json::from_str(include_str!("../../../docs/schema/serve-v1.json")).unwrap()
    }

    fn schema_required() -> Vec<String> {
        schema_doc()["required"]
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
    fn serve_schema_lists_auth_error_codes() {
        let schema = schema_doc();
        let codes = schema["properties"]["error"]["properties"]["code"]["enum"]
            .as_array()
            .expect("error.code enum");
        for want in [
            "invalid",
            "refused",
            "internal",
            "unauthorized",
            "rate_limited",
        ] {
            assert!(
                codes.iter().any(|code| code.as_str() == Some(want)),
                "missing {want} in {codes:?}"
            );
        }
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
            let (status, http) = handle_v1(&save_http, method, path, &raw);
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
        let (status, value) = handle_v1(
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
        let (status, value) = handle_v1(
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
        let (status, state) = handle_v1(&save, "GET", "/v1/state", b"");
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
        let (tick_get, body) = handle_v1(&save, "GET", "/v1/tick", b"");
        assert_eq!(tick_get, 404, "{body}");
        let (tick_post, body) = handle_v1(&save, "POST", "/v1/tick", br#"{"advance":"60"}"#);
        assert_eq!(tick_post, 404, "{body}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn v1_rejects_unauthenticated_requests() {
        let save = unique_save();
        seed_save(&save);
        let mut auth = Auth::single(&save, TEST_TOKEN);
        let (status, value) = handle(&save, "GET", "/v1/state", b"", None, &mut auth, 1_000);
        assert_eq!(status, 401, "{value}");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "unauthorized");
        assert_schema(&value);
        let (status, value) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some("Bearer "),
            &mut auth,
            1_000,
        );
        assert_eq!(status, 401, "{value}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn v1_accepts_token_for_this_save_and_refuses_another() {
        let save = unique_save();
        seed_save(&save);
        let other = unique_save();
        seed_save(&other);
        let mine = format!(
            "tok-this-save-{}-{}",
            std::process::id(),
            UNIQUE.load(Ordering::Relaxed)
        );
        let theirs = format!(
            "tok-other-save-{}-{}",
            std::process::id(),
            UNIQUE.load(Ordering::Relaxed)
        );
        let tokens = vec![
            ScopedToken {
                secret: mine.clone(),
                save: save.clone(),
            },
            ScopedToken {
                secret: theirs.clone(),
                save: other.clone(),
            },
        ];
        let mut auth = Auth::with_tokens(tokens, DEFAULT_RATE_LIMIT, DEFAULT_RATE_WINDOW_SECS);
        let header = format!("Bearer {mine}");
        let (status, value) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some(&header),
            &mut auth,
            1_000,
        );
        assert_eq!(status, 200, "{value}");
        assert_eq!(value["ok"], true);
        let other_header = format!("Bearer {theirs}");
        let (status, value) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some(&other_header),
            &mut auth,
            1_001,
        );
        assert_eq!(status, 401, "{value}");
        assert_eq!(value["error"]["code"], "unauthorized");
        let bytes = std::fs::read(&save).unwrap();
        assert!(
            !bytes.windows(mine.len()).any(|w| w == mine.as_bytes()),
            "token must not land in the save"
        );
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(&other);
        let _ = std::fs::remove_file(save_lock_path(&save));
        let _ = std::fs::remove_file(save_lock_path(&other));
    }

    #[test]
    fn v1_rate_limits_per_token_and_action() {
        let save = unique_save();
        seed_save(&save);
        let mut auth = Auth::with_tokens(
            vec![ScopedToken {
                secret: "mine".into(),
                save: save.clone(),
            }],
            2,
            10,
        );
        let (a, _) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some("Bearer mine"),
            &mut auth,
            1_000,
        );
        let (b, _) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some("Bearer mine"),
            &mut auth,
            1_001,
        );
        let (c, limited) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some("Bearer mine"),
            &mut auth,
            1_002,
        );
        assert_eq!(a, 200);
        assert_eq!(b, 200);
        assert_eq!(c, 429, "{limited}");
        assert_eq!(limited["error"]["code"], "rate_limited");
        assert_schema(&limited);
        let (other_action, value) = handle(
            &save,
            "POST",
            "/v1/build",
            br#"{"planet":"home","building":"metal_mine"}"#,
            Some("Bearer mine"),
            &mut auth,
            1_002,
        );
        assert_eq!(other_action, 200, "{value}");
        let (after_window, value) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some("Bearer mine"),
            &mut auth,
            1_012,
        );
        assert_eq!(after_window, 200, "{value}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn healthz_stays_open_without_a_token() {
        let save = unique_save();
        let mut auth = Auth::empty();
        let (status, value) = handle(&save, "GET", "/healthz", b"", None, &mut auth, 1);
        assert_eq!(status, 200, "{value}");
        assert_eq!(value["ok"], true);
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }
}
