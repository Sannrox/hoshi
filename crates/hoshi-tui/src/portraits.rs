//! Commander, admiral, pirate and planet portraits.
//!
//! ASCII is the layout-stable path. When images are on, the same 8x8 RGB grid
//! becomes half-block cells, and terminals that advertise an inline graphics
//! protocol can receive the raw RGB after the frame is drawn. Disabled images
//! never touch the RGB blobs.

use hoshi_core::{GameEvent, PlanetType};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::io::{self, Write};

pub const PIXEL_SIZE: u32 = 8;
pub const ASCII_ROWS: usize = 4;
pub const ART_COLS: u16 = 8;
pub const PORTRAIT_COLS: u16 = 9;
pub const MIN_WIDTH_FOR_PORTRAIT: u16 = 50;
#[allow(dead_code)]
pub const MAX_ASSET_BYTES: u64 = 2048;
#[allow(dead_code)]
pub const MAX_PACK_BYTES: u64 = 32_768;

const MOODS: &str = include_str!("../assets/portraits/moods.conf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Commander,
    Admiral,
    Pirate,
    Planet(PlanetType),
    RarePlanet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Neutral,
    Pleased,
    Alarmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolChoice {
    Auto,
    Off,
    Graphics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    None,
    Graphics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortraitView {
    pub enabled: bool,
    pub protocol: Protocol,
}

impl Default for PortraitView {
    fn default() -> Self {
        Self {
            enabled: true,
            protocol: Protocol::None,
        }
    }
}

impl Slot {
    fn key(self) -> &'static str {
        match self {
            Slot::Commander => "commander",
            Slot::Admiral => "admiral",
            Slot::Pirate => "pirate",
            Slot::Planet(PlanetType::Rocky) => "planet-rocky",
            Slot::Planet(PlanetType::Ice) => "planet-ice",
            Slot::Planet(PlanetType::GasGiant) => "planet-gas_giant",
            Slot::RarePlanet => "planet-rare",
        }
    }

    fn uses_mood(self) -> bool {
        matches!(self, Slot::Commander | Slot::Admiral | Slot::Pirate)
    }
}

impl Mood {
    fn as_key(self) -> &'static str {
        match self {
            Mood::Neutral => "neutral",
            Mood::Pleased => "pleased",
            Mood::Alarmed => "alarmed",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "neutral" => Mood::Neutral,
            "pleased" => Mood::Pleased,
            "alarmed" => Mood::Alarmed,
            _ => return None,
        })
    }
}

impl ProtocolChoice {
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "auto" => ProtocolChoice::Auto,
            "off" => ProtocolChoice::Off,
            "graphics" => ProtocolChoice::Graphics,
            _ => return None,
        })
    }

    pub fn as_key(self) -> &'static str {
        match self {
            ProtocolChoice::Auto => "auto",
            ProtocolChoice::Off => "off",
            ProtocolChoice::Graphics => "graphics",
        }
    }
}

/// Split a content area so a portrait can sit on the right without wrapping the header.
pub fn split_area(area: Rect) -> (Rect, Option<Rect>) {
    if area.width < MIN_WIDTH_FOR_PORTRAIT {
        return (area, None);
    }
    let chunks = Layout::horizontal([
        Constraint::Min(1),
        Constraint::Length(PORTRAIT_COLS.min(area.width)),
    ])
    .split(area);
    (chunks[0], Some(chunks[1]))
}

pub fn detect_protocol(choice: ProtocolChoice, env: impl Fn(&str) -> Option<String>) -> Protocol {
    match choice {
        ProtocolChoice::Off => Protocol::None,
        ProtocolChoice::Graphics => Protocol::Graphics,
        ProtocolChoice::Auto => {
            if let Some(force) = env("HOSHI_PORTRAIT_PROTOCOL") {
                return match force.as_str() {
                    "graphics" => Protocol::Graphics,
                    "off" => Protocol::None,
                    _ => infer_protocol(env),
                };
            }
            infer_protocol(env)
        }
    }
}

