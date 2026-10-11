//! GitHub merged-PR and finished-workflow events via `gh api` or a local webhook.
//!
//! No tokens are stored. The default fetch uses the user's existing `gh` login.
//! Live network tests are ignored.

use crate::{ActivityEvent, ActivityKind, BridgeConfig, FeedError, ReadBatch, ensure_parent};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_POLL_SECS: u64 = 60;
const PAGE_SIZE: u32 = 30;
/// New events land on page 1. Ten pages (300 items) bounds a burst in one poll.
const MAX_PAGES: u32 = 10;

pub type GithubFetch = fn(&str, Option<&str>) -> Result<HttpFetch, FeedError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFetch {
    pub status: u16,
    pub etag: Option<String>,
    pub body: String,
}

pub struct GithubPoller {
    enabled: bool,
    repos: Vec<String>,
    poll_every: Duration,
    last_poll: Option<Instant>,
    seen_path: PathBuf,
    etag_path: PathBuf,
    seen: HashSet<String>,
    etags: HashMap<String, String>,
    fetch: GithubFetch,
    webhook: Option<TcpListener>,
    webhook_note: Option<String>,
    status: String,
}

impl GithubPoller {
    pub fn open(config: &BridgeConfig) -> Result<Self, FeedError> {
        if !config.enabled {
            return Ok(Self::disabled());
        }
        let dir = config
            .feed_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let seen_path = dir.join("github-seen.json");
        let etag_path = dir.join("github-etags.json");
        let mut webhook_note = None;
        let webhook = match config.webhook_bind.as_deref() {
            None => None,
            Some(addr) => match bind_webhook(addr) {
                Ok(listener) => Some(listener),
                Err(e) => {
                    webhook_note = Some(format!("webhook bind failed: {e}"));
                    None
                }
            },
        };
        let github = if config.github_repos.is_empty() {
            "bridge on"
        } else {
            "bridge on, github idle"
        };
        Ok(Self {
            enabled: true,
            repos: config.github_repos.clone(),
            poll_every: Duration::from_secs(if config.poll_secs == 0 {
                0
            } else {
                config.poll_secs
            }),
            last_poll: None,
            seen: load_seen(&seen_path)?,
            etags: load_etags(&etag_path)?,
            seen_path,
            etag_path,
            fetch: gh_cli_fetch,
            webhook,
            webhook_note: webhook_note.clone(),
            status: compose_status(github, webhook_note.as_deref()),
        })
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            repos: Vec::new(),
            poll_every: Duration::from_secs(DEFAULT_POLL_SECS),
            last_poll: None,
            seen_path: PathBuf::new(),
            etag_path: PathBuf::new(),
            seen: HashSet::new(),
            etags: HashMap::new(),
            fetch: gh_cli_fetch,
            webhook: None,
            webhook_note: None,
            status: "bridge off".into(),
        }
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn set_fetch(&mut self, fetch: GithubFetch) {
        self.fetch = fetch;
    }

    pub fn poll(&mut self, now_unix: u64) -> Result<ReadBatch, FeedError> {
        let mut batch = ReadBatch::default();
        if !self.enabled {
            return Ok(batch);
        }
        if let Some(listener) = &self.webhook {
            drain_webhook(listener, &self.repos, &mut self.seen, now_unix, &mut batch);
            save_seen(&self.seen_path, &self.seen)?;
        }
        if self.repos.is_empty() {
            return Ok(batch);
        }
        if let Some(last) = self.last_poll
            && !self.poll_every.is_zero()
            && last.elapsed() < self.poll_every
        {
            return Ok(batch);
        }
        self.last_poll = Some(Instant::now());
        match self.poll_github(now_unix) {
            Ok(extra) => {
                batch.events.extend(extra.events);
                batch.rejected.extend(extra.rejected);
                self.status = compose_status("bridge on, github ok", self.webhook_note.as_deref());
            }
            Err(e) => {
                self.status = compose_status(
                    &format!("github unreachable: {e}"),
                    self.webhook_note.as_deref(),
                );
            }
        }
        Ok(batch)
    }

