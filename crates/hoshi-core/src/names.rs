//! Player-chosen world names and the one label every surface shows.
//!
//! A name is player text that reaches other players' terminals on a shard,
//! so every name passes [`validate_world_name`] before it is stored or
//! rendered: at rename, at save load, and when a host hands one over.

use std::fmt;

use unicode_width::UnicodeWidthStr;

use crate::{Coord, GameState, HistoryDraft, HistoryKind};

/// Longest name, in terminal columns. Wide characters count two.
pub const MAX_NAME_COLUMNS: usize = 24;

/// Most characters a name may hold. Combining marks take no column, so the
/// column limit alone would not bound what is stored and sent to tenants.
pub const MAX_NAME_CHARS: usize = 2 * MAX_NAME_COLUMNS;

/// What home is called until the player renames it.
pub const HOME_NAME: &str = "Home";

/// Why a name was refused. Display is plain words for the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong {
        columns: usize,
    },
    Control,
    Bidi,
    /// An invisible character follows `after` (the visible text before it).
    ZeroWidth {
        after: String,
    },
    NotOwned {
        coord: Coord,
    },
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::Empty => write!(f, "A name needs at least one character."),
            NameError::TooLong { columns } => write!(
                f,
                "Too long: {columns} columns, {MAX_NAME_COLUMNS} at most."
            ),
            NameError::Control => write!(f, "Names can't contain control characters."),
            NameError::Bidi => write!(f, "Names can't contain text direction marks."),
            NameError::ZeroWidth { after } if after.is_empty() => {
                write!(f, "The name starts with an invisible character. Delete it.")
            }
            NameError::ZeroWidth { after } => {
                write!(f, "An invisible character follows \"{after}\". Delete it.")
            }
            NameError::NotOwned { coord } => write!(f, "You don't hold {coord}."),
        }
    }
}

impl std::error::Error for NameError {}

fn is_bidi(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{180E}' | '\u{00AD}'
    )
}

/// A character no name may hold: control, bidi or zero-width. Renderers
/// of typed-but-unsaved text draw these as a marker, never raw.
pub fn is_unsafe_name_char(c: char) -> bool {
    c.is_control() || is_bidi(c) || is_zero_width(c)
}

/// Display columns of `text` in a terminal.
pub fn name_columns(text: &str) -> usize {
    text.width()
}

/// Check a stored name: non-empty, already trimmed, at most
/// [`MAX_NAME_COLUMNS`] columns, and free of control, bidi and zero-width
/// characters. ANSI escapes and newlines are control characters.
pub fn validate_world_name(name: &str) -> Result<(), NameError> {
    if name.trim().is_empty() {
        return Err(NameError::Empty);
    }
    for (at, c) in name.char_indices() {
        if c.is_control() {
            return Err(NameError::Control);
        }
        if is_bidi(c) {
            return Err(NameError::Bidi);
        }
        if is_zero_width(c) {
            return Err(NameError::ZeroWidth {
                after: name[..at].to_string(),
            });
        }
    }
    if name.trim() != name {
        return Err(NameError::Empty);
    }
    let columns = name_columns(name);
    if columns == 0 {
        return Err(NameError::Empty);
    }
    if columns > MAX_NAME_COLUMNS || name.chars().count() > MAX_NAME_CHARS {
        return Err(NameError::TooLong { columns });
    }
    Ok(())
}

/// Trim what the player typed. `Ok(None)` clears the name.
pub fn clean_world_name(raw: &str) -> Result<Option<String>, NameError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    validate_world_name(trimmed)?;
    Ok(Some(trimmed.to_string()))
}

/// `Name (1:3:5)`, or the bare coord with no name.
pub fn label(name: Option<&str>, coord: Coord) -> String {
    match name {
        Some(name) => format!("{name} ({coord})"),
        None => coord.to_string(),
    }
}

/// [`label`] cut to `width` columns. The coord is never cut; the name loses
/// its tail to "…" first. A coord that alone does not fit is returned whole.
pub fn fit_label(name: Option<&str>, coord: Coord, width: usize) -> String {
    let full = label(name, coord);
    let Some(name) = name else {
        return full;
    };
    if name_columns(&full) <= width {
        return full;
    }
    let tail = format!("… ({coord})");
    let room = width.saturating_sub(name_columns(&tail));
    if room == 0 {
        return coord.to_string();
    }
    let mut kept = String::new();
    let mut used = 0;
    for c in name.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > room {
            break;
        }
        kept.push(c);
        used += w;
    }
    format!("{}{tail}", kept.trim_end())
}

impl GameState {
    /// Home or one of the player's colonies.
    pub fn owns_world(&self, coord: Coord) -> bool {
        coord == self.home || self.colonies.iter().any(|c| c.coord == coord)
    }

    /// The name shown for an owned world: the chosen one, else `Home` for
    /// home. `None` for an unnamed colony or a world the player lost.
    pub fn world_name(&self, coord: Coord) -> Option<&str> {
        if !self.owns_world(coord) {
            return None;
        }
        match self.world_names.get(&coord) {
            Some(name) => Some(name),
            None if coord == self.home => Some(HOME_NAME),
            None => None,
        }
    }

    /// The one label every surface shows for `coord`.
    pub fn world_label(&self, coord: Coord) -> String {
        label(self.world_name(coord), coord)
    }

    /// The label `raw` would give `coord` once saved, or why it can't be.
    pub fn preview_world_label(&self, coord: Coord, raw: &str) -> Result<String, NameError> {
        let name = clean_world_name(raw)?;
        let name = name
            .as_deref()
            .or((coord == self.home).then_some(HOME_NAME));
        Ok(label(name, coord))
    }

