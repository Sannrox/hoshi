//! Template commentary derived from a combat log. Optional local rephrase is
//! compiled behind `commentary-model` and never talks to a remote host.

use crate::combat::{CombatFrame, CombatReport, CombatWinner, next_u64};
use crate::ships::ShipKind;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_COMMENTARY: &str = include_str!("../data/commentary.toml");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentaryError {
    Parse(String),
    EmptyCatalog,
    DuplicateId(String),
    EmptyId,
    EmptyLines(String),
}

impl fmt::Display for CommentaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommentaryError::Parse(msg) => write!(f, "commentary: {msg}"),
            CommentaryError::EmptyCatalog => write!(f, "commentary: catalog is empty"),
            CommentaryError::DuplicateId(id) => write!(f, "commentary: duplicate id {id}"),
            CommentaryError::EmptyId => write!(f, "commentary: empty id"),
            CommentaryError::EmptyLines(id) => write!(f, "commentary: {id} has no lines"),
        }
    }
}

impl std::error::Error for CommentaryError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentaryCatalog {
    events: BTreeMap<String, Vec<String>>,
}

impl CommentaryCatalog {
    pub fn bundled() -> Self {
        Self::parse(DEFAULT_COMMENTARY).expect("bundled commentary.toml must be valid")
    }

    pub fn parse(text: &str) -> Result<Self, CommentaryError> {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            events: Vec<FileEvent>,
        }
        #[derive(Deserialize)]
        struct FileEvent {
            id: String,
            #[serde(default)]
            lines: Vec<String>,
        }
        let file: File = toml::from_str(text).map_err(|e| CommentaryError::Parse(e.to_string()))?;
        let mut events = BTreeMap::new();
        for row in file.events {
            if row.id.is_empty() {
                return Err(CommentaryError::EmptyId);
            }
            if events.contains_key(&row.id) {
                return Err(CommentaryError::DuplicateId(row.id));
            }
            let lines: Vec<String> = row
                .lines
                .into_iter()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            if lines.len() < 2 {
                return Err(CommentaryError::EmptyLines(row.id));
            }
            events.insert(row.id, lines);
        }
        if events.len() < 10 {
            return Err(CommentaryError::EmptyCatalog);
        }
        Ok(Self { events })
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// One optional line per report frame. Deterministic for `report.seed`.
    pub fn lines_for(&self, report: &CombatReport) -> Vec<Option<String>> {
        let mut out = vec![None; report.frames.len()];
        let mut rng = report.seed;
        let mut seen_blood = false;
        let mut prev_cmp: Option<std::cmp::Ordering> = None;
        for (i, slot) in out.iter_mut().enumerate() {
            let kind = event_for(report, i, seen_blood, prev_cmp);
            if let Some(frame) = report.frames.get(i) {
                if frame.explosions > 0 {
                    seen_blood = true;
                }
                prev_cmp = Some(cmp_totals(frame));
            }
            let Some(kind) = kind else {
                continue;
            };
            let Some(lines) = self.events.get(kind) else {
                continue;
            };
            let pick = next_u64(&mut rng) as usize % lines.len();
            *slot = Some(lines[pick].clone());
        }
        out
    }
}

fn cmp_totals(frame: &CombatFrame) -> std::cmp::Ordering {
    frame.attacker.total().cmp(&frame.defender.total())
}

