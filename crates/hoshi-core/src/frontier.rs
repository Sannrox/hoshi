//! Frontier galaxy: richer worlds unlocked by research or a wonder, with optional
//! bridge reveals that only speed up sectors the normal path also opens.

use crate::combat::next_u64;
use crate::fleets::{Coord, FleetError};
use crate::outposts::OutpostConfig;
use crate::planets::{PlanetType, planet_profile};
use crate::state::GameState;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fmt;

/// One generated frontier world. Bonuses are visible once its sector is revealed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontierWorld {
    pub coord: Coord,
    pub sector: u32,
    pub kind: PlanetType,
    pub size: u8,
    pub rare: Option<String>,
}

/// Persisted unlock progress for this galaxy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrontierBoard {
    pub revealed: u32,
    pub research_open: bool,
    pub wonder_open: bool,
    pub bridge_sectors: u32,
    pub seen: BTreeSet<String>,
    pub worlds: Vec<FrontierWorld>,
}

/// Data-driven unlock and generation knobs. Lives in `[frontier]` of balance.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontierConfig {
    pub galaxy: u16,
    pub sectors: u32,
    pub planets_per_sector: u32,
    pub production_bonus: f64,
    pub fields_bonus: u8,
    pub rare_chance: f64,
    pub drive_id: String,
    pub drive_level: u32,
    pub astro_id: String,
    pub astro_level: u32,
    pub flight_drive_id: String,
    pub flight_drive_level: u32,
    pub research_sectors: u32,
    pub wonder_sectors: u32,
    pub release_sectors: u32,
    pub epic_sectors: u32,
    pub bridge_sector_cap: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontierError {
    Invalid(String),
}

impl fmt::Display for FrontierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrontierError::Invalid(msg) => write!(f, "frontier: {msg}"),
        }
    }
}

impl std::error::Error for FrontierError {}

impl Default for FrontierConfig {
    fn default() -> Self {
        Self {
            galaxy: 2,
            sectors: 4,
            planets_per_sector: 3,
            production_bonus: 0.25,
            fields_bonus: 8,
            rare_chance: 0.55,
            drive_id: "impulse_drive".into(),
            drive_level: 1,
            astro_id: "astrophysics".into(),
            astro_level: 1,
            flight_drive_id: "impulse_drive".into(),
            flight_drive_level: 1,
            research_sectors: 2,
            wonder_sectors: 2,
            release_sectors: 1,
            epic_sectors: 1,
            bridge_sector_cap: 2,
        }
    }
}

impl FrontierConfig {
    pub fn validate(&self) -> Result<(), FrontierError> {
        if self.galaxy < 2 {
            return Err(FrontierError::Invalid(
                "galaxy must be >= 2 so the home galaxy stays 1".into(),
            ));
        }
        if self.production_bonus.is_nan() || self.production_bonus < 0.0 {
            return Err(FrontierError::Invalid(
                "production_bonus must be >= 0".into(),
            ));
        }
        if !self.rare_chance.is_finite() || !(0.0..=1.0).contains(&self.rare_chance) {
            return Err(FrontierError::Invalid(
                "rare_chance must be in [0, 1]".into(),
            ));
        }
        if self.drive_id.is_empty() || self.astro_id.is_empty() || self.flight_drive_id.is_empty() {
            return Err(FrontierError::Invalid(
                "drive_id, astro_id and flight_drive_id must not be empty".into(),
            ));
        }
        if self.sectors > 0
            && self.research_sectors.saturating_add(self.wonder_sectors) < self.sectors
        {
            return Err(FrontierError::Invalid(
                "research_sectors + wonder_sectors must cover every sector".into(),
            ));
        }
        if self.bridge_sector_cap > self.sectors {
            return Err(FrontierError::Invalid(
                "bridge_sector_cap cannot exceed sectors".into(),
            ));
        }
        Ok(())
    }

    pub fn gameplay_sectors(&self, research_open: bool, wonder_open: bool) -> u32 {
        let mut n = 0_u32;
        if research_open {
            n = n.saturating_add(self.research_sectors);
        }
        if wonder_open {
            n = n.saturating_add(self.wonder_sectors);
        }
        n.min(self.sectors)
    }