fn infer_protocol(env: impl Fn(&str) -> Option<String>) -> Protocol {
    let term = env("TERM").unwrap_or_default().to_ascii_lowercase();
    let program = env("TERM_PROGRAM").unwrap_or_default();
    if term.contains("kitty")
        || program == "WezTerm"
        || env("KITTY_WINDOW_ID").is_some()
        || env("WEZTERM_EXECUTABLE").is_some()
    {
        Protocol::Graphics
    } else {
        Protocol::None
    }
}

/// Latest matching mood for a slot. Missing variants stay Neutral at the call site.
pub fn mood_for(slot: Slot, events: &[GameEvent]) -> Mood {
    if !slot.uses_mood() {
        return Mood::Neutral;
    }
    let rules = mood_rules();
    for event in events.iter().rev() {
        for (rule_slot, needle, mood) in &rules {
            if *rule_slot == slot && event.text.contains(needle) {
                return *mood;
            }
        }
    }
    Mood::Neutral
}

fn mood_rules() -> Vec<(Slot, String, Mood)> {
    let mut rules = Vec::new();
    for raw in MOODS.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() != 3 {
            continue;
        }
        let slot = match parts[0] {
            "commander" => Slot::Commander,
            "admiral" => Slot::Admiral,
            "pirate" => Slot::Pirate,
            _ => continue,
        };
        let Some(mood) = Mood::from_key(parts[2]) else {
            continue;
        };
        rules.push((slot, parts[1].to_string(), mood));
    }
    rules
}

fn asset_name(slot: Slot, mood: Mood) -> String {
    if slot.uses_mood() {
        format!("{}-{}", slot.key(), mood.as_key())
    } else {
        slot.key().to_string()
    }
}

fn ascii_blob(name: &str) -> Option<&'static str> {
    Some(match name {
        "commander-neutral" => include_str!("../assets/portraits/commander-neutral.txt"),
        "commander-pleased" => include_str!("../assets/portraits/commander-pleased.txt"),
        "commander-alarmed" => include_str!("../assets/portraits/commander-alarmed.txt"),
        "admiral-neutral" => include_str!("../assets/portraits/admiral-neutral.txt"),
        "pirate-neutral" => include_str!("../assets/portraits/pirate-neutral.txt"),
        "pirate-pleased" => include_str!("../assets/portraits/pirate-pleased.txt"),
        "pirate-alarmed" => include_str!("../assets/portraits/pirate-alarmed.txt"),
        "planet-rocky" => include_str!("../assets/portraits/planet-rocky.txt"),
        "planet-ice" => include_str!("../assets/portraits/planet-ice.txt"),
        "planet-gas_giant" => include_str!("../assets/portraits/planet-gas_giant.txt"),
        "planet-rare" => include_str!("../assets/portraits/planet-rare.txt"),
        _ => return None,
    })
}

fn rgb_blob(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "commander-neutral" => include_bytes!("../assets/portraits/commander-neutral.rgb"),
        "commander-pleased" => include_bytes!("../assets/portraits/commander-pleased.rgb"),
        "commander-alarmed" => include_bytes!("../assets/portraits/commander-alarmed.rgb"),
        "admiral-neutral" => include_bytes!("../assets/portraits/admiral-neutral.rgb"),
        "pirate-neutral" => include_bytes!("../assets/portraits/pirate-neutral.rgb"),
        "pirate-pleased" => include_bytes!("../assets/portraits/pirate-pleased.rgb"),
        "pirate-alarmed" => include_bytes!("../assets/portraits/pirate-alarmed.rgb"),
        "planet-rocky" => include_bytes!("../assets/portraits/planet-rocky.rgb"),
        "planet-ice" => include_bytes!("../assets/portraits/planet-ice.rgb"),
        "planet-gas_giant" => include_bytes!("../assets/portraits/planet-gas_giant.rgb"),
        "planet-rare" => include_bytes!("../assets/portraits/planet-rare.rgb"),
        _ => return None,
    })
}

