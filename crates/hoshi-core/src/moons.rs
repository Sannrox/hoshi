//! Moons formed from battle debris. Phalanx sensors and jump gates live here.

use crate::combat::{CombatReport, next_u64};
use crate::fleets::{Coord, Mission};
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, Resources};
use serde::Deserialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoonFacility {
    Phalanx,
    JumpGate,
}

impl MoonFacility {
    pub fn as_key(self) -> &'static str {
        match self {
            MoonFacility::Phalanx => "phalanx",
            MoonFacility::JumpGate => "jump_gate",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "phalanx" => Some(Self::Phalanx),
            "jump_gate" => Some(Self::JumpGate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MoonError {
    NoMoon { at: Coord },
    AlreadyBuilt,
    Insufficient { missing: Resources },
    NoGate { at: Coord },
    Cooling { ready_at: u64 },
    EmptyFleet,
    NotEnoughShips,
}

impl fmt::Display for MoonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoonError::NoMoon { at } => write!(f, "no moon at {at}"),
            MoonError::AlreadyBuilt => write!(f, "that facility is already built"),
            MoonError::Insufficient { missing } => write!(f, "need {}", missing.brief()),
            MoonError::NoGate { at } => write!(f, "no jump gate at {at}"),
            MoonError::Cooling { ready_at } => write!(f, "jump ready at {ready_at}"),
            MoonError::EmptyFleet => write!(f, "no ships to jump"),
            MoonError::NotEnoughShips => write!(f, "not enough ships at origin"),
        }
    }
}

impl std::error::Error for MoonError {}

#[derive(Debug, Clone, PartialEq)]
pub struct Moon {
    pub parent: Coord,
    pub phalanx: u32,
    pub jump_gate: u32,
    pub jump_ready_at: u64,
}

impl Moon {
    pub fn new(parent: Coord) -> Self {
        Self {
            parent,
            phalanx: 0,
            jump_gate: 0,
            jump_ready_at: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhalanxContact {
    pub id: u64,
    pub origin: Coord,
    pub dest: Coord,
    pub phase: String,
    pub eta_secs: u64,
    pub ships: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MoonConfig {
    pub enabled: bool,
    pub debris_per_hull: f64,
    pub form_threshold: f64,
    pub form_chance_per_debris: f64,
    pub form_chance_cap: f64,
    pub phalanx_range_systems: u16,
    pub jump_cooldown_secs: u64,
    pub phalanx_cost: Resources,
    pub gate_cost: Resources,
}

impl MoonConfig {
    pub fn inactive() -> Self {
        Self {
            enabled: false,
            debris_per_hull: 1.0,
            form_threshold: 1_000.0,
            form_chance_per_debris: 0.0,
            form_chance_cap: 0.0,
            phalanx_range_systems: 5,
            jump_cooldown_secs: 3_600,
            phalanx_cost: Resources::ZERO,
            gate_cost: Resources::ZERO,
        }
    }
}

impl Default for MoonConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            debris_per_hull: 1.0,
            form_threshold: 400.0,
            form_chance_per_debris: 0.002,
            form_chance_cap: 0.35,
            phalanx_range_systems: 5,
            jump_cooldown_secs: 3_600,
            phalanx_cost: Resources::from_parts(400.0, 200.0, 0.0),
            gate_cost: Resources::from_parts(800.0, 400.0, 100.0),
        }
    }
}

#[derive(Deserialize)]
pub struct FileMoons {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_debris_hull")]
    pub debris_per_hull: f64,
    #[serde(default = "default_threshold")]
    pub form_threshold: f64,
    #[serde(default = "default_chance")]
    pub form_chance_per_debris: f64,
    #[serde(default = "default_cap")]
    pub form_chance_cap: f64,
    #[serde(default = "default_range")]
    pub phalanx_range_systems: u16,
    #[serde(default = "default_cool")]
    pub jump_cooldown_secs: u64,
    #[serde(default)]
    pub phalanx_metal: f64,
    #[serde(default)]
    pub phalanx_crystal: f64,
    #[serde(default)]
    pub phalanx_deuterium: f64,
    #[serde(default)]
    pub gate_metal: f64,
    #[serde(default)]
    pub gate_crystal: f64,
    #[serde(default)]
    pub gate_deuterium: f64,
}

fn default_true() -> bool {
    true
}
fn default_debris_hull() -> f64 {
    1.0
}
fn default_threshold() -> f64 {
    400.0
}
fn default_chance() -> f64 {
    0.002
}
fn default_cap() -> f64 {
    0.35
}
fn default_range() -> u16 {
    5
}
fn default_cool() -> u64 {
    3_600
}

impl FileMoons {
    pub fn into_config(self) -> MoonConfig {
        MoonConfig {
            enabled: self.enabled,
            debris_per_hull: self.debris_per_hull.max(0.0),
            form_threshold: self.form_threshold.max(0.0),
            form_chance_per_debris: self.form_chance_per_debris.max(0.0),
            form_chance_cap: self.form_chance_cap.clamp(0.0, 1.0),
            phalanx_range_systems: self.phalanx_range_systems,
            jump_cooldown_secs: self.jump_cooldown_secs,
            phalanx_cost: Resources::from_parts(
                self.phalanx_metal,
                self.phalanx_crystal,
                self.phalanx_deuterium,
            ),
            gate_cost: Resources::from_parts(
                self.gate_metal,
                self.gate_crystal,
                self.gate_deuterium,
            ),
        }
    }
}

pub fn hull_lost(before: Fleet, after: Fleet, hull: impl Fn(ShipKind) -> f64) -> f64 {
    let mut lost = 0.0;
    for kind in ShipKind::ALL {
        let n = before.get(kind).saturating_sub(after.get(kind));
        lost += f64::from(n) * hull(kind);
    }
    lost
}

impl GameState {
    pub fn moon_at(&self, parent: Coord) -> Option<&Moon> {
        self.moons.iter().find(|m| m.parent == parent)
    }

