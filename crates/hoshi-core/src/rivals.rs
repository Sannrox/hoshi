//! Data-driven NPC rival empires. Off by default; the same Intent layer as the player.

use crate::buildings::{BuildJob, BuildingKind, Buildings};
use crate::combat::{next_u64, plunder_from, rapid_fire_table, resolve_combat, stats_from_catalog};
use crate::espionage::{reveals, spy_score, spy_tier};
use crate::fleets::Coord;
use crate::intel::{
    apply_amount_error, intel_confidence, light_delay_secs, light_range_au, noisy_fleet,
    sensor_error, signed_unit,
};
use crate::intent::{Intent, apply, propose};
use crate::leaders::Actor;
use crate::market::{OrderSide, TraderId};
use crate::research::ResearchJob;
use crate::ships::{Fleet, ShipJob, ShipKind};
use crate::state::{GameState, ResourceKind, Resources};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_RIVALS: &str = include_str!("../data/rivals.toml");

#[cfg(test)]
const PLAYER_ACTIONS: [&str; 6] = ["build", "research", "place_order", "launch", "ship", "idle"];

/// Catalog plus per-galaxy difficulty. Empires are empty until spawn.
#[derive(Debug, Clone, PartialEq)]
pub struct RivalBoard {
    pub enabled: bool,
    pub tick_secs: u64,
    pub aggression: f64,
    pub efficiency: f64,
    pub starting_bonus: f64,
    pub empires: Vec<RivalEmpire>,
}

impl Default for RivalBoard {
    fn default() -> Self {
        Self {
            enabled: false,
            tick_secs: 3600,
            aggression: 1.0,
            efficiency: 1.0,
            starting_bonus: 0.0,
            empires: Vec::new(),
        }
    }
}

impl RivalBoard {
    pub fn settings(&self) -> RivalSettings {
        RivalSettings {
            enabled: self.enabled,
            tick_secs: self.tick_secs,
            aggression: self.aggression,
            efficiency: self.efficiency,
            starting_bonus: self.starting_bonus,
            personalities: self
                .empires
                .iter()
                .map(|row| row.personality.clone())
                .collect(),
        }
    }

    fn act_secs(&self) -> u64 {
        let secs = self.tick_secs as f64 / self.efficiency.max(0.1);
        secs.round().clamp(1.0, 86_400.0 * 7.0) as u64
    }
}

/// Opt-in knobs for one galaxy. Defaults match `rivals.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct RivalSettings {
    pub enabled: bool,
    pub tick_secs: u64,
    pub aggression: f64,
    pub efficiency: f64,
    pub starting_bonus: f64,
    pub personalities: Vec<String>,
}

impl Default for RivalSettings {
    fn default() -> Self {
        let cat = RivalCatalog::default();
        Self {
            enabled: false,
            tick_secs: cat.tick_secs,
            aggression: cat.aggression,
            efficiency: cat.efficiency,
            starting_bonus: cat.starting_bonus,
            personalities: Vec::new(),
        }
    }
}

impl RivalSettings {
    pub fn on() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }
}

/// One rival empire. Production and queues live here; catch-up uses a pocket state.
#[derive(Debug, Clone, PartialEq)]
pub struct RivalEmpire {
    pub id: String,
    pub personality: String,
    pub home: Coord,
    pub resources: Resources,
    pub buildings: Buildings,
    pub build_queue: Vec<BuildJob>,
    pub fleet: Fleet,
    pub tech_levels: BTreeMap<String, u32>,
    pub research: Option<ResearchJob>,
    pub ship_queue: Vec<ShipJob>,
    pub last_tick: u64,
    pub next_act_at: u64,
    pub last_action: String,
}

/// Player-visible slice, gated by espionage research.
#[derive(Debug, Clone, PartialEq)]
pub struct RivalIntel {
    pub id: String,
    pub personality: String,
    pub name: String,
    pub home: Coord,
    pub last_action: String,
    pub resources: Option<Resources>,
    pub fleet: Option<Fleet>,
    pub buildings: Option<Buildings>,
    pub source: String,
    pub observed_at: u64,
    pub age_secs: u64,
    pub confidence: f64,
}

