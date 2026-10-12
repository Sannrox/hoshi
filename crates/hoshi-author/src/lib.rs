//! Offline drafts of story events. The game never calls this crate.

use hoshi_core::{StoryCatalog, StoryError, advisor_endpoint_allowed, model_connect_addr};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MODEL_BUDGET: Duration = Duration::from_secs(2);
const MAX_COUNT: u32 = 20;

/// Inputs for `events draft`.
#[derive(Debug, Clone)]
pub struct DraftRequest {
    pub theme: String,
    pub count: u32,
    pub out_dir: PathBuf,
    pub endpoint: String,
    pub model: String,
    pub allow_lan: bool,
}

/// Optional `author.toml` beside the working directory. Empty means skeleton.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorConfig {
    pub endpoint: String,
    pub model: String,
    pub allow_lan: bool,
}

#[derive(Debug)]
pub enum DraftError {
    Usage(String),
    Remote(String),
    Validate(StoryError),
    Io(std::io::Error),
}

impl std::fmt::Display for DraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DraftError::Usage(msg) | DraftError::Remote(msg) => f.write_str(msg),
            DraftError::Validate(err) => write!(f, "{err}"),
            DraftError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for DraftError {}

impl From<std::io::Error> for DraftError {
    fn from(value: std::io::Error) -> Self {
        DraftError::Io(value)
    }
}

impl AuthorConfig {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut cfg = Self::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = strip_toml_string(value.trim());
            match key {
                "endpoint" => cfg.endpoint = value,
                "model" => cfg.model = value,
                "allow_lan" => cfg.allow_lan = value == "true",
                _ => {}
            }
        }
        cfg
    }
}

/// Theme slug used in ids and the output file name.
pub fn slug_theme(theme: &str) -> Result<String, DraftError> {
    let mut out = String::new();
    let mut prev_us = false;
    for c in theme.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_us = false;
        } else if !out.is_empty() && !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        return Err(DraftError::Usage(
            "theme must contain a letter or digit".into(),
        ));
    }
    if out.len() > 32 {
        return Ok(out[..32].trim_end_matches('_').to_string());
    }
    Ok(out)
}

/// Templated catalog that already passes [`StoryCatalog::parse`].
pub fn skeleton_toml(theme: &str, count: u32) -> Result<String, DraftError> {
    if count == 0 || count > MAX_COUNT {
        return Err(DraftError::Usage(format!("count must be 1..={MAX_COUNT}")));
    }
    let slug = slug_theme(theme)?;
    let mut out = String::from(
        "# Draft — not shipped. Edit, then copy [[event]] blocks into crates/hoshi-core/data/story.toml.\n\n",
    );
    out.push_str("first_delay_secs = 1800\ncooldown_secs = 3600\n");
    for i in 1..=count {
        let id = format!("draft_{slug}_{i}");
        let title = toml_escape(&format!("{theme} draft {i}"));
        let text = toml_escape(&format!(
            "Placeholder about {theme}. Rewrite before moving into story.toml."
        ));
        out.push_str(&format!(
            "
[[event]]
id = \"{id}\"
title = {title}
text = {text}
weight = 8
timeout_secs = 86400
timeout_choice = \"leave\"
min_elapsed_secs = 0

[[event.choices]]
id = \"act\"
label = \"Act\"
metal = 50.0

[[event.choices]]
id = \"leave\"
label = \"Leave\"
"
        ));
    }
    Ok(out)
}

/// Validate with the same loader as shipped `story.toml`.
pub fn validate_draft(toml: &str) -> Result<StoryCatalog, DraftError> {
    StoryCatalog::parse(toml).map_err(DraftError::Validate)
}

/// Write a validated draft, optionally polishing through a local endpoint.
pub fn write_draft(req: &DraftRequest) -> Result<PathBuf, DraftError> {
    let mut toml = skeleton_toml(&req.theme, req.count)?;
    if !req.endpoint.is_empty() {
        if !advisor_endpoint_allowed(&req.endpoint, req.allow_lan) {
            return Err(DraftError::Remote(
                "only loopback, or a private IPv4 host when LAN is allowed".into(),
            ));
        }
        if let Some(polished) =
            polish_draft(&toml, &req.endpoint, req.allow_lan, &req.model, &req.theme)
            && let Ok(cat) = validate_draft(&polished)
            && cat.len() == req.count as usize
        {
            toml = polished;
        }
    }
    let cat = validate_draft(&toml)?;
    if cat.len() != req.count as usize {
        return Err(DraftError::Usage(format!(
            "draft has {} events, expected {}",
            cat.len(),
            req.count
        )));
    }
    fs::create_dir_all(&req.out_dir)?;
    let path = req
        .out_dir
        .join(format!("{}.toml", slug_theme(&req.theme)?));
    fs::write(&path, toml)?;
    Ok(path)
}