    pub fn moon_at_mut(&mut self, parent: Coord) -> Option<&mut Moon> {
        self.moons.iter_mut().find(|m| m.parent == parent)
    }

    pub fn note_fight_debris(&mut self, at: u64, parent: Coord, report: &CombatReport) {
        let hull = |kind| self.config.ships.get(kind).hull;
        let atk = hull_lost(report.attacker_before, report.attacker_after, hull);
        let def = hull_lost(report.defender_before, report.defender_after, hull);
        self.add_battle_debris(at, parent, atk, def);
    }

    pub fn add_battle_debris(&mut self, at: u64, parent: Coord, atk_lost: f64, def_lost: f64) {
        if !self.config.moons.enabled {
            return;
        }
        let add = (atk_lost + def_lost) * self.config.moons.debris_per_hull;
        if add <= 0.0 {
            return;
        }
        let debris = self.debris.entry(parent).or_insert(0.0);
        *debris += add;
        self.maybe_form_moon(at, parent);
    }

    fn maybe_form_moon(&mut self, at: u64, parent: Coord) {
        if self.moon_at(parent).is_some() {
            return;
        }
        if !self.owned_coords().contains(&parent) {
            return;
        }
        let debris = *self.debris.get(&parent).unwrap_or(&0.0);
        let cfg = &self.config.moons;
        if debris < cfg.form_threshold {
            return;
        }
        let over = debris - cfg.form_threshold;
        let chance = (over * cfg.form_chance_per_debris).min(cfg.form_chance_cap);
        if chance <= 0.0 {
            return;
        }
        let roll = next_u64(&mut self.rng_seed);
        let hit = (roll as f64) / (u64::MAX as f64) < chance;
        if !hit {
            return;
        }
        self.debris.insert(parent, 0.0);
        self.moons.push(Moon::new(parent));
        self.push_event(at, format!("moon formed at {parent}"));
    }

    pub fn build_moon_facility(
        &mut self,
        parent: Coord,
        facility: MoonFacility,
        now: u64,
    ) -> Result<(), MoonError> {
        self.catch_up(now);
        let Some(moon) = self.moon_at(parent) else {
            return Err(MoonError::NoMoon { at: parent });
        };
        let already = match facility {
            MoonFacility::Phalanx => moon.phalanx > 0,
            MoonFacility::JumpGate => moon.jump_gate > 0,
        };
        if already {
            return Err(MoonError::AlreadyBuilt);
        }
        let cost = match facility {
            MoonFacility::Phalanx => self.config.moons.phalanx_cost,
            MoonFacility::JumpGate => self.config.moons.gate_cost,
        };
        let have = self
            .resources_at(parent)
            .copied()
            .unwrap_or(Resources::ZERO);
        let missing = Resources::from_parts(
            (cost.iron - have.iron).max(0.0),
            (cost.silicon - have.silicon).max(0.0),
            (cost.deuterium - have.deuterium).max(0.0),
        );
        if missing.iron > 0.0 || missing.silicon > 0.0 || missing.deuterium > 0.0 {
            return Err(MoonError::Insufficient { missing });
        }
        if let Ok(res) = self.resources_at_mut(parent) {
            res.iron = (res.iron - cost.iron).max(0.0);
            res.silicon = (res.silicon - cost.silicon).max(0.0);
            res.deuterium = (res.deuterium - cost.deuterium).max(0.0);
        }
        let moon = self.moon_at_mut(parent).expect("moon still present");
        match facility {
            MoonFacility::Phalanx => moon.phalanx = 1,
            MoonFacility::JumpGate => moon.jump_gate = 1,
        }
        self.push_event(
            self.last_tick_unix,
            format!("moon: {} at {parent}", facility.as_key()),
        );
        Ok(())
    }

