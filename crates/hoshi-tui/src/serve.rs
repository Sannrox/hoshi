//! `hoshi serve`: remote view of one local save, including HTTP `/v1` actions.

use hoshi_core::{
    BalanceConfig, Clock, GameState, HistoryEvent, HistoryKind, SaveError, SaveLock, SystemClock,
    lock_save, open_storage, resolve_save_path,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::ws;

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
    pub admin: bool,
    pub secret: String,
    pub save: PathBuf,
}

/// Cookie that carries a paired browser's device session secret (ADR 0016).
pub const SESSION_COOKIE: &str = "hoshi_session";

/// What a request presented to authenticate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// The raw `Authorization` header value.
    Header(String),
    /// A device session secret from the [`SESSION_COOKIE`]; never matched
    /// against operator tokens.
    Cookie(String),
}

impl Credential {
    /// The one credential in `headers`. The header and the cookie are
    /// alternatives; a request carrying both is refused (`400`).
    fn from_headers(headers: &str) -> Result<Option<Self>, (u16, Value)> {
        let header = header_value(headers, "authorization");
        let cookie = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(name, _)| name.trim().eq_ignore_ascii_case("cookie"))
            .flat_map(|(_, value)| value.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(name, value)| *name == SESSION_COOKIE && !value.is_empty())
            .map(|(_, value)| value.to_string());
        match (header, cookie) {
            (Some(_), Some(_)) => Err((
                400,
                error_body("invalid", "send the token or the session cookie, not both"),
            )),
            (Some(header), None) => Ok(Some(Self::Header(header))),
            (None, cookie) => Ok(cookie.map(Self::Cookie)),
        }
    }

    fn is_cookie(credential: Option<&Self>) -> bool {
        matches!(credential, Some(Self::Cookie(_)))
    }
}

/// Bearer gate and sliding-window limits for `/v1`. Accepts operator tokens
/// and the device sessions of paired browsers (ADR 0016).
#[derive(Debug, Clone)]
pub struct Auth {
    tokens: Vec<ScopedToken>,
    devices: Option<crate::devices::Devices>,
    limit: u32,
    window_secs: u64,
    hits: HashMap<(Caller, String), Vec<u64>>,
}

/// Who a request authenticated as, for per-caller rate limits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Caller {
    Token(usize),
    Device(String),
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
                admin: false,
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
            devices: None,
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
                admin: false,
                secret: config_token.into(),
                save: save.to_path_buf(),
            });
        }
        if let Ok(env) = std::env::var("HOSHI_SERVE_TOKEN")
            && !env.is_empty()
        {
            tokens.push(ScopedToken {
                admin: false,
                secret: env,
                save: save.to_path_buf(),
            });
        }
        if let Ok(secret) = std::env::var("HOSHI_SERVE_ADMIN_TOKEN")
            && !secret.is_empty()
        {
            tokens.push(ScopedToken {
                admin: true,
                secret,
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

    pub fn with_admin_token(mut self, save: &Path, secret: &str) -> Self {
        if !secret.is_empty() {
            self.tokens.push(ScopedToken {
                admin: true,
                secret: secret.into(),
                save: save.to_path_buf(),
            });
        }
        self
    }

    /// Accept device sessions for the save they were loaded from.
    pub fn with_devices(mut self, devices: crate::devices::Devices) -> Self {
        self.devices = Some(devices);
        self
    }

    fn allow(
        &mut self,
        save: &Path,
        action: &str,
        credential: Option<&Credential>,
        now: u64,
    ) -> Result<(), (u16, Value)> {
        let unauthorized = || (401, error_body("unauthorized", "unauthorized"));
        let (presented, cookie) = match credential {
            Some(Credential::Header(raw)) => (bearer(Some(raw)).ok_or_else(unauthorized)?, false),
            Some(Credential::Cookie(secret)) => (secret.as_str(), true),
            None => return Err(unauthorized()),
        };
        let token = if cookie {
            None
        } else {
            self.tokens
                .iter()
                .position(|token| eq_secret(token.secret.as_bytes(), presented.as_bytes()))
        };
        let (caller, admin) = match token {
            Some(idx) if same_save(&self.tokens[idx].save, save) => {
                (Caller::Token(idx), self.tokens[idx].admin)
            }
            Some(_) => return Err(unauthorized()),
            None => {
                let devices = self
                    .devices
                    .as_mut()
                    .filter(|devices| same_save(devices.save(), save))
                    .ok_or_else(unauthorized)?;
                let id = devices
                    .authenticate(presented, now)
                    .ok_or_else(unauthorized)?;
                (Caller::Device(id), false)
            }
        };
        if action == "/v1/admin/support-report" && !admin {
            return Err((403, error_body("unauthorized", "admin scope required")));
        }
        let window = self.window_secs;
        let limit = self.limit;
        let hits = self.hits.entry((caller, action.to_string())).or_default();
        hits.retain(|at| now.saturating_sub(*at) < window);
        if hits.len() as u32 >= limit {
            return Err((429, error_body("rate_limited", "rate limited")));
        }
        hits.push(now);
        Ok(())
    }

    /// Bind a new secret to this process save. Never written to SQLite.
    pub fn mint(&mut self, save: &Path, secret: String) {
        if secret.is_empty() {
            return;
        }
        self.tokens.push(ScopedToken {
            admin: false,
            secret,
            save: save.to_path_buf(),
        });
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
        let (admin, save) = save
            .strip_prefix("admin:")
            .map(|path| (true, path))
            .unwrap_or((false, save));
        out.push(ScopedToken {
            admin,
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

pub(crate) fn eq_secret(left: &[u8], right: &[u8]) -> bool {
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

/// Decrements the accept-loop inflight count when a worker (or a failed spawn) drops.
struct Inflight(Arc<AtomicUsize>);

impl Drop for Inflight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// In-memory save for the process. Catch-up stays here; disk writes are throttled.
#[derive(Clone)]
struct Live {
    updates: Option<crate::update::CheckSession>,
    config_dir: PathBuf,
    stats: ServerStats,
    state: GameState,
    last_save_tick: u64,
    saved_next_id: u64,
    /// The browser terminal holding the save, if any. Never in the save.
    terminal: Option<crate::terminal::Lease>,
    terminal_settings: crate::terminal::Settings,
    /// The operator's key bindings, so the browser terminal answers to the
    /// same keys as the local TUI.
    terminal_keys: crate::keymap::Keymap,
    next_terminal_id: u64,
}

impl Live {
    /// While a terminal session holds the save, `/v1` writes and a second
    /// terminal are refused, like the TUI's save lock.
    fn terminal_holds(&self, now: u64) -> bool {
        self.terminal
            .is_some_and(|lease| lease.holds(now, self.terminal_settings.grace_secs))
    }
}

#[derive(Clone)]
struct ServerStats {
    started_at: u64,
    clients: usize,
    requests: u64,
    rate_limit_hits: u64,
}
impl ServerStats {
    fn new() -> Self {
        Self {
            started_at: SystemClock.now_unix(),
            clients: 0,
            requests: 0,
            rate_limit_hits: 0,
        }
    }
    fn value(&self) -> Value {
        json!({"uptime_secs": SystemClock.now_unix().saturating_sub(self.started_at), "connected_clients": self.clients, "requests": self.requests, "rate_limit_hits": self.rate_limit_hits})
    }
}
struct Client<'a>(&'a Mutex<Live>);
impl Drop for Client<'_> {
    fn drop(&mut self) {
        let mut live = lock_poison(self.0);
        live.stats.clients = live.stats.clients.saturating_sub(1);
    }
}

/// Match the TUI idle save cadence so an open socket does not rewrite every second.
const SAVE_IDLE_SECS: u64 = 5;

/// Listener plus the exclusive save lock.
pub struct Server {
    listener: TcpListener,
    _lock: SaveLock,
    save: PathBuf,
    auth: Auth,
    live: Arc<Mutex<Live>>,
    push: Arc<Mutex<crate::push::Gate>>,
    oidc: Arc<Mutex<crate::oidc::Gate>>,
    ui: Arc<Option<crate::web_ui::Ui>>,
}

impl Server {
    pub fn open(opts: ServeOptions) -> Result<Self, ServeError> {
        let lock = lock_save(&opts.save)?;
        let mut store = open_storage(&opts.save)?;
        let state = store
            .load_or_new(SystemClock.now_unix(), serve_balance()?)
            .map_err(|e| ServeError {
                message: e.to_string(),
                exit: crate::cli::EXIT_INTERNAL,
            })?;
        state.require_fair_play().map_err(|e| ServeError {
            message: e.to_string(),
            exit: crate::cli::EXIT_REFUSED,
        })?;
        let live = Live {
            updates: None,
            config_dir: opts.save.parent().unwrap_or(Path::new(".")).to_path_buf(),
            stats: ServerStats::new(),
            last_save_tick: state.last_tick_unix,
            saved_next_id: state.history.next_id,
            state,
            terminal: None,
            terminal_settings: crate::terminal::Settings::default(),
            terminal_keys: crate::keymap::Keymap::default(),
            next_terminal_id: 1,
        };
        let push = crate::push::Gate::new(
            &crate::push::Settings::default(),
            &live.state,
            &opts.save,
            None,
        );
        let listener = TcpListener::bind(opts.bind)?;
        Ok(Self {
            listener,
            _lock: lock,
            save: opts.save,
            auth: Auth::empty(),
            live: Arc::new(Mutex::new(live)),
            push: Arc::new(Mutex::new(push)),
            oidc: Arc::new(Mutex::new(crate::oidc::Gate::noop(
                crate::oidc::Settings::default(),
            ))),
            ui: Arc::new(crate::web_ui::Ui::new(None)),
        })
    }

    /// Serve the client from `dir` instead of the embedded bundle.
    pub fn with_ui_dir(mut self, dir: PathBuf) -> Self {
        self.ui = Arc::new(crate::web_ui::Ui::new(Some(dir)));
        self
    }

    pub fn with_updates(self, updates: crate::update::CheckSession) -> Self {
        lock_poison(&self.live).updates = Some(updates);
        self
    }

    pub fn with_terminal(
        self,
        settings: crate::terminal::Settings,
        keys: crate::keymap::Keymap,
    ) -> Self {
        {
            let mut live = lock_poison(&self.live);
            live.terminal_settings = settings;
            live.terminal_keys = keys;
        }
        self
    }

    pub fn with_auth(mut self, auth: Auth) -> Self {
        self.auth = auth;
        self
    }

    pub fn with_push(self, settings: crate::push::Settings) -> Self {
        self.with_push_transport(settings, None)
    }

    /// `None` sends over HTTPS; tests inject a recording transport.
    fn with_push_transport(
        mut self,
        settings: crate::push::Settings,
        transport: Option<Arc<dyn crate::push::Transport>>,
    ) -> Self {
        let gate = {
            let live = lock_poison(&self.live);
            crate::push::Gate::new(&settings, &live.state, &self.save, transport)
        };
        self.push = Arc::new(Mutex::new(gate));
        self
    }

    pub fn with_oidc(mut self, settings: crate::oidc::Settings) -> Self {
        self.oidc = Arc::new(Mutex::new(crate::oidc::Gate::noop(settings)));
        self
    }

    #[cfg(test)]
    pub fn with_oidc_exchange(
        mut self,
        settings: crate::oidc::Settings,
        transport: Box<dyn crate::oidc::TokenExchange>,
    ) -> Self {
        self.oidc = Arc::new(Mutex::new(crate::oidc::Gate::new(settings, transport)));
        self
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ServeError> {
        Ok(self.listener.local_addr()?)
    }

    /// Serve until the listener fails. Tests call [`serve_one`] instead.
    pub fn run(self) -> Result<(), ServeError> {
        let bind = self.listener.local_addr()?;
        let save = Arc::new(self.save);
        let auth = Arc::new(Mutex::new(self.auth));
        let live = self.live;
        let push = self.push;
        let oidc = self.oidc;
        let ui = self.ui;
        let inflight = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        if lock_poison(push.as_ref()).armed() {
            inflight.fetch_add(1, Ordering::SeqCst);
            let save = Arc::clone(&save);
            let live = Arc::clone(&live);
            let push = Arc::clone(&push);
            let stop = Arc::clone(&stop);
            let worker = Inflight(Arc::clone(&inflight));
            let _ = std::thread::Builder::new()
                .name("hoshi-push".into())
                .spawn(move || {
                    let _worker = worker;
                    let outboxes = spawn_push_delivery(Arc::clone(&push));
                    while !stop.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_secs(1));
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        if !lock_poison(push.as_ref()).armed() {
                            continue;
                        }
                        let now = SystemClock.now_unix();
                        {
                            let mut live = lock_poison(live.as_ref());
                            if !live.state.deterministic {
                                live.state.catch_up(now);
                            }
                            lock_poison(push.as_ref()).poll(&live.state, now);
                            let _ = persist(save.as_path(), &mut live, false);
                        }
                        let outbox = lock_poison(push.as_ref()).take_outbox();
                        if !outbox.is_empty()
                            && let Some(outboxes) = &outboxes
                            && outboxes.try_send(outbox).is_err()
                        {
                            crate::diagnostics::log(
                                "warn",
                                "web push backlog full; notice dropped",
                            );
                        }
                    }
                });
        }
        let accept_err = loop {
            let (stream, _) = match self.listener.accept() {
                Ok(pair) => pair,
                Err(err) => break err,
            };
            inflight.fetch_add(1, Ordering::SeqCst);
            let save = Arc::clone(&save);
            let auth = Arc::clone(&auth);
            let live = Arc::clone(&live);
            let push = Arc::clone(&push);
            let oidc = Arc::clone(&oidc);
            let ui = Arc::clone(&ui);
            let worker = Inflight(Arc::clone(&inflight));
            let _ = std::thread::Builder::new()
                .name("hoshi-serve".into())
                .spawn(move || {
                    let _worker = worker;
                    let _ = serve_one(
                        save.as_path(),
                        stream,
                        &auth,
                        live.as_ref(),
                        push.as_ref(),
                        oidc.as_ref(),
                        ui.as_ref().as_ref(),
                        bind,
                    );
                });
        };
        stop.store(true, Ordering::SeqCst);
        while inflight.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(accept_err.into())
    }
}

fn serve_balance() -> Result<BalanceConfig, ServeError> {
    match std::env::var_os("HOSHI_BALANCE") {
        Some(path) => BalanceConfig::load_file(Path::new(&path)).map_err(|err| ServeError {
            message: err.to_string(),
            exit: crate::cli::EXIT_INVALID,
        }),
        None => Ok(BalanceConfig::default()),
    }
}

/// JSON body for `GET /healthz`.
pub fn healthz_body() -> Value {
    let mut body = crate::update::version_body(None);
    body["ok"] = json!(true);
    body
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
        if a == "--save" || a == "--balance" || a == "--bind" || a == "--config" || a == "--ui-dir"
        {
            i += 2;
            continue;
        }
        if a.starts_with("--save=") || a.starts_with("--bind=") || a.starts_with("--config=") {
            i += 1;
            continue;
        }
        if matches!(a, "--update-check" | "--offline") {
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

/// Bind, save, config dir and `--ui-dir`, each when given.
type ServeArgs = (
    Option<String>,
    Option<String>,
    Option<PathBuf>,
    Option<PathBuf>,
);

pub fn parse_serve_args(args: &[String]) -> Result<ServeArgs, ServeError> {
    let mut bind = None;
    let mut save = None;
    let mut config_dir = None;
    let mut ui_dir = None;
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
        if a == "--ui-dir" {
            let value = args.get(i + 1).ok_or_else(|| ServeError {
                message: "missing --ui-dir directory".into(),
                exit: crate::cli::EXIT_INVALID,
            })?;
            ui_dir = Some(PathBuf::from(value));
            i += 2;
            continue;
        }
        if matches!(a, "--update-check" | "--offline") {
            i += 1;
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
    Ok((bind, save, config_dir, ui_dir))
}

fn error_body(code: &str, message: &str) -> Value {
    json!({
        "schema_version": crate::cli::SCHEMA_VERSION,
        "ok": false,
        "error": { "code": code, "message": message },
    })
}

/// Public stream type for a history kind. Hidden kinds stay off the wire.
fn stream_type(kind: HistoryKind) -> Option<&'static str> {
    match kind {
        HistoryKind::Build => Some("build"),
        HistoryKind::Research => Some("research"),
        HistoryKind::Fleet => Some("fleet"),
        HistoryKind::Combat => Some("raid"),
        _ => None,
    }
}

fn event_frame(event: &HistoryEvent) -> Option<Value> {
    let kind = stream_type(event.kind)?;
    Some(json!({
        "schema_version": crate::cli::SCHEMA_VERSION,
        "ok": true,
        "id": event.id,
        "cursor": event.id,
        "at": event.at,
        "type": kind,
        "action": event.action,
        "objects": event.objects,
        "outcome": event.outcome,
    }))
}

/// History rows with `id > cursor` that the stream is allowed to send.
fn events_after(events: &[HistoryEvent], cursor: u64) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event.id > cursor)
        .filter_map(event_frame)
        .collect()
}

fn parse_cursor(path: &str) -> Result<u64, (u16, Value)> {
    let Some((_, query)) = path.split_once('?') else {
        return Ok(0);
    };
    for pair in query.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name == "cursor" {
            return value
                .parse()
                .map_err(|_| (400, error_body("invalid", "invalid cursor")));
        }
    }
    Ok(0)
}

fn lock_poison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn persist(save: &Path, live: &mut Live, force: bool) -> Result<(), ServeError> {
    let due = force
        || live.state.history.next_id != live.saved_next_id
        || live
            .state
            .last_tick_unix
            .saturating_sub(live.last_save_tick)
            >= SAVE_IDLE_SECS;
    if !due {
        return Ok(());
    }
    let mut store = open_storage(save)?;
    store.save(&live.state)?;
    live.last_save_tick = live.state.last_tick_unix;
    live.saved_next_id = live.state.history.next_id;
    Ok(())
}

fn status_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
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
        "/v1/state"
            | "/v1/build"
            | "/v1/dismantle"
            | "/v1/fusion/mode"
            | "/v1/research"
            | "/v1/cancel"
            | "/v1/fleet/send"
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
        "/v1/build" | "/v1/dismantle" => vec![
            if path == "/v1/dismantle" {
                "dismantle".into()
            } else {
                "build".into()
            },
            require_str(body, "planet")
                .map_err(|m| (400, error_body("invalid", &m)))?
                .into(),
            require_str(body, "building")
                .map_err(|m| (400, error_body("invalid", &m)))?
                .into(),
        ],
        "/v1/fusion/mode" => vec![
            "fusion".into(),
            "mode".into(),
            require_str(body, "planet")
                .map_err(|m| (400, error_body("invalid", &m)))?
                .into(),
            require_str(body, "mode")
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
#[cfg(test)]
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
    let credential = authorization.map(|header| Credential::Header(header.into()));
    if path.starts_with("/v1")
        && let Err(err) = auth.allow(save, path, credential.as_ref(), now)
    {
        return err;
    }
    if path == "/v1/events" {
        return (400, error_body("invalid", "websocket upgrade required"));
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
    command_result(out)
}

/// Parse JSON and map the route before touching Live. Invalid calls must not catch up.
fn parse_v1(method: &str, path: &str, body: &[u8]) -> Result<Vec<String>, (u16, Value)> {
    let path = path.split('?').next().unwrap_or(path);
    let json_body = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(value) => value,
            Err(_) => return Err((400, error_body("invalid", "invalid json"))),
        }
    };
    v1_parts(method, path, &json_body)
}

fn command_result(out: crate::cli::CliOutcome) -> (u16, Value) {
    let value: Value = serde_json::from_str(&out.json)
        .unwrap_or_else(|_| error_body("internal", "malformed command result"));
    (http_status(out.exit), value)
}

/// Catch up, apply, and persist. A save error restores Live so a retry cannot double-apply.
fn mutate_live(save: &Path, live: &mut Live, parts: &[String], now: u64) -> (u16, Value) {
    let snapshot = live.clone();
    if !live.state.deterministic {
        live.state.catch_up(now);
    }
    let words: Vec<&str> = parts.iter().map(String::as_str).collect();
    let (status, value) = command_result(crate::cli::apply_unlocked(&mut live.state, save, &words));
    if persist(save, live, true).is_err() {
        *live = snapshot;
        return (500, error_body("internal", "save failed"));
    }
    (status, value)
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

fn write_http(
    stream: &mut TcpStream,
    status: u16,
    value: &Value,
    extra: &str,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    let header = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        status_phrase(status),
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&body)
}

fn callback_addr(stream: &TcpStream, bind: SocketAddr) -> SocketAddr {
    concrete_socket(stream.local_addr().unwrap_or(bind))
}

fn concrete_socket(addr: SocketAddr) -> SocketAddr {
    if !addr.ip().is_unspecified() {
        return addr;
    }
    match addr {
        SocketAddr::V6(v6) => SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, v6.port())),
        SocketAddr::V4(v4) => SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, v4.port())),
    }
}