    /// Names of worlds still held, for saving. A lost colony's name drops.
    pub fn held_world_names(&self) -> impl Iterator<Item = (Coord, &str)> {
        self.world_names
            .iter()
            .filter(|(coord, _)| self.owns_world(**coord))
            .map(|(coord, name)| (*coord, name.as_str()))
    }

    /// Name an owned world. An empty `raw` clears it (home reads `Home`).
    pub fn rename_world(
        &mut self,
        coord: Coord,
        raw: &str,
        now_unix: u64,
    ) -> Result<(), NameError> {
        if !self.owns_world(coord) {
            return Err(NameError::NotOwned { coord });
        }
        let name = clean_world_name(raw)?;
        let at = now_unix.max(self.last_tick_unix);
        let outcome = match &name {
            Some(name) => name.clone(),
            None => "cleared".to_string(),
        };
        match name {
            Some(name) => self.world_names.insert(coord, name),
            None => self.world_names.remove(&coord),
        };
        self.record_history(HistoryDraft {
            at,
            kind: HistoryKind::Other,
            actor: "player".into(),
            action: "rename_world".into(),
            objects: vec![coord.to_string()],
            outcome,
            ..HistoryDraft::default()
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;

    fn colony_at(state: &mut GameState, coord: Coord) {
        state.colonies.push(crate::Colony {
            coord,
            kind: crate::PlanetType::Rocky,
            size: 8,
            resources: crate::Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            ship_queue: Vec::new(),
            rare: None,
            goods: std::collections::BTreeMap::new(),
        });
    }

    fn quiet() -> GameState {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        GameState::with_config(1_000, cfg)
    }

    const COLONY: Coord = Coord {
        galaxy: 1,
        system: 3,
        position: 5,
    };

    #[test]
    fn home_reads_home_until_renamed_and_clearing_returns_it() {
        let mut state = quiet();
        let home = state.home;
        assert_eq!(state.world_label(home), format!("Home ({home})"));
        state.rename_world(home, "  Forge ", 1_000).unwrap();
        assert_eq!(state.world_label(home), format!("Forge ({home})"));
        state.rename_world(home, "   ", 1_000).unwrap();
        assert_eq!(state.world_label(home), format!("Home ({home})"));
        let renames = state
            .history
            .search(&crate::HistoryQuery::default())
            .into_iter()
            .filter(|e| e.action == "rename_world")
            .count();
        assert_eq!(renames, 2);
    }

    #[test]
    fn an_unnamed_colony_shows_its_coord_and_a_lost_one_drops_its_name() {
        let mut state = quiet();
        colony_at(&mut state, COLONY);
        assert_eq!(state.world_label(COLONY), "1:3:5");
        state.rename_world(COLONY, "Ridge", 1_000).unwrap();
        assert_eq!(state.world_label(COLONY), "Ridge (1:3:5)");
        state.colonies.retain(|c| c.coord != COLONY);
        assert_eq!(state.world_label(COLONY), "1:3:5");
        assert_eq!(state.held_world_names().count(), 0);
    }

    #[test]
    fn renaming_a_world_you_do_not_hold_fails_and_changes_nothing() {
        let mut state = quiet();
        let before = state.clone();
        assert_eq!(
            state.rename_world(COLONY, "Mine", 1_000),
            Err(NameError::NotOwned { coord: COLONY })
        );
        assert_eq!(state, before);
    }

    #[test]
    fn validation_refuses_unsafe_or_long_names() {
        let refused = [
            ("", NameError::Empty),
            (
                "Northern Reach Forge Two!",
                NameError::TooLong { columns: 25 },
            ),
            (
                "星星星星星星星星星星星星星",
                NameError::TooLong { columns: 26 },
            ),
            ("Forge\n", NameError::Control),
            ("Forge\u{1b}[31m", NameError::Control),
            ("Forge\u{202E}", NameError::Bidi),
            ("Forge\u{2067}x", NameError::Bidi),
            (
                "Forge\u{200B}",
                NameError::ZeroWidth {
                    after: "Forge".into(),
                },
            ),
            ("A\u{FEFF}", NameError::ZeroWidth { after: "A".into() }),
            ("\u{301}\u{301}", NameError::Empty),
            (
                &format!("A{}", "\u{301}".repeat(MAX_NAME_CHARS)),
                NameError::TooLong { columns: 1 },
            ),
        ];
        for (name, err) in refused {
            assert_eq!(validate_world_name(name), Err(err), "{name:?}");
        }
        for ok in [
            "Forge",
            "Ålesund-7, the Old Gate",
            "星の港",
            "Northern Reach Forge Two",
            "Ridge #2 (north)",
        ] {
            assert_eq!(validate_world_name(ok), Ok(()), "{ok}");
        }
    }

    #[test]
    fn a_bad_rename_keeps_the_old_name() {
        let mut state = quiet();
        let home = state.home;
        state.rename_world(home, "Forge", 1_000).unwrap();
        let err = state.rename_world(home, "Bad\u{7}", 1_000).unwrap_err();
        assert_eq!(err.to_string(), "Names can't contain control characters.");
        assert_eq!(state.world_name(home), Some("Forge"));
    }

    #[test]
    fn a_fitted_label_cuts_the_name_and_keeps_the_coord() {
        let coord = Coord {
            galaxy: 1,
            system: 1,
            position: 7,
        };
        let name = Some("Northern Reach Forge Two");
        assert_eq!(
            fit_label(name, coord, 40),
            "Northern Reach Forge Two (1:1:7)"
        );
        let cut = fit_label(name, coord, 25);
        assert_eq!(cut, "Northern Reach F… (1:1:7)");
        assert!(name_columns(&cut) <= 25);
        assert_eq!(fit_label(name, coord, 8), "1:1:7");
        assert_eq!(fit_label(None, coord, 3), "1:1:7");
    }
}
