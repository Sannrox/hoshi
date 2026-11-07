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
const PLAYER_ACTIONS: [&str; 7] = [
    "build",
    "research",
    "place_order",
    "launch",
    "ship",
    "colonize",
    "idle",
];

/// Catalog plus per-galaxy difficulty. Empires are empty until spawn.
#[derive(Debug, Clone, PartialEq)]
pub struct RivalBoard {
    pub enabled: bool,
    pub tick_secs: u64,
    pub aggression: f64,
    pub efficiency: f64,
    pub starting_bonus: f64,
    pub empires: Vec<RivalEmpire>,
    /// Physical claims, including those whose light has not reached the player.
    pub colonies: Vec<RivalColony>,
    /// Hidden outbound colony ships. Launch is never a player mission.
    pub flights: Vec<RivalColonizeFlight>,
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
            colonies: Vec::new(),
            flights: Vec::new(),
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

/// A rival-owned slot in the player's home system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RivalColony {
    pub rival_id: String,
    pub coord: Coord,
    pub founded_at: u64,
    /// Arrival plus light-time from the rival home. The map names the owner then.
    pub visible_at: u64,
}

/// One hidden rival colony ship. Not a player mission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RivalColonizeFlight {
    pub rival_id: String,
    pub dest: Coord,
    pub origin: Coord,
    pub launched_at: u64,
    pub arrives_at: u64,
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
            colonies: Vec::new(),
            flights: Vec::new(),
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
            fleet.add(ShipKind::ColonyShip, 1);
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
            colonies: Vec::new(),
            flights: Vec::new(),
        };
        Ok(())
    }

    pub(crate) fn next_rivals_at(&self) -> Option<u64> {
        if !self.rivals.enabled {
            return None;
        }
        let act = self
            .rivals
            .empires
            .iter()
            .map(|row| row.next_act_at)
            .min()
            .filter(|&at| at > 0);
        let flight = self
            .rivals
            .flights
            .iter()
            .map(|row| row.arrives_at)
            .min()
            .filter(|&at| at > 0);
        match (act, flight) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }
    }

    pub(crate) fn tick_rivals(&mut self, at: u64) {
        if !self.rivals.enabled {
            return;
        }
        self.settle_rival_colonies(at);
        let n = self.rivals.empires.len();
        for i in 0..n {
            if self.rivals.empires[i].next_act_at == at {
                self.act_rival(i, at);
            }
        }
    }

    /// Physical claim, including one whose light has not arrived.
    pub fn rival_colony_at(&self, coord: Coord) -> Option<&RivalColony> {
        self.rivals.colonies.iter().find(|row| row.coord == coord)
    }

    /// Claim the star map and continue screen may name.
    pub fn visible_rival_colony_at(&self, coord: Coord) -> Option<&RivalColony> {
        self.rival_colony_at(coord)
            .filter(|row| row.visible_at <= self.last_tick_unix)
    }

    /// Empty home-system slots a rival Colonize may pick.
    pub fn rival_colonize_dest(&self, index: usize) -> Option<Coord> {
        pick_colonize_dest(self, index)
    }

    /// Spend that rival's colony ship on the seeded dest. `None` if it cannot fly.
    #[cfg(test)]
    pub(crate) fn launch_rival_colonize(&mut self, index: usize, at: u64) -> Option<Coord> {
        let dest = pick_colonize_dest(self, index)?;
        let mut ships = Fleet::default();
        ships.add(ShipKind::ColonyShip, 1);
        let (outbound_secs, fuel) = self.flight_from(self.home, dest, ships, 100).ok()?;
        let empire = self.rivals.empires.get_mut(index)?;
        if empire.fleet.get(ShipKind::ColonyShip) == 0 || empire.resources.deuterium + 1e-9 < fuel {
            return None;
        }
        empire.fleet.saturating_sub(ShipKind::ColonyShip, 1);
        empire.resources.deuterium = (empire.resources.deuterium - fuel).max(0.0);
        let origin = empire.home;
        let rival_id = empire.id.clone();
        self.rivals.flights.push(RivalColonizeFlight {
            rival_id,
            dest,
            origin,
            launched_at: at,
            arrives_at: at.saturating_add(outbound_secs),
        });
        Some(dest)
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
        let chosen = choose_action(self, &pocket, &spec, self.rivals.aggression, index, at);
        let colonize = match &chosen {
            Act::Colonize {
                dest,
                outbound_secs,
                fuel,
            } => Some((*dest, *outbound_secs, *fuel)),
            _ => None,
        };
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
            Act::Colonize { fuel, .. } => {
                if pocket.fleet.get(ShipKind::ColonyShip) > 0
                    && pocket.resources.deuterium + 1e-9 >= *fuel
                {
                    pocket.fleet.saturating_sub(ShipKind::ColonyShip, 1);
                    pocket.resources.deuterium = (pocket.resources.deuterium - *fuel).max(0.0);
                    "colonize".into()
                } else {
                    "idle".into()
                }
            }
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
        if self.rivals.empires[index].last_action == "colonize"
            && let Some((dest, outbound_secs, _)) = colonize
        {
            let origin = self.rivals.empires[index].home;
            let rival_id = self.rivals.empires[index].id.clone();
            self.rivals.flights.push(RivalColonizeFlight {
                rival_id,
                dest,
                origin,
                launched_at: at,
                arrives_at: at.saturating_add(outbound_secs),
            });
        }
    }

    fn settle_rival_colonies(&mut self, at: u64) {
        let due: Vec<RivalColonizeFlight> = self
            .rivals
            .flights
            .iter()
            .filter(|flight| flight.arrives_at == at)
            .cloned()
            .collect();
        self.rivals.flights.retain(|flight| flight.arrives_at != at);
        for flight in due {
            if self.occupied(flight.dest) {
                if let Some(empire) = self
                    .rivals
                    .empires
                    .iter_mut()
                    .find(|row| row.id == flight.rival_id)
                {
                    empire.fleet.add(ShipKind::ColonyShip, 1);
                }
                continue;
            }
            let delay = light_delay_secs(flight.origin, flight.dest, &self.config.orbits);
            let visible_at = at.saturating_add(delay);
            self.rivals.colonies.push(RivalColony {
                rival_id: flight.rival_id.clone(),
                coord: flight.dest,
                founded_at: at,
                visible_at,
            });
            self.push_event(
                visible_at,
                format!("rival {} claimed {}", flight.rival_id, flight.dest),
            );
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
        pocket.set_raid_schedule(None);
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
    Colonize {
        dest: Coord,
        outbound_secs: u64,
        fuel: f64,
    },
    Idle,
}

fn choose_action(
    live: &GameState,
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
    if pocket.fleet.get(ShipKind::ColonyShip) > 0
        && let Some(dest) = pick_colonize_dest(live, index)
    {
        let mut ships = Fleet::default();
        ships.add(ShipKind::ColonyShip, 1);
        if let Ok((outbound_secs, fuel)) = live.flight_from(live.home, dest, ships, 100)
            && pocket.resources.deuterium + 1e-9 >= fuel
        {
            consider(
                &mut best,
                0.4,
                6,
                Act::Colonize {
                    dest,
                    outbound_secs,
                    fuel,
                },
            );
        }
    }
    best.map(|(_, act)| act).unwrap_or(Act::Idle)
}

fn pick_colonize_dest(state: &GameState, index: usize) -> Option<Coord> {
    let mut candidates = rival_colonize_candidates(state);
    if candidates.is_empty() {
        return None;
    }
    candidates.sort();
    let mut rng = state
        .galaxy_seed
        .wrapping_add((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    for coord in &candidates {
        rng ^= u64::from(coord.galaxy).wrapping_shl(32)
            ^ u64::from(coord.system).wrapping_shl(16)
            ^ u64::from(coord.position);
    }
    for (id, standing) in &state.diplomacy.standing {
        rng = rng
            .wrapping_mul(1_099_511_628_211)
            .wrapping_add(standing.to_bits());
        for b in id.as_bytes() {
            rng = rng
                .wrapping_mul(1_099_511_628_211)
                .wrapping_add(u64::from(*b));
        }
    }
    let pick = (next_u64(&mut rng) as usize) % candidates.len();
    Some(candidates[pick])
}

fn rival_colonize_candidates(state: &GameState) -> Vec<Coord> {
    let home = state.home;
    let last = state.config.expedition.dest_position.max(1);
    (1..=last)
        .map(|position| Coord {
            galaxy: home.galaxy,
            system: home.system,
            position,
        })
        .filter(|coord| {
            *coord != home
                && !state.colonies.iter().any(|row| row.coord == *coord)
                && !state.outposts.iter().any(|row| row.coord == *coord)
                && !state
                    .sites
                    .iter()
                    .any(|site| site.coord == *coord && site.outcome.is_none())
                && !state.rivals.colonies.iter().any(|row| row.coord == *coord)
                && !state.rivals.flights.iter().any(|row| row.dest == *coord)
        })
        .collect()
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

    fn freeze_rival_acts(state: &mut GameState) {
        for empire in &mut state.rivals.empires {
            empire.next_act_at = u64::MAX;
        }
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

    #[test]
    fn same_seed_and_standing_pick_the_same_colonize_dest() {
        let a = spawn_on(quiet(1_000), 60);
        let b = spawn_on(quiet(1_000), 60);
        assert_eq!(a.rival_colonize_dest(0), b.rival_colonize_dest(0));
        assert!(a.rival_colonize_dest(0).is_some());
        let mut same_standing = spawn_on(quiet(1_000), 60);
        same_standing
            .diplomacy
            .set_standing("traders", a.diplomacy.standing("traders"));
        assert_eq!(
            a.rival_colonize_dest(0),
            same_standing.rival_colonize_dest(0)
        );
    }

    #[test]
    fn an_open_expedition_site_is_not_a_colonize_dest() {
        let mut state = spawn_on(quiet(1_000), 60);
        let dest = state.rival_colonize_dest(0).expect("empty home slot");
        state.sites.push(crate::ExpeditionSite {
            id: 1,
            coord: dest,
            kind: crate::SiteKind::Wreck,
            iron: 0.0,
            silicon: 0.0,
            deuterium: 0.0,
            ships: 0,
            outcome: None,
        });
        assert_ne!(state.rival_colonize_dest(0), Some(dest));
        state
            .sites
            .iter_mut()
            .find(|site| site.coord == dest)
            .expect("pushed site")
            .outcome = Some(crate::ExpeditionKind::Nothing);
        assert_eq!(state.rival_colonize_dest(0), Some(dest));
    }

    #[test]
    fn rival_colonize_arrival_matches_stepped_ticks_and_waits_for_light() {
        let mut gap = spawn_on(quiet(1_000), 60);
        gap.rivals.empires[0].resources.deuterium = 50_000.0;
        let dest = gap.launch_rival_colonize(0, 1_000).expect("colony ship");
        freeze_rival_acts(&mut gap);
        let arrives = gap.rivals.flights[0].arrives_at;
        let origin = gap.rivals.empires[0].home;
        let delay = light_delay_secs(origin, dest, &gap.config.orbits);
        let visible = arrives.saturating_add(delay);
        assert!(visible >= arrives);
        let mut step = gap.clone();
        gap.catch_up(visible);
        // catch_up already jumps to event times; a 1s loop hangs for ~40 AU of light.
        step.catch_up(arrives);
        assert_eq!(step.rivals.colonies.len(), 1);
        assert!(step.visible_rival_colony_at(dest).is_none());
        step.catch_up(visible);
        assert_eq!(gap.rivals.colonies, step.rivals.colonies);
        assert_eq!(gap.events, step.events);
        assert_eq!(gap.rivals.colonies[0].coord, dest);
        assert_eq!(gap.rivals.colonies[0].visible_at, visible);
        assert!(
            gap.delivered_events()
                .any(|event| event.text.contains("claimed")
                    && event.text.contains(&dest.to_string())),
            "{:?}",
            gap.events
        );
        let mut early = spawn_on(quiet(1_000), 60);
        early.rivals.empires[0].resources.deuterium = 50_000.0;
        early.launch_rival_colonize(0, 1_000);
        freeze_rival_acts(&mut early);
        early.catch_up(arrives);
        assert_eq!(early.rivals.colonies.len(), 1);
        assert!(early.visible_rival_colony_at(dest).is_none());
        assert!(
            early
                .delivered_events()
                .all(|event| !event.text.contains("claimed"))
        );
        early.catch_up(visible);
        assert!(early.visible_rival_colony_at(dest).is_some());
    }

    #[test]
    fn player_colonize_of_a_rival_claim_fails_and_another_slot_still_works() {
        let mut state = spawn_on(quiet(1_000), 60);
        state.rivals.empires[0].resources.deuterium = 50_000.0;
        state.resources.deuterium = 50_000.0;
        state.tech_levels.insert("colonization".into(), 1);
        state.fleet.add(ShipKind::ColonyShip, 2);
        let dest = state.launch_rival_colonize(0, 1_000).expect("colony ship");
        freeze_rival_acts(&mut state);
        let arrives = state.rivals.flights[0].arrives_at;
        let origin = state.rivals.empires[0].home;
        let visible = arrives.saturating_add(light_delay_secs(origin, dest, &state.config.orbits));
        state.catch_up(visible);
        let err = state
            .launch_colony(state.home, dest, 100, visible)
            .unwrap_err();
        assert!(
            matches!(err, crate::FleetError::RivalClaim { dest: taken, .. } if taken == dest),
            "{err}"
        );
        let other = state
            .rival_colonize_dest(1)
            .filter(|coord| *coord != dest)
            .or_else(|| {
                (1..=state.config.expedition.dest_position)
                    .map(|position| Coord {
                        galaxy: state.home.galaxy,
                        system: state.home.system,
                        position,
                    })
                    .find(|coord| *coord != state.home && *coord != dest)
            })
            .expect("another empty slot");
        state
            .launch_colony(state.home, other, 100, visible)
            .expect("other empty coord still colonizes");
    }

    #[test]
    fn player_colonize_first_sends_the_rival_ship_home() {
        let mut state = spawn_on(quiet(1_000), 60);
        state.rivals.empires[0].resources.deuterium = 50_000.0;
        state.resources.deuterium = 50_000.0;
        state.tech_levels.insert("colonization".into(), 2);
        state.fleet.add(ShipKind::ColonyShip, 1);
        let dest = state.rival_colonize_dest(0).expect("dest");
        freeze_rival_acts(&mut state);
        state.launch_colony(state.home, dest, 100, 1_000).unwrap();
        let player_arrives = state.missions[0].phase_ends_at;
        state.catch_up(player_arrives);
        assert!(state.colonies.iter().any(|c| c.coord == dest));
        let ships_before = state.rivals.empires[0].fleet.get(ShipKind::ColonyShip);
        let origin = state.rivals.empires[0].home;
        let rival_id = state.rivals.empires[0].id.clone();
        let now = state.last_tick_unix;
        state.rivals.empires[0]
            .fleet
            .saturating_sub(ShipKind::ColonyShip, 1);
        state.rivals.flights.push(RivalColonizeFlight {
            rival_id,
            dest,
            origin,
            launched_at: now,
            arrives_at: now.saturating_add(10),
        });
        state.catch_up(now.saturating_add(10));
        assert!(state.rivals.colonies.is_empty());
        assert_eq!(
            state.rivals.empires[0].fleet.get(ShipKind::ColonyShip),
            ships_before
        );
    }
}