fn write_redirect(stream: &mut TcpStream, location: &str) -> std::io::Result<()> {
    let location = location.replace(['\r', '\n'], "");
    let header = format!(
        "HTTP/1.1 302 {}\r\nLocation: {location}\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        status_phrase(302),
    );
    stream.write_all(header.as_bytes())
}

/// `/` and `/ui` redirect to `ui/` (relative, so a proxy sub-path keeps
/// working); `/ui/...` serves the client with a strict CSP.
fn serve_ui(
    stream: &mut TcpStream,
    method: &str,
    route: &str,
    ui: &crate::web_ui::Ui,
) -> std::io::Result<()> {
    if method != "GET" && method != "HEAD" {
        return write_http(
            stream,
            405,
            &error_body("invalid", "method not allowed"),
            "Allow: GET, HEAD\r\n",
        );
    }
    let Some(rel) = route.strip_prefix("/ui/") else {
        return write_redirect(stream, "ui/");
    };
    let asset = match ui.get(rel) {
        Some(crate::web_ui::Reply::File(asset)) => asset,
        Some(crate::web_ui::Reply::Redirect(location)) => return write_redirect(stream, &location),
        None => return write_http(stream, 404, &error_body("invalid", "not found"), ""),
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\nContent-Security-Policy: {}\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        asset.content_type,
        asset.body.len(),
        asset.cache_control,
        crate::web_ui::CSP,
    );
    stream.write_all(header.as_bytes())?;
    if method == "GET" {
        stream.write_all(&asset.body)?;
    }
    Ok(())
}

/// Network sends run on their own thread, without the game or gate lock, so a
/// slow push service never delays the next raid-warning poll. The queue is
/// bounded; the thread ends when the sender drops.
fn spawn_push_delivery(
    push: Arc<Mutex<crate::push::Gate>>,
) -> Option<std::sync::mpsc::SyncSender<crate::push::Outbox>> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<crate::push::Outbox>(4);
    std::thread::Builder::new()
        .name("hoshi-push-send".into())
        .spawn(move || {
            for outbox in rx {
                for endpoint in outbox.deliver() {
                    let _ = lock_poison(push.as_ref()).unsubscribe(&endpoint);
                }
            }
        })
        .ok()?;
    Some(tx)
}

