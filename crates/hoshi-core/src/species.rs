//! Playable species: data-driven traits, tree overlays and unique mechanics.

use crate::balance::BalanceConfig;
use crate::buildings::BuildingKind;
use crate::fleets::{Coord, distance};
use crate::planets::Colony;
use crate::prestige::Prestige;
use crate::research::{TechBranch, TechModifier, TechPrereq, TechSpec};
use crate::ships::Fleet;
use crate::state::{GameState, ResourceKind, Resources};
use crate::victory::VictoryPath;
use serde::Deserialize;

/// Flagship speed in flight points per second; its moves stay linear in distance.
const FLAGSHIP_SPEED: f64 = 10.0;
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_SPECIES: &str = include_str!("../data/species.toml");

/// Recorded on a save so load can re-apply the overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeciesRun {
    pub id: String,
    pub name: String,
    pub portrait: String,
    pub construction: f64,
    pub research: f64,
    pub ship_cost: f64,
    pub expedition: f64,
    pub espionage: f64,
    pub queue_slots: u32,
    pub energy_curve: f64,
    pub energy_surplus: f64,
    pub transmute_rate: f64,
    pub brew_metal: f64,
    pub brew_secs: u64,
    pub brew_until: Option<u64>,
    pub flagship: Option<Flagship>,
}

