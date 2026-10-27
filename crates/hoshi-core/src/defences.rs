//! Planet defences: batteries, shield dome, orbital stations, interceptors, garrison.

use crate::combat::{CombatWinner, GroundReport, next_u64, resolve_ground};
use crate::ships::{Fleet, ShipKind, ShipStats};
use crate::state::Resources;
use serde::Deserialize;
use std::fmt;

const DEFAULT_DEFENCES: &str = include_str!("../data/defences.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DefenceKind {
    LightCannon,
    HeavyCannon,
    IonCannon,
    MissileSilo,
    Interceptor,
    ShieldDome,
    OrbitalStation,
    Garrison,
}

pub const DEFENCE_KINDS: usize = 8;

impl DefenceKind {
    pub const ALL: [DefenceKind; DEFENCE_KINDS] = [
        DefenceKind::LightCannon,
        DefenceKind::HeavyCannon,
        DefenceKind::IonCannon,
        DefenceKind::MissileSilo,
        DefenceKind::Interceptor,
        DefenceKind::ShieldDome,
        DefenceKind::OrbitalStation,
        DefenceKind::Garrison,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            DefenceKind::LightCannon => "light_cannon",
            DefenceKind::HeavyCannon => "heavy_cannon",
            DefenceKind::IonCannon => "ion_cannon",
            DefenceKind::MissileSilo => "missile_silo",
            DefenceKind::Interceptor => "interceptor",
            DefenceKind::ShieldDome => "shield_dome",
            DefenceKind::OrbitalStation => "orbital_station",
            DefenceKind::Garrison => "garrison",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "light_cannon" => DefenceKind::LightCannon,
            "heavy_cannon" => DefenceKind::HeavyCannon,
            "ion_cannon" => DefenceKind::IonCannon,
            "missile_silo" => DefenceKind::MissileSilo,
            "interceptor" => DefenceKind::Interceptor,
            "shield_dome" => DefenceKind::ShieldDome,
            "orbital_station" => DefenceKind::OrbitalStation,
            "garrison" => DefenceKind::Garrison,
            _ => return None,
        })
    }

    pub fn index(self) -> usize {
        match self {
            DefenceKind::LightCannon => 0,
            DefenceKind::HeavyCannon => 1,
            DefenceKind::IonCannon => 2,
            DefenceKind::MissileSilo => 3,
            DefenceKind::Interceptor => 4,
            DefenceKind::ShieldDome => 5,
            DefenceKind::OrbitalStation => 6,
            DefenceKind::Garrison => 7,
        }
    }
}

impl fmt::Display for DefenceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DefenceKind::LightCannon => "light cannon",
            DefenceKind::HeavyCannon => "heavy cannon",
            DefenceKind::IonCannon => "ion cannon",
            DefenceKind::MissileSilo => "missile silo",
            DefenceKind::Interceptor => "interceptor",
            DefenceKind::ShieldDome => "shield dome",
            DefenceKind::OrbitalStation => "orbital station",
            DefenceKind::Garrison => "garrison",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefenceRole {
    Battery,
    Missile,
    Interceptor,
    Dome,
    Orbital,
    Garrison,
}

impl DefenceRole {
    fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "battery" => DefenceRole::Battery,
            "missile" => DefenceRole::Missile,
            "interceptor" => DefenceRole::Interceptor,
            "dome" => DefenceRole::Dome,
            "orbital" => DefenceRole::Orbital,
            "garrison" => DefenceRole::Garrison,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefenceSpec {
    pub kind: DefenceKind,
    pub name: String,
    pub role: DefenceRole,
    pub iron: f64,
    pub silicon: f64,
    pub deuterium: f64,
    pub base_seconds: f64,
    pub shipyard: u32,
    pub attack: f64,
    pub shield: f64,
    pub hull: f64,
}

impl DefenceSpec {
    pub fn unit_cost(&self) -> Resources {
        Resources::from_parts(self.iron, self.silicon, self.deuterium)
    }