#[allow(clippy::too_many_arguments)]
fn serve_one(
    save: &Path,
    mut stream: TcpStream,
    auth: &Mutex<Auth>,
    live: &Mutex<Live>,
    push: &Mutex<crate::push::Gate>,
    oidc: &Mutex<crate::oidc::Gate>,
    ui: Option<&crate::web_ui::Ui>,
    bind: SocketAddr,
) -> std::io::Result<()> {
    {
        let mut live = lock_poison(live);
        live.stats.clients += 1;
        live.stats.requests += 1;
    }
    let _client = Client(live);
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
    let now = SystemClock.now_unix();
    let route = path.split('?').next().unwrap_or(path);
    let credential = match Credential::from_headers(&headers) {
        Ok(credential) => credential,
        Err((status, value)) if route.starts_with("/v1") => {
            return write_http(&mut stream, status, &value, "");
        }
        Err(_) => None,
    };
    // A cookie rides along on cross-site requests; a cross-site form cannot
    // send JSON, so writes with the cookie must.
    if route.starts_with("/v1")
        && Credential::is_cookie(credential.as_ref())
        && !matches!(method, "GET" | "HEAD" | "OPTIONS")
        && !header_value(&headers, "content-type").is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return write_http(
            &mut stream,
            403,
            &error_body("unauthorized", "cookie requests must send application/json"),
            "",
        );
    }
    if route == "/v1/bug"
        && crate::config::UserConfig::load(&lock_poison(live).config_dir).bug_report_target == "off"
    {
        return write_http(&mut stream, 404, &error_body("invalid", "not found"), "");
    }
    if route == "/healthz" {
        if method != "GET" {
            return write_http(
                &mut stream,
                405,
                &error_body("invalid", "method not allowed"),
                "",
            );
        }
        let mut body = healthz_body();
        body["latest"] = crate::update::version_body(
            lock_poison(live)
                .updates
                .as_ref()
                .and_then(crate::update::CheckSession::release),
        )["latest"]
            .clone();
        return write_http(&mut stream, 200, &body, "");
    }
    if let Some(ui) = ui
        && (route == "/" || route == "/ui" || route.starts_with("/ui/"))
    {
        return serve_ui(&mut stream, method, route, ui);
    }
    if route == "/v1/admin/support-report" {
        return serve_support_report(
            save,
            &mut stream,
            method,
            credential.as_ref(),
            auth,
            live,
            bind,
        );
    }
    if route == "/v1/oidc/start" || route == "/v1/oidc/callback" {
        return serve_oidc(save, &mut stream, method, path, auth, oidc, bind);
    }
    if route == "/v1/events" {
        if method != "GET" {
            return write_http(
                &mut stream,
                405,
                &error_body("invalid", "method not allowed"),
                "",
            );
        }
        return serve_events(
            save,
            &mut stream,
            path,
            &headers,
            credential.as_ref(),
            auth,
            live,
            push,
        );
    }
    if route == "/v1/terminal" {
        if method != "GET" {
            return write_http(
                &mut stream,
                405,
                &error_body("invalid", "method not allowed"),
                "",
            );
        }
        return serve_terminal(save, &mut stream, &headers, credential.as_ref(), auth, live);
    }
    if !route.starts_with("/v1") {
        return write_http(&mut stream, 404, &error_body("invalid", "not found"), "");
    }
    let gate = {
        let mut auth = lock_poison(auth);
        match auth.allow(save, route, credential.as_ref(), now) {
            Ok(()) => Ok(auth.window_secs),
            Err((status, value)) => Err((status, value, auth.window_secs)),
        }
    };
    if let Err((status, value, retry_after)) = gate {
        crate::diagnostics::log(
            "warn",
            if status == 429 {
                "server rate limit exceeded"
            } else {
                "server authentication failed"
            },
        );
        if status == 429 {
            lock_poison(live).stats.rate_limit_hits += 1;
        }
        let extra = match status {
            401 => "WWW-Authenticate: Bearer\r\n".to_string(),
            429 => format!("Retry-After: {retry_after}\r\n"),
            _ => String::new(),
        };
        return write_http(&mut stream, status, &value, &extra);
    }
    if route == "/v1/bug" {
        let config_dir = lock_poison(live).config_dir.clone();
        let target = crate::config::UserConfig::load(&config_dir).bug_report_target;
        if method == "GET" {
            return write_http(&mut stream, 200, &json!({"ok": true, "target": target}), "");
        }
        if method != "POST" {
            return write_http(
                &mut stream,
                405,
                &error_body("invalid", "method not allowed"),
                "",
            );
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Request {
            fields: crate::bug::Fields,
            #[serde(default)]
            confirm: bool,
        }
        let request: Request = match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(_) => {
                return write_http(
                    &mut stream,
                    400,
                    &error_body("invalid", "invalid bug-report fields"),
                    "",
                );
            }
        };
        let prepared = match crate::bug::prepare(save, &config_dir, &request.fields, &target) {
            Ok(prepared) => prepared,
            Err(_) => {
                return write_http(
                    &mut stream,
                    400,
                    &error_body("invalid", "bug report could not be prepared"),
                    "",
                );
            }
        };
        let report_id = if request.confirm {
            match crate::bug::save(&prepared, save) {
                Ok(path) => path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
                Err(_) => {
                    return write_http(
                        &mut stream,
                        500,
                        &error_body("internal", "report could not be saved"),
                        "",
                    );
                }
            }
        } else {
            None
        };
        return write_http(
            &mut stream,
            200,
            &json!({"ok": true, "url": prepared.url, "search_url": prepared.search_url, "report_id": report_id, "submitted": false}),
            "",
        );
    }
    if route == "/v1/push/key" || route == "/v1/push/subscription" {
        return serve_push(&mut stream, method, route, &body, push);
    }
    if route == "/v1/version" {
        if method != "GET" {
            return write_http(
                &mut stream,
                405,
                &error_body("invalid", "method not allowed"),
                "",
            );
        }
        let latest = lock_poison(live)
            .updates
            .as_ref()
            .and_then(crate::update::CheckSession::release);
        return write_http(&mut stream, 200, &crate::update::version_body(latest), "");
    }
    let parts = match parse_v1(method, path, &body) {
        Ok(parts) => parts,
        Err((status, value)) => return write_http(&mut stream, status, &value, ""),
    };
    let (status, value) = {
        let mut live = lock_poison(live);
        // Game actions are POSTs; reads (GET /v1/state) keep working.
        if method != "GET" && live.terminal_holds(now) {
            drop(live);
            return write_http(
                &mut stream,
                409,
                &error_body("refused", crate::terminal::HELD),
                "",
            );
        }
        let result = mutate_live(save, &mut live, &parts, now);
        lock_poison(push).poll(&live.state, now);
        result
    };
    write_http(&mut stream, status, &value, "")
}

/// Browser opt-in: read the VAPID key, register or remove a subscription.
/// Subscriptions go to the file beside the save, never into the save.
fn serve_push(
    stream: &mut TcpStream,
    method: &str,
    route: &str,
    body: &[u8],
    push: &Mutex<crate::push::Gate>,
) -> std::io::Result<()> {
    use crate::push::Refused;
    let mut gate = lock_poison(push);
    let Some(key) = gate.public_key().map(str::to_string) else {
        return write_http(stream, 404, &error_body("invalid", "not found"), "");
    };
    let result = match (route, method) {
        ("/v1/push/key", "GET") => {
            return write_http(stream, 200, &json!({"ok": true, "key": key}), "");
        }
        ("/v1/push/subscription", "POST") => {
            match serde_json::from_slice::<crate::push::Subscription>(body) {
                Ok(sub) => gate.subscribe(sub).map(|()| json!({"ok": true})),
                Err(_) => Err(Refused::Invalid),
            }
        }
        ("/v1/push/subscription", "DELETE") => {
            #[derive(serde::Deserialize)]
            struct Request {
                endpoint: String,
            }
            match serde_json::from_slice::<Request>(body) {
                Ok(request) => gate
                    .unsubscribe(&request.endpoint)
                    .map(|removed| json!({"ok": true, "removed": removed})),
                Err(_) => Err(Refused::Invalid),
            }
        }
        _ => {
            return write_http(
                stream,
                405,
                &error_body("invalid", "method not allowed"),
                "",
            );
        }
    };
    drop(gate);
    let (status, value) = match result {
        Ok(value) => (200, value),
        Err(Refused::Off) => (404, error_body("invalid", "not found")),
        Err(Refused::Invalid) => (400, error_body("invalid", "invalid push subscription")),
        Err(Refused::Full) => (409, error_body("refused", "too many push subscriptions")),
        Err(Refused::Storage) => (
            500,
            error_body("internal", "subscription could not be saved"),
        ),
    };
    write_http(stream, status, &value, "")
}

fn serve_support_report(
    save: &Path,
    stream: &mut TcpStream,
    method: &str,
    credential: Option<&Credential>,
    auth: &Mutex<Auth>,
    live: &Mutex<Live>,
    bind: SocketAddr,
) -> std::io::Result<()> {
    let config_dir = lock_poison(live).config_dir.clone();
    let dir = config_dir.as_path();
    if !crate::config::UserConfig::load(dir).serve_support_report {
        return write_http(stream, 404, &error_body("invalid", "not found"), "");
    }
    if method != "GET" {
        return write_http(
            stream,
            405,
            &error_body("invalid", "method not allowed"),
            "",
        );
    }
    let gate = {
        let mut auth = lock_poison(auth);
        if auth.tokens.is_empty()
            && credential.is_none()
            && bind.ip().is_loopback()
            && stream.peer_addr()?.ip().is_loopback()
        {
            Ok(())
        } else {
            auth.allow(
                save,
                "/v1/admin/support-report",
                credential,
                SystemClock.now_unix(),
            )
        }
    };
    if let Err((status, body)) = gate {
        if status == 429 {
            lock_poison(live).stats.rate_limit_hits += 1;
        }
        crate::diagnostics::log("warn", "support report access refused");
        return write_http(stream, status, &body, "");
    }
    let mut report = match crate::support::build(
        save,
        false,
        100,
        std::env::var_os("HOSHI_BALANCE").as_deref().map(Path::new),
        false,
        Some(dir),
    ) {
        Ok(report) => report,
        Err(error) => {
            crate::diagnostics::log("error", &error);
            return write_http(
                stream,
                500,
                &error_body("internal", "support report unavailable"),
                "",
            );
        }
    };
    let stats = lock_poison(live).stats.value();
    let health = healthz_body();
    let extra = crate::redact::Redactor::for_dir(dir)
        .redact(&format!(
            "\n## Live server\n\nHealth: {health}\nStatistics: {stats}\n"
        ))
        .text;
    report.markdown.push_str(&extra);
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/markdown; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        report.markdown.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(report.markdown.as_bytes())
}

