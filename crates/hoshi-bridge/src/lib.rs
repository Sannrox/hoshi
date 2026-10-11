//! Local agent activity feed: line-delimited JSON from a file or a Unix socket.
//!
//! Readers do not execute event content and do not treat payload strings as paths.
//! The feed is inactive until [`BridgeConfig::enabled`] is true.

mod github;

pub use github::{
    GithubPoller, events_from_pulls_json, events_from_runs_json, events_from_webhook,
};

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_LINE_BYTES: usize = 8192;

/// Known activity kinds. Anything else is ignored, not rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityKind {
    #[serde(rename = "agent_started")]
    AgentStarted,
    #[serde(rename = "agent_waiting_for_input")]
    AgentWaitingForInput,
    #[serde(rename = "agent_finished")]
    AgentFinished,
    #[serde(rename = "agent_failed")]
    AgentFailed,
    #[serde(rename = "pr_merged")]
    PrMerged,
    #[serde(rename = "workflow_finished")]
    WorkflowFinished,
}

impl ActivityKind {
    pub fn as_key(self) -> &'static str {
        match self {
            ActivityKind::AgentStarted => "agent_started",
            ActivityKind::AgentWaitingForInput => "agent_waiting_for_input",
            ActivityKind::AgentFinished => "agent_finished",
            ActivityKind::AgentFailed => "agent_failed",
            ActivityKind::PrMerged => "pr_merged",
            ActivityKind::WorkflowFinished => "workflow_finished",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "agent_started" => ActivityKind::AgentStarted,
            "agent_waiting_for_input" => ActivityKind::AgentWaitingForInput,
            "agent_finished" => ActivityKind::AgentFinished,
            "agent_failed" => ActivityKind::AgentFailed,
            "pr_merged" => ActivityKind::PrMerged,
            "workflow_finished" => ActivityKind::WorkflowFinished,
            _ => return None,
        })
    }
}

/// One accepted activity line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEvent {
    #[serde(rename = "type")]
    pub kind: ActivityKind,
    pub source: String,
    pub ts: u64,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// File and socket readers stay off until `enabled` is true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub enabled: bool,
    pub max_line_bytes: usize,
    pub feed_path: PathBuf,
    pub socket_path: Option<PathBuf>,
    pub github_repos: Vec<String>,
    pub poll_secs: u64,
    pub webhook_bind: Option<String>,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            feed_path: default_feed_path(),
            socket_path: None,
            github_repos: Vec::new(),
            poll_secs: 60,
            webhook_bind: None,
        }
    }
}

/// Feed file next to the save, never the cwd.
pub fn default_feed_path() -> PathBuf {
    feed_path_for_save(&hoshi_core::default_save_path())
}

/// `feed.jsonl` in the same directory as `save`.
pub fn feed_path_for_save(save: &Path) -> PathBuf {
    let mut path = save.to_path_buf();
    path.set_file_name("feed.jsonl");
    path
}

/// Config for a state directory: feed beside the save, off unless `bridge.toml`
/// has `enabled = true` or `HOSHI_BRIDGE` is set.
pub fn config_for_state_dir(dir: &Path) -> BridgeConfig {
    let text = fs::read_to_string(dir.join("bridge.toml")).unwrap_or_default();
    let enabled = std::env::var_os("HOSHI_BRIDGE").is_some()
        || text.lines().any(|line| line.trim() == "enabled = true");
    let mut github_repos = Vec::new();
    let mut poll_secs = 60;
    let mut webhook_bind = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(repo) = line.strip_prefix("github_repo = ") {
            github_repos.push(repo.trim().trim_matches('"').to_string());
        }
        if let Some(secs) = line.strip_prefix("poll_secs = ")
            && let Ok(n) = secs.trim().parse()
        {
            poll_secs = n;
        }
        if let Some(bind) = line.strip_prefix("webhook_bind = ") {
            webhook_bind = Some(bind.trim().trim_matches('"').to_string());
        }
    }
    BridgeConfig {
        enabled,
        max_line_bytes: DEFAULT_MAX_LINE_BYTES,
        feed_path: dir.join("feed.jsonl"),
        socket_path: enabled.then(|| dir.join("feed.sock")),
        github_repos,
        poll_secs,
        webhook_bind,
    }
}