/// Void Wanderer city-ship. Production stays on the home planet until it anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flagship {
    pub coord: Coord,
    pub arrives_at: Option<u64>,
    pub anchored: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeciesSpec {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub available: bool,
    pub unlock_cost: u32,
    pub victory_lean: VictoryPath,
    pub portrait: String,
    pub strengths: String,
    pub weaknesses: String,
    pub construction: f64,
    pub research: f64,
    pub ship_cost: f64,
    pub expedition: f64,
    pub espionage: f64,
    pub queue_slots: u32,
    pub energy_curve: f64,
    pub energy_surplus: f64,
    pub transmute_rate: f64,
    pub brew_metal: f64,
    pub brew_secs: u64,
    pub flagship: bool,
    pub standing: BTreeMap<String, f64>,
    pub tree_remove: Vec<String>,
    pub techs: Vec<TechSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeciesCatalog {
    rows: Vec<SpeciesSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeciesError {
    Parse(String),
    Invalid(String),
    Unknown(String),
    Locked(String),
    Unavailable(&'static str),
}

impl fmt::Display for SpeciesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpeciesError::Parse(msg) | SpeciesError::Invalid(msg) => write!(f, "{msg}"),
            SpeciesError::Unknown(id) => write!(f, "unknown species {id}"),
            SpeciesError::Locked(id) => write!(f, "species {id} is locked"),
            SpeciesError::Unavailable(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for SpeciesError {}

impl SpeciesCatalog {
    pub fn parse(toml: &str) -> Result<Self, SpeciesError> {
        let file: File = toml::from_str(toml).map_err(|e| SpeciesError::Parse(e.to_string()))?;
        file.into_catalog()
    }

    pub fn get(&self, id: &str) -> Option<&SpeciesSpec> {
        self.rows.iter().find(|row| row.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SpeciesSpec> {
        self.rows.iter()
    }

    pub fn is_unlocked(&self, id: &str, prestige: &Prestige) -> bool {
        let Some(spec) = self.get(id) else {
            return false;
        };
        spec.available || prestige.unlocked_species.contains(id)
    }

    pub fn overlay(
        &self,
        id: &str,
        mut cfg: BalanceConfig,
    ) -> Result<(BalanceConfig, &SpeciesSpec), SpeciesError> {
        let spec = self
            .get(id)
            .ok_or_else(|| SpeciesError::Unknown(id.into()))?;
        spec.apply(&mut cfg)?;
        Ok((cfg, spec))
    }
}

impl Default for SpeciesCatalog {
    fn default() -> Self {
        Self::parse(DEFAULT_SPECIES).expect("bundled species.toml must be valid")
    }
}

impl SpeciesSpec {
    fn apply(&self, cfg: &mut BalanceConfig) -> Result<(), SpeciesError> {
        cfg.techs
            .apply_overrides(&self.tree_remove, self.techs.clone())
            .map_err(|e| SpeciesError::Invalid(e.to_string()))?;
        for faction in self.standing.keys() {
            if cfg.diplomacy.get(faction).is_none() {
                return Err(SpeciesError::Invalid(format!(
                    "unknown faction {faction} on species {}",
                    self.id
                )));
            }
        }
        cfg.expedition.reward_mult *= self.expedition.max(0.0);
        Ok(())
    }

    pub fn run(&self) -> SpeciesRun {
        SpeciesRun {
            id: self.id.clone(),
            name: self.name.clone(),
            portrait: if self.portrait.is_empty() {
                self.id.clone()
            } else {
                self.portrait.clone()
            },
            construction: self.construction.max(0.05),
            research: self.research.max(0.05),
            ship_cost: self.ship_cost.max(0.05),
            expedition: self.expedition.max(0.05),
            espionage: self.espionage.max(0.0),
            queue_slots: self.queue_slots,
            energy_curve: self.energy_curve.max(0.05),
            energy_surplus: self.energy_surplus.max(0.0),
            transmute_rate: self.transmute_rate.max(0.0),
            brew_metal: self.brew_metal.max(0.0),
            brew_secs: self.brew_secs,
            brew_until: None,
            flagship: self.flagship.then_some(Flagship {
                coord: Coord::HOME,
                arrives_at: None,
                anchored: false,
            }),
        }
    }
}

impl GameState {
    pub fn new_with_species(
        now_unix: u64,
        catalog: &SpeciesCatalog,
        id: &str,
        base: BalanceConfig,
        seed: u64,
        prestige: Prestige,
    ) -> Result<Self, SpeciesError> {
        if !catalog.is_unlocked(id, &prestige) {
            return Err(SpeciesError::Locked(id.into()));
        }
        let (config, spec) = catalog.overlay(id, base)?;
        let mut state = Self::new_live_galaxy(now_unix, config, seed);
        state.inherit_prestige(prestige);
        for (faction, delta) in &spec.standing {
            let next = state.diplomacy.standing(faction) + *delta;
            state.diplomacy.set_standing(faction, next);
        }
        state.species = Some(spec.run());
        Ok(state)
    }

    pub fn apply_loaded_species(&mut self, catalog: &SpeciesCatalog) -> Result<(), SpeciesError> {
        let Some(run) = self.species.as_ref() else {
            return Ok(());
        };
        let brew = run.brew_until;
        let flag = run.flagship.clone();
        let (config, spec) = catalog.overlay(&run.id, self.config.clone())?;
        self.config = config;
        let mut next = spec.run();
        next.brew_until = brew;
        next.flagship = flag;
        self.species = Some(next);
        Ok(())
    }

    pub fn unlock_species(
        &mut self,
        catalog: &SpeciesCatalog,
        id: &str,
    ) -> Result<(), SpeciesError> {
        let spec = catalog
            .get(id)
            .ok_or_else(|| SpeciesError::Unknown(id.into()))?;
        if spec.available || self.prestige.unlocked_species.contains(id) {
            return Err(SpeciesError::Invalid(format!("{id} is already unlocked")));
        }
        if spec.unlock_cost == 0 {
            return Err(SpeciesError::Invalid(format!("{id} cannot be bought")));
        }
        if self.prestige.currency < spec.unlock_cost {
            return Err(SpeciesError::Unavailable("not enough prestige"));
        }
        self.prestige.currency -= spec.unlock_cost;
        self.prestige.unlocked_species.insert(id.into());
        Ok(())
    }

    pub fn transmute(
        &mut self,
        from: ResourceKind,
        to: ResourceKind,
        amount: f64,
    ) -> Result<(), SpeciesError> {
        let rate = self
            .species
            .as_ref()
            .map(|run| run.transmute_rate)
            .unwrap_or(0.0);
        if rate <= 0.0 {
            return Err(SpeciesError::Unavailable("transmutation is off"));
        }
        if from == to {
            return Err(SpeciesError::Unavailable("pick two different resources"));
        }
        if !amount.is_finite() || amount <= 0.0 {
            return Err(SpeciesError::Unavailable("amount must be > 0"));
        }
        let have = self.resources.get(from);
        if have + 1e-9 < amount {
            return Err(SpeciesError::Unavailable("not enough stockpile"));
        }
        self.resources.set(from, have - amount);
        let gained = amount * rate;
        self.resources.set(to, self.resources.get(to) + gained);
        Ok(())
    }

    pub fn brew_buff(&mut self, now: u64) -> Result<(), SpeciesError> {
        self.catch_up(now);
        let (cost, secs) = {
            let run = self
                .species
                .as_ref()
                .ok_or(SpeciesError::Unavailable("no species"))?;
            if run.brew_metal <= 0.0 || run.brew_secs == 0 {
                return Err(SpeciesError::Unavailable("brewing is off"));
            }
            (run.brew_metal, run.brew_secs)
        };
        if self.resources.iron + 1e-9 < cost {
            return Err(SpeciesError::Unavailable("not enough metal"));
        }
        self.resources.iron -= cost;
        let run = self
            .species
            .as_mut()
            .ok_or(SpeciesError::Unavailable("no species"))?;
        run.brew_until = Some(now.saturating_add(secs));
        Ok(())
    }

    pub fn brew_active(&self, now: u64) -> bool {
        self.species
            .as_ref()
            .and_then(|run| run.brew_until)
            .is_some_and(|until| until > now)
    }

    pub fn relocate_flagship(&mut self, dest: Coord, now: u64) -> Result<(), SpeciesError> {
        self.catch_up(now);
        let from = {
            let ship = self
                .species
                .as_ref()
                .and_then(|run| run.flagship.as_ref())
                .ok_or(SpeciesError::Unavailable("no flagship"))?;
            if ship.anchored {
                return Err(SpeciesError::Unavailable("flagship is anchored"));
            }
            if ship.arrives_at.is_some() {
                return Err(SpeciesError::Unavailable("flagship is in flight"));
            }
            if dest == ship.coord {
                return Err(SpeciesError::Unavailable("already there"));
            }
            ship.coord
        };
        self.check_frontier_flight(from, dest)
            .map_err(|e| SpeciesError::Invalid(e.to_string()))?;
        let dist = distance(from, dest, &self.config.flight);
        // The flagship keeps its own slow linear clock: distance / 10.
        let secs = (dist / FLAGSHIP_SPEED).ceil() as u64;
        let ship = self
            .species
            .as_mut()
            .and_then(|run| run.flagship.as_mut())
            .ok_or(SpeciesError::Unavailable("no flagship"))?;
        ship.coord = dest;
        ship.arrives_at = Some(now.saturating_add(secs.max(1)));
        Ok(())
    }

    pub fn tick_flagship(&mut self, now: u64) {
        let Some(ship) = self.species.as_mut().and_then(|run| run.flagship.as_mut()) else {
            return;
        };
        if let Some(at) = ship.arrives_at
            && at <= now
        {
            ship.arrives_at = None;
        }
    }

    pub fn anchor_flagship(&mut self, now: u64) -> Result<(), SpeciesError> {
        self.catch_up(now);
        let coord = {
            let Some(run) = self.species.as_ref() else {
                return Err(SpeciesError::Unavailable("no species"));
            };
            let Some(ship) = run.flagship.as_ref() else {
                return Err(SpeciesError::Unavailable("no flagship"));
            };
            if ship.anchored {
                return Err(SpeciesError::Unavailable("already anchored"));
            }
            if ship.arrives_at.is_some() {
                return Err(SpeciesError::Unavailable("flagship is in flight"));
            }
            ship.coord
        };
        if coord.galaxy == self.config.frontier.galaxy && !self.frontier_coord_visible(coord) {
            return Err(SpeciesError::Invalid(format!(
                "frontier sector at {coord} is still locked"
            )));
        }
        {
            let Some(run) = self.species.as_mut() else {
                return Err(SpeciesError::Unavailable("no species"));
            };
            let Some(ship) = run.flagship.as_mut() else {
                return Err(SpeciesError::Unavailable("no flagship"));
            };
            ship.anchored = true;
        }
        if coord != Coord::HOME && !self.colonies.iter().any(|c| c.coord == coord) {
            let (kind, size) =
                crate::planets::planet_profile(self.galaxy_seed, coord, self.generator_version);
            self.colonies.push(Colony {
                coord,
                kind,
                size,
                resources: Resources::ZERO,
                buildings: Default::default(),
                fleet: Fleet::default(),
                defences: crate::DefenceGrid::default(),
                build_queue: Vec::new(),
                rare: None,
                goods: Default::default(),
            });
        }
        self.push_event(now, format!("flagship anchored at {coord}"));
        Ok(())
    }
}

#[derive(Deserialize)]
struct File {
    version: u32,
    #[serde(rename = "species")]
    rows: Vec<FileSpecies>,
}

#[derive(Deserialize)]
struct FileSpecies {
    id: String,
    name: String,
    #[serde(default)]
    blurb: String,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    unlock_cost: u32,
    victory_lean: String,
    #[serde(default)]
    portrait: String,
    #[serde(default)]
    strengths: String,
    #[serde(default)]
    weaknesses: String,
    #[serde(default = "one")]
    construction: f64,
    #[serde(default = "one")]
    research: f64,
    #[serde(default = "one")]
    ship_cost: f64,
    #[serde(default = "one")]
    expedition: f64,
    #[serde(default = "one")]
    espionage: f64,
    #[serde(default)]
    queue_slots: u32,
    #[serde(default = "one")]
    energy_curve: f64,
    #[serde(default)]
    energy_surplus: f64,
    #[serde(default)]
    transmute_rate: f64,
    #[serde(default)]
    brew_metal: f64,
    #[serde(default)]
    brew_secs: u64,
    #[serde(default)]
    flagship: bool,
    #[serde(default)]
    standing: BTreeMap<String, f64>,
    #[serde(default)]
    tree_remove: Vec<String>,
    #[serde(default, rename = "tech")]
    techs: Vec<FileTech>,
}

fn one() -> f64 {
    1.0
}

#[derive(Deserialize)]
struct FileTech {
    id: String,
    name: String,
    #[serde(alias = "metal")]
    iron: f64,
    #[serde(alias = "crystal")]
    silicon: f64,
    deuterium: f64,
    cost_growth: f64,
    base_seconds: f64,
    time_growth: f64,
    #[serde(default)]
    prereq_buildings: Vec<FileBuildingPrereq>,
    #[serde(default)]
    prereq_techs: Vec<FileTechPrereq>,
    #[serde(default)]
    modifiers: Vec<TechModifier>,
    #[serde(default)]
    branch: TechBranch,
    #[serde(default)]
    tier: u32,
    #[serde(default)]
    exclusive_group: Option<String>,
    #[serde(default)]
    keystone: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    max_level: Option<u32>,
}

#[derive(Deserialize)]
struct FileBuildingPrereq {
    building: String,
    level: u32,
}

#[derive(Deserialize)]
struct FileTechPrereq {
    id: String,
    level: u32,
}

impl File {
    fn into_catalog(self) -> Result<SpeciesCatalog, SpeciesError> {
        if self.version != 1 {
            return Err(SpeciesError::Invalid(format!(
                "unsupported species version {} (want 1)",
                self.version
            )));
        }
        if self.rows.len() < 5 {
            return Err(SpeciesError::Invalid(
                "at least five species are required".into(),
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut rows = Vec::with_capacity(self.rows.len());
        for row in self.rows {
            if row.id.is_empty() || row.name.is_empty() {
                return Err(SpeciesError::Invalid(
                    "species id and name are required".into(),
                ));
            }
            if !ids.insert(row.id.clone()) {
                return Err(SpeciesError::Invalid(format!(
                    "duplicate species {}",
                    row.id
                )));
            }
            let lean = VictoryPath::from_key(&row.victory_lean).ok_or_else(|| {
                SpeciesError::Invalid(format!("unknown victory lean {}", row.victory_lean))
            })?;
            let mut techs = Vec::new();
            for raw in row.techs {
                techs.push(tech_from_file(raw)?);
            }
            rows.push(SpeciesSpec {
                id: row.id,
                name: row.name,
                blurb: row.blurb,
                available: row.available,
                unlock_cost: row.unlock_cost,
                victory_lean: lean,
                portrait: row.portrait,
                strengths: row.strengths,
                weaknesses: row.weaknesses,
                construction: row.construction,
                research: row.research,
                ship_cost: row.ship_cost,
                expedition: row.expedition,
                espionage: row.espionage,
                queue_slots: row.queue_slots,
                energy_curve: row.energy_curve,
                energy_surplus: row.energy_surplus,
                transmute_rate: row.transmute_rate,
                brew_metal: row.brew_metal,
                brew_secs: row.brew_secs,
                flagship: row.flagship,
                standing: row.standing,
                tree_remove: row.tree_remove,
                techs,
            });
        }
        if !rows.iter().any(|r| r.id == "humans" && r.available) {
            return Err(SpeciesError::Invalid(
                "humans must be available from the start".into(),
            ));
        }
        let cat = SpeciesCatalog { rows };
        for spec in &cat.rows {
            let mut cfg = BalanceConfig::default();
            spec.apply(&mut cfg)?;
        }
        Ok(cat)
    }
}

fn tech_from_file(raw: FileTech) -> Result<TechSpec, SpeciesError> {
    Ok(TechSpec {
        id: raw.id.clone(),
        name: raw.name,
        iron: raw.iron,
        silicon: raw.silicon,
        deuterium: raw.deuterium,
        cost_growth: raw.cost_growth,
        base_seconds: raw.base_seconds,
        time_growth: raw.time_growth,
        prereq_buildings: raw
            .prereq_buildings
            .into_iter()
            .map(|p| {
                let building = BuildingKind::from_key(&p.building).ok_or_else(|| {
                    SpeciesError::Invalid(format!("unknown building {}", p.building))
                })?;
                Ok(crate::research::BuildingPrereq {
                    building,
                    level: p.level,
                })
            })
            .collect::<Result<Vec<_>, SpeciesError>>()?,
        prereq_techs: raw
            .prereq_techs
            .into_iter()
            .map(|p| TechPrereq {
                id: p.id,
                level: p.level,
            })
            .collect(),
        modifiers: raw.modifiers,
        branch: raw.branch,
        tier: raw.tier,
        exclusive_group: raw.exclusive_group,
        keystone: raw.keystone,
        hidden: raw.hidden || raw.branch == TechBranch::Ancient,
        max_level: raw.max_level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_has_five_species_and_valid_trees() {
        let cat = SpeciesCatalog::default();
        assert!(cat.get("humans").unwrap().available);
        assert_eq!(cat.iter().count(), 5);
        for id in ["kryth", "aurelians", "void_wanderers", "synths"] {
            assert!(!cat.get(id).unwrap().available);
            let mut cfg = BalanceConfig::default();
            cat.get(id).unwrap().apply(&mut cfg).unwrap();
        }
        assert!(cat.get("kryth").is_some());
        let mut cfg = BalanceConfig::default();
        cat.get("kryth").unwrap().apply(&mut cfg).unwrap();
        assert!(cfg.techs.get("swarm_doctrine").is_some());
        assert!(cfg.techs.get("shield_doctrine").is_none());
    }

    #[test]
    fn aurelian_transmute_and_brew() {
        let cat = SpeciesCatalog::default();
        let mut prestige = Prestige::new(0);
        prestige.unlocked_species.insert("aurelians".into());
        let mut state = GameState::new_with_species(
            1_000,
            &cat,
            "aurelians",
            BalanceConfig::default(),
            1,
            prestige,
        )
        .unwrap();
        state.resources.iron = 100.0;
        state.resources.silicon = 0.0;
        state
            .transmute(ResourceKind::Iron, ResourceKind::Silicon, 10.0)
            .unwrap();
        assert!((state.resources.iron - 90.0).abs() < 1e-9);
        assert!((state.resources.silicon - 8.0).abs() < 1e-9);
        state.resources.iron = 500.0;
        state.brew_buff(1_000).unwrap();
        assert!(state.brew_active(1_000));
        assert!(!state.brew_active(1_000 + 601));
    }

    #[test]
    fn void_flagship_moves_then_anchors() {
        let cat = SpeciesCatalog::default();
        let mut prestige = Prestige::new(0);
        prestige.unlocked_species.insert("void_wanderers".into());
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state =
            GameState::new_with_species(1_000, &cat, "void_wanderers", cfg, 1, prestige).unwrap();
        let dest = Coord {
            galaxy: 1,
            system: 2,
            position: 4,
        };
        state.relocate_flagship(dest, 1_000).unwrap();
        let eta = state
            .species
            .as_ref()
            .unwrap()
            .flagship
            .as_ref()
            .unwrap()
            .arrives_at
            .unwrap();
        assert!(eta > 1_000);
        assert!(state.anchor_flagship(eta.saturating_sub(1)).is_err());
        state.anchor_flagship(eta).unwrap();
        assert!(
            state
                .species
                .as_ref()
                .unwrap()
                .flagship
                .as_ref()
                .unwrap()
                .arrives_at
                .is_none()
        );
        assert!(
            state
                .species
                .as_ref()
                .unwrap()
                .flagship
                .as_ref()
                .unwrap()
                .anchored
        );
        assert!(state.colonies.iter().any(|c| c.coord == dest));
    }

    #[test]
    fn void_flagship_cannot_enter_the_frontier_without_the_drive() {
        let cat = SpeciesCatalog::default();
        let mut prestige = Prestige::new(0);
        prestige.unlocked_species.insert("void_wanderers".into());
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state =
            GameState::new_with_species(1_000, &cat, "void_wanderers", cfg, 1, prestige).unwrap();
        let dest = Coord {
            galaxy: 2,
            system: 1,
            position: 1,
        };
        let err = state.relocate_flagship(dest, 1_000).unwrap_err();
        assert!(
            err.to_string().contains("frontier") || err.to_string().contains("drive"),
            "{err}"
        );
    }

    #[test]
    fn void_flagship_can_anchor_after_the_drive_is_gone() {
        let cat = SpeciesCatalog::default();
        let mut prestige = Prestige::new(0);
        prestige.unlocked_species.insert("void_wanderers".into());
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state =
            GameState::new_with_species(1_000, &cat, "void_wanderers", cfg, 1, prestige).unwrap();
        state.completed_wonders.push("dyson_ring".into());
        state.evaluate_frontier(1_000);
        let dest = state.frontier.worlds[0].coord;
        state
            .tech_levels
            .insert(state.config.frontier.flight_drive_id.clone(), 1);
        state.relocate_flagship(dest, 1_000).unwrap();
        let eta = state
            .species
            .as_ref()
            .unwrap()
            .flagship
            .as_ref()
            .unwrap()
            .arrives_at
            .unwrap();
        state.catch_up(eta);
        state
            .tech_levels
            .remove(&state.config.frontier.flight_drive_id.clone());
        state.anchor_flagship(eta).unwrap();
        assert!(state.colonies.iter().any(|c| c.coord == dest));
    }

    #[test]
    fn synth_energy_curve_is_steeper() {
        let cat = SpeciesCatalog::default();
        let mut prestige = Prestige::new(0);
        prestige.unlocked_species.insert("synths".into());
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut synth =
            GameState::new_with_species(1_000, &cat, "synths", cfg.clone(), 1, prestige).unwrap();
        synth.buildings.metal_mine = 8;
        synth.buildings.solar_plant = 1;
        let mut plain = GameState::with_config(1_000, cfg);
        plain.buildings.metal_mine = 8;
        plain.buildings.solar_plant = 1;
        assert!(synth.energy_factor() + 1e-9 < plain.energy_factor());
    }

    #[test]
    fn locked_species_need_prestige() {
        let cat = SpeciesCatalog::default();
        let err = GameState::new_with_species(
            1_000,
            &cat,
            "kryth",
            BalanceConfig::default(),
            1,
            Prestige::new(0),
        )
        .unwrap_err();
        assert!(matches!(err, SpeciesError::Locked(_)));
    }
}