    pub fn duration_secs(&self) -> u64 {
        self.base_seconds.ceil().max(1.0) as u64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefenceConfig {
    pub rebuild_chance: f64,
    pub missile_per_pirate: f64,
    pub missile_damage: f64,
    pub intercept_chance: f64,
    /// Ground-strength multiplier. The tactical layer (#45) writes this; 1.0 until then.
    pub tactical_ground_mult: f64,
    pub dome_recharge_share: f64,
    pub hangar_per_station: u32,
    pub types: Vec<DefenceSpec>,
}

impl DefenceConfig {
    pub fn get(&self, kind: DefenceKind) -> &DefenceSpec {
        self.types
            .iter()
            .find(|s| s.kind == kind)
            .expect("catalog always contains every DefenceKind")
    }

    pub fn parse(toml: &str) -> Result<Self, DefenceCatalogError> {
        let file: File =
            toml::from_str(toml).map_err(|e| DefenceCatalogError::Parse(e.to_string()))?;
        file.into_config()
    }
}

impl Default for DefenceConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_DEFENCES).expect("bundled defences.toml must be valid")
    }
}

#[derive(Deserialize)]
struct File {
    #[serde(default = "default_rebuild")]
    rebuild_chance: f64,
    #[serde(default)]
    missile_per_pirate: f64,
    #[serde(default = "default_missile_damage")]
    missile_damage: f64,
    #[serde(default = "default_intercept")]
    intercept_chance: f64,
    #[serde(default = "default_tactical")]
    tactical_ground_mult: f64,
    #[serde(default = "default_recharge")]
    dome_recharge_share: f64,
    #[serde(default = "default_hangar")]
    hangar_per_station: u32,
    #[serde(rename = "defence")]
    types: Vec<FileDefence>,
}

fn default_rebuild() -> f64 {
    0.7
}
fn default_missile_damage() -> f64 {
    50.0
}
fn default_intercept() -> f64 {
    0.5
}
fn default_tactical() -> f64 {
    1.0
}
fn default_recharge() -> f64 {
    0.2
}
fn default_hangar() -> u32 {
    2
}

#[derive(Deserialize)]
struct FileDefence {
    id: String,
    name: String,
    role: String,
    #[serde(alias = "metal")]
    iron: f64,
    #[serde(alias = "crystal")]
    silicon: f64,
    deuterium: f64,
    base_seconds: f64,
    shipyard: u32,
    attack: f64,
    shield: f64,
    hull: f64,
}

impl File {
    fn into_config(self) -> Result<DefenceConfig, DefenceCatalogError> {
        let mut types = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in self.types {
            let kind = DefenceKind::from_key(&row.id)
                .ok_or_else(|| DefenceCatalogError::UnknownId(row.id.clone()))?;
            if !seen.insert(kind) {
                return Err(DefenceCatalogError::DuplicateId(row.id));
            }
            let role = DefenceRole::from_key(&row.role).ok_or_else(|| {
                DefenceCatalogError::UnknownRole {
                    id: row.id.clone(),
                    role: row.role.clone(),
                }
            })?;
            types.push(DefenceSpec {
                kind,
                name: row.name,
                role,
                iron: row.iron,
                silicon: row.silicon,
                deuterium: row.deuterium,
                base_seconds: row.base_seconds,
                shipyard: row.shipyard,
                attack: row.attack,
                shield: row.shield,
                hull: row.hull,
            });
        }
        for kind in DefenceKind::ALL {
            if !types.iter().any(|s| s.kind == kind) {
                return Err(DefenceCatalogError::MissingId(kind.as_key()));
            }
        }
        Ok(DefenceConfig {
            rebuild_chance: self.rebuild_chance,
            missile_per_pirate: self.missile_per_pirate,
            missile_damage: self.missile_damage,
            intercept_chance: self.intercept_chance,
            tactical_ground_mult: self.tactical_ground_mult,
            dome_recharge_share: self.dome_recharge_share,
            hangar_per_station: self.hangar_per_station,
            types,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefenceCatalogError {
    Parse(String),
    UnknownId(String),
    DuplicateId(String),
    MissingId(&'static str),
    UnknownRole { id: String, role: String },
}

impl fmt::Display for DefenceCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefenceCatalogError::Parse(e) => write!(f, "defences.toml: {e}"),
            DefenceCatalogError::UnknownId(id) => write!(f, "unknown defence id {id}"),
            DefenceCatalogError::DuplicateId(id) => write!(f, "duplicate defence id {id}"),
            DefenceCatalogError::MissingId(id) => write!(f, "missing defence id {id}"),
            DefenceCatalogError::UnknownRole { id, role } => {
                write!(f, "defence {id} has unknown role {role}")
            }
        }
    }
}

impl std::error::Error for DefenceCatalogError {}

/// Per-planet defence counts. Dome and station counts are levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DefenceGrid {
    counts: [u32; DEFENCE_KINDS],
}

impl DefenceGrid {
    pub fn get(self, kind: DefenceKind) -> u32 {
        self.counts[kind.index()]
    }