pub fn default_offset_path(feed: &Path) -> PathBuf {
    let mut name = feed.as_os_str().to_os_string();
    name.push(".offset");
    PathBuf::from(name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadBatch {
    pub events: Vec<ActivityEvent>,
    pub rejected: Vec<Reject>,
    pub ignored_unknown: usize,
}

#[derive(Debug)]
pub enum FeedError {
    Io(io::Error),
    Json(serde_json::Error),
    Usage(String),
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedError::Io(e) => write!(f, "feed: {e}"),
            FeedError::Json(e) => write!(f, "feed json: {e}"),
            FeedError::Usage(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for FeedError {}

impl From<io::Error> for FeedError {
    fn from(value: io::Error) -> Self {
        FeedError::Io(value)
    }
}

impl From<serde_json::Error> for FeedError {
    fn from(value: serde_json::Error) -> Self {
        FeedError::Json(value)
    }
}

/// Parse one JSONL record. Unknown types are `Ok(None)`. Malformed is `Err`.
pub fn parse_line(line: &str, max_line_bytes: usize) -> Result<Option<ActivityEvent>, String> {
    if line.len() > max_line_bytes {
        return Err("line exceeds size cap".into());
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("malformed json: {e}"))?;
    let kind = match value.get("type") {
        Some(serde_json::Value::String(s)) => match ActivityKind::from_key(s) {
            Some(kind) => kind,
            None => return Ok(None),
        },
        Some(_) => return Err("type must be a string".into()),
        None => return Err("missing type".into()),
    };
    let raw: RawLine = serde_json::from_value(value).map_err(|e| format!("malformed json: {e}"))?;
    if raw.source.is_empty() {
        return Err("missing source".into());
    }
    Ok(Some(ActivityEvent {
        kind,
        source: raw.source,
        ts: raw.ts,
        payload: raw.payload,
    }))
}

#[derive(Deserialize)]
struct RawLine {
    source: String,
    ts: u64,
    #[serde(default)]
    payload: serde_json::Value,
}

/// Append-only JSONL reader that stores a byte offset beside the feed.
pub struct FileFeed {
    config: BridgeConfig,
    offset_path: PathBuf,
    offset: u64,
}

impl FileFeed {
    pub fn open(config: BridgeConfig) -> Result<Self, FeedError> {
        let offset_path = default_offset_path(&config.feed_path);
        let offset = if config.enabled {
            read_offset(&offset_path)?
        } else {
            0
        };
        Ok(Self {
            config,
            offset_path,
            offset,
        })
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// New complete lines since the last poll. No-op when the bridge is off.
    pub fn poll(&mut self) -> Result<ReadBatch, FeedError> {
        if !self.config.enabled {
            return Ok(ReadBatch::default());
        }
        let path = &self.config.feed_path;
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ReadBatch::default()),
            Err(e) => return Err(e.into()),
        };
        let len = file.metadata()?.len();
        if self.offset > len {
            self.offset = 0;
        }
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;
        let mut batch = ReadBatch::default();
        let max = self.config.max_line_bytes;
        let mut offset = self.offset;
        loop {
            let mut buf = Vec::new();
            let n = read_capped_line(&mut reader, &mut buf, max)?;
            if n == 0 {
                break;
            }
            if !buf.ends_with(b"\n") {
                break;
            }
            offset += n as u64;
            if buf.last() == Some(&b'\n') {
                buf.pop();
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
            }
            push_parsed(&mut batch, &buf, max);
        }
        write_offset(&self.offset_path, offset)?;
        self.offset = offset;
        Ok(batch)
    }
}

/// File feed plus an optional Unix socket. Both stay off when `enabled` is false.
pub struct Bridge {
    file: FileFeed,
    github: GithubPoller,
    #[cfg(unix)]
    socket: Option<std::os::unix::net::UnixListener>,
    #[cfg(unix)]
    pending: Vec<SocketConn>,
    #[cfg(unix)]
    max_line_bytes: usize,
}

#[cfg(unix)]
struct SocketConn {
    reader: BufReader<std::os::unix::net::UnixStream>,
    partial: Vec<u8>,
    skip_rest: bool,
}

impl Bridge {
    pub fn open(config: BridgeConfig) -> Result<Self, FeedError> {
        let github = GithubPoller::open(&config)?;
        #[cfg(unix)]
        let max_line_bytes = config.max_line_bytes;
        #[cfg(unix)]
        let socket = if config.enabled {
            config.socket_path.as_deref().and_then(|path| {
                // File feed stays up if the socket is occupied or the path is not a socket.
                bind_socket(path).ok()
            })
        } else {
            None
        };
        Ok(Self {
            file: FileFeed::open(config)?,
            github,
            #[cfg(unix)]
            socket,
            #[cfg(unix)]
            pending: Vec::new(),
            #[cfg(unix)]
            max_line_bytes,
        })
    }

    pub fn status(&self) -> &str {
        self.github.status()
    }

    pub fn poll(&mut self) -> Result<ReadBatch, FeedError> {
        let mut batch = self.file.poll()?;
        #[cfg(unix)]
        {
            let mut fresh = Vec::new();
            if let Some(listener) = &self.socket {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(true)?;
                            fresh.push(SocketConn {
                                reader: BufReader::new(stream),
                                partial: Vec::new(),
                                skip_rest: false,
                            });
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            self.pending.extend(fresh);
            let max = self.max_line_bytes;
            let mut i = 0;
            while i < self.pending.len() {
                match drain_pending(&mut self.pending[i], max, &mut batch) {
                    Ok(true) => i += 1,
                    Ok(false) => {
                        self.pending.remove(i);
                    }
                    Err(e) => {
                        self.pending.remove(i);
                        return Err(e);
                    }
                }
            }
        }
        let extra = self.github.poll(github::now_unix())?;
        batch.events.extend(extra.events);
        batch.rejected.extend(extra.rejected);
        batch.ignored_unknown += extra.ignored_unknown;
        Ok(batch)
    }
}

/// Drain complete lines. Keep the connection when the last line is still open.
#[cfg(unix)]
fn drain_pending(
    conn: &mut SocketConn,
    max: usize,
    batch: &mut ReadBatch,
) -> Result<bool, FeedError> {
    loop {
        if conn.skip_rest {
            match conn.reader.fill_buf() {
                Ok([]) => return Ok(false),
                Ok(data) => {
                    if let Some(i) = data.iter().position(|&b| b == b'\n') {
                        conn.reader.consume(i + 1);
                        conn.skip_rest = false;
                        continue;
                    }
                    let n = data.len();
                    conn.reader.consume(n);
                    continue;
                }
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    return Ok(true);
                }
                Err(e) => return Err(e.into()),
            }
        }
        let n = read_capped_line(&mut conn.reader, &mut conn.partial, max)?;
        if n == 0 {
            return match conn.reader.fill_buf() {
                Ok([]) => Ok(false),
                Ok(_) => Ok(true),
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    Ok(true)
                }
                Err(e) => Err(e.into()),
            };
        }
        if conn.partial.len() > max {
            batch.rejected.push(Reject {
                reason: "line exceeds size cap".into(),
            });
            if conn.partial.ends_with(b"\n") {
                conn.partial.clear();
                continue;
            }
            conn.partial.clear();
            conn.skip_rest = true;
            continue;
        }
        if !conn.partial.ends_with(b"\n") {
            return Ok(true);
        }
        let mut line = std::mem::take(&mut conn.partial);
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        push_parsed(batch, &line, max);
    }
}

#[cfg(unix)]
fn bind_socket(path: &Path) -> Result<std::os::unix::net::UnixListener, FeedError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileTypeExt;
                if meta.file_type().is_socket() {
                    match std::os::unix::net::UnixStream::connect(path) {
                        Ok(_) => {
                            return Err(FeedError::Usage(format!(
                                "socket path {} is already in use",
                                path.display()
                            )));
                        }
                        Err(_) => fs::remove_file(path)?,
                    }
                } else {
                    return Err(FeedError::Usage(format!(
                        "socket path {} exists and is not a socket",
                        path.display()
                    )));
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    ensure_parent(path)?;
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn push_parsed(batch: &mut ReadBatch, buf: &[u8], max: usize) {
    if buf.len() > max {
        batch.rejected.push(Reject {
            reason: "line exceeds size cap".into(),
        });
        return;
    }
    let line = match std::str::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => {
            batch.rejected.push(Reject {
                reason: "line is not utf-8".into(),
            });
            return;
        }
    };
    match parse_line(line, max) {
        Ok(Some(event)) => batch.events.push(event),
        Ok(None) => {
            if !line.trim().is_empty() {
                batch.ignored_unknown += 1;
            }
        }
        Err(reason) => batch.rejected.push(Reject { reason }),
    }
}

fn read_capped_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> io::Result<usize> {
    let mut n = 0usize;
    loop {
        let available = {
            match reader.fill_buf() {
                Ok([]) => return Ok(n),
                Ok(data) => data,
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    return Ok(n);
                }
                Err(e) => return Err(e),
            }
        };
        if let Some(i) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..=i]);
            let take = i + 1;
            reader.consume(take);
            n += take;
            return Ok(n);
        }
        buf.extend_from_slice(available);
        let take = available.len();
        reader.consume(take);
        n += take;
        if buf.len() > max {
            // Drain the rest of the oversize line so the next poll can resume.
            loop {
                let available = match reader.fill_buf() {
                    Ok([]) => return Ok(n),
                    Ok(data) => data,
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut =>
                    {
                        return Ok(n);
                    }
                    Err(e) => return Err(e),
                };
                if let Some(i) = available.iter().position(|&b| b == b'\n') {
                    let take = i + 1;
                    reader.consume(take);
                    n += take;
                    buf.push(b'\n');
                    return Ok(n);
                }
                let take = available.len();
                reader.consume(take);
                n += take;
            }
        }
    }
}