    pub fn revealed_count(&self, board: &FrontierBoard) -> u32 {
        let play = self.gameplay_sectors(board.research_open, board.wonder_open);
        let bridge = board.bridge_sectors.min(self.bridge_sector_cap);
        play.saturating_add(bridge).min(self.sectors)
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct FileFrontier {
    galaxy: Option<u16>,
    sectors: Option<u32>,
    planets_per_sector: Option<u32>,
    production_bonus: Option<f64>,
    fields_bonus: Option<u8>,
    rare_chance: Option<f64>,
    drive_id: Option<String>,
    drive_level: Option<u32>,
    astro_id: Option<String>,
    astro_level: Option<u32>,
    flight_drive_id: Option<String>,
    flight_drive_level: Option<u32>,
    research_sectors: Option<u32>,
    wonder_sectors: Option<u32>,
    release_sectors: Option<u32>,
    epic_sectors: Option<u32>,
    bridge_sector_cap: Option<u32>,
}

impl FileFrontier {
    pub(crate) fn into_config(self) -> Result<FrontierConfig, FrontierError> {
        let d = FrontierConfig::default();
        let cfg = FrontierConfig {
            galaxy: self.galaxy.unwrap_or(d.galaxy),
            sectors: self.sectors.unwrap_or(d.sectors),
            planets_per_sector: self.planets_per_sector.unwrap_or(d.planets_per_sector),
            production_bonus: self.production_bonus.unwrap_or(d.production_bonus),
            fields_bonus: self.fields_bonus.unwrap_or(d.fields_bonus),
            rare_chance: self.rare_chance.unwrap_or(d.rare_chance),
            drive_id: self.drive_id.unwrap_or(d.drive_id),
            drive_level: self.drive_level.unwrap_or(d.drive_level),
            astro_id: self.astro_id.unwrap_or(d.astro_id),
            astro_level: self.astro_level.unwrap_or(d.astro_level),
            flight_drive_id: self.flight_drive_id.unwrap_or(d.flight_drive_id),
            flight_drive_level: self.flight_drive_level.unwrap_or(d.flight_drive_level),
            research_sectors: self.research_sectors.unwrap_or(d.research_sectors),
            wonder_sectors: self.wonder_sectors.unwrap_or(d.wonder_sectors),
            release_sectors: self.release_sectors.unwrap_or(d.release_sectors),
            epic_sectors: self.epic_sectors.unwrap_or(d.epic_sectors),
            bridge_sector_cap: self.bridge_sector_cap.unwrap_or(d.bridge_sector_cap),
        };
        cfg.validate()?;
        Ok(cfg)
    }
}

/// Bridge signals that only speed up sector reveals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontierSignal {
    ReleasePublished,
    EpicClosed,
}

impl FrontierSignal {
    pub fn as_key(self) -> &'static str {
        match self {
            FrontierSignal::ReleasePublished => "release_published",
            FrontierSignal::EpicClosed => "epic_closed",
        }
    }
}

fn unscoped_frontier_id(id: &str) -> Option<&str> {
    if !id.contains('/') {
        return None;
    }
    id.rsplit_once(':')
        .map(|(_, raw)| raw)
        .filter(|raw| !raw.is_empty())
}

impl FrontierBoard {
    pub fn visible_worlds(&self) -> impl Iterator<Item = &FrontierWorld> {
        let revealed = self.revealed;
        self.worlds.iter().filter(move |w| w.sector <= revealed)
    }

    pub fn world_at(&self, coord: Coord) -> Option<&FrontierWorld> {
        self.worlds.iter().find(|w| w.coord == coord)
    }
}