fn event_for(
    report: &CombatReport,
    index: usize,
    seen_blood: bool,
    prev_cmp: Option<std::cmp::Ordering>,
) -> Option<&'static str> {
    let frames = &report.frames;
    let curr = frames.get(index)?;
    let last = index + 1 == frames.len();
    if last {
        return Some(match report.winner {
            CombatWinner::Defender => "victory",
            CombatWinner::Attacker => "defeat",
            CombatWinner::Draw => "stalemate",
        });
    }
    if index == 0 {
        return None;
    }
    let prev = &frames[index - 1];
    if curr.defender.total() == 0 || curr.attacker.total() == 0 {
        return Some("wipe");
    }
    if lost_kind(prev, curr, ShipKind::Battleship)
        || lost_kind(prev, curr, ShipKind::Cruiser)
        || lost_kind(prev, curr, ShipKind::HeavyFighter)
    {
        return Some("flagship_lost");
    }
    if !seen_blood && curr.explosions > 0 {
        return Some("first_blood");
    }
    if lost_kind(prev, curr, ShipKind::LightFighter)
        && (curr.attacker.get(ShipKind::LightFighter) == 0
            || curr.defender.get(ShipKind::LightFighter) == 0)
    {
        return Some("screen_breaks");
    }
    if lost_kind(prev, curr, ShipKind::SmallCargo) || lost_kind(prev, curr, ShipKind::LargeCargo) {
        return Some("cargo_lost");
    }
    if lost_kind(prev, curr, ShipKind::EspionageProbe) {
        return Some("probe_sniped");
    }
    if curr.round == 1 && curr.shots > 0 {
        return Some("opening_volley");
    }
    let now = cmp_totals(curr);
    if let Some(before) = prev_cmp
        && before != std::cmp::Ordering::Equal
        && now != before
        && now != std::cmp::Ordering::Equal
    {
        return Some("reversal");
    }
    let min = curr.attacker.total().min(curr.defender.total());
    let max = curr.attacker.total().max(curr.defender.total());
    if min > 0 && min <= 3 && max >= min.saturating_mul(3) {
        return Some("last_stand");
    }
    None
}

fn lost_kind(prev: &CombatFrame, curr: &CombatFrame, kind: ShipKind) -> bool {
    prev.attacker.get(kind) > curr.attacker.get(kind)
        || prev.defender.get(kind) > curr.defender.get(kind)
}

#[cfg(feature = "commentary-model")]
const MODEL_BUDGET: std::time::Duration = std::time::Duration::from_millis(100);

/// Rephrase template lines in one local request. Compiled out unless
/// `commentary-model` is enabled; always falls back to the templates and never
/// spends more than [`MODEL_BUDGET`]. Loopback only.
pub fn rephrase_lines(lines: &mut [Option<String>], endpoint: &str) {
    rephrase_lines_allow(lines, endpoint, false);
}

/// Same as [`rephrase_lines`], but a private IPv4 host is allowed when
/// `allow_lan` is set. Public addresses stay refused.
pub fn rephrase_lines_allow(lines: &mut [Option<String>], endpoint: &str, allow_lan: bool) {
    rephrase_lines_named(lines, endpoint, allow_lan, "");
}

/// Same as [`rephrase_lines_allow`], sending `model` in the JSON body when
/// non-empty.
pub fn rephrase_lines_named(
    lines: &mut [Option<String>],
    endpoint: &str,
    allow_lan: bool,
    model: &str,
) {
    #[cfg(feature = "commentary-model")]
    rephrase_lines_model(lines, endpoint, allow_lan, model);
    #[cfg(not(feature = "commentary-model"))]
    {
        let _ = (lines, endpoint, allow_lan, model);
    }
}

/// Rephrase a template line through a local endpoint. Compiled out unless
/// `commentary-model` is enabled; always falls back to the template.
pub fn rephrase_local(line: &str, endpoint: &str) -> String {
    let mut lines = [Some(line.to_string())];
    rephrase_lines(&mut lines, endpoint);
    lines[0].take().unwrap_or_else(|| line.to_string())
}