    fn poll_github(&mut self, now_unix: u64) -> Result<ReadBatch, FeedError> {
        let mut scratch = PollScratch {
            seen: self.seen.clone(),
            etags: self.etags.clone(),
            batch: ReadBatch::default(),
        };
        let repos = self.repos.clone();
        for repo in &repos {
            self.fill_pages(
                &mut scratch,
                repo,
                &format!("repos/{repo}/pulls?state=closed&per_page={PAGE_SIZE}"),
                now_unix,
                events_from_pulls_json,
            )?;
            self.fill_pages(
                &mut scratch,
                repo,
                &format!("repos/{repo}/actions/runs?per_page={PAGE_SIZE}"),
                now_unix,
                events_from_runs_json,
            )?;
        }
        save_seen(&self.seen_path, &scratch.seen)?;
        save_etags(&self.etag_path, &scratch.etags)?;
        self.seen = scratch.seen;
        self.etags = scratch.etags;
        Ok(scratch.batch)
    }

    fn fill_pages(
        &self,
        scratch: &mut PollScratch,
        repo: &str,
        base_path: &str,
        now_unix: u64,
        parse: fn(&str, &str, u64) -> Result<Vec<ActivityEvent>, String>,
    ) -> Result<(), FeedError> {
        for page in 1..=MAX_PAGES {
            let path = if page == 1 {
                base_path.to_string()
            } else {
                format!("{base_path}&page={page}")
            };
            let n = self.fill_endpoint(scratch, repo, &path, now_unix, parse)?;
            if n < PAGE_SIZE as usize {
                break;
            }
        }
        Ok(())
    }

    fn fill_endpoint(
        &self,
        scratch: &mut PollScratch,
        repo: &str,
        path: &str,
        now_unix: u64,
        parse: fn(&str, &str, u64) -> Result<Vec<ActivityEvent>, String>,
    ) -> Result<usize, FeedError> {
        let etag = scratch.etags.get(path).cloned();
        let fetched = (self.fetch)(path, etag.as_deref())?;
        if fetched.status == 304 {
            // Unchanged page. Keep walking if a later page was fetched before.
            if scratch.etags.contains_key(&next_page_path(path)) {
                return Ok(PAGE_SIZE as usize);
            }
            return Ok(0);
        }
        if fetched.status != 200 {
            return Err(FeedError::Usage(format!(
                "github {path} status {}",
                fetched.status
            )));
        }
        if let Some(tag) = fetched.etag {
            scratch.etags.insert(path.to_string(), tag);
        }
        let n = list_page_len(&fetched.body);
        match parse(repo, &fetched.body, now_unix) {
            Ok(events) => {
                for event in events {
                    let id = event_id(&event);
                    if scratch.seen.insert(id) {
                        scratch.batch.events.push(event);
                    }
                }
            }
            Err(reason) => scratch.batch.rejected.push(crate::Reject { reason }),
        }
        Ok(n)
    }
}

struct PollScratch {
    seen: HashSet<String>,
    etags: HashMap<String, String>,
    batch: ReadBatch,
}

pub fn events_from_pulls_json(
    repo: &str,
    body: &str,
    ts: u64,
) -> Result<Vec<ActivityEvent>, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let list = value
        .as_array()
        .ok_or_else(|| "pulls json is not an array".to_string())?;
    let mut events = Vec::new();
    for item in list {
        if item.get("merged_at").and_then(Value::as_str).is_none() {
            continue;
        }
        events.push(ActivityEvent {
            kind: ActivityKind::PrMerged,
            source: "github".into(),
            ts,
            payload: serde_json::json!({
                "repo": repo,
                "id": item.get("id"),
                "number": item.get("number"),
                "title": item.get("title"),
                "merged_at": item.get("merged_at"),
            }),
        });
    }
    Ok(events)
}

pub fn events_from_runs_json(
    repo: &str,
    body: &str,
    ts: u64,
) -> Result<Vec<ActivityEvent>, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let list = value
        .get("workflow_runs")
        .and_then(Value::as_array)
        .ok_or_else(|| "runs json missing workflow_runs".to_string())?;
    let mut events = Vec::new();
    for item in list {
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        if status != "completed" {
            continue;
        }
        events.push(ActivityEvent {
            kind: ActivityKind::WorkflowFinished,
            source: "github".into(),
            ts,
            payload: serde_json::json!({
                "repo": repo,
                "id": item.get("id"),
                "name": item.get("name"),
                "conclusion": item.get("conclusion"),
                "status": status,
            }),
        });
    }
    Ok(events)
}