fn serve_oidc(
    save: &Path,
    stream: &mut TcpStream,
    method: &str,
    path: &str,
    auth: &Mutex<Auth>,
    oidc: &Mutex<crate::oidc::Gate>,
    bind: SocketAddr,
) -> std::io::Result<()> {
    let route = path.split('?').next().unwrap_or(path);
    if method != "GET" {
        return write_http(
            stream,
            405,
            &error_body("invalid", "method not allowed"),
            "",
        );
    }
    let redirect = format!("http://{}/v1/oidc/callback", callback_addr(stream, bind));
    if route == "/v1/oidc/start" {
        let location = {
            let mut oidc = lock_poison(oidc);
            if !oidc.armed() {
                return write_http(stream, 404, &error_body("invalid", "not found"), "");
            }
            let entropy = match crate::oidc::entropy() {
                Ok(bytes) => bytes,
                Err(_) => {
                    return write_http(stream, 500, &error_body("internal", "entropy failed"), "");
                }
            };
            match oidc.start(&redirect, entropy) {
                Ok(location) => location,
                Err(_) => {
                    return write_http(stream, 404, &error_body("invalid", "not found"), "");
                }
            }
        };
        return write_redirect(stream, &location);
    }
    let code = crate::oidc::query_value(path, "code").unwrap_or_default();
    let state = crate::oidc::query_value(path, "state").unwrap_or_default();
    {
        let mut oidc = lock_poison(oidc);
        if !oidc.armed() {
            return write_http(stream, 404, &error_body("invalid", "not found"), "");
        }
        match oidc.callback(&code, &state, &redirect) {
            Ok(_) => {}
            Err(crate::oidc::CallbackError::Off) => {
                return write_http(stream, 404, &error_body("invalid", "not found"), "");
            }
            Err(crate::oidc::CallbackError::Invalid) => {
                return write_http(stream, 400, &error_body("invalid", "invalid"), "");
            }
            Err(_) => {
                return write_http(
                    stream,
                    401,
                    &error_body("unauthorized", "unauthorized"),
                    "WWW-Authenticate: Bearer\r\n",
                );
            }
        }
    }
    let secret = match crate::oidc::secret_entropy() {
        Ok(bytes) => crate::oidc::secret_from_entropy(bytes),
        Err(_) => {
            return write_http(stream, 500, &error_body("internal", "entropy failed"), "");
        }
    };
    {
        let mut auth = lock_poison(auth);
        auth.mint(save, secret.clone());
    }
    write_http(
        stream,
        200,
        &json!({
            "schema_version": crate::cli::SCHEMA_VERSION,
            "ok": true,
            "token": secret,
        }),
        "Cache-Control: no-store\r\n",
    )
}

/// A checked WebSocket upgrade, not yet answered.
struct Upgrade {
    key: String,
    v1: bool,
}

impl Upgrade {
    fn open(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        ws::write_opening(stream, &self.key, self.v1.then_some(ws::PROTOCOL))
    }
}

/// Auth and handshake checks shared by `/v1/events` and `/v1/terminal`. The
/// token may come in the `Authorization` header or the browser subprotocol,
/// not both. On refusal the HTTP error is written and `None` returned.
fn authorize_upgrade(
    save: &Path,
    stream: &mut TcpStream,
    route: &str,
    headers: &str,
    credential: Option<&Credential>,
    auth: &Mutex<Auth>,
    now: u64,
) -> std::io::Result<Option<Upgrade>> {
    let Ok(offer) = ws::protocol_offer(headers) else {
        write_http(
            stream,
            400,
            &error_body("invalid", "invalid websocket protocol"),
            "",
        )?;
        return Ok(None);
    };
    let credential = match (credential, offer.token) {
        (Some(_), Some(_)) => {
            write_http(
                stream,
                400,
                &error_body("invalid", "send the token in one place"),
                "",
            )?;
            return Ok(None);
        }
        (None, Some(token)) => Some(Credential::Header(format!("Bearer {token}"))),
        (credential, None) => credential.cloned(),
    };
    // Any site can open a socket here and the browser adds the cookie, so a
    // cookie upgrade must come from a page on this host.
    if Credential::is_cookie(credential.as_ref()) && !same_origin(headers) {
        write_http(
            stream,
            403,
            &error_body("unauthorized", "cross-origin socket refused"),
            "",
        )?;
        return Ok(None);
    }
    let gate = lock_poison(auth).allow(save, route, credential.as_ref(), now);
    if let Err((status, value)) = gate {
        let extra = match status {
            401 => "WWW-Authenticate: Bearer\r\n",
            _ => "",
        };
        write_http(stream, status, &value, extra)?;
        return Ok(None);
    }
    let key = ws::websocket_key(headers)
        .filter(|_| ws::is_upgrade(headers) && ws::websocket_version_ok(headers));
    let Some(key) = key else {
        write_http(
            stream,
            400,
            &error_body("invalid", "websocket upgrade required"),
            "",
        )?;
        return Ok(None);
    };
    Ok(Some(Upgrade { key, v1: offer.v1 }))
}

/// `Origin` names the host the request was sent to (`Host`), any scheme.
fn same_origin(headers: &str) -> bool {
    let (Some(origin), Some(host)) = (
        header_value(headers, "origin"),
        header_value(headers, "host"),
    ) else {
        return false;
    };
    origin
        .split_once("://")
        .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host.trim()))
}

/// Swaps the server's game state into the session's app for one step and
/// back on drop, so the server keeps the only copy even if the step panics.
struct Lent<'a> {
    live: std::sync::MutexGuard<'a, Live>,
    session: &'a mut crate::terminal::Session,
}

impl<'a> Lent<'a> {
    fn new(live: &'a Mutex<Live>, session: &'a mut crate::terminal::Session) -> Self {
        let mut live = lock_poison(live);
        std::mem::swap(&mut session.app.state, &mut live.state);
        Self { live, session }
    }
}

impl Drop for Lent<'_> {
    fn drop(&mut self) {
        std::mem::swap(&mut self.session.app.state, &mut self.live.state);
    }
}

/// How a terminal session ended.
enum Ending {
    /// Quit, close frame or idle timeout: the save is free at once.
    Clean,
    /// The socket broke: the save stays held for the grace period.
    Dropped,
}

/// `GET /v1/terminal`: run the TUI for one browser terminal. See
/// [`crate::terminal`] and docs/server.md.
fn serve_terminal(
    save: &Path,
    stream: &mut TcpStream,
    headers: &str,
    credential: Option<&Credential>,
    auth: &Mutex<Auth>,
    live: &Mutex<Live>,
) -> std::io::Result<()> {
    let now = SystemClock.now_unix();
    let Some(upgrade) =
        authorize_upgrade(save, stream, "/v1/terminal", headers, credential, auth, now)?
    else {
        return Ok(());
    };
    let (lease_id, settings, keys, state) = {
        let mut live = lock_poison(live);
        if live.terminal_holds(now) {
            drop(live);
            return write_http(
                stream,
                409,
                &error_body("refused", crate::terminal::HELD),
                "",
            );
        }
        let id = live.next_terminal_id;
        live.next_terminal_id += 1;
        live.terminal = Some(crate::terminal::Lease { id, left_at: None });
        (
            id,
            live.terminal_settings,
            live.terminal_keys.clone(),
            live.state.clone(),
        )
    };
    let mut release = Release {
        save,
        live,
        lease_id,
        ending: Ending::Dropped,
    };
    if upgrade.open(stream).is_ok() {
        release.ending = run_terminal(save, stream, live, state, keys, settings);
    }
    Ok(())
}

/// Gives the save back when a terminal session ends, even by a panic: a
/// clean end frees it at once, anything else starts the grace period.
struct Release<'a> {
    save: &'a Path,
    live: &'a Mutex<Live>,
    lease_id: u64,
    ending: Ending,
}

impl Drop for Release<'_> {
    fn drop(&mut self) {
        let mut live = lock_poison(self.live);
        // The session's App turned on round-by-round raids for its player;
        // the headless server finishes fights on its own again.
        live.state.interactive_tactics = false;
        let _ = persist(self.save, &mut live, true);
        // A newer session may have taken over after the grace; leave it alone.
        if live.terminal.is_some_and(|lease| lease.id == self.lease_id) {
            live.terminal = match self.ending {
                Ending::Clean => None,
                Ending::Dropped => Some(crate::terminal::Lease {
                    id: self.lease_id,
                    left_at: Some(SystemClock.now_unix()),
                }),
            };
        }
    }
}

/// Frames per second the session redraws at when nothing happens.
const TERMINAL_TICK: Duration = Duration::from_millis(250);
/// The server pings this often; browsers answer pings on their own.
const TERMINAL_PING: Duration = Duration::from_secs(5);
/// No frame at all from the client for this long counts as a dropped
/// socket, so a half-open connection cannot hold the save past the grace.
const TERMINAL_SILENCE: Duration = Duration::from_secs(15);

/// Save the server's state. Only call it while no [`Lent`] is alive: during a
/// step the server holds the session's placeholder, not the game.
fn persist_live(save: &Path, live: &Mutex<Live>) {
    let _ = persist(save, &mut lock_poison(live), false);
}

