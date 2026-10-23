//! `GET /v1/terminal`: the TUI on a WebSocket for a browser terminal.
//!
//! Binary frames carry terminal bytes both ways: keys in, rendered frames
//! out. Text frames carry JSON control messages (`{"type":"resize",...}`).
//! The session draws the ordinary [`App`] into a fixed-size buffer, not a
//! host PTY or child process. One session holds the save at a time; see
//! [`Lease`].

use crate::app::{Action, App};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

/// How long a dropped session keeps the save before another socket may take
/// it over, so a slow-closing old socket and a new one never both write.
pub const DEFAULT_GRACE_SECS: u64 = 10;
/// An attached session with no input for this long is closed; 0 never closes.
pub const DEFAULT_IDLE_SECS: u64 = 1_800;
const MIN_COLS: u16 = 20;
const MIN_ROWS: u16 = 8;
const MAX_CELLS: u16 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub grace_secs: u64,
    pub idle_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            grace_secs: DEFAULT_GRACE_SECS,
            idle_secs: DEFAULT_IDLE_SECS,
        }
    }
}

/// Who holds the save for the terminal. Absent when no session ever ran or
/// the last one quit cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub id: u64,
    /// `None` while a socket is attached; the drop time once it went away.
    pub left_at: Option<u64>,
}

impl Lease {
    /// The save is held: a socket is attached, or one dropped less than
    /// `grace_secs` ago.
    pub fn holds(&self, now: u64, grace_secs: u64) -> bool {
        self.left_at
            .is_none_or(|left| now.saturating_sub(left) < grace_secs)
    }
}

pub const HELD: &str = "a terminal session holds the save";

/// Decode terminal input bytes (xterm sequences) into keys the TUI knows.
pub fn decode_keys(bytes: &[u8]) -> Vec<KeyCode> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        let (key, used) = match rest {
            [0x1b, b'[', b'A', ..] => (Some(KeyCode::Up), 3),
            [0x1b, b'[', b'B', ..] => (Some(KeyCode::Down), 3),
            [0x1b, b'[', b'C', ..] => (Some(KeyCode::Right), 3),
            [0x1b, b'[', b'D', ..] => (Some(KeyCode::Left), 3),
            [0x1b, b'[', b'H', ..] => (Some(KeyCode::Home), 3),
            [0x1b, b'[', b'F', ..] => (Some(KeyCode::End), 3),
            [0x1b, b'[', b'Z', ..] => (Some(KeyCode::BackTab), 3),
            [0x1b, b'[', b'5', b'~', ..] => (Some(KeyCode::PageUp), 4),
            [0x1b, b'[', b'6', b'~', ..] => (Some(KeyCode::PageDown), 4),
            [0x1b, b'[', b'3', b'~', ..] => (Some(KeyCode::Delete), 4),
            // SS3: arrows in application cursor mode, F1–F4 and the like.
            [0x1b, b'O', b'A', ..] => (Some(KeyCode::Up), 3),
            [0x1b, b'O', b'B', ..] => (Some(KeyCode::Down), 3),
            [0x1b, b'O', b'C', ..] => (Some(KeyCode::Right), 3),
            [0x1b, b'O', b'D', ..] => (Some(KeyCode::Left), 3),
            [0x1b, b'O', _, ..] => (None, 3),
            [0x1b, b'[', ..] => {
                // Unknown CSI: skip to its final byte.
                let end = rest[2..]
                    .iter()
                    .position(|b| (0x40..=0x7e).contains(b))
                    .map_or(rest.len(), |p| p + 3);
                (None, end)
            }
            [0x1b, ..] => (Some(KeyCode::Esc), 1),
            [b'\r' | b'\n', ..] => (Some(KeyCode::Enter), 1),
            [b'\t', ..] => (Some(KeyCode::Tab), 1),
            [0x7f | 0x08, ..] => (Some(KeyCode::Backspace), 1),
            [b, ..] if b.is_ascii_graphic() || *b == b' ' => {
                (Some(KeyCode::Char(char::from(*b))), 1)
            }
            [b, ..] if *b >= 0x80 => {
                // One UTF-8 character.
                let len = match b {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                }
                .min(rest.len());
                match std::str::from_utf8(&rest[..len])
                    .ok()
                    .and_then(|s| s.chars().next())
                {
                    Some(c) => (Some(KeyCode::Char(c)), len),
                    // Not a character: drop only this byte, keep what follows.
                    None => (None, 1),
                }
            }
            _ => (None, 1),
        };
        out.extend(key);
        i += used.max(1);
    }
    out
}

