//! Moons formed from battle debris. Phalanx sensors and jump gates live here.

use crate::combat::{CombatReport, next_u64};
use crate::fleets::{Coord, Mission};
use crate::intel::{
    apply_count_error, light_delay_secs, light_range_au, sensor_error, signed_unit,
};
use crate::research::ModifierKind;
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, ResourceKind, Resources};
use serde::Deserialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateKind {
    Moon,
    Station,
}

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
    InvalidCargo,
    CargoExceedsCapacity { need: f64, cap: f64 },
    DestFull,
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
            MoonError::InvalidCargo => {
                write!(f, "cargo amounts must be finite and non-negative")
            }
            MoonError::CargoExceedsCapacity { need, cap } => {
                write!(f, "cargo {need:.0} exceeds capacity {cap:.0}")
            }
            MoonError::DestFull => write!(f, "destination storage is full"),
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

/// One fleet a phalanx sees. `id` is the mission id; the announced home
/// raid has no mission and reads `None` with phase `raid`. `delay_secs` is
/// the light-time stamped on the reading; own mission tracks read 0.
#[derive(Debug, Clone, PartialEq)]
pub struct PhalanxContact {
    pub id: Option<u64>,
    pub origin: Coord,
    pub dest: Coord,
    pub phase: String,
    pub eta_secs: u64,
    pub ships: u32,
    pub delay_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MoonConfig {
    pub enabled: bool,
    pub debris_per_hull: f64,
    /// Share of recycled debris that comes back as iron; the rest is silicon.
    pub debris_iron_share: f64,
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
            debris_iron_share: 0.6,
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
            debris_iron_share: 0.6,
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
    #[serde(default = "default_iron_share")]
    pub debris_iron_share: f64,
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
fn default_iron_share() -> f64 {
    0.6
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
            debris_iron_share: if self.debris_iron_share.is_finite() {
                self.debris_iron_share.clamp(0.0, 1.0)
            } else {
                0.6
            },
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

    /// Debris one fight leaves behind, by the same hull-loss rule as
    /// [`Self::note_fight_debris`]. Zero when moons are off.
    pub fn fight_debris(&self, report: &CombatReport) -> f64 {
        if !self.config.moons.enabled {
            return 0.0;
        }
        let hull = |kind| self.config.ships.get(kind).hull;
        let atk = hull_lost(report.attacker_before, report.attacker_after, hull);
        let def = hull_lost(report.defender_before, report.defender_after, hull);
        ((atk + def) * self.config.moons.debris_per_hull).max(0.0)
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

    /// Jump `ships` and `cargo` from one gated owned world to another: no
    /// fuel, no flight time, and both gates start their cooldown. Used by
    /// `hoshi moon jump` and the dispatch form's jump mission.
    pub fn jump_between(
        &mut self,
        from: Coord,
        to: Coord,
        ships: Fleet,
        cargo: Resources,
        now: u64,
    ) -> Result<(), MoonError> {
        self.catch_up(now);
        let now = self.last_tick_unix;
        let (from_site, to_site) = self.jump_sites(from, to, ships, cargo, now)?;
        let has_cargo = ResourceKind::ALL.iter().any(|&kind| cargo.get(kind) > 1e-9);
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
        if has_cargo {
            if let Ok(res) = self.resources_at_mut(from) {
                *res = res.saturating_sub(cargo);
            }
            if let Ok(res) = self.resources_at_mut(to) {
                *res = res.saturating_add(cargo);
            }
        }
        let ready = now.saturating_add(self.config.moons.jump_cooldown_secs);
        self.set_gate_ready(from, from_site, ready);
        self.set_gate_ready(to, to_site, ready);
        self.push_event(now, format!("jump: {from} -> {to}"));
        Ok(())
    }

    /// Why a jump from `from` to `to` would be refused at `now`, checked
    /// without moving anything: the same refusals `jump_between` gives.
    pub fn check_jump(
        &self,
        from: Coord,
        to: Coord,
        ships: Fleet,
        cargo: Resources,
        now: u64,
    ) -> Result<(), MoonError> {
        self.jump_sites(from, to, ships, cargo, now).map(|_| ())
    }

    /// Whether `at` has a jump gate built, on its moon or as a station,
    /// ready or cooling.
    pub fn has_jump_gate(&self, at: Coord) -> bool {
        self.moon_at(at).is_some_and(|moon| moon.jump_gate > 0)
            || self
                .buildings_at(at)
                .is_some_and(|buildings| buildings.jump_gate > 0)
    }

    fn jump_sites(
        &self,
        from: Coord,
        to: Coord,
        ships: Fleet,
        cargo: Resources,
        now: u64,
    ) -> Result<(GateKind, GateKind), MoonError> {
        if ships.is_empty() {
            return Err(MoonError::EmptyFleet);
        }
        if !cargo.is_finite_nonneg() {
            return Err(MoonError::InvalidCargo);
        }
        let from_site = self.gate_site(from, now)?;
        let to_site = self.gate_site(to, now)?;
        let parked = parked_fleet(self, from);
        for kind in ShipKind::ALL {
            if parked.get(kind) < ships.get(kind) {
                return Err(MoonError::NotEnoughShips);
            }
        }
        let has_cargo = ResourceKind::ALL.iter().any(|&kind| cargo.get(kind) > 1e-9);
        if has_cargo {
            let cap = self.cargo_capacity(ships);
            let need = cargo.total();
            if need > cap + 1e-9 {
                return Err(MoonError::CargoExceedsCapacity { need, cap });
            }
            let have = self.resources_at(from).copied().unwrap_or(Resources::ZERO);
            if !have.can_afford(cargo) {
                return Err(MoonError::Insufficient {
                    missing: have.missing(cargo),
                });
            }
            for kind in ResourceKind::ALL {
                let amount = cargo.get(kind);
                if amount <= 1e-9 {
                    continue;
                }
                let dest_have = self.resources_at(to).map(|r| r.get(kind)).unwrap_or(0.0);
                let room = (self.storage_cap_at(to, kind) - dest_have).max(0.0);
                if amount > room + 1e-9 {
                    return Err(MoonError::DestFull);
                }
            }
        }
        Ok((from_site, to_site))
    }

    /// A ready moon gate wins when both stand; a cooling moon yields to a
    /// ready station gate on the same world.
    fn gate_site(&self, at: Coord, now: u64) -> Result<GateKind, MoonError> {
        let moon = self.moon_at(at).filter(|moon| moon.jump_gate > 0);
        let station = self
            .buildings_at(at)
            .is_some_and(|buildings| buildings.jump_gate > 0);
        let moon_ready_at = moon.map(|moon| moon.jump_ready_at).unwrap_or(0);
        let station_ready_at = self.station_gate_ready.get(&at).copied().unwrap_or(0);
        if moon.is_some() && moon_ready_at <= now {
            return Ok(GateKind::Moon);
        }
        if station && station_ready_at <= now {
            return Ok(GateKind::Station);
        }
        if moon.is_some() {
            return Err(MoonError::Cooling {
                ready_at: moon_ready_at,
            });
        }
        if station {
            return Err(MoonError::Cooling {
                ready_at: station_ready_at,
            });
        }
        Err(MoonError::NoGate { at })
    }

    fn set_gate_ready(&mut self, at: Coord, site: GateKind, ready: u64) {
        match site {
            GateKind::Moon => {
                if let Some(moon) = self.moon_at_mut(at) {
                    moon.jump_ready_at = ready;
                }
            }
            GateKind::Station => {
                self.station_gate_ready.insert(at, ready);
            }
        }
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
                id: Some(mission.id),
                origin: mission.origin,
                dest: mission.dest,
                phase: mission.phase.as_key().into(),
                eta_secs: mission.phase_ends_at.saturating_sub(now),
                ships,
                delay_secs: 0,
            });
        }
        out.extend(self.phalanx_raid_contact(parent, now));
        Ok(out)
    }