/// Sticky sample key: one draw per empire per hour of observed time.
fn rival_sample_rng(galaxy_seed: u64, observed_at: u64, row: &RivalEmpire) -> u64 {
    let bucket = observed_at / 3600;
    let mut rng =
        galaxy_seed ^ bucket ^ (u64::from(row.home.system) << 16) ^ u64::from(row.home.position);
    for b in row.id.as_bytes() {
        rng = rng
            .wrapping_mul(1_099_511_628_211)
            .wrapping_add(u64::from(*b));
    }
    rng
}

#[derive(Debug, Clone, PartialEq)]
pub struct RivalPersonality {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub build: f64,
    pub research: f64,
    pub trade: f64,
    pub raid: f64,
    pub ship: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RivalCatalog {
    pub enabled: bool,
    pub tick_secs: u64,
    pub aggression: f64,
    pub efficiency: f64,
    pub starting_bonus: f64,
    rows: Vec<RivalPersonality>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RivalError {
    Parse(String),
    Invalid(String),
    Unknown(String),
}

impl fmt::Display for RivalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RivalError::Parse(msg) | RivalError::Invalid(msg) => write!(f, "{msg}"),
            RivalError::Unknown(id) => write!(f, "unknown rival personality {id}"),
        }
    }
}

impl std::error::Error for RivalError {}

impl RivalCatalog {
    pub fn parse(toml: &str) -> Result<Self, RivalError> {
        let file: File = toml::from_str(toml).map_err(|e| RivalError::Parse(e.to_string()))?;
        file.into_catalog()
    }

