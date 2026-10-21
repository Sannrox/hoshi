//! Achievements from `data/achievements.toml`. Unlocks persist across prestige.

use crate::buildings::BuildingKind;
use crate::diplomacy::DiplomacyConfig;
use crate::research::TechTree;
use crate::ships::ShipKind;
use crate::state::GameState;
use crate::wonders::WonderSpec;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fmt;

const DEFAULT_ACHIEVEMENTS: &str = include_str!("../data/achievements.toml");

#[derive(Debug, Clone, PartialEq)]
pub enum AchievementError {
    Parse(String),
    DuplicateId(String),
    EmptyId,
    EmptyTitle(String),
    UnknownBuilding(String),
    UnknownTech(String),
    UnknownShip(String),
    UnknownWonder(String),
    UnknownFaction(String),
    UnknownExclusive(String),
    ZeroTarget(String),
}

impl fmt::Display for AchievementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AchievementError::Parse(msg) => write!(f, "achievements: {msg}"),
            AchievementError::DuplicateId(id) => write!(f, "achievements: duplicate id {id}"),
            AchievementError::EmptyId => write!(f, "achievements: empty id"),
            AchievementError::EmptyTitle(id) => write!(f, "achievements: {id} has no title"),
            AchievementError::UnknownBuilding(id) => {
                write!(f, "achievements: unknown building {id}")
            }
            AchievementError::UnknownTech(id) => write!(f, "achievements: unknown tech {id}"),
            AchievementError::UnknownShip(id) => write!(f, "achievements: unknown ship {id}"),
            AchievementError::UnknownWonder(id) => write!(f, "achievements: unknown wonder {id}"),
            AchievementError::UnknownFaction(id) => {
                write!(f, "achievements: unknown faction {id}")
            }
            AchievementError::UnknownExclusive(id) => {
                write!(f, "achievements: {id} is not an exclusive pick")
            }
            AchievementError::ZeroTarget(id) => {
                write!(f, "achievements: {id} target must be greater than zero")
            }
        }
    }
}

impl std::error::Error for AchievementError {}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AchievementCondition {
    BuildingLevel {
        id: String,
        level: u32,
    },
    TechLevel {
        id: String,
        level: u32,
    },
    ColonyCount {
        count: u32,
    },
    FleetShips {
        count: u32,
    },
    ShipCount {
        id: String,
        count: u32,
    },
    Wonder {
        #[serde(default)]
        id: Option<String>,
    },
    PrestigeRuns {
        count: u32,
    },
    RaidSurvived {
        count: u32,
    },
    Standing {
        id: String,
        min: f64,
    },
    ExclusivePick {
        id: String,
    },
    OutpostConquered {
        count: u32,
    },
    SeasonComplete {
        #[serde(default)]
        id: Option<String>,
    },
}

impl AchievementCondition {
    fn validate(
        &self,
        techs: &TechTree,
        factions: &DiplomacyConfig,
    ) -> Result<(), AchievementError> {
        match self {
            AchievementCondition::BuildingLevel { id, level } => {
                if *level == 0 {
                    return Err(AchievementError::ZeroTarget(id.clone()));
                }
                BuildingKind::from_key(id)
                    .ok_or_else(|| AchievementError::UnknownBuilding(id.clone()))?;
            }
            AchievementCondition::TechLevel { id, level } => {
                if *level == 0 {
                    return Err(AchievementError::ZeroTarget(id.clone()));
                }
                if techs.get(id).is_none() {
                    return Err(AchievementError::UnknownTech(id.clone()));
                }
            }
            AchievementCondition::ColonyCount { count }
            | AchievementCondition::FleetShips { count }
            | AchievementCondition::PrestigeRuns { count }
            | AchievementCondition::RaidSurvived { count }
            | AchievementCondition::OutpostConquered { count } => {
                if *count == 0 {
                    return Err(AchievementError::ZeroTarget("count".into()));
                }
            }
            AchievementCondition::ShipCount { id, count } => {
                if *count == 0 {
                    return Err(AchievementError::ZeroTarget(id.clone()));
                }
                ShipKind::from_key(id).ok_or_else(|| AchievementError::UnknownShip(id.clone()))?;
            }
            AchievementCondition::Wonder { .. } => {}
            AchievementCondition::Standing { id, min } => {
                if !min.is_finite() || *min <= 0.0 {
                    return Err(AchievementError::ZeroTarget(id.clone()));
                }
                if factions.get(id).is_none() {
                    return Err(AchievementError::UnknownFaction(id.clone()));
                }
            }
            AchievementCondition::ExclusivePick { id } => {
                let spec = techs
                    .get(id)
                    .ok_or_else(|| AchievementError::UnknownTech(id.clone()))?;
                if spec.exclusive_group.is_none() {
                    return Err(AchievementError::UnknownExclusive(id.clone()));
                }
            }
            AchievementCondition::SeasonComplete { .. } => {}
        }
        Ok(())
    }