/// Seeded placement. Same seed and config always yield the same coords and rares.
pub fn generate_frontier(
    galaxy_seed: u64,
    cfg: &FrontierConfig,
    rares: &OutpostConfig,
    version: u32,
) -> Vec<FrontierWorld> {
    if cfg.sectors == 0 || cfg.planets_per_sector == 0 {
        return Vec::new();
    }
    let mut seed = galaxy_seed ^ 0xF20A_71E2_F20A_71E2;
    let mut out = Vec::new();
    let mut used = BTreeSet::new();
    for sector in 1..=cfg.sectors {
        let mut placed = 0_u32;
        let mut attempts = 0_u32;
        while placed < cfg.planets_per_sector && attempts < 2_000 {
            attempts += 1;
            let roll = next_u64(&mut seed);
            let system = 1 + ((sector.saturating_sub(1) * 12) + (roll % 12) as u32) as u16;
            let position = 3 + ((roll >> 16) % 6) as u16;
            let coord = Coord {
                galaxy: cfg.galaxy,
                system: system.max(1),
                position: position.max(1),
            };
            if !used.insert(coord) {
                continue;
            }
            let (kind, size) = planet_profile(galaxy_seed, coord, version);
            let rare = pick_rare(&mut seed, rares, cfg.rare_chance);
            let extra = rare
                .as_ref()
                .and_then(|id| rares.rare.iter().find(|row| row.id == *id))
                .map(|row| row.fields)
                .unwrap_or(0);
            out.push(FrontierWorld {
                coord,
                sector,
                kind,
                size: size.saturating_add(extra).saturating_add(cfg.fields_bonus),
                rare,
            });
            placed += 1;
        }
    }
    out.sort_by_key(|w| (w.sector, w.coord.galaxy, w.coord.system, w.coord.position));
    out
}

fn pick_rare(seed: &mut u64, cfg: &OutpostConfig, chance: f64) -> Option<String> {
    if cfg.rare.is_empty() || chance <= 0.0 {
        return None;
    }
    let span = (chance.clamp(0.0, 1.0) * 10_000.0).round() as u64;
    if next_u64(seed) % 10_000 >= span {
        return None;
    }
    let total: u64 = cfg
        .rare
        .iter()
        .map(|row| u64::from(row.weight.max(1)))
        .sum();
    if total == 0 {
        return None;
    }
    let mut pick = next_u64(seed) % total;
    for row in &cfg.rare {
        let w = u64::from(row.weight.max(1));
        if pick < w {
            return Some(row.id.clone());
        }
        pick -= w;
    }
    cfg.rare.last().map(|row| row.id.clone())
}

impl GameState {
    pub(crate) fn seed_frontier(&mut self) {
        let cfg = self.config.frontier.clone();
        self.frontier.worlds = generate_frontier(
            self.galaxy_seed,
            &cfg,
            &self.config.outposts,
            self.generator_version,
        );
        self.frontier.revealed = if self.cheats.reveal_map {
            cfg.sectors
        } else {
            cfg.revealed_count(&self.frontier)
        };
    }

    pub fn frontier_glance(&self) -> FrontierGlance {
        let cfg = &self.config.frontier;
        FrontierGlance {
            galaxy: cfg.galaxy,
            sectors: cfg.sectors,
            revealed: self.frontier.revealed,
            research_open: self.frontier.research_open,
            wonder_open: self.frontier.wonder_open,
            bridge_sectors: self.frontier.bridge_sectors,
            worlds: self.frontier.visible_worlds().cloned().collect(),
        }
    }

    pub(crate) fn evaluate_frontier(&mut self, at: u64) {
        let cfg = self.config.frontier.clone();
        if cfg.sectors == 0 {
            return;
        }
        if self.frontier.worlds.is_empty() {
            self.seed_frontier();
        }
        let research = self.research_unlocks_frontier();
        let wonder = !self.completed_wonders.is_empty();
        let mut dirty = false;
        if research && !self.frontier.research_open {
            self.frontier.research_open = true;
            dirty = true;
        }
        if wonder && !self.frontier.wonder_open {
            self.frontier.wonder_open = true;
            dirty = true;
        }
        let next = cfg.revealed_count(&self.frontier);
        if next > self.frontier.revealed {
            self.frontier.revealed = next;
            dirty = true;
        }
        if dirty {
            self.push_event(
                at,
                format!(
                    "frontier: galaxy {} sector {}/{} open",
                    cfg.galaxy, self.frontier.revealed, cfg.sectors
                ),
            );
            self.record_history(crate::HistoryDraft {
                at,
                kind: crate::HistoryKind::Other,
                actor: "simulation".into(),
                action: "reveal".into(),
                objects: vec![format!("frontier:{}", self.frontier.revealed)],
                outcome: format!("{} sectors", self.frontier.revealed),
                ..crate::HistoryDraft::default()
            });
        }
    }