    /// The announced home raid as the phalanx at `parent` sees it, derived at
    /// scan time and never stored. Like passive rival sensors it is a noisy
    /// present-time glance stamped with the light delay from the nearest
    /// pirate outpost: the ETA is the raid clock, and the ship count carries
    /// the probe sensor error over that range. The noise is seeded per raid
    /// so rescans agree, and a non-zero error never echoes the scheduler's
    /// exact fleet.
    fn phalanx_raid_contact(&self, parent: Coord, now: u64) -> Option<PhalanxContact> {
        if !self.config.raid.enabled
            || system_steps(parent, self.home) > self.config.moons.phalanx_range_systems
        {
            return None;
        }
        let lands = self.upcoming_raid_at()?;
        let eta_secs = lands.saturating_sub(now);
        if eta_secs > self.config.raid.min_warning_secs {
            return None;
        }
        let origin = self
            .outposts
            .iter()
            .map(|o| o.coord)
            .min_by_key(|&c| light_delay_secs(c, parent, &self.config.orbits))?;
        let ships = self.phalanx_count(self.scheduled_raid_ships(), origin, parent, lands);
        Some(PhalanxContact {
            id: None,
            origin,
            dest: self.home,
            phase: "raid".into(),
            eta_secs,
            ships,
            delay_secs: light_delay_secs(origin, parent, &self.config.orbits),
        })
    }