    fn validate_wonder(&self, wonders: &[WonderSpec]) -> Result<(), AchievementError> {
        let AchievementCondition::Wonder { id: Some(id) } = self else {
            return Ok(());
        };
        if wonders.iter().any(|w| w.id == *id) {
            Ok(())
        } else {
            Err(AchievementError::UnknownWonder(id.clone()))
        }
    }

    /// Current progress toward this condition.
    pub fn current(&self, state: &GameState) -> f64 {
        match self {
            AchievementCondition::BuildingLevel { id, .. } => {
                let Some(kind) = BuildingKind::from_key(id) else {
                    return 0.0;
                };
                f64::from(building_level(state, kind))
            }
            AchievementCondition::TechLevel { id, .. } => f64::from(state.tech_level(id)),
            AchievementCondition::ColonyCount { .. } => state.colonies.len() as f64,
            AchievementCondition::FleetShips { .. } => f64::from(fleet_ships(state)),
            AchievementCondition::ShipCount { id, .. } => {
                let Some(kind) = ShipKind::from_key(id) else {
                    return 0.0;
                };
                f64::from(ship_count(state, kind))
            }
            AchievementCondition::Wonder { id } => {
                let done = match id {
                    Some(want) => state.completed_wonders.iter().any(|w| w == want),
                    None => !state.completed_wonders.is_empty(),
                };
                if done { 1.0 } else { 0.0 }
            }
            AchievementCondition::PrestigeRuns { .. } => state.prestige.runs.len() as f64,
            AchievementCondition::RaidSurvived { .. } => f64::from(state.raids_survived),
            AchievementCondition::Standing { id, .. } => state.diplomacy.standing(id),
            AchievementCondition::ExclusivePick { id } => {
                if state.exclusive_picks.values().any(|picked| picked == id) {
                    1.0
                } else {
                    0.0
                }
            }
            AchievementCondition::OutpostConquered { .. } => f64::from(state.outposts_conquered),
            AchievementCondition::SeasonComplete { id } => {
                let Some(run) = state.season.as_ref() else {
                    return 0.0;
                };
                if id.as_ref().is_some_and(|want| want != &run.ruleset_id) {
                    return 0.0;
                }
                if state.victory.is_some() { 1.0 } else { 0.0 }
            }
        }
    }

    pub fn target(&self) -> f64 {
        match self {
            AchievementCondition::BuildingLevel { level, .. } => f64::from(*level),
            AchievementCondition::TechLevel { level, .. } => f64::from(*level),
            AchievementCondition::ColonyCount { count }
            | AchievementCondition::FleetShips { count }
            | AchievementCondition::ShipCount { count, .. }
            | AchievementCondition::PrestigeRuns { count }
            | AchievementCondition::RaidSurvived { count }
            | AchievementCondition::OutpostConquered { count } => f64::from(*count),
            AchievementCondition::Wonder { .. }
            | AchievementCondition::ExclusivePick { .. }
            | AchievementCondition::SeasonComplete { .. } => 1.0,
            AchievementCondition::Standing { min, .. } => *min,
        }
    }

    pub fn met(&self, state: &GameState) -> bool {
        self.current(state) + 1e-9 >= self.target()
    }