    pub fn add(&mut self, kind: DefenceKind, n: u32) {
        let i = kind.index();
        self.counts[i] = self.counts[i].saturating_add(n);
    }

    pub fn set(&mut self, kind: DefenceKind, n: u32) {
        self.counts[kind.index()] = n;
    }

    pub fn saturating_sub(&mut self, kind: DefenceKind, n: u32) {
        let i = kind.index();
        self.counts[i] = self.counts[i].saturating_sub(n);
    }

    pub fn is_empty(self) -> bool {
        self.counts.iter().all(|&n| n == 0)
    }

    pub fn battery_total(self, cfg: &DefenceConfig) -> u32 {
        DefenceKind::ALL
            .iter()
            .filter(|k| {
                matches!(
                    cfg.get(**k).role,
                    DefenceRole::Battery | DefenceRole::Missile | DefenceRole::Interceptor
                )
            })
            .map(|k| self.get(*k))
            .sum()
    }

    pub fn combat_points(self, cfg: &DefenceConfig) -> f64 {
        DefenceKind::ALL
            .iter()
            .map(|&kind| {
                let spec = cfg.get(kind);
                f64::from(self.get(kind)) * (spec.attack + spec.hull)
            })
            .sum()
    }
}

struct Shot {
    hull: f64,
    shield_max: f64,
    shield: f64,
    attack: f64,
    tag: u8,
}

impl Shot {
    fn alive(&self) -> bool {
        self.hull > 0.0
    }

    fn hit(&mut self, attack: f64) {
        let absorbed = self.shield.min(attack);
        self.shield -= absorbed;
        self.hull -= attack - absorbed;
    }
}

fn unit_f64(rng: &mut u64) -> f64 {
    (next_u64(rng) as f64) / ((u64::MAX as f64) + 1.0)
}

fn living(units: &[Shot]) -> Vec<usize> {
    units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.alive())
        .map(|(i, _)| i)
        .collect()
}

const TAG_STATION: u8 = 100;
const TAG_HANGAR: u8 = 101;
const TAG_BATTERY: u8 = 110;

fn rf_kind(tag: u8) -> Option<ShipKind> {
    if tag == TAG_HANGAR {
        Some(ShipKind::LightFighter)
    } else {
        ShipKind::ALL.get(tag as usize).copied()
    }
}

fn volley(
    firers: &[Shot],
    fire: &[usize],
    targets: &mut [Shot],
    rapid_fire: &[[u32; crate::SHIP_KINDS]; crate::SHIP_KINDS],
    rng: &mut u64,
) {
    for &i in fire {
        loop {
            let live = living(targets);
            if live.is_empty() {
                return;
            }
            let pick = live[(next_u64(rng) as usize) % live.len()];
            targets[pick].hit(firers[i].attack);
            let Some(from) = rf_kind(firers[i].tag) else {
                break;
            };
            let Some(into) = rf_kind(targets[pick].tag) else {
                break;
            };
            let value = rapid_fire[from.index()][into.index()];
            if value < 2 || next_u64(rng).is_multiple_of(u64::from(value)) {
                break;
            }
        }
    }
}