/// A `{"type":"resize","cols":N,"rows":N}` control message, clamped to a
/// usable and bounded size.
pub fn parse_resize(text: &[u8]) -> Option<(u16, u16)> {
    let value: serde_json::Value = serde_json::from_slice(text).ok()?;
    if value.get("type")?.as_str()? != "resize" {
        return None;
    }
    let dim = |key: &str, min: u16| {
        value
            .get(key)?
            .as_u64()
            .map(|n| (n.min(u64::from(MAX_CELLS)) as u16).max(min))
    };
    Some((dim("cols", MIN_COLS)?, dim("rows", MIN_ROWS)?))
}

/// Bytes the backend wrote since the last take. The terminal and the screen
/// share it, so frames can leave without reaching into the backend.
#[derive(Clone, Default)]
struct Outbox(Rc<RefCell<Vec<u8>>>);

impl Write for Outbox {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The TUI drawn into a byte buffer at a fixed size.
pub struct Screen {
    terminal: Terminal<CrosstermBackend<Outbox>>,
    outbox: Outbox,
}

impl Screen {
    pub fn new(cols: u16, rows: u16) -> std::io::Result<Self> {
        let outbox = Outbox::default();
        let terminal = Terminal::with_options(
            CrosstermBackend::new(outbox.clone()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, cols, rows)),
            },
        )?;
        let mut screen = Self { terminal, outbox };
        screen.clear()?;
        Ok(screen)
    }

    /// Clear the client screen and forget the last frame, so the next draw
    /// repaints every cell.
    pub fn clear(&mut self) -> std::io::Result<()> {
        self.terminal.clear()?;
        self.outbox.write_all(b"\x1b[2J\x1b[H")
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.terminal.resize(Rect::new(0, 0, cols, rows))?;
        self.clear()
    }

    /// Draw `app` and return the bytes the client has not seen yet.
    pub fn draw(&mut self, app: &App) -> std::io::Result<Vec<u8>> {
        self.terminal.draw(|frame| app.draw(frame))?;
        Ok(std::mem::take(&mut *self.outbox.0.borrow_mut()))
    }
}

/// One attached session's UI. The game state is swapped in from the server
/// for each step, so the server's copy is the only one.
pub struct Session {
    pub app: App,
    pub screen: Screen,
}

impl Session {
    pub fn new(app: App) -> std::io::Result<Self> {
        Ok(Self {
            app,
            screen: Screen::new(80, 24)?,
        })
    }

    /// Feed input bytes; true when the player quit.
    pub fn input(&mut self, bytes: &[u8], now: u64) -> bool {
        decode_keys(bytes)
            .into_iter()
            .any(|key| matches!(self.app.handle_key(key, now), Action::Quit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_keys_arrows_and_control_bytes() {
        let keys = decode_keys(b"q\r\x1b[A\x1b[6~\x1b\t\x7f\x1b[1;5C2");
        assert_eq!(
            keys,
            vec![
                KeyCode::Char('q'),
                KeyCode::Enter,
                KeyCode::Up,
                KeyCode::PageDown,
                KeyCode::Esc,
                KeyCode::Tab,
                KeyCode::Backspace,
                KeyCode::Char('2'),
            ]
        );
        assert_eq!(decode_keys("›".as_bytes()), vec![KeyCode::Char('›')]);
        assert_eq!(
            decode_keys(b"\x1bOP\x1bOA"),
            vec![KeyCode::Up],
            "F1 is skipped"
        );
        assert_eq!(
            decode_keys(b"\x80ab"),
            vec![KeyCode::Char('a'), KeyCode::Char('b')]
        );
    }

    #[test]
    fn resize_is_clamped_and_other_messages_are_ignored() {
        assert_eq!(
            parse_resize(br#"{"type":"resize","cols":120,"rows":40}"#),
            Some((120, 40))
        );
        assert_eq!(
            parse_resize(br#"{"type":"resize","cols":1,"rows":99999}"#),
            Some((MIN_COLS, MAX_CELLS))
        );
        assert_eq!(parse_resize(br#"{"type":"ping"}"#), None);
        assert_eq!(parse_resize(b"nope"), None);
    }

    #[test]
    fn a_lease_holds_while_attached_and_through_the_grace() {
        let attached = Lease {
            id: 1,
            left_at: None,
        };
        assert!(attached.holds(1_000, 10));
        let dropped = Lease {
            id: 1,
            left_at: Some(1_000),
        };
        assert!(dropped.holds(1_009, 10));
        assert!(!dropped.holds(1_010, 10));
    }
}