    /// Progress that can be read from career totals without loading a live state.
    pub fn measured(&self, meters: &CareerMeters) -> Option<(f64, f64)> {
        let current = match self {
            AchievementCondition::ColonyCount { .. } => f64::from(meters.colonies),
            AchievementCondition::FleetShips { .. } => f64::from(meters.fleet_ships),
            AchievementCondition::PrestigeRuns { .. } => f64::from(meters.prestige_runs),
            AchievementCondition::RaidSurvived { .. } => f64::from(meters.raids_survived),
            AchievementCondition::OutpostConquered { .. } => f64::from(meters.outposts_conquered),
            AchievementCondition::Wonder { id } => {
                let done = match id {
                    Some(want) => meters.wonder_ids.contains(want),
                    None => !meters.wonder_ids.is_empty() || meters.wonders > 0,
                };
                if done { 1.0 } else { 0.0 }
            }
            AchievementCondition::BuildingLevel { .. }
            | AchievementCondition::TechLevel { .. }
            | AchievementCondition::ShipCount { .. }
            | AchievementCondition::Standing { .. }
            | AchievementCondition::ExclusivePick { .. }
            | AchievementCondition::SeasonComplete { .. } => return None,
        };
        Some((current, self.target()))
    }
}

/// Live counters aggregated from read-only saves, used when a full `GameState` is not loaded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CareerMeters {
    pub colonies: u32,
    pub fleet_ships: u32,
    pub prestige_runs: u32,
    pub raids_survived: u32,
    pub outposts_conquered: u32,
    pub wonders: u32,
    pub wonder_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AchievementSpec {
    pub id: String,
    pub title: String,
    pub description: String,
    pub hidden: bool,
    pub condition: AchievementCondition,
}

impl AchievementSpec {
    pub fn listed_title(&self, unlocked: bool) -> &str {
        if unlocked || !self.hidden {
            &self.title
        } else {
            "???"
        }
    }