    pub fn get(&self, id: &str) -> Option<&RivalPersonality> {
        self.rows.iter().find(|row| row.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &RivalPersonality> {
        self.rows.iter()
    }
}

impl Default for RivalCatalog {
    fn default() -> Self {
        Self::parse(DEFAULT_RIVALS).expect("bundled rivals.toml")
    }
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_tick")]
    tick_secs: u64,
    #[serde(default = "default_one")]
    aggression: f64,
    #[serde(default = "default_one")]
    efficiency: f64,
    #[serde(default)]
    starting_bonus: f64,
    #[serde(default)]
    personality: Vec<FilePersonality>,
}

fn default_tick() -> u64 {
    3600
}

fn default_one() -> f64 {
    1.0
}

#[derive(Deserialize)]
struct FilePersonality {
    id: String,
    name: String,
    #[serde(default)]
    blurb: String,
    #[serde(default)]
    build: f64,
    #[serde(default)]
    research: f64,
    #[serde(default)]
    trade: f64,
    #[serde(default)]
    raid: f64,
    #[serde(default)]
    ship: f64,
}

impl File {
    fn into_catalog(self) -> Result<RivalCatalog, RivalError> {
        if self.tick_secs == 0 {
            return Err(RivalError::Invalid("tick_secs must be > 0".into()));
        }
        if self.personality.len() < 3 {
            return Err(RivalError::Invalid(
                "rivals.toml needs at least three personalities".into(),
            ));
        }
        let mut seen = BTreeMap::new();
        let mut rows = Vec::new();
        for row in self.personality {
            if row.id.is_empty() {
                return Err(RivalError::Invalid("personality id is empty".into()));
            }
            if !row.build.is_finite()
                || !row.research.is_finite()
                || !row.trade.is_finite()
                || !row.raid.is_finite()
                || !row.ship.is_finite()
            {
                return Err(RivalError::Invalid(format!(
                    "personality {} has a non-finite weight",
                    row.id
                )));
            }
            if seen.insert(row.id.clone(), ()).is_some() {
                return Err(RivalError::Invalid(format!(
                    "duplicate personality {}",
                    row.id
                )));
            }
            rows.push(RivalPersonality {
                id: row.id,
                name: row.name,
                blurb: row.blurb,
                build: row.build.max(0.0),
                research: row.research.max(0.0),
                trade: row.trade.max(0.0),
                raid: row.raid.max(0.0),
                ship: row.ship.max(0.0),
            });
        }
        Ok(RivalCatalog {
            enabled: self.enabled,
            tick_secs: self.tick_secs,
            aggression: self.aggression.max(0.0),
            efficiency: self.efficiency.max(0.1),
            starting_bonus: self.starting_bonus.max(0.0),
            rows,
        })
    }
}

impl GameState {
    /// Turn rivals on for this galaxy. No-op when `settings.enabled` is false.
    pub fn spawn_rivals(
        &mut self,
        catalog: &RivalCatalog,
        settings: RivalSettings,
    ) -> Result<(), RivalError> {
        if !settings.enabled {
            self.rivals = RivalBoard::default();
            return Ok(());
        }
        validate_settings(&settings)?;
        let ids = if settings.personalities.is_empty() {
            catalog.iter().map(|row| row.id.clone()).collect::<Vec<_>>()
        } else {
            settings.personalities.clone()
        };
        let mut empires = Vec::new();
        let tick = RivalBoard {
            enabled: true,
            tick_secs: settings.tick_secs.max(1),
            aggression: settings.aggression.max(0.0),
            efficiency: settings.efficiency.max(0.1),
            starting_bonus: settings.starting_bonus.max(0.0),
            empires: Vec::new(),
        }
        .act_secs();
        let bonus = 1.0 + settings.starting_bonus.max(0.0);
        for (i, id) in ids.iter().enumerate() {
            let spec = catalog
                .get(id)
                .ok_or_else(|| RivalError::Unknown(id.clone()))?;
            let home = rival_home(self.galaxy_seed, i, self.home, self.pirate_home);
            let mut resources = self.config.starting_resources;
            resources.iron *= bonus;
            resources.silicon *= bonus;
            resources.deuterium *= bonus;
            let mut fleet = Fleet::default();
            if spec.raid > spec.build {
                fleet.add(ShipKind::LightFighter, 4);
            }
            empires.push(RivalEmpire {
                id: format!("rival-{i}-{id}"),
                personality: spec.id.clone(),
                home,
                resources,
                buildings: self.config.starting_buildings,
                build_queue: Vec::new(),
                fleet,
                tech_levels: BTreeMap::new(),
                research: None,
                ship_queue: Vec::new(),
                last_tick: self.last_tick_unix,
                next_act_at: self.last_tick_unix.saturating_add(tick),
                last_action: "idle".into(),
            });
        }
        self.rivals = RivalBoard {
            enabled: true,
            tick_secs: settings.tick_secs.max(1),
            aggression: settings.aggression.max(0.0),
            efficiency: settings.efficiency.max(0.1),
            starting_bonus: settings.starting_bonus.max(0.0),
            empires,
        };
        Ok(())
    }

    pub(crate) fn next_rivals_at(&self) -> Option<u64> {
        if !self.rivals.enabled || self.rivals.empires.is_empty() {
            return None;
        }
        self.rivals
            .empires
            .iter()
            .map(|row| row.next_act_at)
            .min()
            .filter(|&at| at > 0)
    }

    pub(crate) fn tick_rivals(&mut self, at: u64) {
        if !self.rivals.enabled {
            return;
        }
        let n = self.rivals.empires.len();
        for i in 0..n {
            if self.rivals.empires[i].next_act_at == at {
                self.act_rival(i, at);
            }
        }
    }

    /// Espionage-gated glance used by the intel panel and `hoshi rivals`.
    pub fn rival_intel(&self) -> Vec<RivalIntel> {
        let catalog = RivalCatalog::default();
        let own = f64::from(self.tech_level("espionage"));
        let score = spy_score(1, own, &self.config.espionage);
        let tier = spy_tier(score, &self.config.espionage)
            .map(|t| t.id.as_str())
            .unwrap_or("activity");
        let now = self.last_tick_unix;
        let quality = 1.0 + own;
        self.rivals
            .empires
            .iter()
            .map(|row| {
                let name = catalog
                    .get(&row.personality)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| row.personality.clone());
                let range = light_range_au(self.home, row.home, &self.config.orbits);
                let delay = light_delay_secs(self.home, row.home, &self.config.orbits);
                let error = sensor_error(range, quality, &self.config.espionage);
                let observed_at = now.saturating_sub(delay);
                let confidence = intel_confidence(error, delay, false, &self.config.espionage);
                let mut rng = rival_sample_rng(self.galaxy_seed, observed_at, row);
                let resources = reveals(tier, "resources", &self.config.espionage).then(|| {
                    let r = row.resources;
                    Resources {
                        iron: apply_amount_error(r.iron, error, signed_unit(&mut rng), false, 0.0),
                        silicon: apply_amount_error(
                            r.silicon,
                            error,
                            signed_unit(&mut rng),
                            false,
                            0.0,
                        ),
                        rare_earths: apply_amount_error(
                            r.rare_earths,
                            error,
                            signed_unit(&mut rng),
                            false,
                            0.0,
                        ),
                        water_ice: apply_amount_error(
                            r.water_ice,
                            error,
                            signed_unit(&mut rng),
                            false,
                            0.0,
                        ),
                        helium_3: apply_amount_error(
                            r.helium_3,
                            error,
                            signed_unit(&mut rng),
                            false,
                            0.0,
                        ),
                        deuterium: apply_amount_error(
                            r.deuterium,
                            error,
                            signed_unit(&mut rng),
                            false,
                            0.0,
                        ),
                    }
                });
                let fleet = reveals(tier, "fleet", &self.config.espionage)
                    .then(|| noisy_fleet(row.fleet, error, false, 0.0, &mut rng));
                RivalIntel {
                    id: row.id.clone(),
                    personality: row.personality.clone(),
                    name,
                    home: row.home,
                    last_action: row.last_action.clone(),
                    resources,
                    fleet,
                    buildings: reveals(tier, "defences", &self.config.espionage)
                        .then_some(row.buildings),
                    source: "sensor".into(),
                    observed_at,
                    age_secs: delay,
                    confidence,
                }
            })
            .collect()
    }

    fn act_rival(&mut self, index: usize, at: u64) {
        let catalog = RivalCatalog::default();
        let personality = self.rivals.empires[index].personality.clone();
        let Some(spec) = catalog.get(&personality).cloned() else {
            self.rivals.empires[index].next_act_at = at.saturating_add(self.rivals.act_secs());
            return;
        };
        let mut pocket = self.make_pocket(index);
        pocket.catch_up(at);
        let chosen = choose_action(&pocket, &spec, self.rivals.aggression, index, at);
        let action = match &chosen {
            Act::Intent(intent) => match propose(&pocket, intent, at) {
                Ok(proposal) => {
                    let key = intent.as_key().to_string();
                    let _ = apply(&mut pocket, &proposal, at);
                    key
                }
                Err(_) => "idle".into(),
            },
            Act::Ship => {
                if pocket.enqueue_ships(ShipKind::LightFighter, 1, at).is_ok() {
                    "ship".into()
                } else {
                    "idle".into()
                }
            }
            Act::Raid => "launch".into(),
            Act::Idle => "idle".into(),
        };
        if matches!(chosen, Act::Intent(Intent::PlaceOrder { .. })) {
            retag_new_orders(&mut pocket, self.board.next_order_id);
        }
        let raid = matches!(chosen, Act::Raid);
        self.merge_pocket(index, pocket, at, action);
        if raid {
            self.resolve_rival_raid(index, at);
        }
    }

    fn make_pocket(&self, index: usize) -> GameState {
        let rival = &self.rivals.empires[index];
        let mut pocket = self.clone();
        pocket.rivals = RivalBoard::default();
        pocket.resources = rival.resources;
        pocket.buildings = rival.buildings;
        pocket.build_queue = rival.build_queue.clone();
        pocket.fleet = rival.fleet;
        pocket.tech_levels = rival.tech_levels.clone();
        pocket.research = rival.research.clone();
        pocket.research_queue.clear();
        pocket.ship_queue = rival.ship_queue.clone();
        pocket.home = rival.home;
        pocket.colonies.clear();
        pocket.missions.clear();
        pocket.routes.clear();
        pocket.next_raid_at = None;
        pocket.config.raid.enabled = false;
        pocket.wonder = None;
        pocket.pending_tactical = None;
        pocket.planned.clear();
        pocket.species = None;
        pocket.history = crate::HistoryLog::default();
        pocket.events.clear();
        pocket.last_tick_unix = rival.last_tick;
        pocket
    }

    fn merge_pocket(&mut self, index: usize, pocket: GameState, at: u64, action: String) {
        let next_act = at.saturating_add(self.rivals.act_secs());
        let empire = &mut self.rivals.empires[index];
        empire.resources = pocket.resources;
        empire.buildings = pocket.buildings;
        empire.build_queue = pocket.build_queue;
        empire.fleet = pocket.fleet;
        empire.tech_levels = pocket.tech_levels;
        empire.research = pocket.research;
        empire.ship_queue = pocket.ship_queue;
        empire.last_tick = pocket.last_tick_unix;
        empire.next_act_at = next_act;
        empire.last_action = action;
        self.market = pocket.market;
        self.board = pocket.board;
    }

    fn resolve_rival_raid(&mut self, index: usize, at: u64) {
        let n = self.rivals.empires[index]
            .fleet
            .get(ShipKind::LightFighter)
            .min(8);
        if n == 0 {
            self.rivals.empires[index].last_action = "idle".into();
            return;
        }
        let mut wave = Fleet::default();
        wave.add(ShipKind::LightFighter, n);
        self.rivals.empires[index]
            .fleet
            .saturating_sub(ShipKind::LightFighter, n);
        let mut atk = stats_from_catalog(&self.config.ships);
        if self.cheats.god_mode {
            for stats in &mut atk {
                stats.attack = 0.0;
            }
        }
        let def = self.player_combat_stats();
        let rf = rapid_fire_table(&self.config.ships);
        let mut rng = self
            .galaxy_seed
            .wrapping_add(self.rivals.empires[index].id.len() as u64)
            .wrapping_add(at);
        let mut report = resolve_combat(
            wave,
            self.fleet,
            &atk,
            &def,
            self.config.raid.max_rounds,
            &rf,
            &mut rng,
        );
        if self.cheats.god_mode && report.winner == crate::CombatWinner::Attacker {
            report.winner = crate::CombatWinner::Draw;
        }
        self.rivals.empires[index].fleet.add(
            ShipKind::LightFighter,
            report.attacker_after.get(ShipKind::LightFighter),
        );
        self.fleet = report.defender_after;
        if report.winner == crate::CombatWinner::Attacker {
            let loot = plunder_from(
                self.plunderable(),
                self.config.raid.plunder_share,
                self.config.raid.plunder_cap,
            );
            self.resources = self.resources.saturating_sub(loot);
            self.rivals.empires[index].resources =
                self.rivals.empires[index].resources.saturating_add(loot);
            self.push_event(
                at,
                format!(
                    "rival {}: raid won, took {:.0} iron",
                    self.rivals.empires[index].id, loot.iron
                ),
            );
        } else {
            self.push_event(
                at,
                format!("rival {}: raid beaten", self.rivals.empires[index].id),
            );
        }
        self.rivals.empires[index].last_action = "launch".into();
    }
}

fn retag_new_orders(pocket: &mut GameState, cutoff: u64) {
    for order in pocket.board.orders.values_mut() {
        if order.id >= cutoff && order.trader == TraderId::Player {
            order.trader = TraderId::Npc;
        }
    }
}

#[derive(Clone)]
enum Act {
    Intent(Intent),
    Ship,
    Raid,
    Idle,
}

fn choose_action(
    pocket: &GameState,
    spec: &RivalPersonality,
    aggression: f64,
    index: usize,
    at: u64,
) -> Act {
    let mut best: Option<(f64, Act)> = None;
    let consider = |best: &mut Option<(f64, Act)>, weight: f64, tag: u64, act: Act| {
        if weight <= 0.0 {
            return;
        }
        let score = weight * mix(pocket.galaxy_seed, index, at, tag);
        match best {
            Some((cur, _)) if *cur >= score => {}
            _ => *best = Some((score, act)),
        }
    };
    if spec.build > 0.0 {
        let kind = pick_building(pocket, spec);
        let intent = Intent::Build {
            planet: pocket.home,
            building: kind,
            actor: Actor::Player,
        };
        if propose(pocket, &intent, at).is_ok() {
            consider(&mut best, spec.build, 1, Act::Intent(intent));
        }
    }
    if spec.research > 0.0
        && let Some(tech) = pick_research(pocket, at)
    {
        let intent = Intent::Research { tech };
        consider(&mut best, spec.research, 2, Act::Intent(intent));
    }
    if spec.trade > 0.0 && pocket.config.market.enabled {
        let amount = (pocket.resources.iron * 0.1).clamp(1.0, pocket.config.market.npc_order_size);
        if amount + 1e-9 <= pocket.resources.iron {
            let intent = Intent::PlaceOrder {
                region: Coord {
                    galaxy: pocket.config.market.hub_galaxy,
                    system: pocket.config.market.hub_system,
                    position: pocket.config.market.hub_position,
                },
                side: OrderSide::Sell,
                good: ResourceKind::Iron,
                amount,
                limit: None,
                ttl_secs: pocket.config.market.order_ttl_secs,
            };
            if propose(pocket, &intent, at).is_ok() {
                consider(&mut best, spec.trade, 3, Act::Intent(intent));
            }
        }
    }
    if spec.ship > 0.0 {
        let mut trial = pocket.clone();
        if trial.enqueue_ships(ShipKind::LightFighter, 1, at).is_ok() {
            consider(&mut best, spec.ship, 4, Act::Ship);
        }
    }
    let raid_w = spec.raid * aggression.max(0.0);
    if raid_w > 0.0 && pocket.fleet.get(ShipKind::LightFighter) > 0 {
        consider(&mut best, raid_w, 5, Act::Raid);
    }
    best.map(|(_, act)| act).unwrap_or(Act::Idle)
}

fn pick_building(pocket: &GameState, spec: &RivalPersonality) -> BuildingKind {
    if pocket.energy_balance() < 0.0 {
        return BuildingKind::SolarPlant;
    }
    if spec.raid > spec.build && pocket.buildings.shipyard < 3 {
        return BuildingKind::Shipyard;
    }
    if spec.ship > spec.raid && pocket.buildings.shipyard < 2 {
        return BuildingKind::Shipyard;
    }
    let turtle = spec.build >= spec.raid && spec.trade < spec.build;
    if turtle {
        let storage = [
            BuildingKind::MetalStorage,
            BuildingKind::CrystalStorage,
            BuildingKind::DeuteriumStorage,
        ];
        return *storage
            .iter()
            .min_by_key(|kind| pocket.buildings.get(**kind))
            .unwrap_or(&BuildingKind::MetalStorage);
    }
    let mines = [
        BuildingKind::MetalMine,
        BuildingKind::CrystalMine,
        BuildingKind::SolarPlant,
    ];
    *mines
        .iter()
        .min_by_key(|kind| pocket.buildings.get(**kind))
        .unwrap_or(&BuildingKind::MetalMine)
}

/// Rivals research one tech at a time: a busy lab picks nothing rather than
/// paying into the player-facing queue.
fn pick_research(pocket: &GameState, at: u64) -> Option<String> {
    if pocket.research.is_some() {
        return None;
    }
    for spec in pocket.config.techs.iter() {
        let intent = Intent::Research {
            tech: spec.id.clone(),
        };
        if propose(pocket, &intent, at).is_ok() {
            return Some(spec.id.clone());
        }
    }
    None
}

fn mix(seed: u64, index: usize, at: u64, tag: u64) -> f64 {
    let mut rng = seed
        .wrapping_add((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(at)
        .wrapping_add(tag.wrapping_mul(0xD1B5_4A32_D192_ED03));
    (next_u64(&mut rng) as f64) / ((u64::MAX as f64) + 1.0)
}

fn validate_settings(settings: &RivalSettings) -> Result<(), RivalError> {
    let finite_range = |name: &str, value: f64, min: f64, max: f64| {
        if !value.is_finite() || !(min..=max).contains(&value) {
            Err(RivalError::Invalid(format!(
                "{name} must be a finite number in {min}..={max}"
            )))
        } else {
            Ok(())
        }
    };
    finite_range("aggression", settings.aggression, 0.0, 10.0)?;
    finite_range("efficiency", settings.efficiency, 0.0, 10.0)?;
    finite_range("starting_bonus", settings.starting_bonus, 0.0, 10.0)?;
    if settings.tick_secs == 0 {
        return Err(RivalError::Invalid("tick_secs must be > 0".into()));
    }
    Ok(())
}

fn rival_home(seed: u64, index: usize, player: Coord, pirate: Coord) -> Coord {
    let mut system = 2u16.saturating_add(index as u16);
    loop {
        let coord = Coord {
            galaxy: 1,
            system,
            position: 8 + ((seed.wrapping_add(index as u64) % 6) as u16),
        };
        if coord != player && coord != pirate {
            return coord;
        }
        system = system.saturating_add(1);
        if system > 80 {
            return Coord {
                galaxy: 2,
                system: 1,
                position: 8,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;

    fn quiet(now: u64) -> GameState {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        GameState::with_seed(now, cfg, 42)
    }

    fn spawn_on(mut state: GameState, tick: u64) -> GameState {
        let mut settings = RivalSettings::on();
        settings.tick_secs = tick;
        state
            .spawn_rivals(&RivalCatalog::default(), settings)
            .unwrap();
        state
    }

    #[test]
    fn catalog_ships_three_named_personalities() {
        let cat = RivalCatalog::default();
        assert!(!cat.enabled);
        assert!(cat.get("trader").is_some());
        assert!(cat.get("turtle").is_some());
        assert!(cat.get("raider").is_some());
        assert_eq!(cat.iter().count(), 3);
    }

    #[test]
    fn disabled_by_default_and_catch_up_is_a_noop() {
        let mut state = quiet(1_000);
        assert!(!state.rivals.enabled);
        assert!(state.rivals.empires.is_empty());
        state.catch_up(1_000 + 86_400);
        assert!(state.rivals.empires.is_empty());
    }

    #[test]
    fn rival_raids_do_not_advance_the_player_rng() {
        let mut state = spawn_on(quiet(1_000), 20);
        let before = state.rng_seed;
        state.catch_up(1_000 + 20 * 12);
        assert_eq!(state.rng_seed, before);
    }

    #[test]
    fn same_seed_repeats_actions() {
        let a = spawn_on(quiet(1_000), 60);
        let b = spawn_on(quiet(1_000), 60);
        let mut a = a;
        let mut b = b;
        a.catch_up(1_000 + 60 * 5);
        b.catch_up(1_000 + 60 * 5);
        let left: Vec<_> = a
            .rivals
            .empires
            .iter()
            .map(|e| {
                (
                    e.last_action.clone(),
                    e.buildings.metal_mine,
                    e.resources.iron,
                )
            })
            .collect();
        let right: Vec<_> = b
            .rivals
            .empires
            .iter()
            .map(|e| {
                (
                    e.last_action.clone(),
                    e.buildings.metal_mine,
                    e.resources.iron,
                )
            })
            .collect();
        assert_eq!(left, right);
    }

    #[test]
    fn long_gap_matches_step_ticks() {
        let mut bulk = spawn_on(quiet(1_000), 30);
        let mut step = spawn_on(quiet(1_000), 30);
        bulk.catch_up(1_000 + 30 * 8);
        for i in 1..=8 {
            step.catch_up(1_000 + 30 * i);
        }
        assert_eq!(bulk.rivals, step.rivals);
        assert_eq!(bulk.resources.iron, step.resources.iron);
    }

    #[test]
    fn actions_stay_on_the_player_layer() {
        let mut state = spawn_on(quiet(1_000), 20);
        state.catch_up(1_000 + 20 * 12);
        for empire in &state.rivals.empires {
            assert!(
                PLAYER_ACTIONS.contains(&empire.last_action.as_str()),
                "unexpected action {}",
                empire.last_action
            );
        }
    }

    #[test]
    fn intel_hides_stockpiles_without_espionage() {
        let mut state = spawn_on(quiet(1_000), 60);
        let intel = state.rival_intel();
        assert_eq!(intel.len(), 3);
        assert!(intel.iter().all(|row| row.resources.is_none()));
        assert!(intel.iter().all(|row| row.fleet.is_none()));
        state.tech_levels.insert("espionage".into(), 8);
        let intel = state.rival_intel();
        assert!(intel.iter().any(|row| row.resources.is_some()));
    }

    #[test]
    fn rival_sensor_noise_is_stable_inside_an_hour() {
        let mut state = spawn_on(quiet(1_000_000), 60);
        state.tech_levels.insert("espionage".into(), 8);
        let first = state.rival_intel();
        state.last_tick_unix += 1;
        let second = state.rival_intel();
        assert_eq!(first[0].resources, second[0].resources);
        assert_eq!(first[0].fleet, second[0].fleet);
        state.last_tick_unix += 3600;
        let third = state.rival_intel();
        assert_ne!(first[0].resources, third[0].resources);
    }

    #[test]
    fn duplicate_personality_ids_stay_distinct() {
        let mut state = quiet(1_000);
        let mut settings = RivalSettings::on();
        settings.tick_secs = 60;
        settings.personalities = vec!["trader".into(), "trader".into()];
        state
            .spawn_rivals(&RivalCatalog::default(), settings)
            .unwrap();
        assert_eq!(state.rivals.empires.len(), 2);
        assert_ne!(state.rivals.empires[0].id, state.rivals.empires[1].id);
        let mut conn = crate::open_save(std::path::Path::new(":memory:")).unwrap();
        crate::save_state(&mut conn, &state).unwrap();
        let loaded = crate::load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.rivals.empires.len(), 2);
        assert_ne!(loaded.rivals.empires[0].id, loaded.rivals.empires[1].id);
    }

    #[test]
    fn spawn_rejects_non_finite_difficulty() {
        let mut state = quiet(1_000);
        let mut settings = RivalSettings::on();
        settings.starting_bonus = f64::INFINITY;
        let err = state
            .spawn_rivals(&RivalCatalog::default(), settings)
            .unwrap_err();
        assert!(matches!(err, RivalError::Invalid(_)));
    }

    #[test]
    fn rival_pocket_never_inherits_the_player_research_queue() {
        let mut state = quiet(1_000);
        state.buildings.research_lab = 1;
        state.resources = crate::Resources::from_parts(50_000.0, 50_000.0, 50_000.0);
        state.queue_research("energy", 1_000).unwrap();
        state.queue_research("energy", 1_000).unwrap();
        let state = spawn_on(state, 60);
        assert_eq!(state.research_queue.len(), 1);
        assert!(state.make_pocket(0).research_queue.is_empty());
    }

    #[test]
    fn player_intent_still_refuses_unknown_tech() {
        let state = spawn_on(quiet(1_000), 60);
        let err = propose(
            &state,
            &Intent::Research {
                tech: "not-a-tech".into(),
            },
            1_000,
        )
        .unwrap_err();
        assert!(matches!(err, crate::IntentError::Research(_)));
    }
}