fn fight(
    a: &mut [Shot],
    b: &mut [Shot],
    max_rounds: u8,
    rapid_fire: &[[u32; crate::SHIP_KINDS]; crate::SHIP_KINDS],
    rng: &mut u64,
) -> u8 {
    let mut rounds = 0;
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    while rounds < max_rounds {
        if living(a).is_empty() || living(b).is_empty() {
            break;
        }
        for unit in a.iter_mut().chain(b.iter_mut()) {
            if unit.alive() {
                unit.shield = unit.shield_max;
            }
        }
        let af = living(a);
        let bf = living(b);
        volley(a, &af, b, rapid_fire, rng);
        volley(b, &bf, a, rapid_fire, rng);
        rounds += 1;
    }
    rounds
}

fn expand_fleet(fleet: Fleet, stats: &[ShipStats; crate::SHIP_KINDS]) -> Vec<Shot> {
    let mut out = Vec::new();
    for kind in ShipKind::ALL {
        let s = stats[kind.index()];
        for _ in 0..fleet.get(kind) {
            out.push(Shot {
                hull: s.hull,
                shield_max: s.shield,
                shield: s.shield,
                attack: s.attack,
                tag: kind.index() as u8,
            });
        }
    }
    out
}

fn compress_fleet(units: &[Shot]) -> Fleet {
    let mut fleet = Fleet::default();
    for unit in units {
        if unit.alive()
            && let Some(kind) = ShipKind::ALL.get(unit.tag as usize).copied()
        {
            fleet.add(kind, 1);
        }
    }
    fleet
}

/// One layered planet defence battle. Empty extra layers leave fleet combat to the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanetBattle {
    pub intercept_in: u32,
    pub intercept_down: u32,
    pub orbital_rounds: u8,
    pub stations_before: u32,
    pub stations_after: u32,
    pub hangar_before: u32,
    pub hangar_after: u32,
    pub dome_collapsed: bool,
    pub dome_absorbed: f64,
    pub batteries_before: u32,
    pub batteries_after: u32,
    pub rebuilt: u32,
    /// Batteries still gone once the repair crews are done.
    pub batteries_missing: u32,
    pub ground: Option<GroundReport>,
    pub winner: CombatWinner,
    pub attacker_after: Fleet,
    pub fleet_after: Fleet,
    pub defences_after: DefenceGrid,
}

pub fn intercept_missiles(incoming: u32, interceptors: u32, chance: f64, rng: &mut u64) -> u32 {
    let chance = chance.clamp(0.0, 1.0);
    let mut down = 0;
    let mut left = incoming;
    for _ in 0..interceptors {
        if left == 0 {
            break;
        }
        if unit_f64(rng) < chance {
            down += 1;
            left -= 1;
        }
    }
    down
}

fn apply_missiles(grid: &mut DefenceGrid, leftover: u32, damage: f64, cfg: &DefenceConfig) {
    if leftover == 0 || damage <= 0.0 {
        return;
    }
    let mut hp = leftover as f64 * damage;
    for kind in DefenceKind::ALL {
        if !matches!(
            cfg.get(kind).role,
            DefenceRole::Battery | DefenceRole::Missile | DefenceRole::Interceptor
        ) {
            continue;
        }
        let spec = cfg.get(kind);
        let unit = (spec.hull + spec.shield).max(1.0);
        let have = grid.get(kind);
        if have == 0 {
            continue;
        }
        let killed = ((hp / unit).floor() as u32).min(have);
        grid.saturating_sub(kind, killed);
        hp -= f64::from(killed) * unit;
        if hp <= 0.0 {
            break;
        }
    }
}