    pub fn listed_description(&self, unlocked: bool) -> &str {
        if unlocked || !self.hidden {
            &self.description
        } else {
            "???"
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AchievementConfig {
    entries: Vec<AchievementSpec>,
}

impl AchievementConfig {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn parse(toml: &str) -> Result<Self, AchievementError> {
        let file: File =
            toml::from_str(toml).map_err(|e| AchievementError::Parse(e.to_string()))?;
        file.into_config()
    }

    pub fn get(&self, id: &str) -> Option<&AchievementSpec> {
        self.entries.iter().find(|s| s.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AchievementSpec> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Wonder ids live in balance config, so this runs after that catalog loads.
    pub fn validate_wonders(&self, wonders: &[WonderSpec]) -> Result<(), AchievementError> {
        for spec in &self.entries {
            spec.condition.validate_wonder(wonders)?;
        }
        Ok(())
    }

    /// Custom `--balance` files may omit or rename bundled wonders. Drop those
    /// rows instead of making the named wonder mandatory.
    pub fn drop_unknown_wonders(&mut self, wonders: &[WonderSpec]) {
        self.entries
            .retain(|spec| spec.condition.validate_wonder(wonders).is_ok());
    }
}

impl Default for AchievementConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_ACHIEVEMENTS).expect("bundled achievements.toml must be valid")
    }
}

#[derive(Deserialize)]
struct File {
    achievements: Vec<FileRow>,
}

#[derive(Deserialize)]
struct FileRow {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    hidden: bool,
    condition: AchievementCondition,
}

impl File {
    fn into_config(self) -> Result<AchievementConfig, AchievementError> {
        let techs = TechTree::default();
        let factions = DiplomacyConfig::default();
        let mut entries = Vec::with_capacity(self.achievements.len());
        let mut seen = std::collections::BTreeSet::new();
        for row in self.achievements {
            if row.id.is_empty() {
                return Err(AchievementError::EmptyId);
            }
            if row.title.is_empty() {
                return Err(AchievementError::EmptyTitle(row.id));
            }
            if !seen.insert(row.id.clone()) {
                return Err(AchievementError::DuplicateId(row.id));
            }
            row.condition.validate(&techs, &factions)?;
            entries.push(AchievementSpec {
                id: row.id,
                title: row.title,
                description: row.description,
                hidden: row.hidden,
                condition: row.condition,
            });
        }
        Ok(AchievementConfig { entries })
    }
}

impl GameState {
    /// Award every unmet achievement whose condition now holds. Each id is stored once.
    pub fn evaluate_achievements(&mut self, now: u64) {
        if self.is_modified() {
            return;
        }
        let due: Vec<String> = self
            .config
            .achievements
            .iter()
            .filter(|spec| !self.achievements.contains_key(&spec.id))
            .filter(|spec| spec.condition.met(self))
            .map(|spec| spec.id.clone())
            .collect();
        for id in due {
            let title = self
                .config
                .achievements
                .get(&id)
                .map(|s| s.title.clone())
                .unwrap_or_else(|| id.clone());
            self.achievements.insert(id, now);
            self.push_event(now, format!("achievement: {title}"));
        }
    }
}

fn building_level(state: &GameState, kind: BuildingKind) -> u32 {
    let mut best = state.buildings.get(kind);
    for colony in &state.colonies {
        best = best.max(colony.buildings.get(kind));
    }
    best
}

fn fleet_ships(state: &GameState) -> u32 {
    let mut n = state.fleet.total();
    for colony in &state.colonies {
        n = n.saturating_add(colony.fleet.total());
    }
    for mission in &state.missions {
        n = n.saturating_add(mission.ships.total());
    }
    n
}

fn ship_count(state: &GameState, kind: ShipKind) -> u32 {
    let mut n = state.fleet.get(kind);
    for colony in &state.colonies {
        n = n.saturating_add(colony.fleet.get(kind));
    }
    for mission in &state.missions {
        n = n.saturating_add(mission.ships.get(kind));
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::buildings::Buildings;
    use crate::fleets::Coord;
    use crate::planets::{Colony, PlanetType};
    use crate::ships::Fleet;
    use crate::state::Resources;
    use std::collections::BTreeMap;

    fn catalog() -> AchievementConfig {
        AchievementConfig::default()
    }

    #[test]
    fn bundled_catalog_has_at_least_twenty_validated_rows() {
        let cfg = catalog();
        assert!(cfg.len() >= 20, "catalog size {}", cfg.len());
        let wonders = BalanceConfig::default().wonders;
        cfg.validate_wonders(&wonders).unwrap();
        assert!(cfg.get("first_cut").is_some());
        assert!(cfg.get("armada").unwrap().hidden);
        assert!(cfg.get("hidden_smuggler").unwrap().hidden);
    }

    #[test]
    fn loader_rejects_non_positive_standing_thresholds() {
        let err = AchievementConfig::parse(
            r#"
[[achievements]]
id = "bad"
title = "Bad"
condition = { kind = "standing", id = "traders", min = -1.0 }
"#,
        )
        .unwrap_err();
        assert!(
            matches!(err, AchievementError::ZeroTarget(ref id) if id == "traders"),
            "{err}"
        );
    }

    #[test]
    fn loader_rejects_unknown_building_ids() {
        let err = AchievementConfig::parse(
            r#"
[[achievements]]
id = "bad"
title = "Bad"
condition = { kind = "building_level", id = "not_a_mine", level = 1 }
"#,
        )
        .unwrap_err();
        assert!(
            matches!(err, AchievementError::UnknownBuilding(ref id) if id == "not_a_mine"),
            "{err}"
        );
    }

    #[test]
    fn hidden_rows_list_as_unknown_until_earned() {
        let spec = catalog().get("armada").unwrap().clone();
        assert_eq!(spec.listed_title(false), "???");
        assert_eq!(spec.listed_description(false), "???");
        assert_eq!(spec.listed_title(true), "Armada");
        assert_eq!(spec.listed_description(true), "Field a fleet of 100 ships.");
    }

    #[test]
    fn measured_wonder_progress_keeps_specific_ids() {
        let mut meters = CareerMeters {
            wonders: 1,
            ..CareerMeters::default()
        };
        meters.wonder_ids.insert("observatory".into());
        let any = AchievementCondition::Wonder { id: None };
        let observatory = AchievementCondition::Wonder {
            id: Some("observatory".into()),
        };
        let ring = AchievementCondition::Wonder {
            id: Some("dyson_ring".into()),
        };
        assert_eq!(any.measured(&meters), Some((1.0, 1.0)));
        assert_eq!(observatory.measured(&meters), Some((1.0, 1.0)));
        assert_eq!(ring.measured(&meters), Some((0.0, 1.0)));
    }

    #[test]
    fn catch_up_awards_each_achievement_once() {
        let mut state = GameState::new(10);
        state.buildings.metal_mine = 2;
        state.catch_up(11);
        let first = *state.achievements.get("first_cut").expect("awarded");
        assert_eq!(first, 11);
        assert_eq!(
            state
                .events
                .iter()
                .filter(|e| e.text == "achievement: First cut")
                .count(),
            1
        );
        state.catch_up(20);
        assert_eq!(state.achievements.get("first_cut"), Some(&11));
        assert_eq!(
            state
                .events
                .iter()
                .filter(|e| e.text == "achievement: First cut")
                .count(),
            1
        );
    }

    #[test]
    fn prestige_keeps_unlocks_and_dates() {
        let mut cfg = BalanceConfig::default();
        cfg.prestige.min_score = 1;
        cfg.prestige.wonder_weight = 40;
        let mut state = GameState::with_config(0, cfg);
        state.buildings.metal_mine = 2;
        state.completed_wonders.push("observatory".into());
        state.catch_up(5);
        assert!(state.achievements.contains_key("first_cut"));
        let at = state.achievements["first_cut"];
        state.prestige_reset(10).unwrap();
        assert_eq!(state.achievements.get("first_cut"), Some(&at));
        assert_eq!(state.buildings.metal_mine, 1);
        assert!(state.achievements.contains_key("prestige_once"));
    }

    #[test]
    fn colony_and_fleet_conditions_read_live_stats() {
        let mut state = GameState::new(1);
        state.config.achievements = catalog();
        state.colonies.push(Colony {
            coord: Coord {
                galaxy: 1,
                system: 1,
                position: 4,
            },
            kind: PlanetType::Rocky,
            size: 100,
            resources: Resources::ZERO,
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
        state.fleet.add(ShipKind::LightFighter, 20);
        state.catch_up(2);
        assert!(state.achievements.contains_key("first_colony"));
        assert!(state.achievements.contains_key("first_fighter"));
        assert!(state.achievements.contains_key("wing"));
        assert!(!state.achievements.contains_key("armada"));
    }

    #[test]
    fn catch_up_awards_at_the_completion_instant_even_if_a_later_raid_wipes_the_fleet() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.min_ships = 80;
        cfg.raid.strength_factor = 10.0;
        cfg.raid.first_delay_secs = 86_400;
        let mut state = GameState::with_config(0, cfg);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        state.enqueue_ships(ShipKind::LightFighter, 1, 0).unwrap();
        let done = state.ship_queue[0].completes_at.expect("queued");
        state.next_raid_at = Some(done.saturating_add(1));
        state.catch_up(done.saturating_add(10_000));
        assert_eq!(state.achievements.get("first_fighter"), Some(&done));
        assert_eq!(state.fleet.get(ShipKind::LightFighter), 0);
    }

    #[test]
    fn raid_survived_uses_a_counter_not_the_event_log() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.strength_factor = 0.0;
        cfg.raid.min_ships = 1;
        let mut state = GameState::with_config(10, cfg);
        state.fleet.add(ShipKind::Battleship, 8);
        for i in 0..32 {
            state.push_event(10, format!("noise {i}"));
        }
        let report = state.trigger_raid(11);
        assert_ne!(report.winner, crate::CombatWinner::Attacker);
        assert_eq!(state.raids_survived, 1);
        assert!(state.achievements.contains_key("raid_survived"));
        assert!(!state.events.iter().any(|e| e.text.starts_with("noise 0")));
        for _ in 0..4 {
            state.trigger_raid(state.last_tick_unix.saturating_add(1));
        }
        assert!(state.raids_survived >= 5);
        assert!(state.achievements.contains_key("raid_veteran"));
    }
}