    /// Optional bridge extras. The same `id` is ignored forever, including after a reload.
    pub fn apply_frontier_bridge(
        &mut self,
        signal: FrontierSignal,
        id: &str,
        ts: u64,
        now_unix: u64,
    ) -> crate::ActivityApply {
        if !self.config.activity.enabled || self.config.frontier.sectors == 0 {
            return crate::ActivityApply::Skipped;
        }
        if ts > now_unix {
            return crate::ActivityApply::Skipped;
        }
        let key = format!("{}:{id}", signal.as_key());
        if id.is_empty() || self.frontier.seen.contains(&key) {
            return crate::ActivityApply::Skipped;
        }
        if let Some(raw) = unscoped_frontier_id(id) {
            let legacy = format!("{}:{raw}", signal.as_key());
            if self.frontier.seen.contains(&legacy) {
                return crate::ActivityApply::Skipped;
            }
        }
        self.frontier.seen.insert(key);
        let add = match signal {
            FrontierSignal::ReleasePublished => self.config.frontier.release_sectors,
            FrontierSignal::EpicClosed => self.config.frontier.epic_sectors,
        };
        self.frontier.bridge_sectors = self.frontier.bridge_sectors.saturating_add(add);
        self.catch_up(ts.min(now_unix));
        self.evaluate_frontier(self.last_tick_unix.max(ts));
        crate::ActivityApply::Applied
    }

    pub fn research_unlocks_frontier(&self) -> bool {
        let cfg = &self.config.frontier;
        self.tech_level(&cfg.drive_id) >= cfg.drive_level
            && self.tech_level(&cfg.astro_id) >= cfg.astro_level
    }

    pub fn can_fly_to_frontier(&self) -> bool {
        let cfg = &self.config.frontier;
        self.tech_level(&cfg.flight_drive_id) >= cfg.flight_drive_level
    }

    pub fn frontier_coord_visible(&self, dest: Coord) -> bool {
        let cfg = &self.config.frontier;
        if dest.galaxy != cfg.galaxy {
            return true;
        }
        self.frontier
            .world_at(dest)
            .is_some_and(|w| w.sector <= self.frontier.revealed)
    }

    pub(crate) fn check_frontier_flight(
        &self,
        origin: Coord,
        dest: Coord,
    ) -> Result<(), FleetError> {
        let cfg = &self.config.frontier;
        if dest.galaxy != cfg.galaxy && origin.galaxy != cfg.galaxy {
            return Ok(());
        }
        if dest.galaxy == cfg.galaxy && !self.frontier_coord_visible(dest) {
            return Err(FleetError::FrontierLocked { dest });
        }
        if !self.can_fly_to_frontier() {
            return Err(FleetError::NeedDrive {
                id: cfg.flight_drive_id.clone(),
                required: cfg.flight_drive_level,
                actual: self.tech_level(&cfg.flight_drive_id),
            });
        }
        Ok(())
    }

    pub fn frontier_production_bonus(&self, coord: Coord) -> f64 {
        if coord.galaxy == self.config.frontier.galaxy {
            self.config.frontier.production_bonus
        } else {
            0.0
        }
    }
}