/// Probe a local model URL with a 100ms TCP budget. Public hosts are refused
/// even when `allow_lan` is set; private IPv4 is allowed only then.
pub fn probe_model_endpoint(endpoint: &str, allow_lan: bool) -> Result<(), String> {
    if endpoint.is_empty() {
        return Err("no endpoint".into());
    }
    let Some((addr, _, _)) = model_connect_addr(endpoint, allow_lan) else {
        return Err("only loopback, or a private IPv4 host when LAN is allowed".into());
    };
    match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(100)) {
        Ok(_) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

/// Parse an `http://` endpoint into a connect address. Loopback always;
/// private IPv4 only when `allow_lan` is set. Public IPs and hostnames fail.
pub fn model_connect_addr(
    endpoint: &str,
    allow_lan: bool,
) -> Option<(std::net::SocketAddr, String, String)> {
    let url = endpoint.strip_prefix("http://")?;
    let (hostport, rest) = url.split_once('/').unwrap_or((url, ""));
    let (host, port) = hostport.split_once(':').unwrap_or((hostport, "80"));
    let port: u16 = port.parse().ok()?;
    let ip = if host.eq_ignore_ascii_case("localhost") {
        std::net::Ipv4Addr::LOCALHOST
    } else {
        host.parse().ok()?
    };
    if !(ip.is_loopback() || (allow_lan && ip.is_private())) {
        return None;
    }
    let path = if rest.is_empty() {
        "/".to_string()
    } else {
        format!("/{rest}")
    };
    Some((
        std::net::SocketAddr::from((ip, port)),
        host.to_string(),
        path,
    ))
}

#[cfg(feature = "commentary-model")]
fn rephrase_lines_model(
    lines: &mut [Option<String>],
    endpoint: &str,
    allow_lan: bool,
    model: &str,
) {
    let originals: Vec<String> = lines.iter().filter_map(|slot| slot.clone()).collect();
    if originals.is_empty() {
        return;
    }
    let Some(out) = rephrase_local_batch(&originals, endpoint, allow_lan, model) else {
        return;
    };
    if out.len() != originals.len() {
        return;
    }
    let mut i = 0;
    for slot in lines.iter_mut() {
        if slot.is_some() {
            *slot = Some(out[i].clone());
            i += 1;
        }
    }
}

#[cfg(feature = "commentary-model")]
fn remaining(deadline: std::time::Instant) -> Option<std::time::Duration> {
    deadline
        .checked_duration_since(std::time::Instant::now())
        .filter(|d| !d.is_zero())
}

#[cfg(feature = "commentary-model")]
fn rephrase_local_batch(
    lines: &[String],
    endpoint: &str,
    allow_lan: bool,
    model: &str,
) -> Option<Vec<String>> {
    let (addr, host, path) = model_connect_addr(endpoint, allow_lan)?;
    let deadline = std::time::Instant::now() + MODEL_BUDGET;
    let mut stream = std::net::TcpStream::connect_timeout(&addr, remaining(deadline)?).ok()?;
    stream.set_read_timeout(remaining(deadline)).ok()?;
    stream.set_write_timeout(remaining(deadline)).ok()?;
    let escaped: Vec<String> = lines
        .iter()
        .map(|line| format!("\"{}\"", line.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    let model_field = if model.is_empty() {
        String::new()
    } else {
        format!(
            "\"model\":\"{}\",",
            model.replace('\\', "\\\\").replace('"', "\\\"")
        )
    };
    let body = format!("{{{model_field}\"lines\":[{}]}}", escaped.join(","));
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    use std::io::{Read, Write};
    stream.write_all(req.as_bytes()).ok()?;
    stream.set_read_timeout(remaining(deadline)).ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    let text = buf.split("\r\n\r\n").nth(1)?.trim();
    if text.is_empty() {
        return None;
    }
    let out: Vec<String> = text
        .lines()
        .map(|line| {
            line.chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect::<String>()
        })
        .filter(|line| !line.is_empty())
        .collect();
    (out.len() == lines.len()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GameState;
    use crate::ships::{Fleet, ShipKind};

    #[test]
    fn bundled_catalog_has_at_least_ten_events() {
        let catalog = CommentaryCatalog::bundled();
        assert!(catalog.len() >= 10, "{}", catalog.len());
    }

    #[test]
    fn same_seed_repeats_the_same_lines() {
        let catalog = CommentaryCatalog::bundled();
        let mut a = GameState::new(1_000);
        a.fleet.add(ShipKind::LightFighter, 6);
        a.trigger_raid(1_000);
        let mut b = GameState::new(1_000);
        b.fleet.add(ShipKind::LightFighter, 6);
        b.trigger_raid(1_000);
        let log_a = a.combat_logs.last().expect("log");
        let log_b = b.combat_logs.last().expect("log");
        assert_eq!(
            catalog.lines_for(&log_a.report),
            catalog.lines_for(&log_b.report)
        );
        let lines = catalog.lines_for(&log_a.report);
        assert_eq!(lines.len(), log_a.report.frames.len());
        assert!(
            lines.iter().any(|line| line.is_some()),
            "expected at least one comment"
        );
    }

    #[test]
    fn parse_rejects_too_few_events_or_lines() {
        assert!(matches!(
            CommentaryCatalog::parse("[[events]]\nid = \"only\"\nlines = [\"a\", \"b\"]\n"),
            Err(CommentaryError::EmptyCatalog)
        ));
        let mut text = String::new();
        for i in 0..10 {
            text.push_str(&format!("[[events]]\nid = \"e{i}\"\nlines = [\"one\"]\n"));
        }
        assert!(matches!(
            CommentaryCatalog::parse(&text),
            Err(CommentaryError::EmptyLines(_))
        ));
    }

    #[test]
    fn reversal_fires_on_the_frame_totals_swap() {
        let catalog = CommentaryCatalog::bundled();
        let report = CombatReport {
            rounds: 3,
            attacker_before: counts(10),
            defender_before: counts(5),
            attacker_after: Fleet::default(),
            defender_after: counts(7),
            winner: CombatWinner::Defender,
            ground: None,
            seed: 7,
            frames: vec![
                snap(0, 10, 5, 0, 0),
                snap(1, 8, 6, 4, 1),
                snap(2, 4, 7, 4, 3),
                snap(3, 0, 7, 2, 4),
            ],
            layered: true,
        };
        assert_eq!(event_for(&report, 0, false, None), None);
        assert_eq!(
            event_for(&report, 2, true, Some(cmp_totals(&report.frames[1]))),
            Some("reversal")
        );
        assert_eq!(
            event_for(&report, 3, true, Some(cmp_totals(&report.frames[2]))),
            Some("victory")
        );
        let lines = catalog.lines_for(&report);
        assert!(lines[2].is_some());
        assert!(lines[3].is_some());
    }

    #[test]
    fn rephrase_keeps_the_template_off_loopback() {
        let line = "The screen cannot hold.";
        assert_eq!(rephrase_local(line, "https://example.invalid/v1"), line);
        assert_eq!(rephrase_local(line, "http://8.8.8.8/v1"), line);
        assert_eq!(rephrase_local(line, ""), line);
        let mut lines = [
            Some(line.to_string()),
            None,
            Some("You hold the field.".into()),
        ];
        rephrase_lines(&mut lines, "http://8.8.8.8/v1");
        assert_eq!(lines[0].as_deref(), Some(line));
        assert_eq!(lines[2].as_deref(), Some("You hold the field."));
        assert!(model_connect_addr("http://127.0.0.1:8080/v1", false).is_some());
        assert!(model_connect_addr("http://localhost/v1", false).is_some());
        assert!(model_connect_addr("http://192.168.1.10:8080/v1", true).is_some());
        assert!(model_connect_addr("http://192.168.1.10:8080/v1", false).is_none());
        assert!(model_connect_addr("http://8.8.8.8/v1", true).is_none());
        assert_eq!(
            probe_model_endpoint("http://8.8.8.8/v1", true).unwrap_err(),
            "only loopback, or a private IPv4 host when LAN is allowed"
        );
        assert_eq!(probe_model_endpoint("", false).unwrap_err(), "no endpoint");
        let started = std::time::Instant::now();
        assert!(probe_model_endpoint("http://127.0.0.1:1/v1", false).is_err());
        assert!(started.elapsed() < std::time::Duration::from_millis(400));
    }

    #[cfg(feature = "commentary-model")]
    #[test]
    fn rephrase_times_out_on_closed_loopback() {
        let line = "The screen cannot hold.";
        let started = std::time::Instant::now();
        let mut lines = [
            Some(line.to_string()),
            Some("You hold the field.".into()),
            Some("The line is gone.".into()),
        ];
        rephrase_lines(&mut lines, "http://127.0.0.1:1/v1/chat");
        assert!(started.elapsed() < std::time::Duration::from_millis(400));
        assert_eq!(lines[0].as_deref(), Some(line));
        assert_eq!(rephrase_local(line, "http://127.0.0.1:1/v1/chat"), line);
    }

    fn counts(n: u32) -> Fleet {
        let mut fleet = Fleet::default();
        fleet.add(ShipKind::LightFighter, n);
        fleet
    }

    fn snap(round: u8, atk: u32, def: u32, shots: u32, explosions: u32) -> CombatFrame {
        CombatFrame {
            round,
            attacker: counts(atk),
            defender: counts(def),
            shots,
            explosions,
        }
    }
}