    /// Hull count this save's phalanx reads on another tenant's `mission`
    /// landing at `arrives_at`, from the phalanx moon in range nearest the
    /// fleet's origin by light-time. `None` when no phalanx covers it. The
    /// noise matches the announced pirate raid, salted by the arrival.
    pub(crate) fn phalanx_foreign_ships(&self, mission: &Mission, arrives_at: u64) -> Option<u32> {
        let range = self.config.moons.phalanx_range_systems;
        let parent = self
            .moons
            .iter()
            .filter(|moon| moon.phalanx > 0 && in_phalanx_range(moon.parent, mission, range))
            .map(|moon| moon.parent)
            .min_by_key(|&parent| light_delay_secs(mission.origin, parent, &self.config.orbits))?;
        Some(self.phalanx_count(mission.ships.total(), mission.origin, parent, arrives_at))
    }

    /// `exact` hulls as the phalanx at `parent` reads them from `origin`:
    /// the probe sensor error over that range, seeded per `salt` and moon so
    /// rescans agree. A non-zero error never echoes `exact`.
    fn phalanx_count(&self, exact: u32, origin: Coord, parent: Coord, salt: u64) -> u32 {
        let range = light_range_au(origin, parent, &self.config.orbits);
        let quality = 1.0 + self.modifier(ModifierKind::Espionage);
        let error = sensor_error(range, quality, &self.config.espionage);
        let mut rng = self.galaxy_seed ^ salt ^ coord_key(parent);
        let unit = signed_unit(&mut rng);
        let ships = apply_count_error(exact, error, unit, false, 0.0);
        if error > 0.0 && ships == exact {
            if unit < 0.0 && exact > 0 {
                exact - 1
            } else {
                exact.saturating_add(1)
            }
        } else {
            ships
        }
    }
}