/// CLI / TUI snapshot. Worlds list only revealed sectors.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontierGlance {
    pub galaxy: u16,
    pub sectors: u32,
    pub revealed: u32,
    pub research_open: bool,
    pub wonder_open: bool,
    pub bridge_sectors: u32,
    pub worlds: Vec<FrontierWorld>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::state::GameState;

    fn quiet() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg
    }

    #[test]
    fn generation_repeats_for_a_seed_and_stays_in_the_frontier_galaxy() {
        let cfg = FrontierConfig::default();
        let rares = OutpostConfig::default();
        let a = generate_frontier(7, &cfg, &rares, 1);
        let b = generate_frontier(7, &cfg, &rares, 1);
        assert_eq!(a, b);
        assert_eq!(a.len(), (cfg.sectors * cfg.planets_per_sector) as usize);
        assert!(a.iter().all(|w| w.coord.galaxy == cfg.galaxy));
        let other = generate_frontier(8, &cfg, &rares, 1);
        assert_ne!(a[0].coord, other[0].coord);
        let rares_share = a.iter().filter(|w| w.rare.is_some()).count();
        assert!(
            rares_share >= 2,
            "higher rare_chance should show in the catalog"
        );
        assert!(a.iter().all(|w| w.size > 15));
    }

    #[test]
    fn research_or_wonder_opens_sectors_without_the_bridge() {
        let mut state = GameState::with_seed(0, quiet(), 3);
        state.evaluate_frontier(0);
        assert_eq!(state.frontier.revealed, 0);
        state
            .tech_levels
            .insert(state.config.frontier.drive_id.clone(), 1);
        state.tech_levels.insert("astrophysics".into(), 1);
        state.evaluate_frontier(10);
        assert!(state.frontier.research_open);
        assert_eq!(state.frontier.revealed, 2);
        assert!(state.frontier_glance().worlds.iter().all(|w| w.sector <= 2));
        state.completed_wonders.push("dyson_ring".into());
        state.evaluate_frontier(20);
        assert!(state.frontier.wonder_open);
        assert_eq!(state.frontier.revealed, 4);
    }

    #[test]
    fn bridge_reveals_are_capped_and_do_not_exceed_the_normal_path() {
        let mut state = GameState::with_seed(0, quiet(), 3);
        state.config.activity.enabled = true;
        let first = state.apply_frontier_bridge(FrontierSignal::ReleasePublished, "v1.0.0", 5, 100);
        let again = state.apply_frontier_bridge(FrontierSignal::ReleasePublished, "v1.0.0", 6, 100);
        assert_eq!(first, crate::ActivityApply::Applied);
        assert_eq!(again, crate::ActivityApply::Skipped);
        assert_eq!(state.frontier.revealed, 1);
        let _ = state.apply_frontier_bridge(FrontierSignal::EpicClosed, "42", 7, 100);
        let _ = state.apply_frontier_bridge(FrontierSignal::ReleasePublished, "v1.1.0", 8, 100);
        assert_eq!(state.frontier.revealed, 2);
        assert!(state.frontier.revealed <= state.config.frontier.bridge_sector_cap);
        assert!(state.frontier.revealed < state.config.frontier.sectors);
    }

    #[test]
    fn flight_to_a_locked_sector_is_refused_until_drive_and_reveal() {
        let mut state = GameState::with_seed(0, quiet(), 3);
        state.evaluate_frontier(0);
        let dest = state.frontier.worlds[0].coord;
        let err = state.check_frontier_flight(state.home, dest).unwrap_err();
        assert!(matches!(err, FleetError::FrontierLocked { .. }), "{err}");
        state
            .tech_levels
            .insert(state.config.frontier.drive_id.clone(), 1);
        state.tech_levels.insert("astrophysics".into(), 1);
        state.evaluate_frontier(1);
        state.check_frontier_flight(state.home, dest).unwrap();
        let locked = state
            .frontier
            .worlds
            .iter()
            .find(|w| w.sector == 4)
            .unwrap()
            .coord;
        assert!(matches!(
            state.check_frontier_flight(state.home, locked),
            Err(FleetError::FrontierLocked { .. })
        ));
        state
            .tech_levels
            .remove(&state.config.frontier.drive_id.clone());
        let err = state.check_frontier_flight(state.home, dest).unwrap_err();
        assert!(matches!(err, FleetError::NeedDrive { .. }), "{err}");
    }

    #[test]
    fn colonized_frontier_world_mines_faster_than_the_same_setup_at_home() {
        let mut frontier = GameState::with_seed(0, quiet(), 3);
        frontier
            .tech_levels
            .insert(frontier.config.frontier.drive_id.clone(), 1);
        frontier.tech_levels.insert("astrophysics".into(), 1);
        frontier.evaluate_frontier(0);
        let world = frontier
            .frontier
            .visible_worlds()
            .next()
            .expect("revealed world")
            .clone();
        // Supply ample energy at both radii to isolate the frontier production bonus.
        frontier.config.solar_plant.base_energy /= frontier.config.orbits.solar_flux(world.coord);
        let buildings = crate::Buildings {
            metal_mine: 4,
            solar_plant: 6,
            metal_storage: 4,
            ..crate::Buildings::default()
        };
        let colony = crate::Colony {
            coord: world.coord,
            kind: world.kind,
            size: world.size,
            resources: crate::Resources::ZERO,
            buildings,
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: std::collections::BTreeMap::new(),
        };
        let mut home = colony.clone();
        home.coord = crate::Coord {
            galaxy: 1,
            system: 2,
            position: 4,
        };
        home.kind = world.kind;
        frontier.colonies.push(colony);
        let mut baseline = GameState::with_seed(0, frontier.config.clone(), 3);
        baseline.colonies.push(home);
        frontier.catch_up(3_600);
        baseline.catch_up(3_600);
        let front_metal = frontier
            .colonies
            .iter()
            .find(|c| c.coord == world.coord)
            .unwrap()
            .resources
            .iron;
        let home_metal = baseline.colonies[0].resources.iron;
        assert!(
            front_metal > home_metal + 1.0,
            "frontier {front_metal} vs home {home_metal}"
        );
    }

    #[test]
    fn catch_up_keeps_an_unlock_permanent() {
        let mut state = GameState::with_seed(0, quiet(), 3);
        state
            .tech_levels
            .insert(state.config.frontier.drive_id.clone(), 1);
        state.tech_levels.insert("astrophysics".into(), 1);
        state.catch_up(3_600);
        assert_eq!(state.frontier.revealed, 2);
        state
            .tech_levels
            .remove(&state.config.frontier.drive_id.clone());
        state.catch_up(7_200);
        assert_eq!(state.frontier.revealed, 2);
        assert!(state.frontier.research_open);
    }

    #[test]
    fn transport_and_return_flights_need_the_flight_drive() {
        let mut state = GameState::with_seed(0, quiet(), 3);
        state.completed_wonders.push("dyson_ring".into());
        state.evaluate_frontier(0);
        let dest = state.frontier.worlds[0].coord;
        let mut ships = crate::Fleet::default();
        ships.add(crate::ShipKind::SmallCargo, 1);
        let outbound = state.flight_from(state.home, dest, ships, 100).unwrap_err();
        assert!(
            matches!(outbound, crate::FleetError::NeedDrive { .. }),
            "{outbound}"
        );
        let inbound = state.flight_from(dest, state.home, ships, 100).unwrap_err();
        assert!(
            matches!(inbound, crate::FleetError::NeedDrive { .. }),
            "{inbound}"
        );
        state
            .tech_levels
            .insert(state.config.frontier.flight_drive_id.clone(), 1);
        state.flight_from(state.home, dest, ships, 100).unwrap();
        state.flight_from(dest, state.home, ships, 100).unwrap();
    }

    #[test]
    fn the_same_tag_from_two_repos_each_opens_a_sector() {
        let mut state = GameState::with_seed(0, quiet(), 3);
        state.config.activity.enabled = true;
        let first = state.apply_frontier_bridge(
            FrontierSignal::ReleasePublished,
            "acme/one:v1.0.0",
            5,
            100,
        );
        let second = state.apply_frontier_bridge(
            FrontierSignal::ReleasePublished,
            "acme/two:v1.0.0",
            6,
            100,
        );
        assert_eq!(first, crate::ActivityApply::Applied);
        assert_eq!(second, crate::ActivityApply::Applied);
        assert_eq!(state.frontier.revealed, 2);
        let again = state.apply_frontier_bridge(
            FrontierSignal::ReleasePublished,
            "acme/one:v1.0.0",
            7,
            100,
        );
        assert_eq!(again, crate::ActivityApply::Skipped);
        assert_eq!(state.frontier.revealed, 2);
    }

    #[test]
    fn repo_scoped_ids_do_not_replay_a_legacy_unscoped_event() {
        let mut state = GameState::with_seed(0, quiet(), 3);
        state.config.activity.enabled = true;
        let first = state.apply_frontier_bridge(FrontierSignal::ReleasePublished, "v1.0.0", 5, 100);
        let replay = state.apply_frontier_bridge(
            FrontierSignal::ReleasePublished,
            "acme/game:v1.0.0",
            6,
            100,
        );
        assert_eq!(first, crate::ActivityApply::Applied);
        assert_eq!(replay, crate::ActivityApply::Skipped);
        assert_eq!(state.frontier.revealed, 1);
    }
}