fn resolved_name(slot: Slot, mood: Mood) -> String {
    let wanted = asset_name(slot, mood);
    if ascii_blob(&wanted).is_some() {
        wanted
    } else {
        asset_name(slot, Mood::Neutral)
    }
}

pub fn ascii_lines(slot: Slot, mood: Mood) -> Vec<Line<'static>> {
    let name = resolved_name(slot, mood);
    let text = ascii_blob(&name).unwrap_or("????????\n????????\n????????\n????????\n");
    text.lines()
        .take(ASCII_ROWS)
        .map(|line| {
            let mut row = line.to_string();
            while row.chars().count() < ART_COLS as usize {
                row.push(' ');
            }
            Line::from(row)
        })
        .collect()
}

/// Half-block cells from the RGB grid. `None` when images are disabled or the blob is missing.
pub fn half_block_lines(slot: Slot, mood: Mood) -> Option<Vec<Line<'static>>> {
    let name = resolved_name(slot, mood);
    let rgb = rgb_blob(&name)?;
    if rgb.len() != (PIXEL_SIZE * PIXEL_SIZE * 3) as usize {
        return None;
    }
    let mut lines = Vec::with_capacity(ASCII_ROWS);
    for y in 0..ASCII_ROWS {
        let mut spans = Vec::with_capacity(PIXEL_SIZE as usize);
        for x in 0..PIXEL_SIZE as usize {
            let top = pixel(rgb, x, y * 2);
            let bot = pixel(rgb, x, y * 2 + 1);
            spans.push(Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(top.0, top.1, top.2))
                    .bg(Color::Rgb(bot.0, bot.1, bot.2)),
            ));
        }
        lines.push(Line::from(spans));
    }
    Some(lines)
}

fn pixel(rgb: &[u8], x: usize, y: usize) -> (u8, u8, u8) {
    let i = (y * PIXEL_SIZE as usize + x) * 3;
    (rgb[i], rgb[i + 1], rgb[i + 2])
}

pub fn portrait_lines(view: PortraitView, slot: Slot, mood: Mood) -> Vec<Line<'static>> {
    if view.enabled
        && let Some(lines) = half_block_lines(slot, mood)
    {
        return lines;
    }
    ascii_lines(slot, mood)
}

pub fn graphics_payload(slot: Slot, mood: Mood) -> Option<String> {
    let name = resolved_name(slot, mood);
    let rgb = rgb_blob(&name)?;
    if rgb.len() != (PIXEL_SIZE * PIXEL_SIZE * 3) as usize {
        return None;
    }
    let b64 = base64_encode(rgb);
    Some(format!(
        "\x1b_Ga=T,f=24,s={PIXEL_SIZE},v={PIXEL_SIZE},c={ART_COLS},r={ASCII_ROWS},q=2;{b64}\x1b\\"
    ))
}