fn rebuild_lost(before: DefenceGrid, after: &mut DefenceGrid, chance: f64, rng: &mut u64) -> u32 {
    let chance = chance.clamp(0.0, 1.0);
    let mut rebuilt = 0;
    for kind in DefenceKind::ALL {
        let lost = before.get(kind).saturating_sub(after.get(kind));
        for _ in 0..lost {
            if unit_f64(rng) < chance {
                after.add(kind, 1);
                rebuilt += 1;
            }
        }
    }
    rebuilt
}

/// Fight pirates against fleet + planet defences. Caller supplies fleet combat stats.
/// Space combat always includes the parked fleet plus stations and hangar fighters.
#[allow(clippy::too_many_arguments)]
pub fn resolve_planet_battle(
    attacker: Fleet,
    fleet: Fleet,
    defences: DefenceGrid,
    atk_stats: &[ShipStats; crate::SHIP_KINDS],
    def_stats: &[ShipStats; crate::SHIP_KINDS],
    hangar_stats: ShipStats,
    cfg: &DefenceConfig,
    troop_strength: u32,
    max_rounds: u8,
    rapid_fire: &[[u32; crate::SHIP_KINDS]; crate::SHIP_KINDS],
    rng: &mut u64,
) -> PlanetBattle {
    resolve_planet_battle_protected(
        attacker,
        fleet,
        defences,
        atk_stats,
        def_stats,
        hangar_stats,
        cfg,
        troop_strength,
        max_rounds,
        rapid_fire,
        rng,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_planet_battle_protected(
    mut attacker: Fleet,
    fleet: Fleet,
    mut defences: DefenceGrid,
    atk_stats: &[ShipStats; crate::SHIP_KINDS],
    def_stats: &[ShipStats; crate::SHIP_KINDS],
    hangar_stats: ShipStats,
    cfg: &DefenceConfig,
    troop_strength: u32,
    max_rounds: u8,
    rapid_fire: &[[u32; crate::SHIP_KINDS]; crate::SHIP_KINDS],
    rng: &mut u64,
    protected: bool,
) -> PlanetBattle {
    let original = defences;
    let interceptors = defences.get(DefenceKind::Interceptor);
    let incoming = (f64::from(attacker.total()) * cfg.missile_per_pirate)
        .floor()
        .max(0.0) as u32;
    let intercept_down = intercept_missiles(incoming, interceptors, cfg.intercept_chance, rng);
    apply_missiles(
        &mut defences,
        incoming.saturating_sub(intercept_down),
        cfg.missile_damage,
        cfg,
    );

    let stations_before = defences.get(DefenceKind::OrbitalStation);
    let hangar_before = stations_before.saturating_mul(cfg.hangar_per_station);
    let mut atk = expand_fleet(attacker, atk_stats);
    let mut defn = expand_fleet(fleet, def_stats);
    let station = cfg.get(DefenceKind::OrbitalStation);
    for _ in 0..stations_before {
        defn.push(Shot {
            hull: station.hull,
            shield_max: station.shield,
            shield: station.shield,
            attack: station.attack,
            tag: TAG_STATION,
        });
    }
    for _ in 0..hangar_before {
        defn.push(Shot {
            hull: hangar_stats.hull,
            shield_max: hangar_stats.shield,
            shield: hangar_stats.shield,
            attack: hangar_stats.attack,
            tag: TAG_HANGAR,
        });
    }
    let orbital_rounds = fight(&mut atk, &mut defn, max_rounds, rapid_fire, rng);
    let fleet_after = compress_fleet(&defn);
    let hangar_after = defn
        .iter()
        .filter(|u| u.alive() && u.tag == TAG_HANGAR)
        .count() as u32;
    let stations_after = defn
        .iter()
        .filter(|u| u.alive() && u.tag == TAG_STATION)
        .count() as u32;
    defences.set(DefenceKind::OrbitalStation, stations_after);
    let orbit_held = !fleet_after.is_empty() || stations_after > 0 || hangar_after > 0;

    let mut dome_collapsed = false;
    let mut dome_absorbed = 0.0;
    let mut punched_dome = !orbit_held && !living(&atk).is_empty();
    let dome_level = defences.get(DefenceKind::ShieldDome);
    if punched_dome && dome_level > 0 {
        let spec = cfg.get(DefenceKind::ShieldDome);
        let mut hp = spec.hull * f64::from(dome_level);
        let cap = hp;
        let recharge = cfg.dome_recharge_share.clamp(0.0, 1.0) * cap;
        for _ in 0..max_rounds {
            if living(&atk).is_empty() || hp <= 0.0 {
                break;
            }
            let round_hit: f64 = atk.iter().filter(|u| u.alive()).map(|u| u.attack).sum();
            let absorbed = round_hit.min(hp);
            hp -= absorbed;
            dome_absorbed += absorbed;
            if hp <= 0.0 {
                dome_collapsed = true;
                defences.set(DefenceKind::ShieldDome, 0);
                break;
            }
            hp = (hp + recharge).min(cap);
        }
        punched_dome = dome_collapsed;
        if hp > 0.0 {
            let remain = ((hp / spec.hull).ceil() as u32).max(1);
            defences.set(DefenceKind::ShieldDome, remain.min(dome_level));
        }
    }

    let batteries_before = defences.battery_total(cfg);
    let mut batteries_after = batteries_before;
    if punched_dome && batteries_before > 0 && !living(&atk).is_empty() {
        let mut bats = Vec::new();
        for kind in DefenceKind::ALL {
            let spec = cfg.get(kind);
            if !matches!(
                spec.role,
                DefenceRole::Battery | DefenceRole::Missile | DefenceRole::Interceptor
            ) {
                continue;
            }
            for _ in 0..defences.get(kind) {
                bats.push(Shot {
                    hull: spec.hull,
                    shield_max: spec.shield,
                    shield: spec.shield,
                    attack: spec.attack,
                    tag: TAG_BATTERY + kind.index() as u8,
                });
            }
        }
        fight(&mut atk, &mut bats, max_rounds, rapid_fire, rng);
        for kind in DefenceKind::ALL {
            let spec = cfg.get(kind);
            if !matches!(
                spec.role,
                DefenceRole::Battery | DefenceRole::Missile | DefenceRole::Interceptor
            ) {
                continue;
            }
            let tag = TAG_BATTERY + kind.index() as u8;
            let n = bats.iter().filter(|u| u.alive() && u.tag == tag).count() as u32;
            defences.set(kind, n);
        }
        batteries_after = defences.battery_total(cfg);
    } else if !punched_dome {
        batteries_after = batteries_before;
    }

    attacker = compress_fleet(&atk);
    let mut ground = None;
    if !protected && punched_dome && batteries_after == 0 && !attacker.is_empty() {
        let garrison = defences.get(DefenceKind::Garrison);
        if garrison > 0 {
            let atk_str = ((f64::from(attacker.get(ShipKind::TroopTransport).max(attacker.total()))
                * f64::from(troop_strength.max(1))
                * cfg.tactical_ground_mult)
                .floor() as u32)
                .max(1);
            let def_str = ((f64::from(garrison) * cfg.tactical_ground_mult).floor() as u32).max(1);
            let report = resolve_ground(atk_str, def_str, 0.0, rng);
            if report.winner != CombatWinner::Attacker {
                attacker = Fleet::default();
            } else {
                defences.set(DefenceKind::Garrison, 0);
            }
            ground = Some(report);
        }
    }

    let (defences, rebuilt) = apply_rebuild(original, defences, cfg.rebuild_chance, rng);
    let batteries_missing = original
        .battery_total(cfg)
        .saturating_sub(defences.battery_total(cfg));
    let winner = if protected && !attacker.is_empty() {
        CombatWinner::Draw
    } else if attacker.is_empty() {
        CombatWinner::Defender
    } else if orbit_held {
        CombatWinner::Draw
    } else if !punched_dome {
        CombatWinner::Defender
    } else if stations_after > 0 || hangar_after > 0 || batteries_after > 0 {
        CombatWinner::Draw
    } else {
        CombatWinner::Attacker
    };

    PlanetBattle {
        intercept_in: incoming,
        intercept_down,
        orbital_rounds,
        stations_before,
        stations_after,
        hangar_before,
        hangar_after,
        dome_collapsed,
        dome_absorbed,
        batteries_before,
        batteries_after,
        rebuilt,
        batteries_missing,
        ground,
        winner,
        attacker_after: attacker,
        fleet_after,
        defences_after: defences,
    }
}

/// Apply rebuild chance to units lost between `before` and `after`.
pub fn apply_rebuild(
    before: DefenceGrid,
    after: DefenceGrid,
    chance: f64,
    rng: &mut u64,
) -> (DefenceGrid, u32) {
    let mut grid = after;
    let rebuilt = rebuild_lost(before, &mut grid, chance, rng);
    (grid, rebuilt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats() -> [ShipStats; crate::SHIP_KINDS] {
        crate::combat::stats_from_catalog(&crate::ShipCatalog::default())
    }

    fn rf() -> [[u32; crate::SHIP_KINDS]; crate::SHIP_KINDS] {
        crate::combat::rapid_fire_table(&crate::ShipCatalog::default())
    }

    fn lf(n: u32) -> Fleet {
        let mut f = Fleet::default();
        f.add(ShipKind::LightFighter, n);
        f
    }

    #[test]
    fn bundled_catalog_has_every_kind() {
        let cfg = DefenceConfig::default();
        for kind in DefenceKind::ALL {
            assert_eq!(cfg.get(kind).kind, kind);
        }
    }

    #[test]
    fn intercept_is_deterministic_for_a_seed() {
        let mut a = 9;
        let mut b = 9;
        let first = intercept_missiles(8, 5, 0.5, &mut a);
        let second = intercept_missiles(8, 5, 0.5, &mut b);
        assert_eq!(first, second);
        assert!(first <= 5);
    }

    #[test]
    fn dome_absorbs_then_collapses_and_stays_down() {
        let cfg = DefenceConfig {
            rebuild_chance: 0.0,
            ..DefenceConfig::default()
        };
        let mut grid = DefenceGrid::default();
        grid.add(DefenceKind::ShieldDome, 1);
        let hangar = stats()[ShipKind::LightFighter.index()];
        let catalog = stats();
        let mut rng = 1;
        let battle = resolve_planet_battle(
            lf(80),
            Fleet::default(),
            grid,
            &catalog,
            &catalog,
            hangar,
            &cfg,
            5,
            6,
            &rf(),
            &mut rng,
        );
        assert!(battle.dome_absorbed > 0.0);
        assert!(battle.dome_collapsed);
        assert_eq!(battle.defences_after.get(DefenceKind::ShieldDome), 0);
    }

    #[test]
    fn mixed_fleet_and_batteries_repeat_for_a_seed() {
        let cfg = DefenceConfig::default();
        let mut grid = DefenceGrid::default();
        grid.add(DefenceKind::LightCannon, 4);
        grid.add(DefenceKind::HeavyCannon, 2);
        let hangar = stats()[ShipKind::LightFighter.index()];
        let mut a = 3;
        let mut b = 3;
        let catalog = stats();
        let first = resolve_planet_battle(
            lf(10),
            lf(2),
            grid,
            &catalog,
            &catalog,
            hangar,
            &cfg,
            5,
            6,
            &rf(),
            &mut a,
        );
        let second = resolve_planet_battle(
            lf(10),
            lf(2),
            grid,
            &catalog,
            &catalog,
            hangar,
            &cfg,
            5,
            6,
            &rf(),
            &mut b,
        );
        assert_eq!(first, second);
    }

    #[test]
    fn rebuild_chance_restores_some_lost_units() {
        let mut before = DefenceGrid::default();
        before.add(DefenceKind::LightCannon, 10);
        let after = DefenceGrid::default();
        let mut rng = 2;
        let (grid, n) = apply_rebuild(before, after, 1.0, &mut rng);
        assert_eq!(n, 10);
        assert_eq!(grid.get(DefenceKind::LightCannon), 10);
        let mut rng = 2;
        let (_, none) = apply_rebuild(before, after, 0.0, &mut rng);
        assert_eq!(none, 0);
    }

    #[test]
    fn garrison_blocks_an_attacker_that_reaches_the_ground() {
        let cfg = DefenceConfig::default();
        let mut grid = DefenceGrid::default();
        grid.add(DefenceKind::Garrison, 50);
        let hangar = stats()[ShipKind::LightFighter.index()];
        let mut rng = 4;
        let catalog = stats();
        let battle = resolve_planet_battle(
            lf(1),
            Fleet::default(),
            grid,
            &catalog,
            &catalog,
            hangar,
            &cfg,
            1,
            6,
            &rf(),
            &mut rng,
        );
        assert_eq!(battle.winner, CombatWinner::Defender);
        assert!(battle.ground.is_some());
    }

    #[test]
    fn stations_fight_before_batteries() {
        let cfg = DefenceConfig::default();
        let mut grid = DefenceGrid::default();
        grid.add(DefenceKind::OrbitalStation, 2);
        grid.add(DefenceKind::LightCannon, 8);
        let hangar = stats()[ShipKind::LightFighter.index()];
        let mut rng = 8;
        let catalog = stats();
        let battle = resolve_planet_battle(
            lf(3),
            Fleet::default(),
            grid,
            &catalog,
            &catalog,
            hangar,
            &cfg,
            5,
            6,
            &rf(),
            &mut rng,
        );
        assert!(battle.orbital_rounds > 0);
        assert_eq!(battle.hangar_before, 4);
        assert!(battle.stations_after <= battle.stations_before);
    }

    #[test]
    fn leftover_stations_are_not_an_attacker_win() {
        let cfg = DefenceConfig {
            rebuild_chance: 0.0,
            ..DefenceConfig::default()
        };
        let mut grid = DefenceGrid::default();
        grid.add(DefenceKind::OrbitalStation, 8);
        let hangar = stats()[ShipKind::LightFighter.index()];
        let catalog = stats();
        let mut rng = 1;
        let battle = resolve_planet_battle(
            lf(1),
            Fleet::default(),
            grid,
            &catalog,
            &catalog,
            hangar,
            &cfg,
            5,
            6,
            &rf(),
            &mut rng,
        );
        assert!(battle.stations_after > 0);
        assert_ne!(battle.winner, CombatWinner::Attacker);
        assert!(!battle.dome_collapsed);
        assert_eq!(battle.batteries_after, battle.batteries_before);
    }

    #[test]
    fn leftover_batteries_skip_ground() {
        let cfg = DefenceConfig {
            rebuild_chance: 0.0,
            ..DefenceConfig::default()
        };
        let mut grid = DefenceGrid::default();
        grid.add(DefenceKind::LightCannon, 8);
        grid.add(DefenceKind::Garrison, 20);
        let hangar = stats()[ShipKind::LightFighter.index()];
        let catalog = stats();
        let mut rng = 1;
        let battle = resolve_planet_battle(
            lf(1),
            Fleet::default(),
            grid,
            &catalog,
            &catalog,
            hangar,
            &cfg,
            5,
            6,
            &rf(),
            &mut rng,
        );
        assert!(battle.batteries_after > 0);
        assert!(battle.ground.is_none());
        assert_eq!(battle.defences_after.get(DefenceKind::Garrison), 20);
        assert_ne!(battle.winner, CombatWinner::Attacker);
    }
}