fn run_terminal(
    save: &Path,
    stream: &mut TcpStream,
    live: &Mutex<Live>,
    state: GameState,
    keys: crate::keymap::Keymap,
    settings: crate::terminal::Settings,
) -> Ending {
    let app = crate::app::App::with_keymap(state, keys);
    let Ok(mut session) = crate::terminal::Session::new(app) else {
        return Ending::Dropped;
    };
    let mut incoming = ws::Reader::new();
    let mut last_input = std::time::Instant::now();
    let mut last_seen = std::time::Instant::now();
    let mut last_ping = std::time::Instant::now();
    let mut last_draw: Option<std::time::Instant> = None;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    loop {
        let mut dirty = false;
        let frame = incoming.poll(stream);
        if matches!(frame, Ok(Some(_))) {
            last_seen = std::time::Instant::now();
        }
        match frame {
            Ok(Some(ws::Frame::Binary(bytes))) => {
                last_input = std::time::Instant::now();
                let now = SystemClock.now_unix();
                let quit = Lent::new(live, &mut session).session.input(&bytes, now);
                persist_live(save, live);
                if quit {
                    let _ = ws::write_close(stream);
                    return Ending::Clean;
                }
                dirty = true;
            }
            Ok(Some(ws::Frame::Text(text))) => {
                if let Some((cols, rows)) = crate::terminal::parse_resize(&text) {
                    if session.screen.resize(cols, rows).is_err() {
                        return Ending::Dropped;
                    }
                    dirty = true;
                }
            }
            Ok(Some(ws::Frame::Ping(payload))) => {
                let _ = ws::write_pong(stream, &payload);
            }
            Ok(Some(ws::Frame::Close)) => {
                let _ = ws::write_close(stream);
                return Ending::Clean;
            }
            Ok(Some(ws::Frame::Pong | ws::Frame::Other) | None) => {}
            Err(_) => return Ending::Dropped,
        }
        if last_seen.elapsed() >= TERMINAL_SILENCE {
            return Ending::Dropped;
        }
        if last_ping.elapsed() >= TERMINAL_PING {
            if ws::write_ping(stream).is_err() {
                return Ending::Dropped;
            }
            last_ping = std::time::Instant::now();
        }
        if settings.idle_secs > 0 && last_input.elapsed() >= Duration::from_secs(settings.idle_secs)
        {
            let _ = ws::write_close(stream);
            return Ending::Clean;
        }
        if dirty || last_draw.is_none_or(|at| at.elapsed() >= TERMINAL_TICK) {
            let now = SystemClock.now_unix();
            let bytes = {
                let lent = Lent::new(live, &mut session);
                let session = &mut *lent.session;
                session.app.catch_up_now(now);
                session.screen.draw(&session.app)
            };
            persist_live(save, live);
            match bytes {
                Ok(bytes) if !bytes.is_empty() => {
                    if ws::write_binary(stream, &bytes).is_err() {
                        return Ending::Dropped;
                    }
                }
                Ok(_) => {}
                Err(_) => return Ending::Dropped,
            }
            last_draw = Some(std::time::Instant::now());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn serve_events(
    save: &Path,
    stream: &mut TcpStream,
    path: &str,
    headers: &str,
    credential: Option<&Credential>,
    auth: &Mutex<Auth>,
    live: &Mutex<Live>,
    push: &Mutex<crate::push::Gate>,
) -> std::io::Result<()> {
    let now = SystemClock.now_unix();
    let Some(upgrade) =
        authorize_upgrade(save, stream, "/v1/events", headers, credential, auth, now)?
    else {
        return Ok(());
    };
    let cursor = match parse_cursor(path) {
        Ok(cursor) => cursor,
        Err((status, value)) => return write_http(stream, status, &value, ""),
    };
    upgrade.open(stream)?;
    let mut cursor = cursor;
    let mut announced_update = None;
    if pump_events(save, stream, live, push, &mut cursor, now).is_err() {
        return Ok(());
    }
    let mut incoming = ws::Reader::new();
    loop {
        stream.set_read_timeout(Some(Duration::from_millis(200)))?;
        match incoming.poll(stream) {
            Ok(Some(ws::Frame::Close)) => {
                let _ = ws::write_close(stream);
                return Ok(());
            }
            Ok(Some(ws::Frame::Ping(payload))) => {
                let _ = ws::write_pong(stream, &payload);
            }
            Ok(Some(ws::Frame::Text(payload))) => {
                if payload.len() > 65_536 {
                    return Ok(());
                }
            }
            Ok(Some(ws::Frame::Pong | ws::Frame::Binary(_) | ws::Frame::Other) | None) => {}
            Err(_) => return Ok(()),
        }
        let latest = lock_poison(live)
            .updates
            .as_ref()
            .and_then(crate::update::CheckSession::release);
        if let Some(release) = latest.filter(|release| {
            release.channel == crate::update::channel()
                && crate::update::newer(&release.version, crate::update::running_version())
        }) && announced_update.as_deref() != Some(release.version.as_str())
        {
            let event = json!({ "kind": "server_update_available", "version": release.version, "channel": release.channel });
            if ws::write_text(stream, event.to_string().as_bytes()).is_err() {
                return Ok(());
            }
            announced_update = Some(release.version);
        }
        let now = SystemClock.now_unix();
        if pump_events(save, stream, live, push, &mut cursor, now).is_err() {
            return Ok(());
        }
    }
}

fn pump_events(
    save: &Path,
    stream: &mut TcpStream,
    live: &Mutex<Live>,
    push: &Mutex<crate::push::Gate>,
    cursor: &mut u64,
    now: u64,
) -> std::io::Result<()> {
    let events = {
        let mut live = lock_poison(live);
        if !live.state.deterministic {
            live.state.catch_up(now);
        }
        lock_poison(push).poll(&live.state, now);
        let events = events_after(&live.state.history.events, *cursor);
        persist(save, &mut live, false).map_err(std::io::Error::other)?;
        events
    };
    for event in events {
        let payload = serde_json::to_vec(&event).map_err(std::io::Error::other)?;
        ws::write_text(stream, &payload)?;
        if let Some(id) = event.get("id").and_then(Value::as_u64) {
            *cursor = (*cursor).max(id);
        }
    }
    Ok(())
}

/// Start the serve loop. Prints one JSON line with the bound address, then blocks.
pub fn run(args: &[String]) -> Result<(), ServeError> {
    let (cli_bind, cli_save, config_dir, ui_dir) = parse_serve_args(args)?;
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
    crate::diagnostics::install(
        save.parent().unwrap_or(Path::new(".")).to_path_buf(),
        true,
        std::env::var("HOSHI_LOG").unwrap_or_else(|_| cfg.log_level.clone()),
        &dir,
    );
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
    let auth = auth.with_admin_token(&save, &cfg.serve_admin_token);
    let auth = match crate::devices::Devices::open(&save) {
        Ok(devices) => auth.with_devices(devices),
        Err(_) => {
            crate::diagnostics::log("warn", "device sessions unreadable; paired devices refused");
            auth
        }
    };
    crate::diagnostics::log("info", "server startup");
    let mut server = Server::open(ServeOptions { bind, save })?
        .with_auth(auth)
        .with_push(crate::push::Settings::from_config(&cfg))
        .with_oidc(crate::oidc::Settings::from_config(&cfg))
        .with_terminal(
            crate::terminal::Settings {
                idle_secs: cfg.serve_terminal_idle_secs,
                ..crate::terminal::Settings::default()
            },
            cfg.keys.clone(),
        );
    if let Some(ui_dir) = ui_dir {
        server = server.with_ui_dir(ui_dir);
    }
    lock_poison(&server.live).config_dir = dir.clone();
    if args.iter().any(|arg| arg == "--update-check") {
        let settings = crate::update::settings(&cfg, args);
        if let Ok(key) = crate::update::public_key() {
            server = server.with_updates(crate::update::CheckSession::start(
                dir,
                settings,
                key,
                SystemClock.now_unix(),
            ));
        }
    }
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
    use std::sync::Mutex;
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
        let (status, _headers, body) = get_full(addr, path, authorization)?;
        Ok((status, body))
    }

    fn get_full(
        addr: SocketAddr,
        path: &str,
        authorization: Option<&str>,
    ) -> std::io::Result<(u16, String, String)> {
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
        let (headers, body) = match buf.split_once("\r\n\r\n") {
            Some(pair) => pair,
            None => buf.split_once("\n\n").unwrap_or((buf.as_str(), "")),
        };
        Ok((status, headers.to_string(), body.to_string()))
    }

    fn header_line(headers: &str, want: &str) -> Option<String> {
        for line in headers.lines() {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if name.eq_ignore_ascii_case(want) {
                return Some(value.trim().to_string());
            }
        }
        None
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

    fn push_settings() -> crate::push::Settings {
        crate::push::Settings {
            opt_in: true,
            vapid_private: "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw".into(),
            vapid_subject: "mailto:hoshi@localhost".into(),
        }
    }

    /// Keeps server tests off the network; delivery is covered in `push`.
    struct NoNetwork;

    impl crate::push::Transport for NoNetwork {
        fn send(&self, _to: &crate::push::Subscription, _payload: &Value) -> crate::push::Delivery {
            crate::push::Delivery::Sent
        }
    }

    #[test]
    fn push_routes_are_off_without_vapid_keys() {
        let save = unique_save();
        seed_save(&save);
        let addr = spawn_run(ephemeral(save.clone()));
        let (status, body) =
            get_path_auth(addr, "/v1/push/key", Some(&format!("Bearer {TEST_TOKEN}"))).unwrap();
        assert_eq!(status, 404, "{body}");
        let (status, body) = post_json(addr, "/v1/push/subscription", "{}");
        assert_eq!(status, 404, "{body}");
        let (status, _) = get_path(addr, "/v1/push/key").unwrap();
        assert_eq!(status, 401);
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn browser_subscribes_and_unsubscribes_without_touching_the_save() {
        let save = unique_save();
        seed_save(&save);
        let server =
            ephemeral(save.clone()).with_push_transport(push_settings(), Some(Arc::new(NoNetwork)));
        let addr = spawn_run(server);

        let (status, body) =
            get_path_auth(addr, "/v1/push/key", Some(&format!("Bearer {TEST_TOKEN}"))).unwrap();
        assert_eq!(status, 200, "{body}");
        let key: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            key["key"],
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
        let subscription = r#"{"endpoint":"https://fcm.googleapis.com/fcm/send/hoshi-test-sub-a","expirationTime":null,"keys":{"p256dh":"BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4","auth":"BTBZMqHH6r4Tts7J_aSIgg"}}"#;
        let (status, body) = post_json(addr, "/v1/push/subscription", subscription);
        assert_eq!(status, 200, "{body}");
        let (status, body) = post_json(
            addr,
            "/v1/push/subscription",
            r#"{"endpoint":"https://fcm.googleapis.com/fcm/send/hoshi-test-sub-b","keys":{"p256dh":"x","auth":"y"}}"#,
        );
        assert_eq!(status, 400, "{body}");
        let in_save = std::fs::read(&save).unwrap();
        assert!(
            !in_save.windows(16).any(|w| w == b"hoshi-test-sub-a"),
            "subscription leaked into the save"
        );
        let stored = std::fs::read_to_string(crate::push::store_path(&save)).unwrap();
        assert!(stored.contains("hoshi-test-sub-a"), "{stored}");

        let (status, body) = send_json(
            addr,
            "DELETE",
            "/v1/push/subscription",
            r#"{"endpoint":"https://fcm.googleapis.com/fcm/send/hoshi-test-sub-a"}"#,
        );
        assert_eq!(status, 200, "{body}");
        assert!(body.contains(r#""removed":true"#), "{body}");
        let stored = std::fs::read_to_string(crate::push::store_path(&save)).unwrap();
        assert!(!stored.contains("hoshi-test-sub-a"), "{stored}");
        let _ = std::fs::remove_file(crate::push::store_path(&save));
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    fn oidc_settings() -> crate::oidc::Settings {
        crate::oidc::Settings {
            opt_in: true,
            issuer: "http://127.0.0.1/issuer".into(),
            client_id: "hoshi".into(),
            authorization: "http://127.0.0.1/authorize".into(),
            token: "http://127.0.0.1/token".into(),
        }
    }

    #[test]
    fn oidc_off_leaves_bearer_unchanged_and_start_is_not_a_login() {
        let save = unique_save();
        seed_save(&save);
        let server = ephemeral(save.clone());
        let addr = spawn_run(server);
        let (status, body) = get_path(addr, "/v1/oidc/start").unwrap();
        assert_eq!(status, 404, "{body}");
        let (status, body) =
            get_path_auth(addr, "/v1/state", Some(&format!("Bearer {TEST_TOKEN}"))).unwrap();
        assert_eq!(status, 200, "{body}");
        let (status, body) = get_path(addr, "/v1/state").unwrap();
        assert_eq!(status, 401, "{body}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn oidc_pkce_issues_a_save_scoped_token() {
        let save = unique_save();
        seed_save(&save);
        let rec = std::sync::Arc::new(crate::oidc::Recording::default());
        let server = ephemeral(save.clone())
            .with_auth(Auth::empty())
            .with_oidc_exchange(oidc_settings(), Box::new(std::sync::Arc::clone(&rec)));
        let addr = spawn_run(server);
        let (status, body) = get_path(addr, "/v1/state").unwrap();
        assert_eq!(status, 401, "{body}");
        let (status, headers, _body) = get_full(addr, "/v1/oidc/start", None).unwrap();
        assert_eq!(status, 302, "{headers}");
        let location = header_line(&headers, "location").expect("Location");
        assert!(
            location.contains("code_challenge_method=S256"),
            "{location}"
        );
        assert!(location.contains("response_type=code"), "{location}");
        let query = location.split_once('?').map(|(_, q)| q).unwrap_or("");
        let state = crate::oidc::query_value(&format!("x?{query}"), "state").expect("state");
        let challenge =
            crate::oidc::query_value(&format!("x?{query}"), "code_challenge").expect("challenge");
        let (status, body) = get_path(
            addr,
            &format!("/v1/oidc/callback?code=one-time&state={state}"),
        )
        .unwrap();
        assert_eq!(status, 200, "{body}");
        let value: Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(value["ok"], true);
        let token = value["token"].as_str().expect("token").to_string();
        assert!(!token.is_empty());
        let req = rec
            .last
            .lock()
            .expect("recording")
            .clone()
            .expect("exchanged");
        assert_eq!(req.code, "one-time");
        assert_eq!(crate::oidc::s256(&req.verifier), challenge);
        let (status, body) =
            get_path_auth(addr, "/v1/state", Some(&format!("Bearer {token}"))).unwrap();
        assert_eq!(status, 200, "{body}");
        let bytes = std::fs::read(&save).unwrap();
        assert!(
            !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
            "minted token must not land in the save"
        );
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn oidc_callback_maps_unspecified_bind_to_loopback() {
        let v4: SocketAddr = "0.0.0.0:8787".parse().unwrap();
        assert_eq!(concrete_socket(v4), "127.0.0.1:8787".parse().unwrap());
        let loopback: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert_eq!(concrete_socket(loopback), loopback);
        let v6: SocketAddr = "[::]:8787".parse().unwrap();
        assert_eq!(concrete_socket(v6), "[::1]:8787".parse().unwrap());
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
        send_json(addr, "POST", path, body)
    }

    fn send_json(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TEST_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
            let auth = Mutex::new(server.auth);
            for _ in 0..3 {
                let (stream, _) = server.listener.accept().unwrap();
                serve_one(
                    &server.save,
                    stream,
                    &auth,
                    server.live.as_ref(),
                    server.push.as_ref(),
                    server.oidc.as_ref(),
                    None,
                    addr,
                )
                .unwrap();
            }
        });
        let (status, body) = get_path(addr, "/healthz").unwrap();
        assert_eq!(status, 200, "{body}");
        let value: Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(
            value["contract_versions"],
            json!(CONTRACT_VERSIONS.to_vec())
        );
        assert_eq!(value["version"], crate::update::running_version());
        assert_eq!(value["save_schema"], hoshi_core::SCHEMA_VERSION);
        assert_eq!(value["latest"], Value::Null);
        assert_eq!(get_path(addr, "/v1/version").unwrap().0, 401);
        let (status, body) =
            get_path_auth(addr, "/v1/version", Some(&format!("Bearer {TEST_TOKEN}"))).unwrap();
        assert_eq!(status, 200, "{body}");
        let version: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(version["version"], value["version"]);
        assert_eq!(version["schema_version"], crate::cli::SCHEMA_VERSION);
        thread.join().unwrap();
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn bug_flow_authenticates_honors_target_and_saves_only_after_consent() {
        for target in [
            "public",
            "https://tracker.invalid/new?title={title}&body={body}",
            "off",
        ] {
            let dir = unique_save().with_extension("dir");
            std::fs::create_dir(&dir).unwrap();
            let save = dir.join("save.sqlite");
            std::fs::write(
                dir.join("config.toml"),
                format!("bug_report_target = {target:?}\n"),
            )
            .unwrap();
            let server = ephemeral(save.clone());
            let addr = server.local_addr().unwrap();
            let thread = std::thread::spawn(move || {
                let auth = Mutex::new(server.auth);
                for _ in 0..3 {
                    let (stream, _) = server.listener.accept().unwrap();
                    serve_one(
                        &server.save,
                        stream,
                        &auth,
                        server.live.as_ref(),
                        server.push.as_ref(),
                        server.oidc.as_ref(),
                        None,
                        addr,
                    )
                    .unwrap();
                }
            });
            let (status, _) = get_path(addr, "/v1/bug").unwrap();
            assert_eq!(status, if target == "off" { 404 } else { 401 });
            let request =
                r#"{"fields":{"title":"Failure","happened":"Stopped","expected":"Continue"}}"#;
            let (status, body) = post_json(addr, "/v1/bug", request);
            assert_eq!(status, if target == "off" { 404 } else { 200 }, "{body}");
            assert!(!dir.join("reports").exists());
            let request = r#"{"confirm":true,"fields":{"title":"Failure","happened":"Stopped","expected":"Continue"}}"#;
            let (status, body) = post_json(addr, "/v1/bug", request);
            assert_eq!(status, if target == "off" { 404 } else { 200 }, "{body}");
            if target != "off" {
                let value: Value = serde_json::from_str(&body).unwrap();
                let url = value["url"].as_str().unwrap();
                assert!(url.starts_with(if target == "public" {
                    "https://github.com/Sannrox/hoshi/issues/new"
                } else {
                    "https://tracker.invalid/"
                }));
                assert!(
                    dir.join("reports")
                        .join(value["report_id"].as_str().unwrap())
                        .exists()
                );
                assert_eq!(value["submitted"], false);
            }
            thread.join().unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn tcp_post_build_reads_json_body() {
        let save = unique_save();
        seed_save(&save);
        let server = ephemeral(save.clone());
        let addr = server.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let auth = Mutex::new(server.auth);
            let (stream, _) = server.listener.accept().unwrap();
            serve_one(
                &server.save,
                stream,
                &auth,
                server.live.as_ref(),
                server.push.as_ref(),
                server.oidc.as_ref(),
                None,
                addr,
            )
            .unwrap();
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
        thread.join().unwrap();
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
    fn support_report_requires_admin_redacts_and_can_be_disabled() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("hoshi-support-http-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let save = dir.join("save.sqlite");
        seed_save(&save);
        for (presented, empty, disabled, expected) in [
            (Some(TEST_TOKEN), false, false, 403),
            (Some("admin-fixture"), false, false, 200),
            (None, false, false, 401),
            (None, true, false, 200),
            (Some("admin-fixture"), false, true, 404),
        ] {
            std::fs::write(
                dir.join("config.toml"),
                format!(
                    "serve_token = {:?}\nserve_vapid_private = {:?}\nserve_support_report = {}\n",
                    "credential-fixture", "private-fixture", !disabled
                ),
            )
            .unwrap();
            let auth = if empty {
                Auth::empty()
            } else {
                Auth::single(&save, TEST_TOKEN).with_admin_token(&save, "admin-fixture")
            };
            let server = ephemeral(save.clone()).with_auth(auth);
            let addr = server.local_addr().unwrap();
            let worker = std::thread::spawn(move || {
                let auth = Mutex::new(server.auth);
                let (stream, _) = server.listener.accept().unwrap();
                serve_one(
                    &server.save,
                    stream,
                    &auth,
                    server.live.as_ref(),
                    server.push.as_ref(),
                    server.oidc.as_ref(),
                    None,
                    addr,
                )
                .unwrap();
            });
            let header = presented.map(|token| format!("Bearer {token}"));
            let (status, body) =
                get_path_auth(addr, "/v1/admin/support-report", header.as_deref()).unwrap();
            worker.join().unwrap();
            assert_eq!(status, expected, "{body}");
            if status == 200 {
                assert!(
                    body.contains("Schema:")
                        && body.contains("Last tick:")
                        && !body.contains("Summary unavailable")
                        && body.contains("Live server")
                        && body.contains("uptime_secs")
                        && body.contains("rate_limit_hits"),
                    "{body}"
                );
                assert!(!body.contains("credential-fixture") && !body.contains("private-fixture"));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn first_positional_skips_flags() {
        let flags = vec![
            "hoshi".into(),
            "serve".into(),
            "--update-check".into(),
            "--offline".into(),
        ];
        assert!(parse_serve_args(&flags).is_ok());

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
    fn dismantle_cli_and_server_share_success_bad_input_and_refusal() {
        let save_cli = unique_save();
        let save_http = unique_save();
        for path in [&save_cli, &save_http] {
            seed_save(path);
            let mut conn = hoshi_core::open_save(path).unwrap();
            let mut state = hoshi_core::load_state(&conn, BalanceConfig::default()).unwrap();
            state.next_raid_at = None;
            hoshi_core::save_state(&mut conn, &state).unwrap();
        }
        for (building, expected_exit) in [("metal_mine", 0), ("unknown", 2), ("fission_reactor", 3)]
        {
            let (_, exit) = cli_on(&save_cli, &["dismantle", "home", building]);
            let body = serde_json::to_vec(&json!({"planet":"home", "building":building})).unwrap();
            let (status, http) = handle_v1(&save_http, "POST", "/v1/dismantle", &body);
            assert_eq!(exit, expected_exit, "{building}");
            assert_eq!(status, http_status(exit), "{http}");
        }
        let (cli, _) = cli_on(&save_cli, &["state"]);
        let (_, http) = handle_v1(&save_http, "GET", "/v1/state", &[]);
        assert_eq!(cli, http);
        for path in [&save_cli, &save_http] {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(save_lock_path(path));
        }
    }

    #[test]
    fn fusion_mode_cli_and_server_share_bad_input_and_refusal() {
        let save_cli = unique_save();
        let save_http = unique_save();
        for path in [&save_cli, &save_http] {
            seed_save(path);
        }
        for (mode, expected_exit) in [("warp", 2), ("off", 3)] {
            let (_, exit) = cli_on(&save_cli, &["fusion", "mode", "home", mode]);
            let body = serde_json::to_vec(&json!({"planet":"home", "mode":mode})).unwrap();
            let (status, http) = handle_v1(&save_http, "POST", "/v1/fusion/mode", &body);
            assert_eq!(exit, expected_exit, "{mode}");
            assert_eq!(status, http_status(exit), "{http}");
        }
        for path in [&save_cli, &save_http] {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(save_lock_path(path));
        }
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
                admin: false,
                secret: mine.clone(),
                save: save.clone(),
            },
            ScopedToken {
                admin: false,
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
    fn device_session_authenticates_its_save_across_restarts() {
        let save = unique_save();
        seed_save(&save);
        let other = unique_save();
        seed_save(&other);
        let before = std::fs::read(&save).unwrap();
        let client = crate::devices::Client::from_request(None, None);
        let (_, secret) = crate::devices::Devices::open(&save)
            .unwrap()
            .create(None, client, 1_000)
            .unwrap();
        assert_eq!(std::fs::read(&save).unwrap(), before);
        let header = format!("Bearer {secret}");
        // A fresh Auth is a restarted server: sessions come back from the file.
        let restarted =
            |path: &Path| Auth::empty().with_devices(crate::devices::Devices::open(path).unwrap());
        let (status, value) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some(&header),
            &mut restarted(&save),
            1_001,
        );
        assert_eq!(status, 200, "{value}");
        let (status, _) = handle(
            &other,
            "GET",
            "/v1/state",
            b"",
            Some(&header),
            &mut restarted(&save),
            1_001,
        );
        assert_eq!(status, 401);
        let (status, _) = handle(
            &other,
            "GET",
            "/v1/state",
            b"",
            Some(&header),
            &mut restarted(&other),
            1_001,
        );
        assert_eq!(status, 401);
        let expired = 1_000 + crate::devices::LIFETIME_SECS;
        let (status, _) = handle(
            &save,
            "GET",
            "/v1/state",
            b"",
            Some(&header),
            &mut restarted(&save),
            expired,
        );
        assert_eq!(status, 401);
        for path in [&save, &other] {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(save_lock_path(path));
            let _ = std::fs::remove_file(crate::devices::store_path(path));
        }
    }

    #[test]
    fn v1_rate_limits_per_token_and_action() {
        let save = unique_save();
        seed_save(&save);
        let mut auth = Auth::with_tokens(
            vec![ScopedToken {
                admin: false,
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
    fn ui_redirects_root_serves_files_and_falls_back_to_index() {
        let save = unique_save();
        let dir = save.with_extension("ui");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html>index").unwrap();
        std::fs::write(dir.join("sw.js"), "self.x=1").unwrap();
        std::fs::write(dir.join("manifest.webmanifest"), "{}").unwrap();
        std::fs::write(dir.join("assets/app.js"), "app").unwrap();
        std::fs::write(save.with_extension("secret"), "secret").unwrap();
        let addr = spawn_run(ephemeral(save.clone()).with_ui_dir(dir.clone()));

        let (status, headers, _) = get_full(addr, "/", None).unwrap();
        assert_eq!(status, 302);
        assert_eq!(header_line(&headers, "location").as_deref(), Some("ui/"));
        let (status, headers, _) = get_full(addr, "/ui", None).unwrap();
        assert_eq!(
            (status, header_line(&headers, "location").as_deref()),
            (302, Some("ui/"))
        );
        assert_eq!(send_json(addr, "POST", "/", "{}").0, 405);

        let (status, headers, body) = get_full(addr, "/ui/", None).unwrap();
        assert_eq!((status, body.as_str()), (200, "<!doctype html>index"));
        assert!(
            header_line(&headers, "content-security-policy")
                .is_some_and(|csp| csp.starts_with("default-src 'self'")),
            "{headers}"
        );
        let (status, _, body) = get_full(addr, "/ui/cockpit?tab=queue", None).unwrap();
        assert_eq!((status, body.as_str()), (200, "<!doctype html>index"));
        // Deeper paths would break the page's relative links: send them up.
        let (status, headers, _) = get_full(addr, "/ui/terminal/", None).unwrap();
        assert_eq!(
            (status, header_line(&headers, "location").as_deref()),
            (302, Some("../terminal"))
        );
        for (path, content_type) in [
            ("/ui/sw.js", "text/javascript; charset=utf-8"),
            ("/ui/manifest.webmanifest", "application/manifest+json"),
            ("/ui/assets/app.js", "text/javascript; charset=utf-8"),
        ] {
            let (status, headers, _) = get_full(addr, path, None).unwrap();
            assert_eq!(status, 200, "{path}");
            assert_eq!(
                header_line(&headers, "content-type").as_deref(),
                Some(content_type)
            );
        }
        let secret = format!(
            "/ui/../{}",
            save.with_extension("secret")
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        for path in ["/ui/assets/missing.js", secret.as_str(), "/ui/.hidden.js"] {
            assert_eq!(get_full(addr, path, None).unwrap().0, 404, "{path}");
        }
        assert_eq!(get_path(addr, "/v1/state").unwrap().0, 401);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(save.with_extension("secret"));
    }

    #[test]
    fn root_and_ui_are_not_found_without_a_client() {
        let save = unique_save();
        let addr = spawn_run(ephemeral(save));
        if crate::web_ui::FILES.is_empty() {
            assert_eq!(get_path(addr, "/").unwrap().0, 404);
            assert_eq!(get_path(addr, "/ui/").unwrap().0, 404);
        }
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

    fn spawn_run(server: Server) -> SocketAddr {
        let addr = server.local_addr().unwrap();
        std::thread::spawn(move || {
            let _ = server.run();
        });
        addr
    }

    fn read_http_headers(stream: &mut TcpStream) -> (u16, String) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1];
        loop {
            stream.read_exact(&mut tmp).unwrap();
            buf.push(tmp[0]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
                break;
            }
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, text)
    }

    fn ws_open(
        addr: SocketAddr,
        path: &str,
        authorization: Option<&str>,
    ) -> (TcpStream, u16, String) {
        let auth_line = match authorization {
            Some(value) => format!("Authorization: {value}\r\n"),
            None => String::new(),
        };
        ws_open_with(addr, path, &auth_line)
    }

    /// Open a WebSocket with raw extra header lines (each ending in CRLF).
    fn ws_open_with(addr: SocketAddr, path: &str, extra: &str) -> (TcpStream, u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
        )
        .unwrap();
        let (status, headers) = read_http_headers(&mut stream);
        (stream, status, headers)
    }

    fn read_text_events(stream: &mut TcpStream, want: usize) -> Vec<Value> {
        let mut out = Vec::new();
        let mut incoming = crate::ws::Reader::new();
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let start = std::time::Instant::now();
        while out.len() < want && start.elapsed() < Duration::from_secs(2) {
            match incoming.poll(stream) {
                Ok(Some(crate::ws::Frame::Text(bytes))) => {
                    out.push(serde_json::from_slice(&bytes).expect("event json"));
                }
                Ok(Some(_) | None) => {}
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }
        out
    }

    fn seed_stream_events(path: &Path) {
        seed_save(path);
        let mut store = open_storage(path).unwrap();
        let mut state = store.load(BalanceConfig::default()).unwrap();
        let at = state.last_tick_unix;
        for (kind, action, objects, outcome) in [
            (
                hoshi_core::HistoryKind::Build,
                "complete",
                vec!["home", "metal_mine"],
                "lv 1",
            ),
            (
                hoshi_core::HistoryKind::Research,
                "complete",
                vec!["energy"],
                "lv 1",
            ),
            (
                hoshi_core::HistoryKind::Fleet,
                "arrive",
                vec!["1:1:8"],
                "returned",
            ),
            (
                hoshi_core::HistoryKind::Combat,
                "fight",
                vec!["defender"],
                "1 rounds",
            ),
            (
                hoshi_core::HistoryKind::Other,
                "internal",
                vec!["secret"],
                "hidden",
            ),
        ] {
            state.record_history(hoshi_core::HistoryDraft {
                at,
                kind,
                actor: "player".into(),
                action: action.into(),
                objects: objects.into_iter().map(str::to_string).collect(),
                outcome: outcome.into(),
                ..hoshi_core::HistoryDraft::default()
            });
        }
        store.save(&state).unwrap();
    }

    #[test]
    fn events_after_cursor_keeps_public_kinds_and_replays() {
        let events = vec![
            hoshi_core::HistoryEvent {
                id: 1,
                at: 10,
                kind: hoshi_core::HistoryKind::Build,
                actor: "player".into(),
                action: "queue".into(),
                objects: vec!["home".into()],
                inputs: String::new(),
                outcome: "queued".into(),
                reason: String::new(),
                cause_key: None,
                parent_ids: Vec::new(),
                energy: 1.0,
                heat: 1.0,
                mass: 1.0,
            },
            hoshi_core::HistoryEvent {
                id: 2,
                at: 11,
                kind: hoshi_core::HistoryKind::Other,
                actor: "sim".into(),
                action: "hide".into(),
                objects: Vec::new(),
                inputs: String::new(),
                outcome: "no".into(),
                reason: String::new(),
                cause_key: None,
                parent_ids: Vec::new(),
                energy: 9.0,
                heat: 9.0,
                mass: 9.0,
            },
            hoshi_core::HistoryEvent {
                id: 3,
                at: 12,
                kind: hoshi_core::HistoryKind::Combat,
                actor: "sim".into(),
                action: "fight".into(),
                objects: vec!["defender".into()],
                inputs: String::new(),
                outcome: "1 rounds".into(),
                reason: String::new(),
                cause_key: None,
                parent_ids: Vec::new(),
                energy: 1.0,
                heat: 1.0,
                mass: 1.0,
            },
        ];
        let replay = events_after(&events, 1);
        assert_eq!(replay.len(), 1, "{replay:?}");
        assert_eq!(replay[0]["id"], 3);
        assert_eq!(replay[0]["cursor"], 3);
        assert_eq!(replay[0]["type"], "raid");
        assert!(replay[0].get("energy").is_none());
        assert!(replay[0].get("heat").is_none());
        let all = events_after(&events, 0);
        let types: Vec<&str> = all.iter().map(|v| v["type"].as_str().unwrap()).collect();
        assert_eq!(types, vec!["build", "raid"]);
    }

    fn bearer() -> String {
        format!("Bearer {TEST_TOKEN}")
    }

    /// Terminal output from binary frames until `want` shows up or 3s pass.
    fn read_terminal_until(stream: &mut TcpStream, want: &str) -> String {
        let mut out = Vec::new();
        let mut incoming = crate::ws::Reader::new();
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(3)
            && !String::from_utf8_lossy(&out).contains(want)
        {
            match incoming.poll(stream) {
                Ok(Some(crate::ws::Frame::Binary(bytes))) => out.extend(bytes),
                Ok(Some(_) | None) => {}
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// One request with raw extra header lines (each ending in CRLF).
    fn send_raw(
        addr: SocketAddr,
        method: &str,
        path: &str,
        extra: &str,
        body: &str,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
        let body = buf.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        (status, body.to_string())
    }

    #[test]
    fn session_cookie_authenticates_from_this_origin_only() {
        use crate::devices::{Client, Devices};
        let save = unique_save();
        let now = SystemClock.now_unix();
        let mut devices = Devices::open(&save).unwrap();
        let (_, secret) = devices
            .create(None, Client::from_request(None, None), now)
            .unwrap();
        let (_, expired) = devices
            .create(None, Client::from_request(None, None), 0)
            .unwrap();
        let auth = Auth::single(&save, TEST_TOKEN).with_devices(Devices::open(&save).unwrap());
        let addr = spawn_run(ephemeral(save.clone()).with_auth(auth));
        let cookie = format!("Cookie: theme=dark; {SESSION_COOKIE}={secret}\r\n");
        let json = "Content-Type: application/json\r\n";
        let build = r#"{"planet":"home","building":"metal_mine"}"#;

        let (status, body) = send_raw(addr, "GET", "/v1/state", &cookie, "");
        assert_eq!(status, 200, "{body}");
        let (status, body) = send_raw(addr, "POST", "/v1/build", &format!("{cookie}{json}"), build);
        assert!(![400, 401, 403].contains(&status), "{status} {body}");
        // A cross-site form cannot send JSON; without it a cookie write is refused.
        let form = "Content-Type: application/x-www-form-urlencoded\r\n";
        let (status, _) = send_raw(addr, "POST", "/v1/build", &format!("{cookie}{form}"), build);
        assert_eq!(status, 403);
        let both = format!("{cookie}Authorization: Bearer {TEST_TOKEN}\r\n");
        assert_eq!(send_raw(addr, "GET", "/v1/state", &both, "").0, 400);
        let stale = format!("Cookie: {SESSION_COOKIE}={expired}\r\n");
        assert_eq!(send_raw(addr, "GET", "/v1/state", &stale, "").0, 401);
        // The cookie never stands in for an operator token.
        let operator = format!("Cookie: {SESSION_COOKIE}={TEST_TOKEN}\r\n");
        assert_eq!(send_raw(addr, "GET", "/v1/state", &operator, "").0, 401);

        let here = format!("{cookie}Origin: http://localhost\r\n");
        let elsewhere = format!("{cookie}Origin: https://evil.example\r\n");
        for path in ["/v1/events", "/v1/terminal"] {
            assert_eq!(ws_open_with(addr, path, &elsewhere).1, 403, "{path}");
            assert_eq!(
                ws_open_with(addr, path, &cookie).1,
                403,
                "{path} without Origin"
            );
            let (_socket, status, headers) = ws_open_with(addr, path, &here);
            assert_eq!(status, 101, "{path} {headers}");
        }
        let _ = std::fs::remove_file(crate::devices::store_path(&save));
    }

    #[test]
    fn terminal_input_reaches_the_tui_and_resize_sets_the_frame_size() {
        let save = unique_save();
        let addr = spawn_run(ephemeral(save.clone()));
        let (mut stream, status, _) = ws_open(addr, "/v1/terminal", Some(&bearer()));
        assert_eq!(status, 101);
        let first = read_terminal_until(&mut stream, "HOSHI");
        assert!(first.contains("HOSHI"), "{first}");
        crate::ws::write_text(&mut stream, br#"{"type":"resize","cols":120,"rows":40}"#).unwrap();
        // Crossterm moves the cursor as ESC[row;colH, so row 40 only exists at the new size.
        let resized = read_terminal_until(&mut stream, "\x1b[40;");
        assert!(resized.contains("\x1b[40;"), "{resized:?}");
        crate::ws::write_binary(&mut stream, b"2").unwrap();
        let colony = read_terminal_until(&mut stream, "Colony");
        assert!(colony.contains("Colony"), "{colony:?}");
        crate::ws::write_binary(&mut stream, b"q").unwrap();
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn terminal_actions_reach_the_save_on_disk_while_attached() {
        let save = unique_save();
        let addr = spawn_run(ephemeral(save.clone()));
        let (mut stream, status, _) = ws_open(addr, "/v1/terminal", Some(&bearer()));
        assert_eq!(status, 101);
        let _ = read_terminal_until(&mut stream, "HOSHI");
        // Colony › Buildings, then Enter queues the selected building.
        crate::ws::write_binary(&mut stream, b"2").unwrap();
        let _ = read_terminal_until(&mut stream, "Buildings");
        crate::ws::write_binary(&mut stream, b"\r").unwrap();
        // Poll the file: a redraw already in flight says nothing about the key.
        let start = std::time::Instant::now();
        let queued = loop {
            let on_disk = open_storage(&save)
                .unwrap()
                .load(BalanceConfig::default())
                .unwrap();
            if !on_disk.build_queue.is_empty() || start.elapsed() > Duration::from_secs(3) {
                break on_disk.build_queue.len();
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(queued, 1, "the save holds the queued build");
        crate::ws::write_binary(&mut stream, b"q").unwrap();
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn a_second_writer_is_refused_while_a_terminal_is_attached() {
        let save = unique_save();
        let addr = spawn_run(ephemeral(save.clone()));
        let (mut stream, status, _) = ws_open(addr, "/v1/terminal", Some(&bearer()));
        assert_eq!(status, 101);
        let _ = read_terminal_until(&mut stream, "HOSHI");
        let (_second, status, _) = ws_open(addr, "/v1/terminal", Some(&bearer()));
        assert_eq!(status, 409);
        let (status, body) = post_json(
            addr,
            "/v1/build",
            r#"{"planet":"1:1:1","building":"metal_mine"}"#,
        );
        assert_eq!(status, 409, "{body}");
        assert!(body.contains(crate::terminal::HELD), "{body}");
        let (status, body) = get_path_auth(addr, "/v1/state", Some(&bearer())).unwrap();
        assert_eq!(status, 200, "{body}");
        crate::ws::write_binary(&mut stream, b"q").unwrap();
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn a_dropped_terminal_leaves_a_clean_save_and_is_taken_over_after_the_grace() {
        let save = unique_save();
        let server = ephemeral(save.clone()).with_terminal(
            crate::terminal::Settings {
                grace_secs: 1,
                idle_secs: 60,
            },
            crate::keymap::Keymap::default(),
        );
        let addr = spawn_run(server);
        let (mut stream, status, _) = ws_open(addr, "/v1/terminal", Some(&bearer()));
        assert_eq!(status, 101);
        let _ = read_terminal_until(&mut stream, "HOSHI");
        // Start an action, then drop the socket without a close frame.
        crate::ws::write_binary(&mut stream, b"2\r").unwrap();
        drop(stream);
        let (_early, status, _) = ws_open(addr, "/v1/terminal", Some(&bearer()));
        assert_eq!(status, 409, "the dropped session still holds the save");
        std::thread::sleep(Duration::from_millis(2_100));
        let (mut again, status, _) = ws_open(addr, "/v1/terminal", Some(&bearer()));
        assert_eq!(status, 101);
        let redraw = read_terminal_until(&mut again, "HOSHI");
        assert!(
            redraw.contains("\x1b[2J"),
            "a takeover repaints in full: {redraw:?}"
        );
        crate::ws::write_binary(&mut again, b"q").unwrap();
        let reloaded = open_storage(&save).unwrap().load(BalanceConfig::default());
        assert!(reloaded.is_ok(), "{reloaded:?}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn unknown_http_path_does_not_catch_up_or_write() {
        let save = unique_save();
        seed_save(&save);
        let before = {
            let store = open_storage(&save).unwrap();
            store.load(BalanceConfig::default()).unwrap().last_tick_unix
        };
        let addr = spawn_run(ephemeral(save.clone()));
        let (status, body) = get_path(addr, "/").unwrap();
        assert_eq!(status, 404, "{body}");
        let after = {
            let store = open_storage(&save).unwrap();
            store.load(BalanceConfig::default()).unwrap().last_tick_unix
        };
        assert_eq!(after, before, "unknown path must not advance the save");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn invalid_authenticated_v1_does_not_catch_up_or_write() {
        let save = unique_save();
        seed_save(&save);
        let before = {
            let store = open_storage(&save).unwrap();
            store.load(BalanceConfig::default()).unwrap().last_tick_unix
        };
        let addr = spawn_run(ephemeral(save.clone()));
        let (status, body) =
            get_path_auth(addr, "/v1/nonesuch", Some(&format!("Bearer {TEST_TOKEN}"))).unwrap();
        assert_eq!(status, 404, "{body}");
        let (status, body) = post_json(addr, "/v1/state", "{}");
        assert_eq!(status, 405, "{body}");
        let (status, body) = post_json(addr, "/v1/build", "{}");
        assert_eq!(status, 400, "{body}");
        let after = {
            let store = open_storage(&save).unwrap();
            store.load(BalanceConfig::default()).unwrap().last_tick_unix
        };
        assert_eq!(after, before, "invalid /v1 must not advance the save");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn persist_failure_restores_live_and_returns_internal() {
        let save = unique_save();
        seed_save(&save);
        let state = {
            let store = open_storage(&save).unwrap();
            store.load(BalanceConfig::default()).unwrap()
        };
        let mut live = Live {
            updates: None,
            config_dir: PathBuf::new(),
            stats: ServerStats::new(),
            last_save_tick: state.last_tick_unix,
            saved_next_id: state.history.next_id,
            state,
            terminal: None,
            terminal_settings: crate::terminal::Settings::default(),
            terminal_keys: crate::keymap::Keymap::default(),
            next_terminal_id: 1,
        };
        let before_tick = live.state.last_tick_unix;
        let before_queue = live.state.build_queue.len();
        std::fs::remove_file(&save).unwrap();
        std::fs::create_dir(&save).unwrap();
        let (status, value) = mutate_live(
            &save,
            &mut live,
            &["build".into(), "home".into(), "metal_mine".into()],
            SystemClock.now_unix(),
        );
        assert_eq!(status, 500, "{value}");
        assert_eq!(value["error"]["code"], "internal");
        assert_eq!(live.state.last_tick_unix, before_tick);
        assert_eq!(live.state.build_queue.len(), before_queue);
        let _ = std::fs::remove_dir(&save);
    }

    #[test]
    fn unauthenticated_websocket_is_refused() {
        let save = unique_save();
        seed_save(&save);
        let addr = spawn_run(ephemeral(save.clone()));
        let (_stream, status, headers) = ws_open(addr, "/v1/events", None);
        assert_eq!(status, 401, "{headers}");
        assert!(!headers.contains("Switching Protocols"), "{headers}");
        let (_stream, status, headers) = ws_open(addr, "/v1/events?cursor=nope", None);
        assert_eq!(status, 401, "{headers}");
        assert!(!headers.contains("Switching Protocols"), "{headers}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn websocket_delivers_build_research_fleet_raid() {
        let save = unique_save();
        seed_stream_events(&save);
        let addr = spawn_run(ephemeral(save.clone()));
        let (mut stream, status, headers) =
            ws_open(addr, "/v1/events", Some(&format!("Bearer {TEST_TOKEN}")));
        assert_eq!(status, 101, "{headers}");
        assert!(
            headers.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "{headers}"
        );
        let events = read_text_events(&mut stream, 4);
        let types: Vec<&str> = events
            .iter()
            .map(|v| v["type"].as_str().unwrap_or(""))
            .collect();
        for want in ["build", "research", "fleet", "raid"] {
            assert!(types.contains(&want), "missing {want} in {types:?}");
        }
        assert!(!types.contains(&"other"), "{types:?}");
        for event in &events {
            assert_eq!(event["ok"], true);
            assert_eq!(event["schema_version"], crate::cli::SCHEMA_VERSION);
            assert!(event.get("energy").is_none(), "{event}");
            assert!(event.get("rng").is_none(), "{event}");
            assert!(event.get("next_raid_at").is_none(), "{event}");
        }
        let (health_status, body) = get_path(addr, "/healthz").unwrap();
        assert_eq!(health_status, 200, "{body}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn websocket_reconnect_cursor_replays_missed_events() {
        let save = unique_save();
        seed_stream_events(&save);
        let addr = spawn_run(ephemeral(save.clone()));
        let auth = format!("Bearer {TEST_TOKEN}");
        let (mut first, status, headers) = ws_open(addr, "/v1/events", Some(&auth));
        assert_eq!(status, 101, "{headers}");
        let first_events = read_text_events(&mut first, 4);
        assert!(first_events.len() >= 4, "{first_events:?}");
        let last = first_events
            .iter()
            .filter_map(|v| v["id"].as_u64())
            .max()
            .unwrap();
        drop(first);
        let (post_status, body) = post_json(
            addr,
            "/v1/build",
            r#"{"planet":"home","building":"metal_mine"}"#,
        );
        assert_eq!(post_status, 200, "{body}");
        let (mut second, status, headers) =
            ws_open(addr, &format!("/v1/events?cursor={last}"), Some(&auth));
        assert_eq!(status, 101, "{headers}");
        let replayed = read_text_events(&mut second, 1);
        assert!(!replayed.is_empty(), "missed events dropped");
        for event in &replayed {
            let id = event["id"].as_u64().unwrap();
            assert!(id > last, "replayed id {id} <= cursor {last}");
        }
        assert!(
            replayed.iter().any(|v| v["type"] == "build"),
            "{replayed:?}"
        );
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }
    /// `Sec-WebSocket-Protocol` line carrying `token` the way a browser sends it.
    fn browser_protocol(token: &str) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bits: String = token.bytes().map(|b| format!("{b:08b}")).collect();
        let encoded: String = bits
            .as_bytes()
            .chunks(6)
            .map(|chunk| {
                let mut n = 0usize;
                for (i, bit) in chunk.iter().enumerate() {
                    n |= usize::from(*bit == b'1') << (5 - i);
                }
                T[n] as char
            })
            .collect();
        format!("Sec-WebSocket-Protocol: hoshi.v1, hoshi.bearer.{encoded}\r\n")
    }

    #[test]
    fn browser_token_in_subprotocol_opens_the_event_stream() {
        let save = unique_save();
        seed_stream_events(&save);
        let addr = spawn_run(ephemeral(save.clone()));
        let (mut stream, status, headers) =
            ws_open_with(addr, "/v1/events", &browser_protocol(TEST_TOKEN));
        assert_eq!(status, 101, "{headers}");
        let selected: Vec<&str> = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-protocol"))
            .map(|(_, value)| value.trim())
            .collect();
        assert_eq!(selected, ["hoshi.v1"], "{headers}");
        assert!(!headers.contains(TEST_TOKEN), "{headers}");
        assert!(!read_text_events(&mut stream, 1).is_empty());
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    fn browser_token_is_checked_like_the_header() {
        let save = unique_save();
        seed_save(&save);
        let tokens = vec![
            ScopedToken {
                admin: false,
                secret: TEST_TOKEN.into(),
                save: save.clone(),
            },
            ScopedToken {
                admin: false,
                secret: "other-save-token".into(),
                save: unique_save(),
            },
        ];
        let server = Server::open(ServeOptions {
            bind: parse_bind("127.0.0.1:0").unwrap(),
            save: save.clone(),
        })
        .expect("bind serve")
        .with_auth(Auth::with_tokens(
            tokens,
            DEFAULT_RATE_LIMIT,
            DEFAULT_RATE_WINDOW_SECS,
        ));
        let addr = spawn_run(server);
        for token in ["other-save-token", "unknown-token"] {
            let (_stream, status, headers) =
                ws_open_with(addr, "/v1/events", &browser_protocol(token));
            assert_eq!(status, 401, "{headers}");
            assert!(!headers.contains("Switching Protocols"), "{headers}");
            assert!(!headers.contains(token), "{headers}");
        }
        let both = format!(
            "Authorization: Bearer {TEST_TOKEN}\r\n{}",
            browser_protocol(TEST_TOKEN)
        );
        let (_stream, status, headers) = ws_open_with(addr, "/v1/events", &both);
        assert_eq!(status, 400, "{headers}");
        assert!(!headers.contains("Switching Protocols"), "{headers}");
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }
}
