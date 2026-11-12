//! Contextual onboarding hints. Triggers are game state, never a timer alone.

use crate::state::GameState;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fmt;

const DEFAULT_HINTS: &str = include_str!("../data/hints.toml");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TutorialError {
    Parse(String),
    DuplicateId(String),
    EmptyId,
    EmptyTitle(String),
    UnknownTrigger { id: String, trigger: String },
    UnknownScreen { id: String, screen: String },
    EmptyCatalog,
}

impl fmt::Display for TutorialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TutorialError::Parse(msg) => write!(f, "tutorial: {msg}"),
            TutorialError::DuplicateId(id) => write!(f, "tutorial: duplicate id {id}"),
            TutorialError::EmptyId => write!(f, "tutorial: empty id"),
            TutorialError::EmptyTitle(id) => write!(f, "tutorial: {id} has no title"),
            TutorialError::UnknownTrigger { id, trigger } => {
                write!(f, "tutorial: {id} unknown trigger {trigger}")
            }
            TutorialError::UnknownScreen { id, screen } => {
                write!(f, "tutorial: {id} unknown screen {screen}")
            }
            TutorialError::EmptyCatalog => write!(f, "tutorial: catalog is empty"),
        }
    }
}

impl std::error::Error for TutorialError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintTrigger {
    Opening,
    MetalMine2,
    EnergyShort,
    LabReady,
    StorageHigh,
    ResearchDone,
    RaidSoon,
    FleetOut,
    ShipyardReady,
    FrontierOpen,
}