pub fn events_from_webhook(
    headers: &str,
    body: &str,
    ts: u64,
) -> Result<Vec<ActivityEvent>, String> {
    let event_name = header_value(headers, "x-github-event").unwrap_or("");
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let repo = value
        .pointer("/repository/full_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown/unknown");
    match event_name {
        "pull_request" => {
            let pr = value.get("pull_request").cloned().unwrap_or(Value::Null);
            if pr.get("merged").and_then(Value::as_bool) != Some(true) {
                return Ok(Vec::new());
            }
            events_from_pulls_json(repo, &Value::Array(vec![pr]).to_string(), ts)
        }
        "workflow_run" => {
            let run = value.get("workflow_run").cloned().unwrap_or(Value::Null);
            let wrapped = serde_json::json!({ "workflow_runs": [run] });
            events_from_runs_json(repo, &wrapped.to_string(), ts)
        }
        _ => Ok(Vec::new()),
    }
}

pub fn parse_gh_include(raw: &str) -> Result<HttpFetch, FeedError> {
    let (head, body) = match raw.split_once("\r\n\r\n") {
        Some(pair) => pair,
        None => raw.split_once("\n\n").unwrap_or((raw, "")),
    };
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let etag = header_value(head, "etag").map(str::to_string);
    Ok(HttpFetch {
        status,
        etag,
        body: body.to_string(),
    })
}

pub fn gh_cli_fetch(path: &str, etag: Option<&str>) -> Result<HttpFetch, FeedError> {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(["api", "--include", path]);
    if let Some(tag) = etag {
        cmd.args(["-H", &format!("If-None-Match: {tag}")]);
    }
    let output = cmd
        .output()
        .map_err(|e| FeedError::Usage(format!("gh api: {e}")))?;
    let raw = String::from_utf8_lossy(&output.stdout);
    if raw.trim().is_empty() && !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(FeedError::Usage(format!("gh api failed: {}", err.trim())));
    }
    parse_gh_include(&raw)
}

/// Loopback only. Same local trust as `feed.jsonl` and `feed.sock`; no HMAC.
pub(crate) fn bind_webhook(addr: &str) -> Result<TcpListener, FeedError> {
    let parsed: std::net::SocketAddr = addr
        .parse()
        .map_err(|e| FeedError::Usage(format!("webhook_bind: {e}")))?;
    if !parsed.ip().is_loopback() {
        return Err(FeedError::Usage(
            "webhook_bind must be a loopback address".into(),
        ));
    }
    let listener = TcpListener::bind(parsed)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn drain_webhook(
    listener: &TcpListener,
    allowed_repos: &[String],
    seen: &mut HashSet<String>,
    now_unix: u64,
    batch: &mut ReadBatch,
) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Ok(events) = read_webhook_connection(stream, now_unix) {
                    for event in events {
                        if !repo_allowed(&event, allowed_repos) {
                            continue;
                        }
                        if seen.insert(event_id(&event)) {
                            batch.events.push(event);
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

fn repo_allowed(event: &ActivityEvent, allowed_repos: &[String]) -> bool {
    if allowed_repos.is_empty() {
        return true;
    }
    let repo = event
        .payload
        .get("repo")
        .and_then(Value::as_str)
        .unwrap_or("");
    allowed_repos.iter().any(|r| r == repo)
}

fn read_webhook_connection(
    mut stream: TcpStream,
    ts: u64,
) -> Result<Vec<ActivityEvent>, FeedError> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        headers.push_str(&line);
        if line == "\r\n" || line == "\n" {
            break;
        }
        if headers.len() > 8192 {
            return Err(FeedError::Usage("webhook headers too large".into()));
        }
    }
    let len = header_value(&headers, "content-length")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    if len > 65536 {
        return Err(FeedError::Usage("webhook body too large".into()));
    }
    let mut body = vec![0; len];
    if len > 0 {
        reader.read_exact(&mut body)?;
    }
    let body = String::from_utf8(body).map_err(|e| FeedError::Usage(e.to_string()))?;
    let events = events_from_webhook(&headers, &body, ts).map_err(FeedError::Usage)?;
    let _ = writeln!(
        stream,
        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
    );
    Ok(events)
}

fn next_page_path(path: &str) -> String {
    if let Some((base, rest)) = path.rsplit_once("&page=") {
        let n: u32 = rest.parse().unwrap_or(1);
        format!("{base}&page={}", n + 1)
    } else {
        format!("{path}&page=2")
    }
}

fn compose_status(github: &str, webhook_note: Option<&str>) -> String {
    match webhook_note {
        Some(note) => format!("{github}; {note}"),
        None => github.to_string(),
    }
}

fn list_page_len(body: &str) -> usize {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return 0;
    };
    if let Some(arr) = value.as_array() {
        return arr.len();
    }
    value
        .get("workflow_runs")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    for line in headers.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(rest.trim());
        }
    }
    None
}