/// Write an inline graphics image at `area` after ratatui has drawn the frame.
pub fn paint_inline_graphics(
    out: &mut impl Write,
    area: Rect,
    slot: Slot,
    mood: Mood,
) -> io::Result<()> {
    let Some(payload) = graphics_payload(slot, mood) else {
        return Ok(());
    };
    let row = area.y.saturating_add(1);
    let col = area.x.saturating_add(1);
    write!(out, "\x1b[{row};{col}H{payload}")?;
    out.flush()
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes.get(i + 1).copied().unwrap_or(0);
        let b2 = bytes.get(i + 2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < bytes.len() {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

pub fn slot_for_screen(
    screen_is_planet: bool,
    screen_is_shipyard: bool,
    kind: Option<PlanetType>,
    rare: bool,
    last_event: Option<&str>,
) -> Slot {
    if screen_is_planet {
        if rare {
            return Slot::RarePlanet;
        }
        return Slot::Planet(kind.unwrap_or(PlanetType::Rocky));
    }
    if screen_is_shipyard {
        return Slot::Admiral;
    }
    if last_event.is_some_and(|t| t.starts_with("raid:")) {
        Slot::Pirate
    } else {
        Slot::Commander
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::GameEvent;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn detect_protocol_honors_override_and_term() {
        let off = detect_protocol(ProtocolChoice::Off, env_map(&[("TERM", "xterm-kitty")]));
        assert_eq!(off, Protocol::None);
        let forced = detect_protocol(ProtocolChoice::Graphics, env_map(&[("TERM", "xterm")]));
        assert_eq!(forced, Protocol::Graphics);
        let kitty = detect_protocol(ProtocolChoice::Auto, env_map(&[("TERM", "xterm-kitty")]));
        assert_eq!(kitty, Protocol::Graphics);
        let wez = detect_protocol(
            ProtocolChoice::Auto,
            env_map(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", "WezTerm")]),
        );
        assert_eq!(wez, Protocol::Graphics);
        let plain = detect_protocol(ProtocolChoice::Auto, env_map(&[("TERM", "xterm-256color")]));
        assert_eq!(plain, Protocol::None);
    }

    #[test]
    fn mood_uses_latest_matching_event_and_falls_back() {
        let events = vec![
            GameEvent {
                at: 1,
                text: "activity: merged PR paid 1h of production".into(),
            },
            GameEvent {
                at: 2,
                text: "raid: pirates 3 vs 1. 6 rounds. lost none. pirates lost 3. plunder M0 C0 D0"
                    .into(),
            },
        ];
        assert_eq!(mood_for(Slot::Commander, &events), Mood::Alarmed);
        assert_eq!(mood_for(Slot::Pirate, &events), Mood::Alarmed);
        assert_eq!(mood_for(Slot::Admiral, &[]), Mood::Neutral);
        let pleased = [GameEvent {
            at: 3,
            text: "activity: merged PR paid 1h of production".into(),
        }];
        assert_eq!(mood_for(Slot::Commander, &pleased), Mood::Pleased);
        assert_eq!(mood_for(Slot::Pirate, &pleased), Mood::Pleased);
    }

    #[test]
    fn missing_mood_variant_uses_neutral_ascii() {
        let wanted = ascii_lines(Slot::Admiral, Mood::Pleased);
        let neutral = ascii_lines(Slot::Admiral, Mood::Neutral);
        assert_eq!(wanted, neutral);
        assert!(neutral.iter().any(|l| l.to_string().contains('*')));
    }

    #[test]
    fn graphics_payload_is_rgb_protocol_and_ascii_has_no_escapes() {
        let payload = graphics_payload(Slot::Commander, Mood::Neutral).unwrap();
        assert!(payload.starts_with("\x1b_G"));
        assert!(payload.contains("f=24"));
        assert!(payload.ends_with("\x1b\\"));
        for line in ascii_lines(Slot::Commander, Mood::Neutral) {
            assert!(
                !line.to_string().contains('\u{1b}'),
                "ascii leaked an escape: {line:?}"
            );
        }
    }

    #[test]
    fn disabled_images_do_not_need_rgb() {
        let lines = portrait_lines(
            PortraitView {
                enabled: false,
                protocol: Protocol::None,
            },
            Slot::Commander,
            Mood::Neutral,
        );
        let joined = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains(".==."), "{joined}");
        assert!(!joined.contains('▀'));
    }

    #[test]
    fn asset_budget_holds() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/portraits");
        let mut total = 0u64;
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let len = entry.metadata().unwrap().len();
            assert!(
                len <= MAX_ASSET_BYTES,
                "{} is {len} bytes",
                entry.file_name().to_string_lossy()
            );
            total += len;
        }
        assert!(total <= MAX_PACK_BYTES, "pack is {total} bytes");
        assert_eq!(
            rgb_blob("commander-neutral").unwrap().len(),
            (PIXEL_SIZE * PIXEL_SIZE * 3) as usize
        );
    }
}