fn coord_key(c: Coord) -> u64 {
    (u64::from(c.galaxy) << 32) | (u64::from(c.system) << 16) | u64::from(c.position)
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
            ship_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.moons.push(Moon::new(Coord::HOME));
        state.moons.push(Moon::new(dest));
        state
            .build_moon_facility(Coord::HOME, MoonFacility::JumpGate, 0)
            .unwrap();
        let err = state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 0)
            .unwrap_err();
        assert!(matches!(err, MoonError::NoGate { at } if at == dest));
        state.colonies[0].resources.iron = 1_000.0;
        state
            .build_moon_facility(dest, MoonFacility::JumpGate, 0)
            .unwrap();
        state.fleet.add(ShipKind::LightFighter, 2);
        state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 0)
            .unwrap();
        assert_eq!(state.fleet.get(ShipKind::LightFighter), 1);
        assert_eq!(state.colonies[0].fleet.get(ShipKind::LightFighter), 1);
        let err = state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 0)
            .unwrap_err();
        assert!(matches!(err, MoonError::Cooling { .. }));
        state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 100)
            .unwrap();
    }

    #[test]
    fn phalanx_lists_fleets_inside_range_only() {
        let mut state = GameState::with_seed(0, moon_cfg(), 1);
        state.resources.iron = 1_000.0;
        state.resources.deuterium = 1e12;
        state.resources.water_ice = 1e12;
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
        state
            .launch_fleet(state.home, scout(), near, 100, 0, 0)
            .unwrap();
        state.fleet.add(ShipKind::LightFighter, 1);
        state
            .launch_fleet(state.home, scout(), far, 100, 0, 0)
            .unwrap();
        assert_eq!(state.config.moons.phalanx_range_systems, 5);
        let seen = state.phalanx_contacts(Coord::HOME, 0).unwrap();
        assert!(seen.iter().any(|c| c.dest == near), "{seen:?}");
        assert!(seen.iter().all(|c| c.dest != far), "{seen:?}");
    }

    /// Home moon with a phalanx, one pirate outpost two systems over, and a
    /// raid of `ships` landing at `lands`.
    fn raid_watch(cfg: BalanceConfig, seed: u64, lands: u64, ships: u32) -> GameState {
        let mut state = GameState::with_seed(0, cfg, seed);
        let mut moon = Moon::new(Coord::HOME);
        moon.phalanx = 1;
        state.moons.push(moon);
        state.outposts.truncate(1);
        state.outposts[0].coord = Coord {
            galaxy: 1,
            system: 3,
            position: 4,
        };
        state.next_raid_at = Some(lands);
        state.next_raid_ships = Some(ships);
        state
    }

    fn raid_contacts(state: &GameState, now: u64) -> Vec<PhalanxContact> {
        let seen = state.phalanx_contacts(Coord::HOME, now).unwrap();
        seen.into_iter().filter(|c| c.id.is_none()).collect()
    }

    #[test]
    fn phalanx_sees_the_announced_raid_on_the_raid_clock() {
        let state = raid_watch(moon_cfg(), 3, 10_000, 12);
        let announced = 10_000 - state.config.raid.min_warning_secs;
        assert!(raid_contacts(&state, announced - 1).is_empty());
        let now = announced + 100;
        let seen = raid_contacts(&state, now);
        assert_eq!(seen.len(), 1, "{seen:?}");
        let origin = state.outposts[0].coord;
        assert_eq!(seen[0].phase, "raid");
        assert_eq!(seen[0].origin, origin);
        assert_eq!(seen[0].dest, Coord::HOME);
        assert_eq!(seen[0].eta_secs, 10_000 - now);
        assert_eq!(
            seen[0].delay_secs,
            light_delay_secs(origin, Coord::HOME, &state.config.orbits)
        );
        assert!(seen[0].delay_secs > 0);

        let mut blind = state.clone();
        blind.moons[0].phalanx = 0;
        assert!(blind.phalanx_contacts(Coord::HOME, now).unwrap().is_empty());
    }

    #[test]
    fn raid_count_is_seeded_noise_not_the_scheduled_fleet() {
        let mut cfg = moon_cfg();
        cfg.espionage.error_base = 0.3;
        let a = raid_watch(cfg.clone(), 9, 10_000, 12);
        let b = raid_watch(cfg.clone(), 9, 10_000, 12);
        let first = raid_contacts(&a, 9_000)[0].ships;
        assert_eq!(first, raid_contacts(&b, 9_000)[0].ships);
        assert_eq!(first, raid_contacts(&a, 9_500)[0].ships);
        assert_ne!(first, 12);

        cfg.espionage.error_base = 0.0;
        cfg.espionage.error_per_au = 0.0;
        let exact = raid_watch(cfg, 9, 10_000, 12);
        assert_eq!(raid_contacts(&exact, 9_000)[0].ships, 12);
    }

    #[test]
    fn catch_up_keeps_one_raid_contact() {
        let lands = 3_600;
        let mut state = raid_watch(moon_cfg(), 5, lands, 12);
        state.catch_up(lands - 600);
        let seen = raid_contacts(&state, state.last_tick_unix);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(Some(seen[0].eta_secs), state.raid_countdown_secs());
        state.catch_up(lands - 60);
        assert_eq!(raid_contacts(&state, state.last_tick_unix).len(), 1);
    }

    fn scout() -> Fleet {
        let mut f = Fleet::default();
        f.add(ShipKind::LightFighter, 1);
        f
    }

    fn haul() -> Fleet {
        let mut f = Fleet::default();
        f.add(ShipKind::SmallCargo, 1);
        f
    }

    fn ice(amount: f64) -> Resources {
        let mut cargo = Resources::ZERO;
        cargo.water_ice = amount;
        cargo
    }

    fn gated_pair() -> (GameState, Coord) {
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
            ship_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.moons.push(Moon::new(Coord::HOME));
        state.moons.push(Moon::new(dest));
        state
            .build_moon_facility(Coord::HOME, MoonFacility::JumpGate, 0)
            .unwrap();
        state.colonies[0].resources.iron = 1_000.0;
        state
            .build_moon_facility(dest, MoonFacility::JumpGate, 0)
            .unwrap();
        (state, dest)
    }

    fn stocks(state: &GameState, dest: Coord) -> (Resources, Resources, Fleet, Fleet, u64, u64) {
        (
            state.resources,
            state.colonies[0].resources,
            state.fleet,
            state.colonies[0].fleet,
            state.moon_at(Coord::HOME).unwrap().jump_ready_at,
            state.moon_at(dest).unwrap().jump_ready_at,
        )
    }

    #[test]
    fn a_jump_carries_named_cargo_between_worlds() {
        let (mut state, dest) = gated_pair();
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.water_ice = 500.0;
        state
            .jump_between(Coord::HOME, dest, haul(), ice(500.0), 0)
            .unwrap();
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 0);
        assert_eq!(state.colonies[0].fleet.get(ShipKind::SmallCargo), 1);
        assert!((state.resources.water_ice - 0.0).abs() < 1e-9);
        assert!((state.colonies[0].resources.water_ice - 500.0).abs() < 1e-9);
        let ready = state.config.moons.jump_cooldown_secs;
        assert_eq!(state.moon_at(Coord::HOME).unwrap().jump_ready_at, ready);
        assert_eq!(state.moon_at(dest).unwrap().jump_ready_at, ready);
    }

    #[test]
    fn a_jump_without_cargo_leaves_stocks_alone() {
        let (mut state, dest) = gated_pair();
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.water_ice = 500.0;
        state
            .jump_between(Coord::HOME, dest, haul(), Resources::ZERO, 0)
            .unwrap();
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 0);
        assert_eq!(state.colonies[0].fleet.get(ShipKind::SmallCargo), 1);
        assert!((state.resources.water_ice - 500.0).abs() < 1e-9);
        assert!((state.colonies[0].resources.water_ice - 0.0).abs() < 1e-9);
    }

    #[test]
    fn a_refused_cargo_jump_moves_nothing() {
        let (mut state, dest) = gated_pair();
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.fleet.add(ShipKind::LightFighter, 1);
        state.resources.water_ice = 100.0;
        let before = stocks(&state, dest);

        let over_hold = state
            .jump_between(Coord::HOME, dest, scout(), ice(100.0), 0)
            .unwrap_err();
        assert!(
            matches!(over_hold, MoonError::CargoExceedsCapacity { .. }),
            "{over_hold}"
        );
        assert_eq!(stocks(&state, dest), before);

        let short = state
            .jump_between(Coord::HOME, dest, haul(), ice(500.0), 0)
            .unwrap_err();
        assert!(matches!(short, MoonError::Insufficient { .. }), "{short}");
        assert_eq!(stocks(&state, dest), before);

        let cap = state.storage_cap_at(dest, ResourceKind::WaterIce);
        state.resources.water_ice = 500.0;
        state.colonies[0].resources.water_ice = cap;
        let filled = stocks(&state, dest);
        let full = state
            .jump_between(Coord::HOME, dest, haul(), ice(500.0), 0)
            .unwrap_err();
        assert!(matches!(full, MoonError::DestFull), "{full}");
        assert_eq!(stocks(&state, dest), filled);
    }

    #[test]
    fn a_send_between_gated_worlds_points_at_the_free_jump() {
        let (mut state, dest) = gated_pair();
        // Any drive: the impulse refit leaves no combustion hull to name.
        state.tech_levels.insert("impulse_drive".into(), 4);
        state.fleet.add(ShipKind::SmallCargo, 1);
        let hint = state.expensive_send_hint(Coord::HOME, dest, haul(), 0.0, 0.0);
        assert_eq!(
            hint.as_deref(),
            Some("jump gates at both worlds; jump is free")
        );
        assert_eq!(
            state.check_jump(Coord::HOME, dest, haul(), Resources::ZERO, 0),
            Ok(())
        );
        state.moons.retain(|moon| moon.parent != dest);
        assert!(!state.has_jump_gate(dest));
        assert_eq!(
            state.check_jump(Coord::HOME, dest, haul(), Resources::ZERO, 0),
            Err(MoonError::NoGate { at: dest })
        );
    }

    fn arm_station_gate(state: &mut GameState, coord: Coord) {
        if coord == Coord::HOME {
            state.buildings.jump_gate = 1;
            state.defences.add(crate::DefenceKind::OrbitalStation, 1);
            return;
        }
        let colony = state
            .colonies
            .iter_mut()
            .find(|c| c.coord == coord)
            .expect("colony");
        colony.buildings.jump_gate = 1;
        colony.defences.add(crate::DefenceKind::OrbitalStation, 1);
    }

    fn owned_world(state: &mut GameState, dest: Coord) {
        state.colonies.push(crate::Colony {
            coord: dest,
            kind: crate::PlanetType::Rocky,
            size: 80,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            ship_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
    }

    #[test]
    fn ships_jump_between_station_gates_and_a_moon_gate() {
        let mut state = GameState::with_seed(0, moon_cfg(), 1);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 4,
        };
        owned_world(&mut state, dest);
        arm_station_gate(&mut state, Coord::HOME);
        arm_station_gate(&mut state, dest);
        state.fleet.add(ShipKind::LightFighter, 2);
        state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 0)
            .unwrap();
        assert_eq!(state.fleet.get(ShipKind::LightFighter), 1);
        assert_eq!(state.colonies[0].fleet.get(ShipKind::LightFighter), 1);
        let ready = state.config.moons.jump_cooldown_secs;
        assert_eq!(state.station_gate_ready.get(&Coord::HOME), Some(&ready));
        assert_eq!(state.station_gate_ready.get(&dest), Some(&ready));
        let cooling = state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 0)
            .unwrap_err();
        assert!(matches!(cooling, MoonError::Cooling { ready_at } if ready_at == ready));

        let moon_world = Coord {
            galaxy: 1,
            system: 1,
            position: 5,
        };
        owned_world(&mut state, moon_world);
        state.moons.push(Moon::new(moon_world));
        state.colonies[1].resources.iron = 1_000.0;
        state
            .build_moon_facility(moon_world, MoonFacility::JumpGate, ready)
            .unwrap();
        state
            .jump_between(dest, moon_world, scout(), Resources::ZERO, ready)
            .unwrap();
        assert_eq!(state.colonies[0].fleet.get(ShipKind::LightFighter), 0);
        assert_eq!(state.colonies[1].fleet.get(ShipKind::LightFighter), 1);
        assert_eq!(state.station_gate_ready.get(&dest), Some(&(ready + ready)));
        assert_eq!(
            state.moon_at(moon_world).unwrap().jump_ready_at,
            ready + ready
        );
    }

    #[test]
    fn a_cooling_moon_yields_to_a_ready_station_gate() {
        let (mut state, dest) = gated_pair();
        arm_station_gate(&mut state, Coord::HOME);
        arm_station_gate(&mut state, dest);
        state.fleet.add(ShipKind::LightFighter, 2);
        state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 0)
            .unwrap();
        assert_eq!(state.moon_at(Coord::HOME).unwrap().jump_ready_at, 100);
        state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 0)
            .unwrap();
        assert_eq!(state.station_gate_ready.get(&Coord::HOME), Some(&100));
        assert_eq!(state.station_gate_ready.get(&dest), Some(&100));
        assert_eq!(state.moon_at(Coord::HOME).unwrap().jump_ready_at, 100);
        assert_eq!(state.fleet.get(ShipKind::LightFighter), 0);
        assert_eq!(state.colonies[0].fleet.get(ShipKind::LightFighter), 2);
    }

    #[test]
    fn a_world_without_a_gate_is_refused_as_no_gate() {
        let mut state = GameState::with_seed(0, moon_cfg(), 1);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 4,
        };
        owned_world(&mut state, dest);
        arm_station_gate(&mut state, Coord::HOME);
        state.fleet.add(ShipKind::LightFighter, 1);
        let err = state
            .jump_between(Coord::HOME, dest, scout(), Resources::ZERO, 0)
            .unwrap_err();
        assert!(matches!(err, MoonError::NoGate { at } if at == dest));
    }
}