fn event_id(event: &ActivityEvent) -> String {
    let kind = event.kind.as_key();
    let repo = event
        .payload
        .get("repo")
        .and_then(Value::as_str)
        .unwrap_or("");
    let id = event
        .payload
        .get("id")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "0".into());
    format!("{kind}:{repo}:{id}")
}

fn load_seen(path: &Path) -> Result<HashSet<String>, FeedError> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(FeedError::from),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(e) => Err(e.into()),
    }
}

fn save_seen(path: &Path, seen: &HashSet<String>) -> Result<(), FeedError> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    ensure_parent(path)?;
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, serde_json::to_vec(seen)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn load_etags(path: &Path) -> Result<HashMap<String, String>, FeedError> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(FeedError::from),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.into()),
    }
}

fn save_etags(path: &Path, etags: &HashMap<String, String>) -> Result<(), FeedError> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    ensure_parent(path)?;
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, serde_json::to_vec(etags)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pulls_fixture() -> &'static str {
        include_str!("../tests/fixtures/github-pulls.json")
    }

    fn runs_fixture() -> &'static str {
        include_str!("../tests/fixtures/github-runs.json")
    }

    #[test]
    fn merged_pulls_become_events_and_closed_unmerged_are_skipped() {
        let events = events_from_pulls_json("acme/game", pulls_fixture(), 9).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ActivityKind::PrMerged);
        assert_eq!(events[0].payload["number"], 73);
        assert_eq!(events[0].source, "github");
    }

    #[test]
    fn completed_runs_become_events_and_in_progress_are_skipped() {
        let events = events_from_runs_json("acme/game", runs_fixture(), 9).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ActivityKind::WorkflowFinished);
        assert_eq!(events[0].payload["conclusion"], "success");
    }

    #[test]
    fn webhook_pull_request_merged_matches_the_list_parser() {
        let headers = "X-GitHub-Event: pull_request\r\n";
        let body = include_str!("../tests/fixtures/github-webhook-pr.json");
        let events = events_from_webhook(headers, body, 4).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, ActivityKind::PrMerged);
    }

    #[test]
    fn etag_304_is_parsed() {
        let raw = "HTTP/2.0 304 Not Modified\r\nETag: \"abc\"\r\n\r\n";
        let fetched = parse_gh_include(raw).unwrap();
        assert_eq!(fetched.status, 304);
        assert_eq!(fetched.etag.as_deref(), Some("\"abc\""));
    }

    #[test]
    fn failed_later_endpoint_does_not_keep_seen_ids() {
        let dir = std::env::temp_dir().join(format!("h{}f", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = BridgeConfig {
            enabled: true,
            max_line_bytes: 8192,
            feed_path: dir.join("feed.jsonl"),
            socket_path: None,
            github_repos: vec!["acme/game".into()],
            poll_secs: 0,
            webhook_bind: None,
        };
        fn fail_runs(path: &str, _etag: Option<&str>) -> Result<HttpFetch, FeedError> {
            if path.contains("actions") {
                return Err(FeedError::Usage("runs down".into()));
            }
            Ok(HttpFetch {
                status: 200,
                etag: Some("\"p1\"".into()),
                body: include_str!("../tests/fixtures/github-pulls.json").into(),
            })
        }
        fn ok_both(path: &str, _etag: Option<&str>) -> Result<HttpFetch, FeedError> {
            if path.contains("pulls") {
                Ok(HttpFetch {
                    status: 200,
                    etag: Some("\"p1\"".into()),
                    body: include_str!("../tests/fixtures/github-pulls.json").into(),
                })
            } else {
                Ok(HttpFetch {
                    status: 200,
                    etag: Some("\"r1\"".into()),
                    body: include_str!("../tests/fixtures/github-runs.json").into(),
                })
            }
        }
        let mut a = GithubPoller::open(&cfg).unwrap();
        a.set_fetch(fail_runs);
        assert!(a.poll(10).unwrap().events.is_empty());
        a.set_fetch(ok_both);
        let recovered = a.poll(11).unwrap();
        assert_eq!(recovered.events.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn poller_delivers_each_id_once_across_restarts() {
        let dir = std::env::temp_dir().join(format!("h{}g", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = BridgeConfig {
            enabled: true,
            max_line_bytes: 8192,
            feed_path: dir.join("feed.jsonl"),
            socket_path: None,
            github_repos: vec!["acme/game".into()],
            poll_secs: 0,
            webhook_bind: None,
        };
        fn fake(_path: &str, _etag: Option<&str>) -> Result<HttpFetch, FeedError> {
            if _path.contains("pulls") {
                Ok(HttpFetch {
                    status: 200,
                    etag: Some("\"p1\"".into()),
                    body: include_str!("../tests/fixtures/github-pulls.json").into(),
                })
            } else {
                Ok(HttpFetch {
                    status: 200,
                    etag: Some("\"r1\"".into()),
                    body: include_str!("../tests/fixtures/github-runs.json").into(),
                })
            }
        }
        let mut a = GithubPoller::open(&cfg).unwrap();
        a.set_fetch(fake);
        let first = a.poll(10).unwrap();
        assert_eq!(first.events.len(), 2);
        let second = a.poll(11).unwrap();
        assert!(second.events.is_empty());
        drop(a);
        let mut b = GithubPoller::open(&cfg).unwrap();
        b.set_fetch(fake);
        let third = b.poll(12).unwrap();
        assert!(third.events.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn webhook_bind_rejects_non_loopback() {
        let err = bind_webhook("0.0.0.0:0").unwrap_err();
        assert!(err.to_string().contains("loopback"));
    }

    #[test]
    fn webhook_bind_failure_is_in_status_and_open_succeeds() {
        let dir = std::env::temp_dir().join(format!("h{}b", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = BridgeConfig {
            enabled: true,
            max_line_bytes: 8192,
            feed_path: dir.join("feed.jsonl"),
            socket_path: None,
            github_repos: Vec::new(),
            poll_secs: 0,
            webhook_bind: Some("0.0.0.0:0".into()),
        };
        let p = GithubPoller::open(&cfg).unwrap();
        assert!(p.status().contains("webhook bind failed"));
        assert!(p.status().contains("loopback"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn poller_follows_a_full_page() {
        let dir = std::env::temp_dir().join(format!("h{}p", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = BridgeConfig {
            enabled: true,
            max_line_bytes: 8192,
            feed_path: dir.join("feed.jsonl"),
            socket_path: None,
            github_repos: vec!["acme/game".into()],
            poll_secs: 0,
            webhook_bind: None,
        };
        fn fake(path: &str, _etag: Option<&str>) -> Result<HttpFetch, FeedError> {
            if path.contains("actions") {
                return Ok(HttpFetch {
                    status: 200,
                    etag: None,
                    body: r#"{"workflow_runs":[]}"#.into(),
                });
            }
            if path.contains("page=2") {
                return Ok(HttpFetch {
                    status: 200,
                    etag: None,
                    body:
                        r#"[{"id":99,"number":99,"title":"p2","merged_at":"2026-01-01T00:00:00Z"}]"#
                            .into(),
                });
            }
            let items: Vec<String> = (1..=PAGE_SIZE)
                .map(|i| {
                    format!(
                        r#"{{"id":{i},"number":{i},"title":"p{i}","merged_at":"2026-01-01T00:00:00Z"}}"#
                    )
                })
                .collect();
            Ok(HttpFetch {
                status: 200,
                etag: None,
                body: format!("[{}]", items.join(",")),
            })
        }
        let mut a = GithubPoller::open(&cfg).unwrap();
        a.set_fetch(fake);
        let first = a.poll(10).unwrap();
        assert_eq!(first.events.len(), PAGE_SIZE as usize + 1);
        assert!(
            first
                .events
                .iter()
                .any(|e| e.payload["id"] == serde_json::json!(99))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn poller_stops_after_max_pages() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static PAGE11: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("h{}m", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = BridgeConfig {
            enabled: true,
            max_line_bytes: 8192,
            feed_path: dir.join("feed.jsonl"),
            socket_path: None,
            github_repos: vec!["acme/game".into()],
            poll_secs: 0,
            webhook_bind: None,
        };
        fn fake(path: &str, _etag: Option<&str>) -> Result<HttpFetch, FeedError> {
            if path.contains("actions") {
                return Ok(HttpFetch {
                    status: 200,
                    etag: None,
                    body: r#"{"workflow_runs":[]}"#.into(),
                });
            }
            if path.contains("page=11") {
                PAGE11.fetch_add(1, Ordering::SeqCst);
            }
            let page: u32 = path
                .rsplit_once("&page=")
                .and_then(|(_, rest)| rest.parse().ok())
                .unwrap_or(1);
            let start = (page - 1) * PAGE_SIZE + 1;
            let items: Vec<String> = (start..start + PAGE_SIZE)
                .map(|i| {
                    format!(
                        r#"{{"id":{i},"number":{i},"title":"p{i}","merged_at":"2026-01-01T00:00:00Z"}}"#
                    )
                })
                .collect();
            Ok(HttpFetch {
                status: 200,
                etag: None,
                body: format!("[{}]", items.join(",")),
            })
        }
        let mut a = GithubPoller::open(&cfg).unwrap();
        a.set_fetch(fake);
        let first = a.poll(10).unwrap();
        assert_eq!(first.events.len(), (PAGE_SIZE * MAX_PAGES) as usize);
        assert_eq!(PAGE11.load(Ordering::SeqCst), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_first_page_still_reads_later_pages() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static STEP: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("h{}u", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = BridgeConfig {
            enabled: true,
            max_line_bytes: 8192,
            feed_path: dir.join("feed.jsonl"),
            socket_path: None,
            github_repos: vec!["acme/game".into()],
            poll_secs: 0,
            webhook_bind: None,
        };
        fn fake(path: &str, etag: Option<&str>) -> Result<HttpFetch, FeedError> {
            if path.contains("actions") {
                return Ok(HttpFetch {
                    status: 200,
                    etag: None,
                    body: r#"{"workflow_runs":[]}"#.into(),
                });
            }
            if !path.contains("&page=") {
                if etag == Some("\"p1\"") {
                    return Ok(HttpFetch {
                        status: 304,
                        etag: Some("\"p1\"".into()),
                        body: String::new(),
                    });
                }
                let items: Vec<String> = (1..=PAGE_SIZE)
                    .map(|i| {
                        format!(
                            r#"{{"id":{i},"number":{i},"title":"p{i}","merged_at":"2026-01-01T00:00:00Z"}}"#
                        )
                    })
                    .collect();
                return Ok(HttpFetch {
                    status: 200,
                    etag: Some("\"p1\"".into()),
                    body: format!("[{}]", items.join(",")),
                });
            }
            let merged = if STEP.load(Ordering::SeqCst) == 0 {
                "null"
            } else {
                "\"2026-01-02T00:00:00Z\""
            };
            Ok(HttpFetch {
                status: 200,
                etag: Some("\"p2\"".into()),
                body: format!(r#"[{{"id":99,"number":99,"title":"later","merged_at":{merged}}}]"#),
            })
        }
        let mut a = GithubPoller::open(&cfg).unwrap();
        a.set_fetch(fake);
        let first = a.poll(10).unwrap();
        assert_eq!(first.events.len(), PAGE_SIZE as usize);
        STEP.store(1, Ordering::SeqCst);
        let second = a.poll(11).unwrap();
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].payload["id"], serde_json::json!(99));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn webhook_only_seen_ids_survive_restart() {
        let dir = std::env::temp_dir().join(format!("h{}w", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cfg = BridgeConfig {
            enabled: true,
            max_line_bytes: 8192,
            feed_path: dir.join("feed.jsonl"),
            socket_path: None,
            github_repos: Vec::new(),
            poll_secs: 0,
            webhook_bind: Some("127.0.0.1:0".into()),
        };
        let mut a = GithubPoller::open(&cfg).unwrap();
        let event = ActivityEvent {
            kind: ActivityKind::PrMerged,
            source: "github".into(),
            ts: 1,
            payload: serde_json::json!({"repo":"acme/game","id":3001,"number":12}),
        };
        assert!(a.seen.insert(event_id(&event)));
        a.poll(1).unwrap();
        drop(a);
        let b = GithubPoller::open(&cfg).unwrap();
        assert!(b.seen.contains(&event_id(&event)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_poller_does_not_fetch() {
        static FETCHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        fn boom(_path: &str, _etag: Option<&str>) -> Result<HttpFetch, FeedError> {
            FETCHED.store(true, std::sync::atomic::Ordering::SeqCst);
            Err(FeedError::Usage("should not fetch".into()))
        }
        let mut p = GithubPoller::disabled();
        p.set_fetch(boom);
        let batch = p.poll(1).unwrap();
        assert!(batch.events.is_empty());
        assert_eq!(p.status(), "bridge off");
        assert!(!FETCHED.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    #[ignore = "needs a logged-in gh"]
    fn live_gh_api_conditional_request() {
        let fetched = gh_cli_fetch("rate_limit", None).unwrap();
        assert_eq!(fetched.status, 200);
    }
}