    pub fn jump_between(
        &mut self,
        from: Coord,
        to: Coord,
        ships: Fleet,
        now: u64,
    ) -> Result<(), MoonError> {
        self.catch_up(now);
        if ships.is_empty() {
            return Err(MoonError::EmptyFleet);
        }
        let now = self.last_tick_unix;
        let from_moon = self.moon_at(from).ok_or(MoonError::NoMoon { at: from })?;
        if from_moon.jump_gate == 0 {
            return Err(MoonError::NoGate { at: from });
        }
        if from_moon.jump_ready_at > now {
            return Err(MoonError::Cooling {
                ready_at: from_moon.jump_ready_at,
            });
        }
        let to_moon = self.moon_at(to).ok_or(MoonError::NoMoon { at: to })?;
        if to_moon.jump_gate == 0 {
            return Err(MoonError::NoGate { at: to });
        }
        if to_moon.jump_ready_at > now {
            return Err(MoonError::Cooling {
                ready_at: to_moon.jump_ready_at,
            });
        }
        let parked = parked_fleet(self, from);
        for kind in ShipKind::ALL {
            if parked.get(kind) < ships.get(kind) {
                return Err(MoonError::NotEnoughShips);
            }
        }
        if let Some(fleet) = parked_fleet_mut(self, from) {
            for kind in ShipKind::ALL {
                fleet.saturating_sub(kind, ships.get(kind));
            }
        }
        if let Some(fleet) = parked_fleet_mut(self, to) {
            for kind in ShipKind::ALL {
                fleet.add(kind, ships.get(kind));
            }
        }
        let ready = now.saturating_add(self.config.moons.jump_cooldown_secs);
        if let Some(m) = self.moon_at_mut(from) {
            m.jump_ready_at = ready;
        }
        if let Some(m) = self.moon_at_mut(to) {
            m.jump_ready_at = ready;
        }
        self.push_event(now, format!("jump: {from} -> {to}"));
        Ok(())
    }