fn polish_draft(
    skeleton: &str,
    endpoint: &str,
    allow_lan: bool,
    model: &str,
    theme: &str,
) -> Option<String> {
    let (addr, host, path) = model_connect_addr(endpoint, allow_lan)?;
    let deadline = Instant::now() + MODEL_BUDGET;
    let mut stream = TcpStream::connect_timeout(&addr, remaining(deadline)?).ok()?;
    stream.set_read_timeout(remaining(deadline)).ok()?;
    stream.set_write_timeout(remaining(deadline)).ok()?;
    let body = serde_json::json!({
        "theme": theme,
        "model": model,
        "toml": skeleton,
    })
    .to_string();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).ok()?;
    stream.set_read_timeout(remaining(deadline)).ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    let text = buf.split("\r\n\r\n").nth(1)?.trim();
    extract_toml(text)
}

fn extract_toml(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("```toml") {
        return rest.split("```").next().map(|s| s.trim().to_string());
    }
    if trimmed.contains("[[event]]") {
        return Some(trimmed.to_string());
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    value
        .get("toml")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
}

fn remaining(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
}

fn toml_escape(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn strip_toml_string(value: &str) -> String {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hoshi-author-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn skeleton_passes_the_story_loader() {
        let toml = skeleton_toml("Ice comet", 2).expect("skel");
        let cat = validate_draft(&toml).expect("valid");
        assert_eq!(cat.len(), 2);
        assert!(cat.get("draft_ice_comet_1").is_some());
        assert!(cat.get("draft_ice_comet_2").is_some());
    }

    #[test]
    fn loader_rejects_a_broken_draft() {
        let bad = skeleton_toml("raid", 1)
            .expect("skel")
            .replace("timeout_choice = \"leave\"", "timeout_choice = \"missing\"");
        assert!(matches!(
            validate_draft(&bad),
            Err(DraftError::Validate(StoryError::UnknownTimeout { .. }))
        ));
    }

    #[test]
    fn writes_skeleton_without_a_model() {
        let dir = scratch();
        let path = write_draft(&DraftRequest {
            theme: "salvage".into(),
            count: 1,
            out_dir: dir.clone(),
            endpoint: String::new(),
            model: String::new(),
            allow_lan: false,
        })
        .expect("write");
        assert_eq!(path, dir.join("salvage.toml"));
        let cat = validate_draft(&fs::read_to_string(&path).expect("read")).expect("valid");
        assert_eq!(cat.len(), 1);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn public_endpoint_is_refused() {
        let dir = scratch();
        let err = write_draft(&DraftRequest {
            theme: "raid".into(),
            count: 1,
            out_dir: dir.clone(),
            endpoint: "http://8.8.8.8/v1".into(),
            model: String::new(),
            allow_lan: true,
        })
        .expect_err("remote");
        assert!(matches!(err, DraftError::Remote(_)));
        assert!(!dir.join("raid.toml").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn down_loopback_falls_back_to_skeleton() {
        let dir = scratch();
        let started = Instant::now();
        let path = write_draft(&DraftRequest {
            theme: "traders".into(),
            count: 1,
            out_dir: dir.clone(),
            endpoint: "http://127.0.0.1:1/v1".into(),
            model: String::new(),
            allow_lan: false,
        })
        .expect("skeleton fallback");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            fs::read_to_string(path)
                .expect("read")
                .contains("draft_traders_1")
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn local_model_toml_replaces_the_skeleton() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let polished = skeleton_toml("probe", 1)
            .expect("skel")
            .replace("probe draft 1", "Local model title");
        let reply = polished.clone();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            let _ = stream.write_all(body.as_bytes());
        });
        let dir = scratch();
        let path = write_draft(&DraftRequest {
            theme: "probe".into(),
            count: 1,
            out_dir: dir.clone(),
            endpoint: format!("http://127.0.0.1:{}/v1", addr.port()),
            model: "local".into(),
            allow_lan: false,
        })
        .expect("model");
        let text = fs::read_to_string(path).expect("read");
        assert!(text.contains("Local model title"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn config_defaults_keep_the_model_off() {
        let dir = scratch();
        let path = dir.join("author.toml");
        fs::write(&path, "model = \"llama\"\nallow_lan = false\n").expect("cfg");
        let cfg = AuthorConfig::load(&path);
        assert_eq!(cfg.endpoint, "");
        assert_eq!(cfg.model, "llama");
        assert!(!cfg.allow_lan);
        fs::remove_dir_all(dir).ok();
    }
}