fn read_offset(path: &Path) -> Result<u64, FeedError> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .trim()
            .parse()
            .map_err(|_| FeedError::Usage("corrupt feed offset".into())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn ensure_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

fn write_offset(path: &Path, offset: u64) -> Result<(), FeedError> {
    ensure_parent(path)?;
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, format!("{offset}\n"))?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Append one event as a single JSON line.
pub fn post_event(feed: &Path, event: &ActivityEvent) -> Result<(), FeedError> {
    ensure_parent(feed)?;
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    if line.len() > DEFAULT_MAX_LINE_BYTES {
        return Err(FeedError::Usage("event exceeds size cap".into()));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(feed)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// `hoshi post <type> <source> [payload-json] [--feed PATH] [--save PATH] [--ts UNIX]`
pub fn run_post(args: &[String]) -> Result<(), FeedError> {
    let mut feed = feed_path_for_save(&hoshi_core::resolve_save_path(
        None,
        std::env::var_os("HOSHI_SAVE").as_deref(),
    ));
    let mut kind = None;
    let mut source = None;
    let mut payload = serde_json::Value::Object(serde_json::Map::new());
    let mut ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--feed" {
            let path = rest
                .next()
                .ok_or_else(|| FeedError::Usage("missing --feed path".into()))?;
            feed = PathBuf::from(path);
        } else if arg == "--save" {
            let path = rest
                .next()
                .ok_or_else(|| FeedError::Usage("missing --save path".into()))?;
            feed = feed_path_for_save(Path::new(path));
        } else if arg == "--ts" {
            let raw = rest
                .next()
                .ok_or_else(|| FeedError::Usage("missing --ts value".into()))?;
            ts = raw
                .parse()
                .map_err(|_| FeedError::Usage("ts must be a unix timestamp".into()))?;
        } else if kind.is_none() {
            kind = Some(arg.clone());
        } else if source.is_none() {
            source = Some(arg.clone());
        } else {
            payload = serde_json::from_str(arg)?;
        }
    }
    let kind = kind.ok_or_else(|| {
        FeedError::Usage("usage: hoshi post <type> <source> [payload-json]".into())
    })?;
    let kind = ActivityKind::from_key(&kind)
        .ok_or_else(|| FeedError::Usage(format!("unknown type {kind}")))?;
    let source = source.ok_or_else(|| FeedError::Usage("missing source".into()))?;
    post_event(
        &feed,
        &ActivityEvent {
            kind,
            source,
            ts,
            payload,
        },
    )
}

/// Drain one Unix socket connection's JSONL using the same parser as the file.
#[cfg(unix)]
pub fn read_socket_lines(
    reader: &mut impl BufRead,
    max_line_bytes: usize,
) -> Result<ReadBatch, FeedError> {
    let mut batch = ReadBatch::default();
    loop {
        let mut buf = Vec::new();
        let n = read_capped_line(reader, &mut buf, max_line_bytes)?;
        if n == 0 {
            break;
        }
        if !buf.ends_with(b"\n") {
            break;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        }
        push_parsed(&mut batch, &buf, max_line_bytes);
    }
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn unique_feed(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "hoshi-feed-{}-{}-{tag}.jsonl",
            std::process::id(),
            nanos
        ))
    }

    fn enabled(feed: PathBuf) -> BridgeConfig {
        BridgeConfig {
            enabled: true,
            feed_path: feed,
            ..BridgeConfig::default()
        }
    }

    #[test]
    fn valid_fixture_parses_all_four_kinds() {
        let text = include_str!("../tests/fixtures/valid.jsonl");
        let mut events = Vec::new();
        for line in text.lines() {
            if let Some(event) = parse_line(line, DEFAULT_MAX_LINE_BYTES).unwrap() {
                events.push(event);
            }
        }
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].kind, ActivityKind::AgentStarted);
        assert_eq!(events[1].kind, ActivityKind::AgentWaitingForInput);
        assert_eq!(events[2].kind, ActivityKind::AgentFinished);
        assert_eq!(events[3].kind, ActivityKind::AgentFailed);
        assert_eq!(events[0].source, "shell");
        assert_eq!(events[0].payload["agent"], "local");
    }

    #[test]
    fn unknown_types_are_ignored() {
        let text = include_str!("../tests/fixtures/unknown.jsonl");
        let mut accepted = 0;
        let mut ignored = 0;
        for line in text.lines() {
            match parse_line(line, DEFAULT_MAX_LINE_BYTES).unwrap() {
                Some(_) => accepted += 1,
                None => ignored += 1,
            }
        }
        assert_eq!(accepted, 2);
        assert_eq!(ignored, 3);
    }

    #[test]
    fn malformed_lines_are_rejected() {
        let text = include_str!("../tests/fixtures/malformed.jsonl");
        let mut rejected = 0;
        let mut accepted = 0;
        for line in text.lines() {
            match parse_line(line, DEFAULT_MAX_LINE_BYTES) {
                Ok(Some(_)) => accepted += 1,
                Ok(None) => {}
                Err(_) => rejected += 1,
            }
        }
        assert_eq!(accepted, 2);
        assert!(rejected >= 2);
    }

    #[test]
    fn oversize_line_is_rejected() {
        let huge = format!(
            r#"{{"type":"agent_started","source":"shell","ts":1,"payload":{{"x":"{}"}}}}"#,
            "a".repeat(9000)
        );
        assert!(parse_line(&huge, DEFAULT_MAX_LINE_BYTES).is_err());
    }

    #[test]
    fn oversize_file_line_waits_for_the_newline() {
        let feed = unique_feed("oversize-file");
        let _ = fs::remove_file(&feed);
        let _ = fs::remove_file(default_offset_path(&feed));
        let prefix = "x".repeat(DEFAULT_MAX_LINE_BYTES + 8);
        fs::write(&feed, &prefix).unwrap();
        let mut reader = FileFeed::open(enabled(feed.clone())).unwrap();
        let first = reader.poll().unwrap();
        assert!(first.events.is_empty());
        assert!(first.rejected.is_empty());
        assert_eq!(reader.offset(), 0);
        let mut file = OpenOptions::new().append(true).open(&feed).unwrap();
        writeln!(
            file,
            r#"{{"type":"agent_started","source":"shell","ts":1}}"#
        )
        .unwrap();
        drop(file);
        let second = reader.poll().unwrap();
        assert!(second.events.is_empty());
        assert_eq!(second.rejected.len(), 1);
        assert!(reader.offset() > 0);
        let _ = fs::remove_file(&feed);
        let _ = fs::remove_file(default_offset_path(&feed));
    }

    #[cfg(unix)]
    #[test]
    fn oversize_socket_line_does_not_accept_the_suffix() {
        use std::os::unix::net::UnixStream;
        let dir = std::env::temp_dir().join(format!("h{}o", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s");
        let max = 64usize;
        let mut bridge = Bridge::open(BridgeConfig {
            enabled: true,
            max_line_bytes: max,
            feed_path: dir.join("feed.jsonl"),
            socket_path: Some(sock.clone()),
            ..BridgeConfig::default()
        })
        .unwrap();
        let mut client = UnixStream::connect(&sock).unwrap();
        client.write_all(&vec![b'x'; max + 8]).unwrap();
        client.flush().unwrap();
        let first = bridge.poll().unwrap();
        assert!(first.events.is_empty());
        writeln!(
            client,
            r#"{{"type":"agent_started","source":"shell","ts":1}}"#
        )
        .unwrap();
        drop(client);
        let second = bridge.poll().unwrap();
        assert!(second.events.is_empty());
        assert_eq!(first.rejected.len() + second.rejected.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn payload_paths_are_not_opened() {
        let feed = unique_feed("paths");
        let _ = fs::remove_file(&feed);
        let trap = feed.with_extension("should-not-exist");
        let _ = fs::remove_file(&trap);
        let event = ActivityEvent {
            kind: ActivityKind::AgentStarted,
            source: "shell".into(),
            ts: 1,
            payload: serde_json::json!({
                "path": trap.to_string_lossy(),
                "other": "../etc/passwd"
            }),
        };
        post_event(&feed, &event).unwrap();
        let mut reader = FileFeed::open(enabled(feed.clone())).unwrap();
        let batch = reader.poll().unwrap();
        assert_eq!(batch.events.len(), 1);
        assert!(!trap.exists());
        let _ = fs::remove_file(&feed);
        let _ = fs::remove_file(default_offset_path(&feed));
    }

    #[test]
    fn config_reads_github_repos_from_bridge_toml() {
        let dir = std::env::temp_dir().join(format!("h{}c", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("bridge.toml"),
            "enabled = true\ngithub_repo = \"acme/game\"\npoll_secs = 15\nwebhook_bind = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        let cfg = config_for_state_dir(&dir);
        assert!(cfg.enabled);
        assert_eq!(cfg.github_repos, ["acme/game"]);
        assert_eq!(cfg.poll_secs, 15);
        assert_eq!(cfg.webhook_bind.as_deref(), Some("127.0.0.1:0"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_config_does_not_read() {
        let feed = unique_feed("off");
        let event = ActivityEvent {
            kind: ActivityKind::AgentStarted,
            source: "shell".into(),
            ts: 1,
            payload: serde_json::json!({}),
        };
        post_event(&feed, &event).unwrap();
        let mut cfg = enabled(feed.clone());
        cfg.enabled = false;
        let mut reader = FileFeed::open(cfg).unwrap();
        let batch = reader.poll().unwrap();
        assert!(batch.events.is_empty());
        let _ = fs::remove_file(&feed);
        let _ = fs::remove_file(default_offset_path(&feed));
    }

    #[test]
    fn disabled_open_ignores_a_corrupt_offset() {
        let feed = unique_feed("corrupt-off");
        let offset = default_offset_path(&feed);
        fs::write(&offset, "nope\n").unwrap();
        let mut cfg = enabled(feed.clone());
        cfg.enabled = false;
        let reader = FileFeed::open(cfg).unwrap();
        assert_eq!(reader.offset(), 0);
        let mut on = enabled(feed.clone());
        on.enabled = true;
        assert!(FileFeed::open(on).is_err());
        let _ = fs::remove_file(&feed);
        let _ = fs::remove_file(&offset);
    }

    #[test]
    fn relative_filename_does_not_need_a_parent_dir() {
        ensure_parent(Path::new("feed.jsonl")).unwrap();
        ensure_parent(Path::new("feed.jsonl.offset")).unwrap();
    }

    #[test]
    fn poll_resumes_at_the_stored_offset() {
        let feed = unique_feed("resume");
        let _ = fs::remove_file(&feed);
        let _ = fs::remove_file(default_offset_path(&feed));
        let first = ActivityEvent {
            kind: ActivityKind::AgentStarted,
            source: "shell".into(),
            ts: 1,
            payload: serde_json::json!({}),
        };
        let second = ActivityEvent {
            kind: ActivityKind::AgentFinished,
            source: "shell".into(),
            ts: 2,
            payload: serde_json::json!({}),
        };
        post_event(&feed, &first).unwrap();
        post_event(&feed, &second).unwrap();
        let mut reader = FileFeed::open(enabled(feed.clone())).unwrap();
        let batch = reader.poll().unwrap();
        assert_eq!(batch.events, vec![first.clone(), second.clone()]);
        let saved = reader.offset();
        drop(reader);

        let mut reader = FileFeed::open(enabled(feed.clone())).unwrap();
        assert_eq!(reader.offset(), saved);
        let batch = reader.poll().unwrap();
        assert!(batch.events.is_empty());

        let third = ActivityEvent {
            kind: ActivityKind::AgentFailed,
            source: "hook".into(),
            ts: 3,
            payload: serde_json::json!({}),
        };
        post_event(&feed, &third).unwrap();
        let batch = reader.poll().unwrap();
        assert_eq!(batch.events, vec![third]);
        let _ = fs::remove_file(&feed);
        let _ = fs::remove_file(default_offset_path(&feed));
    }

    #[test]
    fn feed_path_follows_the_save_file() {
        let save = PathBuf::from("/tmp/player/save.sqlite");
        assert_eq!(
            feed_path_for_save(&save),
            PathBuf::from("/tmp/player/feed.jsonl")
        );
        assert_eq!(
            feed_path_for_save(&hoshi_core::resolve_save_path(
                None,
                Some(std::ffi::OsStr::new("/tmp/custom/save.sqlite")),
            )),
            PathBuf::from("/tmp/custom/feed.jsonl")
        );
        let dir = unique_feed("cfgdir");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bridge.toml"), "enabled = true\n").unwrap();
        let cfg = config_for_state_dir(&dir);
        assert!(cfg.enabled);
        assert_eq!(cfg.feed_path, dir.join("feed.jsonl"));
        assert_eq!(
            cfg.socket_path.as_deref(),
            Some(dir.join("feed.sock").as_path())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn post_cli_writes_beside_the_save_flag() {
        let dir = unique_feed("postsave");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let save = dir.join("save.sqlite");
        run_post(&[
            "--save".into(),
            save.to_string_lossy().into(),
            "--ts".into(),
            "7".into(),
            "agent_finished".into(),
            "hook".into(),
        ])
        .unwrap();
        assert!(dir.join("feed.jsonl").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn post_cli_writes_a_valid_line() {
        let feed = unique_feed("cli");
        let _ = fs::remove_file(&feed);
        run_post(&[
            "--feed".into(),
            feed.to_string_lossy().into(),
            "--ts".into(),
            "42".into(),
            "agent_started".into(),
            "shell".into(),
            r#"{"agent":"local"}"#.into(),
        ])
        .unwrap();
        let mut reader = FileFeed::open(enabled(feed.clone())).unwrap();
        let batch = reader.poll().unwrap();
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].ts, 42);
        assert_eq!(batch.events[0].payload["agent"], "local");
        let _ = fs::remove_file(&feed);
        let _ = fs::remove_file(default_offset_path(&feed));
    }

    #[cfg(unix)]
    #[test]
    fn enabled_bridge_accepts_socket_connections() {
        use std::os::unix::net::UnixStream;
        let dir = std::env::temp_dir().join(format!("h{}b", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s");
        let mut bridge = Bridge::open(BridgeConfig {
            enabled: true,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            feed_path: dir.join("feed.jsonl"),
            socket_path: Some(sock.clone()),
            ..BridgeConfig::default()
        })
        .unwrap();
        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(
            client,
            r#"{{"type":"agent_started","source":"shell","ts":9}}"#
        )
        .unwrap();
        drop(client);
        let batch = bridge.poll().unwrap();
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].kind, ActivityKind::AgentStarted);

        let _idle = UnixStream::connect(&sock).unwrap();
        let started = std::time::Instant::now();
        let idle = bridge.poll().unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(idle.events.is_empty());
        let mut split = UnixStream::connect(&sock).unwrap();
        write!(
            split,
            r#"{{"type":"agent_finished","source":"shell","ts":10}}"#
        )
        .unwrap();
        split.flush().unwrap();
        let waiting = bridge.poll().unwrap();
        assert!(waiting.events.is_empty());
        writeln!(split).unwrap();
        drop(split);
        let rest = bridge.poll().unwrap();
        assert_eq!(rest.events.len(), 1);
        assert_eq!(rest.events[0].kind, ActivityKind::AgentFinished);

        let second = Bridge::open(BridgeConfig {
            enabled: true,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            feed_path: dir.join("feed.jsonl"),
            socket_path: Some(sock.clone()),
            ..BridgeConfig::default()
        })
        .unwrap();
        let mut still = UnixStream::connect(&sock).unwrap();
        writeln!(
            still,
            r#"{{"type":"agent_failed","source":"shell","ts":11}}"#
        )
        .unwrap();
        drop(still);
        let kept = bridge.poll().unwrap();
        assert_eq!(kept.events.len(), 1);
        assert_eq!(kept.events[0].kind, ActivityKind::AgentFailed);
        drop(second);

        drop(bridge);
        // A regular file at the configured socket path must be left alone.
        let occupied = dir.join("keep");
        fs::write(&occupied, "keep me").unwrap();
        let opened = Bridge::open(BridgeConfig {
            enabled: true,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            feed_path: dir.join("feed.jsonl"),
            socket_path: Some(occupied.clone()),
            ..BridgeConfig::default()
        })
        .unwrap();
        drop(opened);
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "keep me");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_accepts_the_same_lines() {
        use std::os::unix::net::{UnixListener, UnixStream};
        let sock = unique_feed("sock").with_extension("sock");
        let _ = fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let mut client = UnixStream::connect(&sock).unwrap();
        client
            .write_all(include_str!("../tests/fixtures/valid.jsonl").as_bytes())
            .unwrap();
        drop(client);
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let batch = read_socket_lines(&mut reader, DEFAULT_MAX_LINE_BYTES).unwrap();
        assert_eq!(batch.events.len(), 4);
        assert!(batch.rejected.is_empty());
        let _ = fs::remove_file(&sock);
    }
}