    pub fn phalanx_contacts(
        &self,
        parent: Coord,
        now: u64,
    ) -> Result<Vec<PhalanxContact>, MoonError> {
        let moon = self
            .moon_at(parent)
            .ok_or(MoonError::NoMoon { at: parent })?;
        if moon.phalanx == 0 {
            return Ok(Vec::new());
        }
        let range = self.config.moons.phalanx_range_systems;
        let mut out = Vec::new();
        for mission in &self.missions {
            if !in_phalanx_range(parent, mission, range) {
                continue;
            }
            let ships: u32 = ShipKind::ALL.iter().map(|&k| mission.ships.get(k)).sum();
            out.push(PhalanxContact {
                id: mission.id,
                origin: mission.origin,
                dest: mission.dest,
                phase: mission.phase.as_key().into(),
                eta_secs: mission.phase_ends_at.saturating_sub(now),
                ships,
            });
        }
        Ok(out)
    }
}

fn in_phalanx_range(parent: Coord, mission: &Mission, range: u16) -> bool {
    system_steps(parent, mission.dest) <= range
        || (mission.origin != parent && system_steps(parent, mission.origin) <= range)
}

fn system_steps(a: Coord, b: Coord) -> u16 {
    if a.galaxy != b.galaxy {
        return u16::MAX;
    }
    u16::abs_diff(a.system, b.system)
}

fn parked_fleet(state: &GameState, coord: Coord) -> Fleet {
    if coord == state.home {
        state.fleet
    } else {
        state
            .colonies
            .iter()
            .find(|c| c.coord == coord)
            .map(|c| c.fleet)
            .unwrap_or_default()
    }
}

fn parked_fleet_mut(state: &mut GameState, coord: Coord) -> Option<&mut Fleet> {
    if coord == state.home {
        Some(&mut state.fleet)
    } else {
        state
            .colonies
            .iter_mut()
            .find(|c| c.coord == coord)
            .map(|c| &mut c.fleet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::ships::Fleet;
    use crate::state::GameState;

    fn moon_cfg() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.moons.enabled = true;
        cfg.moons.form_threshold = 10.0;
        cfg.moons.form_chance_per_debris = 1.0;
        cfg.moons.form_chance_cap = 1.0;
        cfg.moons.phalanx_cost = Resources::from_parts(10.0, 0.0, 0.0);
        cfg.moons.gate_cost = Resources::from_parts(10.0, 0.0, 0.0);
        cfg.moons.jump_cooldown_secs = 100;
        cfg.moons.phalanx_range_systems = 5;
        cfg.flight.base_fleet_slots = 4;
        cfg
    }

    #[test]
    fn same_seed_repeats_moon_formation() {
        let cfg = moon_cfg();
        let mut a = GameState::with_seed(0, cfg.clone(), 7);
        let mut b = GameState::with_seed(0, cfg, 7);
        a.resources.iron = 1_000.0;
        b.resources.iron = 1_000.0;
        a.add_battle_debris(0, Coord::HOME, 20.0, 20.0);
        b.add_battle_debris(0, Coord::HOME, 20.0, 20.0);
        assert_eq!(a.moons, b.moons);
        assert_eq!(a.moons.len(), 1);
        assert_eq!(a.moons[0].parent, Coord::HOME);
    }

    #[test]
    fn jump_is_instant_and_enforces_cooldown_and_missing_gate() {
        let mut state = GameState::with_seed(0, moon_cfg(), 1);
        state.resources.iron = 1_000.0;
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 4,
        };
        state.colonies.push(crate::Colony {
            coord: dest,
            kind: crate::PlanetType::Rocky,
            size: 80,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.moons.push(Moon::new(Coord::HOME));
        state.moons.push(Moon::new(dest));
        state
            .build_moon_facility(Coord::HOME, MoonFacility::JumpGate, 0)
            .unwrap();
        let err = state
            .jump_between(Coord::HOME, dest, scout(), 0)
            .unwrap_err();
        assert!(matches!(err, MoonError::NoGate { at } if at == dest));
        state.colonies[0].resources.iron = 1_000.0;
        state
            .build_moon_facility(dest, MoonFacility::JumpGate, 0)
            .unwrap();
        state.fleet.add(ShipKind::LightFighter, 2);
        state.jump_between(Coord::HOME, dest, scout(), 0).unwrap();
        assert_eq!(state.fleet.get(ShipKind::LightFighter), 1);
        assert_eq!(state.colonies[0].fleet.get(ShipKind::LightFighter), 1);
        let err = state
            .jump_between(Coord::HOME, dest, scout(), 0)
            .unwrap_err();
        assert!(matches!(err, MoonError::Cooling { .. }));
        state.jump_between(Coord::HOME, dest, scout(), 100).unwrap();
    }

    #[test]
    fn phalanx_lists_fleets_inside_range_only() {
        let mut state = GameState::with_seed(0, moon_cfg(), 1);
        state.resources.iron = 1_000.0;
        state.resources.deuterium = 10_000.0;
        state.moons.push(Moon::new(Coord::HOME));
        state
            .build_moon_facility(Coord::HOME, MoonFacility::Phalanx, 0)
            .unwrap();
        state.fleet.add(ShipKind::LightFighter, 2);
        let near = Coord {
            galaxy: 1,
            system: 3,
            position: 1,
        };
        let far = Coord {
            galaxy: 1,
            system: 40,
            position: 1,
        };
        state.launch_fleet(scout(), near, 100, 0, 0).unwrap();
        state.fleet.add(ShipKind::LightFighter, 1);
        state.launch_fleet(scout(), far, 100, 0, 0).unwrap();
        assert_eq!(state.config.moons.phalanx_range_systems, 5);
        let seen = state.phalanx_contacts(Coord::HOME, 0).unwrap();
        assert!(seen.iter().any(|c| c.dest == near), "{seen:?}");
        assert!(seen.iter().all(|c| c.dest != far), "{seen:?}");
    }

    fn scout() -> Fleet {
        let mut f = Fleet::default();
        f.add(ShipKind::LightFighter, 1);
        f
    }
}