impl HintTrigger {
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "opening" => Self::Opening,
            "metal_mine_2" => Self::MetalMine2,
            "energy_short" => Self::EnergyShort,
            "lab_ready" => Self::LabReady,
            "storage_high" => Self::StorageHigh,
            "research_done" => Self::ResearchDone,
            "raid_soon" => Self::RaidSoon,
            "fleet_out" => Self::FleetOut,
            "shipyard_ready" => Self::ShipyardReady,
            "frontier_open" => Self::FrontierOpen,
            _ => return None,
        })
    }

    pub fn as_key(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::MetalMine2 => "metal_mine_2",
            Self::EnergyShort => "energy_short",
            Self::LabReady => "lab_ready",
            Self::StorageHigh => "storage_high",
            Self::ResearchDone => "research_done",
            Self::RaidSoon => "raid_soon",
            Self::FleetOut => "fleet_out",
            Self::ShipyardReady => "shipyard_ready",
            Self::FrontierOpen => "frontier_open",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintScreen {
    Overview,
    Planet,
    Shipyard,
    Map,
}

impl HintScreen {
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "overview" => Self::Overview,
            "planet" => Self::Planet,
            "shipyard" => Self::Shipyard,
            "map" => Self::Map,
            _ => return None,
        })
    }

    pub fn as_key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Planet => "planet",
            Self::Shipyard => "shipyard",
            Self::Map => "map",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintSpec {
    pub id: String,
    pub trigger: HintTrigger,
    pub screen: HintScreen,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TutorialBoard {
    pub enabled: bool,
    pub skipped: bool,
    pub seen: BTreeSet<String>,
}

impl TutorialBoard {
    pub fn start() -> Self {
        Self {
            enabled: true,
            skipped: false,
            seen: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TutorialCatalog {
    hints: Vec<HintSpec>,
}

impl TutorialCatalog {
    pub fn bundled() -> Self {
        Self::parse(DEFAULT_HINTS).expect("bundled hints.toml must be valid")
    }

    pub fn empty() -> Self {
        Self { hints: Vec::new() }
    }

    pub fn parse(text: &str) -> Result<Self, TutorialError> {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            hints: Vec<FileHint>,
        }
        #[derive(Deserialize)]
        struct FileHint {
            id: String,
            trigger: String,
            screen: String,
            title: String,
            #[serde(default)]
            body: String,
        }
        let file: File = toml::from_str(text).map_err(|e| TutorialError::Parse(e.to_string()))?;
        let mut seen = BTreeSet::new();
        let mut hints = Vec::new();
        for row in file.hints {
            if row.id.is_empty() {
                return Err(TutorialError::EmptyId);
            }
            if !seen.insert(row.id.clone()) {
                return Err(TutorialError::DuplicateId(row.id));
            }
            if row.title.is_empty() {
                return Err(TutorialError::EmptyTitle(row.id));
            }
            let trigger = HintTrigger::from_key(&row.trigger).ok_or_else(|| {
                TutorialError::UnknownTrigger {
                    id: row.id.clone(),
                    trigger: row.trigger,
                }
            })?;
            let screen =
                HintScreen::from_key(&row.screen).ok_or_else(|| TutorialError::UnknownScreen {
                    id: row.id.clone(),
                    screen: row.screen,
                })?;
            hints.push(HintSpec {
                id: row.id,
                trigger,
                screen,
                title: row.title,
                body: row.body,
            });
        }
        if hints.is_empty() {
            return Err(TutorialError::EmptyCatalog);
        }
        Ok(Self { hints })
    }

    pub fn get(&self, id: &str) -> Option<&HintSpec> {
        self.hints.iter().find(|h| h.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &HintSpec> {
        self.hints.iter()
    }

    pub fn len(&self) -> usize {
        self.hints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hints.is_empty()
    }
}

impl Default for TutorialCatalog {
    fn default() -> Self {
        Self::bundled()
    }
}

impl GameState {
    pub fn pending_hint(&self) -> Option<HintSpec> {
        if !self.tutorial.enabled || self.tutorial.skipped {
            return None;
        }
        TutorialCatalog::bundled()
            .iter()
            .find(|spec| !self.tutorial.seen.contains(&spec.id) && self.hint_ready(spec.trigger))
            .cloned()
    }

    fn hint_ready(&self, trigger: HintTrigger) -> bool {
        match trigger {
            HintTrigger::Opening => true,
            HintTrigger::MetalMine2 => self.buildings.metal_mine >= 2,
            HintTrigger::EnergyShort => self.energy_balance() < 0.0,
            HintTrigger::LabReady => self.buildings.research_lab >= 1,
            HintTrigger::StorageHigh => {
                let cap = self.storage_cap(crate::ResourceKind::Iron);
                cap > 0.0 && self.resources.iron >= cap * 0.9
            }
            HintTrigger::ResearchDone => self.tech_levels.values().any(|&lv| lv > 0),
            HintTrigger::RaidSoon => self.upcoming_raid_at().is_some(),
            HintTrigger::FleetOut => !self.missions.is_empty(),
            HintTrigger::ShipyardReady => self.buildings.shipyard >= 1,
            HintTrigger::FrontierOpen => self.frontier.revealed > 0,
        }
    }

    pub fn dismiss_hint(&mut self, id: &str) {
        if TutorialCatalog::bundled().get(id).is_some() {
            self.tutorial.seen.insert(id.to_string());
        }
    }

    pub fn skip_tutorial(&mut self) {
        self.tutorial.skipped = true;
    }

    pub fn reset_hints(&mut self) {
        self.tutorial.skipped = false;
        self.tutorial.seen.clear();
        self.tutorial.enabled = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::state::GameState;

    #[test]
    fn bundled_catalog_has_at_least_eight_hints() {
        let catalog = TutorialCatalog::bundled();
        assert!(catalog.len() >= 8, "{}", catalog.len());
    }

    #[test]
    fn new_live_game_shows_opening_once() {
        let mut state = GameState::new_live_galaxy(1_000, BalanceConfig::default(), 1);
        let first = state.pending_hint().expect("opening");
        assert_eq!(first.id, "opening");
        let id = first.id.clone();
        state.dismiss_hint(&id);
        let next = state.pending_hint();
        assert!(next.as_ref().map(|h| h.id.as_str()) != Some("opening"));
    }

    #[test]
    fn skip_hides_every_hint_and_reset_restores_opening() {
        let mut state = GameState::new_live_galaxy(1_000, BalanceConfig::default(), 1);
        state.skip_tutorial();
        assert!(state.pending_hint().is_none());
        state.reset_hints();
        assert_eq!(state.pending_hint().unwrap().id, "opening");
    }

    #[test]
    fn mine_upgrade_unlocks_the_mine_hint_from_state_not_a_timer() {
        let cfg = BalanceConfig {
            raid: crate::RaidConfig::inactive(),
            ..BalanceConfig::default()
        };
        let mut state = GameState::new_live_galaxy(1_000, cfg, 1);
        state.dismiss_hint("opening");
        state.buildings.metal_mine = 1;
        assert!(
            state
                .pending_hint()
                .is_none_or(|h| h.trigger != HintTrigger::MetalMine2)
        );
        state.buildings.metal_mine = 2;
        let hint = state.pending_hint().expect("mine hint");
        assert_eq!(hint.trigger, HintTrigger::MetalMine2);
    }

    fn quiet() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg
    }

    fn dismiss_other_hints(state: &mut GameState) {
        for spec in TutorialCatalog::bundled().iter() {
            if spec.trigger != HintTrigger::FrontierOpen {
                state.dismiss_hint(&spec.id);
            }
        }
    }

    #[test]
    fn frontier_hint_waits_for_a_revealed_sector() {
        let mut research = GameState::new_live_galaxy(1_000, quiet(), 1);
        dismiss_other_hints(&mut research);
        research.evaluate_frontier(1_000);
        assert_eq!(research.frontier.revealed, 0);
        assert!(research.pending_hint().is_none());

        let drive = research.config.frontier.drive_id.clone();
        let astro = research.config.frontier.astro_id.clone();
        research.tech_levels.insert(drive, 1);
        research.tech_levels.insert(astro, 1);
        research.evaluate_frontier(1_010);
        assert!(research.frontier.revealed > 0);
        let hint = research.pending_hint().expect("frontier hint");
        assert_eq!(hint.trigger, HintTrigger::FrontierOpen);
        assert_eq!(hint.screen, HintScreen::Map);
        assert!(!hint.title.contains('_'), "{}", hint.title);
        assert!(!hint.body.contains('_'), "{}", hint.body);

        let mut wonder = GameState::new_live_galaxy(2_000, quiet(), 2);
        dismiss_other_hints(&mut wonder);
        wonder.completed_wonders.push("dyson_ring".into());
        wonder.evaluate_frontier(2_010);
        assert!(wonder.frontier.revealed > 0);
        assert_eq!(
            wonder.pending_hint().expect("wonder frontier").trigger,
            HintTrigger::FrontierOpen
        );
    }

    #[test]
    fn dismissed_frontier_hint_does_not_return() {
        let mut state = GameState::new_live_galaxy(1_000, quiet(), 1);
        dismiss_other_hints(&mut state);
        let drive = state.config.frontier.drive_id.clone();
        let astro = state.config.frontier.astro_id.clone();
        state.tech_levels.insert(drive, 1);
        state.tech_levels.insert(astro, 1);
        state.evaluate_frontier(1_010);
        let hint = state.pending_hint().expect("frontier hint");
        let id = hint.id.clone();
        state.dismiss_hint(&id);
        assert!(
            state
                .pending_hint()
                .is_none_or(|h| h.trigger != HintTrigger::FrontierOpen)
        );

        state.completed_wonders.push("dyson_ring".into());
        state.evaluate_frontier(1_020);
        assert!(state.frontier.revealed > 1);
        assert!(
            state
                .pending_hint()
                .is_none_or(|h| h.trigger != HintTrigger::FrontierOpen)
        );

        let mut reloaded = GameState::new_live_galaxy(3_000, quiet(), 3);
        dismiss_other_hints(&mut reloaded);
        reloaded.completed_wonders.push("dyson_ring".into());
        reloaded.evaluate_frontier(3_010);
        reloaded.tutorial = state.tutorial.clone();
        assert!(
            reloaded
                .pending_hint()
                .is_none_or(|h| h.trigger != HintTrigger::FrontierOpen)
        );
    }

    #[test]
    fn parse_rejects_an_unknown_trigger() {
        let err = TutorialCatalog::parse(
            r#"
[[hints]]
id = "a"
trigger = "not_a_trigger"
screen = "map"
title = "A"
"#,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                TutorialError::UnknownTrigger { ref id, ref trigger }
                    if id == "a" && trigger == "not_a_trigger"
            ),
            "{err}"
        );
    }

    #[test]
    fn parse_rejects_a_duplicate_id() {
        let err = TutorialCatalog::parse(
            r#"
[[hints]]
id = "a"
trigger = "opening"
screen = "overview"
title = "A"
[[hints]]
id = "a"
trigger = "opening"
screen = "overview"
title = "B"
"#,
        )
        .unwrap_err();
        assert!(matches!(err, TutorialError::DuplicateId(id) if id == "a"));
    }
}
