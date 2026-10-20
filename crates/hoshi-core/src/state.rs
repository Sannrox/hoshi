//! Planet stockpiles, building levels and offline catch-up.

use crate::activity::{
    ActivityApply, ActivityChannel, ActivityPulse, ActivityTracker, resolve_channel,
};
use crate::balance::BalanceConfig;
use crate::buildings::{BuildError, BuildJob, BuildingKind, Buildings};
use crate::chains::{RecipeRun, apply_recipes, available, component_kind};
use crate::combat::{
    CombatReport, CombatWinner, RoundAction, next_u64, plunder_from, rapid_fire_table,
    resolve_ground, stats_from_catalog,
};
use crate::daily::{DailyAction, DailyBoard, mine_kind};
use crate::defences::{DefenceKind, PlanetBattle, resolve_planet_battle};
use crate::espionage::{SpyReport, resolve_spy, reveals};
use crate::expedition::{ExpeditionResult, roll_expedition};
use crate::fleets::{
    Coord, FleetError, Mission, MissionPhase, TradeRoute, distance, flight_secs, fuel_cost,
};
use crate::intel::{
    intel_confidence, light_delay_secs, light_range_au, noisy_fleet, noisy_resources, sensor_error,
    signed_unit,
};
use crate::outposts::{
    Outpost, RarePlanetBonus, generate_outposts, scaled_defence, scaled_garrison,
};
use crate::planets::{Colony, PlanetType, planet_profile};
use crate::prestige::{
    PRESTIGE_KEPT, PRESTIGE_LOST, Prestige, PrestigeError, PrestigePreview, RunStats,
};
use crate::production::LimitingFactor;
use crate::research::{
    ModifierKind, PrereqGap, ResearchError, ResearchJob, TechBranch, TechNodeState,
};
use crate::ships::{Fleet, ShipError, ShipJob, ShipKind, ShipStats};
use crate::wonders::{WonderError, WonderJob};
use std::collections::BTreeMap;

const EVENT_LOG_CAP: usize = 32;
const LOGISTICS_LOG_CAP: usize = 48;

struct BuildTarget<'a> {
    buildings: Buildings,
    resources: &'a mut Resources,
    queue: &'a mut Vec<BuildJob>,
    robotics: u32,
    cap: usize,
}

/// A timestamped line for the TUI and save. Combat reports land here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameEvent {
    pub at: u64,
    pub text: String,
}

/// Session glance for an offline gap. Not saved; catch-up fills it for the away overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogisticsGlance {
    pub deliveries: Vec<String>,
    pub stalls: Vec<String>,
    pub completed: Vec<String>,
}

/// Stored construction materials. Energy is a balance, not a stockpile.
///
/// Deuterium is kept as already-enriched fusion/rocket fuel. Water ice is the
/// feedstock; the synthesizer pays energy and heat to enrich it (natural D/H
/// in terrestrial ice is about 156 ppm). Helium-3 is a separate giant-planet
/// and regolith product, not a deuterium substitute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resources {
    pub iron: f64,
    pub silicon: f64,
    pub rare_earths: f64,
    pub water_ice: f64,
    pub helium_3: f64,
    pub deuterium: f64,
}

impl Resources {
    pub const ZERO: Self = Self {
        iron: 0.0,
        silicon: 0.0,
        rare_earths: 0.0,
        water_ice: 0.0,
        helium_3: 0.0,
        deuterium: 0.0,
    };

    /// Construction triple used by most costs: iron, silicon, deuterium fuel.
    pub const fn from_parts(iron: f64, silicon: f64, deuterium: f64) -> Self {
        Self {
            iron,
            silicon,
            rare_earths: 0.0,
            water_ice: 0.0,
            helium_3: 0.0,
            deuterium,
        }
    }

    /// Map a pre-v40 metal/crystal/deuterium triple onto the material set.
    /// Metal mass is split 95% iron / 5% rare earths. Crystal is silicon.
    /// Deuterium tankage was already fuel, so it stays deuterium (not ice).
    pub fn from_legacy(metal: f64, crystal: f64, deuterium: f64) -> Self {
        Self {
            iron: metal * 0.95,
            silicon: crystal,
            rare_earths: metal * 0.05,
            water_ice: 0.0,
            helium_3: 0.0,
            deuterium,
        }
    }

    pub fn total(self) -> f64 {
        self.iron
            + self.silicon
            + self.rare_earths
            + self.water_ice
            + self.helium_3
            + self.deuterium
    }

    pub fn with_fuel(mut self, fuel: f64) -> Self {
        self.deuterium += fuel;
        self
    }

    pub fn is_finite_nonneg(self) -> bool {
        ResourceKind::ALL.iter().all(|&kind| {
            let v = self.get(kind);
            v.is_finite() && v >= 0.0
        })
    }

    pub fn get(self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Iron => self.iron,
            ResourceKind::Silicon => self.silicon,
            ResourceKind::RareEarths => self.rare_earths,
            ResourceKind::WaterIce => self.water_ice,
            ResourceKind::Helium3 => self.helium_3,
            ResourceKind::Deuterium => self.deuterium,
        }
    }

    /// Compact Fe/Si/REE/H2O/He3/D label for logs, history and errors.
    pub fn brief(self) -> String {
        ResourceKind::ALL
            .iter()
            .copied()
            .filter(|&kind| self.get(kind) > 0.5 || ResourceKind::QUOTED.contains(&kind))
            .map(|kind| format!("{}{:.0}", kind.abbr(), self.get(kind)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn set(&mut self, kind: ResourceKind, value: f64) {
        match kind {
            ResourceKind::Iron => self.iron = value,
            ResourceKind::Silicon => self.silicon = value,
            ResourceKind::RareEarths => self.rare_earths = value,
            ResourceKind::WaterIce => self.water_ice = value,
            ResourceKind::Helium3 => self.helium_3 = value,
            ResourceKind::Deuterium => self.deuterium = value,
        }
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            iron: self.iron + other.iron,
            silicon: self.silicon + other.silicon,
            rare_earths: self.rare_earths + other.rare_earths,
            water_ice: self.water_ice + other.water_ice,
            helium_3: self.helium_3 + other.helium_3,
            deuterium: self.deuterium + other.deuterium,
        }
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self {
            iron: (self.iron - other.iron).max(0.0),
            silicon: (self.silicon - other.silicon).max(0.0),
            rare_earths: (self.rare_earths - other.rare_earths).max(0.0),
            water_ice: (self.water_ice - other.water_ice).max(0.0),
            helium_3: (self.helium_3 - other.helium_3).max(0.0),
            deuterium: (self.deuterium - other.deuterium).max(0.0),
        }
    }

    pub fn missing(self, cost: Self) -> Self {
        Self {
            iron: (cost.iron - self.iron).max(0.0),
            silicon: (cost.silicon - self.silicon).max(0.0),
            rare_earths: (cost.rare_earths - self.rare_earths).max(0.0),
            water_ice: (cost.water_ice - self.water_ice).max(0.0),
            helium_3: (cost.helium_3 - self.helium_3).max(0.0),
            deuterium: (cost.deuterium - self.deuterium).max(0.0),
        }
    }

    pub fn can_afford(self, cost: Self) -> bool {
        ResourceKind::ALL
            .iter()
            .all(|&kind| self.get(kind) + 1e-9 >= cost.get(kind))
    }
}

/// Stockpiled construction materials. Energy is computed, not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Iron,
    Silicon,
    RareEarths,
    WaterIce,
    Helium3,
    Deuterium,
}

impl ResourceKind {
    pub const ALL: [ResourceKind; 6] = [
        ResourceKind::Iron,
        ResourceKind::Silicon,
        ResourceKind::RareEarths,
        ResourceKind::WaterIce,
        ResourceKind::Helium3,
        ResourceKind::Deuterium,
    ];

    /// Distinct market price/history slots. Extra materials share these quotes.
    pub const QUOTED: [ResourceKind; 3] = [
        ResourceKind::Iron,
        ResourceKind::Silicon,
        ResourceKind::Deuterium,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            ResourceKind::Iron => "iron",
            ResourceKind::Silicon => "silicon",
            ResourceKind::RareEarths => "rare_earths",
            ResourceKind::WaterIce => "water_ice",
            ResourceKind::Helium3 => "helium_3",
            ResourceKind::Deuterium => "deuterium",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "iron" | "metal" => Some(ResourceKind::Iron),
            "silicon" | "crystal" => Some(ResourceKind::Silicon),
            "rare_earths" => Some(ResourceKind::RareEarths),
            "water_ice" => Some(ResourceKind::WaterIce),
            "helium_3" => Some(ResourceKind::Helium3),
            "deuterium" => Some(ResourceKind::Deuterium),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ResourceKind::Iron => "iron",
            ResourceKind::Silicon => "silicon",
            ResourceKind::RareEarths => "rare earths",
            ResourceKind::WaterIce => "water ice",
            ResourceKind::Helium3 => "helium-3",
            ResourceKind::Deuterium => "deuterium",
        }
    }

    pub fn abbr(self) -> &'static str {
        match self {
            ResourceKind::Iron => "Fe",
            ResourceKind::Silicon => "Si",
            ResourceKind::RareEarths => "REE",
            ResourceKind::WaterIce => "H2O",
            ResourceKind::Helium3 => "He3",
            ResourceKind::Deuterium => "D",
        }
    }

    /// Storage building that holds this stockpile.
    pub fn storage_building(self) -> crate::buildings::BuildingKind {
        use crate::buildings::BuildingKind::*;
        match self {
            ResourceKind::Iron | ResourceKind::RareEarths => MetalStorage,
            ResourceKind::Silicon => CrystalStorage,
            ResourceKind::WaterIce | ResourceKind::Helium3 | ResourceKind::Deuterium => {
                DeuteriumStorage
            }
        }
    }
}

struct ChainSite<'a> {
    buildings: Buildings,
    planet: PlanetType,
    coord: Coord,
    extra_iron: f64,
    extra_silicon: f64,
    extra_deut: f64,
    energy_factor: f64,
    boost: f64,
    resources: &'a Resources,
    goods: &'a BTreeMap<String, f64>,
}

/// Game state. All time advancement goes through [`GameState::catch_up`].
#[derive(Debug, Clone, PartialEq)]
pub struct GameState {
    pub resources: Resources,
    /// Intermediate goods on the home planet. Construction parts stay in `resources`.
    pub goods: BTreeMap<String, f64>,
    pub reactor_shutdowns: BTreeMap<Coord, crate::reactor::ReactorShutdown>,
    pub buildings: Buildings,
    pub build_queue: Vec<BuildJob>,
    pub tech_levels: BTreeMap<String, u32>,
    /// Exclusive group id → chosen tech id. Survives cancel; wiped on prestige.
    pub exclusive_picks: BTreeMap<String, String>,
    pub research: Option<ResearchJob>,
    pub fleet: Fleet,
    pub ship_queue: Vec<ShipJob>,
    pub defences: crate::DefenceGrid,
    pub home: Coord,
    pub home_kind: PlanetType,
    pub home_size: u8,
    pub colonies: Vec<Colony>,
    pub missions: Vec<Mission>,
    pub next_mission_id: u64,
    pub routes: Vec<TradeRoute>,
    pub next_route_id: u64,
    pub automation_rules: Vec<crate::AutomationRule>,
    pub next_automation_id: u64,
    pub next_automation_at: Option<u64>,
    pub next_raid_at: Option<u64>,
    pub events: Vec<GameEvent>,
    pub activity: ActivityTracker,
    pub last_tick_unix: u64,
    /// Seeded RNG state. Combat and raids consume it; the save stores the current value.
    pub rng_seed: u64,
    /// Stable seed for planet type and size. Does not change when combat consumes `rng_seed`.
    pub galaxy_seed: u64,
    /// Generator that produced this galaxy. Old saves keep the version they were created with.
    pub generator_version: u32,
    /// When true, `tick --advance` may move the clock. Wall-clock saves stay false.
    pub deterministic: bool,
    pub wonder: Option<WonderJob>,
    pub completed_wonders: Vec<String>,
    pub outposts: Vec<Outpost>,
    pub prestige: Prestige,
    pub story_pending: Option<crate::PendingStory>,
    pub last_story_at: u64,
    pub market: crate::MarketState,
    pub last_market_at: u64,
    pub diplomacy: crate::DiplomacyState,
    pub last_diplomacy_at: u64,
    pub spy_reports: Vec<crate::SpyReport>,
    pub next_report_id: u64,
    pub laws: crate::LawState,
    /// Achievement id → unix unlock time. Survives prestige.
    pub achievements: BTreeMap<String, u64>,
    /// Raids this run where the player was not the attacker. Resets with prestige.
    pub raids_survived: u32,
    /// Successful outpost invasions this run. Resets with prestige.
    pub outposts_conquered: u32,
    /// Recent fights, newest last. Rebuilt from seed plus opening fleets.
    pub combat_logs: Vec<crate::CombatLog>,
    pub daily: DailyBoard,
    pub moons: Vec<crate::Moon>,
    pub debris: BTreeMap<Coord, f64>,
    pub tutorial: crate::TutorialBoard,
    pub fleet_tactics: crate::FleetTactics,
    pub admirals: Vec<crate::Admiral>,
    pub home_admiral_id: Option<u64>,
    pub next_admiral_id: u64,
    pub leaders: Vec<crate::Leader>,
    pub assignments: Vec<crate::Assignment>,
    pub delegations: Vec<crate::Delegation>,
    pub next_leader_id: u64,
    pub reserves: crate::Fleet,
    pub pending_tactical: Option<crate::PendingTactical>,
    /// When true, home raids pause between rounds for a keypress. Catch-up and
    /// saves always auto-finish; the TUI sets this for the live session only.
    pub interactive_tactics: bool,
    /// Deliveries, stalls and completed items from catch-up in this session.
    /// Not persisted; the away overlay reads it after a gap.
    pub logistics: LogisticsGlance,
    /// Append-only structured history with snapshots for time travel.
    pub history: crate::HistoryLog,
    /// Parked named vehicles for orbital mission planning.
    pub vehicles: Vec<crate::Vehicle>,
    pub next_vehicle_id: u64,
    /// In-flight planned missions (orbital transfers).
    pub planned: Vec<crate::PlannedMission>,
    pub next_planned_id: u64,
    /// Markings, role extra-clearance, sharing agreements and leak incidents.
    pub classification: crate::ClassificationBoard,
    /// Regional order books, contracts and shipments. Home barter stays on `market`.
    pub board: crate::MarketBoard,
    /// Pirate fortress for the conquest victory path. Stable for the galaxy.
    pub pirate_home: Coord,
    /// Recorded once. Prestige reset starts a new galaxy.
    pub victory: Option<crate::VictoryRecord>,
    /// First instant every faction met `allied_min`. Cleared if standing drops.
    pub allied_since: Option<u64>,
    /// Present on seasonal saves. Main career prestige ignores this unless opted in.
    pub season: Option<crate::SeasonRun>,
    /// Playable species overlay for this galaxy.
    pub species: Option<crate::SpeciesRun>,
    /// NPC rival empires. Empty and disabled unless the galaxy opted in.
    pub rivals: crate::RivalBoard,
    /// Industrial goal ladder progress for this galaxy.
    pub ladder: crate::LadderBoard,
    /// Frontier galaxy unlock, seen bridge ids, and generated worlds.
    pub frontier: crate::FrontierBoard,
    /// Treasury, department budgets and held spends.
    pub finance: crate::FinanceState,
    pub kpis: crate::KpiState,
    pub incidents: crate::IncidentState,
    pub maintenance: crate::MaintenanceState,
    pub config: BalanceConfig,
}

impl GameState {
    pub fn new(now_unix: u64) -> Self {
        Self::with_config(now_unix, BalanceConfig::default())
    }

    pub fn with_config(now_unix: u64, config: BalanceConfig) -> Self {
        let galaxy_seed = 0;
        let version = crate::GALAXY_GENERATOR_VERSION;
        let (home_kind, home_size) = planet_profile(galaxy_seed, Coord::HOME, version);
        let outposts = generate_outposts(
            galaxy_seed,
            now_unix,
            &config.outposts,
            &[Coord::HOME],
            version,
        );
        let mut state = Self {
            resources: config.starting_resources,
            goods: BTreeMap::new(),
            reactor_shutdowns: BTreeMap::new(),
            buildings: config.starting_buildings,
            build_queue: Vec::new(),
            tech_levels: BTreeMap::new(),
            exclusive_picks: BTreeMap::new(),
            research: None,
            fleet: Fleet::default(),
            ship_queue: Vec::new(),
            defences: crate::DefenceGrid::default(),
            home: Coord::HOME,
            home_kind,
            home_size,
            colonies: Vec::new(),
            missions: Vec::new(),
            next_mission_id: 0,
            routes: Vec::new(),
            next_route_id: 0,
            automation_rules: Vec::new(),
            next_automation_id: 0,
            next_automation_at: None,
            next_raid_at: if config.raid.enabled {
                Some(now_unix.saturating_add(config.raid.first_delay_secs))
            } else {
                None
            },
            events: Vec::new(),
            activity: ActivityTracker::default(),
            last_tick_unix: now_unix,
            rng_seed: 0,
            galaxy_seed,
            generator_version: crate::GALAXY_GENERATOR_VERSION,
            deterministic: false,
            wonder: None,
            completed_wonders: Vec::new(),
            outposts,
            prestige: Prestige::new(now_unix),
            story_pending: None,
            last_story_at: 0,
            market: crate::MarketState::from_config(&config.market),
            last_market_at: 0,
            diplomacy: crate::DiplomacyState::from_config(&config.diplomacy),
            last_diplomacy_at: 0,
            spy_reports: Vec::new(),
            next_report_id: 0,
            laws: crate::LawState::default(),
            achievements: BTreeMap::new(),
            raids_survived: 0,
            outposts_conquered: 0,
            combat_logs: Vec::new(),
            daily: DailyBoard::default(),
            moons: Vec::new(),
            debris: BTreeMap::new(),
            tutorial: crate::TutorialBoard::default(),
            fleet_tactics: crate::FleetTactics {
                retreat_below: config.tactics.default_retreat_below,
                ..crate::FleetTactics::default()
            },
            admirals: Vec::new(),
            home_admiral_id: None,
            next_admiral_id: 3,
            leaders: Vec::new(),
            assignments: Vec::new(),
            delegations: Vec::new(),
            next_leader_id: 3,
            reserves: crate::Fleet::default(),
            pending_tactical: None,
            interactive_tactics: false,
            logistics: LogisticsGlance::default(),
            history: crate::HistoryLog::default(),
            vehicles: Vec::new(),
            next_vehicle_id: 0,
            planned: Vec::new(),
            next_planned_id: 0,
            classification: crate::ClassificationBoard::default(),
            board: crate::MarketBoard::default(),
            pirate_home: Coord::HOME,
            victory: None,
            allied_since: None,
            season: None,
            species: None,
            rivals: crate::RivalBoard::default(),
            ladder: crate::LadderBoard::default(),
            frontier: crate::FrontierBoard::default(),
            finance: crate::FinanceState::new(now_unix, &crate::FinanceConfig::bundled()),
            kpis: crate::KpiState::new(now_unix, &crate::KpiConfig::bundled()),
            incidents: crate::IncidentState::new(now_unix, &crate::IncidentConfig::bundled()),
            maintenance: crate::MaintenanceState::new(
                now_unix,
                &crate::MaintenanceConfig::bundled(),
            ),
            config,
        };
        state.seed_pirate_home(now_unix);
        state.seed_frontier();
        state.install_starter_staff(now_unix);
        state.apply_prestige_start();
        state.sync_daily(now_unix);
        state.sync_maintenance_assets(now_unix);
        state
    }

    /// New game whose RNG and planet rolls follow `seed`. Marks the save deterministic.
    pub fn with_seed(now_unix: u64, config: BalanceConfig, seed: u64) -> Self {
        let version = crate::GALAXY_GENERATOR_VERSION;
        let (home_kind, home_size) = planet_profile(seed, Coord::HOME, version);
        let mut state = Self::with_config(now_unix, config);
        state.rng_seed = seed;
        state.galaxy_seed = seed;
        state.home_kind = home_kind;
        state.home_size = home_size;
        state.deterministic = true;
        state.outposts = generate_outposts(
            seed,
            now_unix,
            &state.config.outposts,
            &[Coord::HOME],
            version,
        );
        state.seed_pirate_home(now_unix);
        state.seed_frontier();
        state.daily = DailyBoard::default();
        state.sync_daily(now_unix);
        state
    }

    /// Live TUI game on a chosen galaxy seed. Wall-clock catch-up stays on.
    pub fn new_live_galaxy(now_unix: u64, config: BalanceConfig, seed: u64) -> Self {
        let mut state = Self::with_seed(now_unix, config, seed);
        state.deterministic = false;
        state.tutorial = crate::TutorialBoard::start();
        state
    }

    /// Advance production, the build queue and research to `now_unix`.
    /// Completions that fall in the gap are applied in timestamp order and
    /// production is recomputed from each completion instant. A backward clock
    /// is a no-op.
    fn next_automation_event(&self) -> Option<u64> {
        if self.automation_rules.iter().any(|r| r.enabled) {
            self.next_automation_at
        } else {
            None
        }
    }

    pub fn catch_up(&mut self, now_unix: u64) {
        if now_unix <= self.last_tick_unix {
            return;
        }
        loop {
            let home_build_at = self.build_queue.first().and_then(|job| job.completes_at);
            let colony_build_at = self
                .colonies
                .iter()
                .filter_map(|c| c.build_queue.first().and_then(|job| job.completes_at))
                .min();
            let build_at = min_time(home_build_at, colony_build_at);
            let research_at = self.research.as_ref().map(|job| job.completes_at);
            let ship_at = self.ship_queue.first().and_then(|job| job.completes_at);
            let mission_at = self.missions.iter().map(|m| m.phase_ends_at).min();
            let route_at = self.routes.iter().map(|r| r.next_at).min();
            let raid_at = if self.config.raid.enabled {
                self.next_raid_at
            } else {
                None
            };
            let tactical_at = self
                .pending_tactical
                .as_ref()
                .map(|p| p.next_at)
                .filter(|&t| t > self.last_tick_unix);
            let prod_boost_at = self
                .activity
                .prod_boost_until
                .filter(|&t| t > self.last_tick_unix);
            let research_boost_at = self
                .activity
                .research_boost_until
                .filter(|&t| t > self.last_tick_unix);
            let wonder_at = self.wonder.as_ref().and_then(|job| job.completes_at);
            let rebuild_at = self
                .outposts
                .iter()
                .map(|o| o.next_rebuild_at(self.config.outposts.rebuild_secs))
                .min();
            let story_at = self.next_story_at();
            let market_at = self.next_market_at();
            let diplomacy_at = self.next_diplomacy_at();
            let law_at = self.next_law_at();
            let logistics_at = self.next_logistics_at();
            let staff_at = self.next_staff_at();
            let planned_at = self.next_planned_at();
            let board_at = self.next_board_at();
            let flagship_at = self
                .species
                .as_ref()
                .and_then(|run| run.flagship.as_ref())
                .and_then(|ship| ship.arrives_at)
                .filter(|&t| t > self.last_tick_unix);
            let rivals_at = self.next_rivals_at();
            let Some(done_at) = min_time(
                min_time(
                    min_time(
                        min_time(
                            min_time(
                                min_time(min_time(build_at, research_at), ship_at),
                                mission_at,
                            ),
                            min_time(raid_at, tactical_at),
                        ),
                        route_at,
                    ),
                    min_time(
                        min_time(prod_boost_at, research_boost_at),
                        min_time(
                            wonder_at,
                            min_time(
                                rebuild_at,
                                min_time(
                                    story_at,
                                    min_time(market_at, min_time(diplomacy_at, law_at)),
                                ),
                            ),
                        ),
                    ),
                ),
                min_time(
                    logistics_at,
                    min_time(
                        staff_at,
                        min_time(
                            planned_at,
                            min_time(
                                board_at,
                                min_time(
                                    flagship_at,
                                    min_time(
                                        self.next_victory_at(),
                                        min_time(
                                            self.next_ladder_at(),
                                            min_time(
                                                rivals_at,
                                                min_time(
                                                    self.next_automation_event(),
                                                    min_time(
                                                        self.next_finance_event(),
                                                        min_time(
                                                            self.next_kpi_event(),
                                                            min_time(
                                                                self.next_incident_event(),
                                                                min_time(
                                                                    self.next_maintenance_event(),
                                                                    self.next_intel_at(),
                                                                ),
                                                            ),
                                                        ),
                                                    ),
                                                ),
                                            ),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ) else {
                self.produce_until(now_unix);
                self.deliver_spy_reports_at(now_unix);
                self.apply_laws_at(now_unix);
                self.apply_automation_at(now_unix);
                self.apply_finance_at(now_unix);
                self.apply_kpis_at(now_unix);
                self.apply_incidents_at(now_unix);
                self.apply_maintenance_at(now_unix);
                self.tick_staff(now_unix);
                self.resolve_story(now_unix);
                self.evaluate_achievements(now_unix);
                self.maybe_declare_victory(now_unix);
                self.evaluate_ladder(now_unix);
                self.evaluate_frontier(now_unix);
                self.note_logistics_stalls(now_unix);
                return;
            };
            if done_at > now_unix {
                self.produce_until(now_unix);
                self.deliver_spy_reports_at(now_unix);
                self.apply_laws_at(now_unix);
                self.apply_automation_at(now_unix);
                self.apply_finance_at(now_unix);
                self.apply_kpis_at(now_unix);
                self.apply_incidents_at(now_unix);
                self.apply_maintenance_at(now_unix);
                self.tick_staff(now_unix);
                self.resolve_story(now_unix);
                self.evaluate_achievements(now_unix);
                self.maybe_declare_victory(now_unix);
                self.evaluate_ladder(now_unix);
                self.evaluate_frontier(now_unix);
                self.note_logistics_stalls(now_unix);
                return;
            }
            self.produce_until(done_at);
            self.deliver_spy_reports_at(done_at);
            self.apply_laws_at(done_at);
            self.apply_automation_at(done_at);
            self.apply_finance_at(done_at);
            self.apply_kpis_at(done_at);
            self.apply_incidents_at(done_at);
            self.apply_maintenance_at(done_at);
            self.tick_staff(done_at);
            if home_build_at == Some(done_at) {
                let job = self.build_queue.remove(0);
                self.buildings.increment(job.building);
                self.start_head(done_at);
                self.note_completed(
                    done_at,
                    format!("{} lv {}", job.building, job.from_level.saturating_add(1)),
                );
                self.history_complete_build(self.home, job.building, done_at);
                if mine_kind(job.building) {
                    self.note_daily(DailyAction::MineUpgrade, done_at);
                }
            }
            let colony_done: Vec<usize> = self
                .colonies
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.build_queue.first().and_then(|job| job.completes_at) == Some(done_at)
                })
                .map(|(i, _)| i)
                .collect();
            for i in colony_done {
                let job = self.colonies[i].build_queue.remove(0);
                self.colonies[i].buildings.increment(job.building);
                self.start_colony_head(i, done_at);
                self.note_completed(
                    done_at,
                    format!("{} lv {}", job.building, job.from_level.saturating_add(1)),
                );
                let coord = self.colonies[i].coord;
                self.history_complete_build(coord, job.building, done_at);
                if mine_kind(job.building) {
                    self.note_daily(DailyAction::MineUpgrade, done_at);
                }
            }
            if research_at == Some(done_at)
                && let Some(job) = self.research.take()
            {
                let tech = job.tech.clone();
                let level = self.tech_levels.entry(job.tech).or_insert(0);
                *level = level.saturating_add(1);
                let level = *level;
                self.note_completed(done_at, format!("research {tech} lv {level}"));
                self.record_history(crate::HistoryDraft {
                    at: done_at,
                    kind: crate::HistoryKind::Research,
                    actor: "simulation".into(),
                    action: "complete".into(),
                    objects: vec![tech.clone()],
                    outcome: format!("lv {level}"),
                    parent_key: Some(format!("research:{tech}")),
                    ..crate::HistoryDraft::default()
                });
                self.note_daily(DailyAction::ResearchComplete, done_at);
            }
            if ship_at == Some(done_at) {
                self.complete_one_ship(done_at);
            }
            if mission_at == Some(done_at) {
                self.advance_missions_at(done_at);
            }
            if planned_at == Some(done_at) {
                self.advance_planned_at(done_at);
            }
            if board_at == Some(done_at) {
                self.advance_board_at(done_at);
            }
            if route_at == Some(done_at) {
                self.fire_routes_at(done_at);
            }
            if raid_at == Some(done_at) {
                self.resolve_raid_at(done_at);
            }
            if tactical_at == Some(done_at) {
                self.progress_tactical(done_at);
            }
            if prod_boost_at == Some(done_at) {
                self.activity.prod_boost_until = None;
                self.activity.prod_boost_factor = 0.0;
            }
            if research_boost_at == Some(done_at) {
                // last_tick already equals `until`, so research_speed() has dropped
                // the activity term. Reconstruct the composed speed that was in effect.
                let old = self
                    .compose_research_speed((1.0 + self.activity.research_boost_factor).max(1.0));
                self.activity.research_boost_until = None;
                self.activity.research_boost_factor = 0.0;
                self.rescale_research(done_at, old, self.research_speed());
            }
            if wonder_at == Some(done_at) {
                self.complete_wonder_stage(done_at);
            }
            if rebuild_at == Some(done_at) {
                self.rebuild_outposts_at(done_at);
            }
            if market_at == Some(done_at) {
                self.tick_market(done_at);
            }
            if diplomacy_at == Some(done_at) {
                self.tick_diplomacy(done_at);
            }
            if rivals_at == Some(done_at) {
                self.tick_rivals(done_at);
            }
            self.resolve_story(done_at);
            self.evaluate_achievements(done_at);
            self.maybe_declare_victory(done_at);
            self.evaluate_ladder(done_at);
            self.evaluate_frontier(done_at);
            self.note_logistics_stalls(done_at);
        }
    }

    /// Apply a bridge pulse at its timestamp. Catch-up runs first so offline
    /// production through that instant is already in the stockpile.
    /// Events dated after `now_unix` are skipped so a bad feed cannot skip time.
    pub fn apply_activity(&mut self, pulse: ActivityPulse, now_unix: u64) -> ActivityApply {
        if !self.config.activity.enabled {
            return ActivityApply::Skipped;
        }
        let Some((channel, ts)) = resolve_channel(pulse, &self.config.activity) else {
            return ActivityApply::Skipped;
        };
        if ts > now_unix || ts < self.last_tick_unix {
            return ActivityApply::Skipped;
        }
        let spec = self.config.activity.get(channel).clone();
        if !spec.enabled {
            return ActivityApply::Skipped;
        }
        if matches!(
            channel,
            ActivityChannel::CiRed | ActivityChannel::AgentStuck
        ) && !self.config.raid.enabled
        {
            return ActivityApply::Skipped;
        }
        if !self
            .activity
            .admit(channel, ts, spec.cooldown_secs, spec.daily_cap)
        {
            return ActivityApply::Skipped;
        }
        self.catch_up(ts);
        let at = self.last_tick_unix.max(ts);
        match channel {
            ActivityChannel::PrMerged => self.grant_payout(at, spec.payout_hours),
            ActivityChannel::CiGreen => self.set_prod_boost(at, spec.boost_factor, spec.boost_secs),
            ActivityChannel::CiRed | ActivityChannel::AgentStuck => {
                let _ = self.trigger_raid(at);
            }
            ActivityChannel::AgentLongRun => {
                self.set_research_boost(at, spec.boost_factor, spec.boost_secs)
            }
        }
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Activity,
            actor: "bridge".into(),
            action: "apply".into(),
            objects: vec![channel.as_key().into()],
            outcome: "applied".into(),
            ..crate::HistoryDraft::default()
        });
        ActivityApply::Applied
    }

    fn install_starter_staff(&mut self, now: u64) {
        let (leaders, assignments, admirals, home_admiral_id, next_id) =
            self.config.leaders.starter(now);
        self.leaders = leaders;
        self.assignments = assignments;
        self.delegations.clear();
        self.admirals = admirals;
        self.home_admiral_id = home_admiral_id;
        self.next_leader_id = next_id;
        self.next_admiral_id = next_id;
    }

    pub(crate) fn sync_admirals_from_leaders(&mut self) {
        let assigned = self
            .assignments
            .iter()
            .find(|row| {
                row.role == crate::RoleKind::Admiral
                    && row.target == crate::AssignmentTarget::HomeFleet
                    && !self
                        .leaders
                        .iter()
                        .find(|l| l.id == row.leader_id)
                        .map(|l| l.retired)
                        .unwrap_or(true)
            })
            .map(|row| row.leader_id);
        self.home_admiral_id = assigned;
        self.admirals = self
            .leaders
            .iter()
            .filter(|leader| !leader.retired)
            .filter(|leader| {
                Some(leader.id) == assigned
                    || self.assignments.iter().any(|row| {
                        row.leader_id == leader.id && row.role == crate::RoleKind::Admiral
                    })
            })
            .map(crate::Leader::as_admiral)
            .collect();
    }

    pub fn leader(&self, id: u64) -> Option<&crate::Leader> {
        self.leaders.iter().find(|row| row.id == id)
    }

    pub fn assignment_of(&self, leader_id: u64) -> Option<&crate::Assignment> {
        self.assignments
            .iter()
            .find(|row| row.leader_id == leader_id)
    }

    fn ready_assignment(
        &self,
        role: crate::RoleKind,
        target: crate::AssignmentTarget,
        now: u64,
    ) -> Option<&crate::Assignment> {
        self.assignments.iter().find(|row| {
            row.role == role
                && row.target == target
                && row.ready(now)
                && self
                    .leaders
                    .iter()
                    .find(|l| l.id == row.leader_id)
                    .is_some_and(|l| !l.retired)
        })
    }

    pub fn governor_factor(&self, coord: Coord) -> f64 {
        let now = self.last_tick_unix;
        match self.ready_assignment(
            crate::RoleKind::Governor,
            crate::AssignmentTarget::Planet(coord),
            now,
        ) {
            Some(row) => self
                .leader(row.leader_id)
                .map(|leader| self.config.leaders.production(&leader.trait_id))
                .unwrap_or(1.0),
            None => (1.0 - self.config.leaders.empty_governor).max(0.0),
        }
    }

    fn next_staff_at(&self) -> Option<u64> {
        let retire = self
            .leaders
            .iter()
            .filter(|row| !row.retired)
            .map(crate::Leader::retires_at)
            .filter(|&at| at > self.last_tick_unix)
            .min();
        let ready = self
            .assignments
            .iter()
            .map(|row| row.ready_at)
            .filter(|&at| at > self.last_tick_unix)
            .min();
        min_time(retire, ready)
    }

    fn tick_staff(&mut self, now: u64) {
        let period = self.config.leaders.period_secs.max(1);
        for row in &mut self.delegations {
            if now.saturating_sub(row.period_start) >= period {
                row.period_start = now;
                row.spent = 0.0;
            }
        }
        let due: Vec<u64> = self
            .leaders
            .iter()
            .filter(|row| !row.retired && row.retires_at() <= now)
            .map(|row| row.id)
            .collect();
        for id in &due {
            if let Some(leader) = self.leaders.iter_mut().find(|row| row.id == *id) {
                leader.retired = true;
            }
            self.assignments.retain(|row| row.leader_id != *id);
            self.delegations.retain(|row| row.leader_id != *id);
            self.push_event(now, format!("leader #{id} retired"));
            self.record_history(crate::HistoryDraft {
                at: now,
                kind: crate::HistoryKind::Other,
                actor: "simulation".into(),
                action: "retire".into(),
                objects: vec![format!("leader:{id}")],
                outcome: "retired".into(),
                ..crate::HistoryDraft::default()
            });
        }
        if !due.is_empty() {
            self.sync_admirals_from_leaders();
        }
        self.tick_classification_leaks(now);
    }

    fn authorize(
        &mut self,
        actor: crate::Actor,
        permission: crate::Permission,
        planet: Option<Coord>,
        cost: f64,
    ) -> Result<(), crate::LeaderError> {
        match actor {
            crate::Actor::Player => match permission {
                crate::Permission::AssignLeader | crate::Permission::Delegate => Ok(()),
                _ => Ok(()),
            },
            crate::Actor::Leader(id) => {
                let now = self.last_tick_unix;
                let leader = self
                    .leader(id)
                    .cloned()
                    .ok_or(crate::LeaderError::UnknownLeader(id))?;
                if leader.retired {
                    return Err(crate::LeaderError::Retired(id));
                }
                let assignment = self
                    .assignment_of(id)
                    .cloned()
                    .ok_or(crate::LeaderError::Unassigned(id))?;
                if !assignment.ready(now) {
                    return Err(crate::LeaderError::Reassigning {
                        ready_at: assignment.ready_at,
                    });
                }
                if !assignment.role.grants(permission) {
                    return Err(crate::LeaderError::MissingPermission {
                        actor: actor.as_key(),
                        needed: permission,
                    });
                }
                match (assignment.role, assignment.target, planet) {
                    (
                        crate::RoleKind::Governor,
                        crate::AssignmentTarget::Planet(coord),
                        Some(at),
                    ) if coord != at => {
                        return Err(crate::LeaderError::MissingPermission {
                            actor: actor.as_key(),
                            needed: permission,
                        });
                    }
                    (crate::RoleKind::Admiral, crate::AssignmentTarget::HomeFleet, _) => {}
                    (crate::RoleKind::Minister, crate::AssignmentTarget::Empire, _) => {}
                    (crate::RoleKind::Governor, crate::AssignmentTarget::Planet(_), Some(_)) => {}
                    _ if permission == crate::Permission::QueueBuild => {
                        return Err(crate::LeaderError::MissingPermission {
                            actor: actor.as_key(),
                            needed: permission,
                        });
                    }
                    _ => {}
                }
                let Some(deleg) = self
                    .delegations
                    .iter_mut()
                    .find(|row| row.leader_id == id && row.permission == permission)
                else {
                    return Err(crate::LeaderError::MissingPermission {
                        actor: actor.as_key(),
                        needed: permission,
                    });
                };
                let period = self.config.leaders.period_secs.max(1);
                if now.saturating_sub(deleg.period_start) >= period {
                    deleg.period_start = now;
                    deleg.spent = 0.0;
                }
                if let Some(cap) = deleg.spend_cap {
                    if !cap.is_finite() || cap < 0.0 {
                        return Err(crate::LeaderError::SpendCap {
                            cap: 0.0,
                            spent: deleg.spent,
                            cost,
                        });
                    }
                    let next = deleg.spent + cost;
                    if !next.is_finite() || next > cap + 1e-9 {
                        return Err(crate::LeaderError::SpendCap {
                            cap,
                            spent: deleg.spent,
                            cost,
                        });
                    }
                }
                Ok(())
            }
        }
    }

    fn charge_delegation(&mut self, actor: crate::Actor, permission: crate::Permission, cost: f64) {
        let crate::Actor::Leader(id) = actor else {
            return;
        };
        if !(cost.is_finite() && cost > 0.0) {
            return;
        }
        if let Some(deleg) = self
            .delegations
            .iter_mut()
            .find(|row| row.leader_id == id && row.permission == permission)
        {
            deleg.spent += cost;
        }
    }

    pub fn recruit_leader(
        &mut self,
        name: &str,
        trait_id: &str,
        now_unix: u64,
    ) -> Result<u64, crate::LeaderError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if self.config.leaders.trait_by_id(trait_id).is_none() {
            return Err(crate::LeaderError::UnknownTrait(trait_id.into()));
        }
        let cost = self.config.leaders.recruit_cost;
        if !self.resources.can_afford(cost) {
            return Err(crate::LeaderError::CannotAfford {
                missing: self.resources.missing(cost),
            });
        }
        self.resources = self.resources.saturating_sub(cost);
        let id = self.next_leader_id;
        self.next_leader_id = id.saturating_add(1);
        self.next_admiral_id = self.next_leader_id;
        let label = if name.trim().is_empty() {
            format!("Leader {id}")
        } else {
            name.trim().to_string()
        };
        self.leaders.push(crate::Leader {
            id,
            name: label.clone(),
            species: "human".into(),
            trait_id: trait_id.to_string(),
            skill_command: 0,
            skill_logistics: 0,
            skill_policy: 0,
            xp: 0,
            born_at: now,
            lifespan_secs: self.config.leaders.lifespan_secs,
            retired: false,
        });
        self.sync_admirals_from_leaders();
        self.push_event(now, format!("recruited {label}"));
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Other,
            actor: crate::LEADER_PLAYER.into(),
            action: "recruit".into(),
            objects: vec![format!("leader:{id}")],
            outcome: "recruited".into(),
            ..crate::HistoryDraft::default()
        });
        self.schedule_disloyal_if_needed(trait_id, now);
        Ok(id)
    }

    pub fn assign_leader(
        &mut self,
        leader_id: u64,
        role: crate::RoleKind,
        target: crate::AssignmentTarget,
        now_unix: u64,
    ) -> Result<(), crate::LeaderError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        self.authorize(
            crate::Actor::Player,
            crate::Permission::AssignLeader,
            None,
            0.0,
        )?;
        let leader = self
            .leader(leader_id)
            .cloned()
            .ok_or(crate::LeaderError::UnknownLeader(leader_id))?;
        if leader.retired {
            return Err(crate::LeaderError::Retired(leader_id));
        }
        if let crate::AssignmentTarget::Planet(coord) = target
            && self.buildings_at(coord).is_none()
        {
            return Err(crate::LeaderError::UnknownPlanet);
        }
        let had_role = self.assignment_of(leader_id).is_some();
        if let Some(current) = self.assignment_of(leader_id)
            && !current.ready(now)
        {
            return Err(crate::LeaderError::Reassigning {
                ready_at: current.ready_at,
            });
        }
        self.assignments.retain(|row| row.leader_id != leader_id);
        self.assignments
            .retain(|row| !(row.role == role && row.target == target));
        let ready_at = if had_role {
            now.saturating_add(self.config.leaders.reassign_secs)
        } else {
            now
        };
        self.assignments.push(crate::Assignment {
            leader_id,
            role,
            target,
            assigned_at: now,
            ready_at,
        });
        self.sync_admirals_from_leaders();
        self.push_event(now, format!("assigned leader #{leader_id} as {role}"));
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Other,
            actor: crate::LEADER_PLAYER.into(),
            action: "assign".into(),
            objects: vec![format!("leader:{leader_id}"), role.as_key().into()],
            outcome: "assigned".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn delegate_leader(
        &mut self,
        leader_id: u64,
        permission: crate::Permission,
        spend_cap: Option<f64>,
        now_unix: u64,
    ) -> Result<(), crate::LeaderError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        self.authorize(crate::Actor::Player, crate::Permission::Delegate, None, 0.0)?;
        let leader = self
            .leader(leader_id)
            .ok_or(crate::LeaderError::UnknownLeader(leader_id))?;
        if leader.retired {
            return Err(crate::LeaderError::Retired(leader_id));
        }
        let assignment = self
            .assignment_of(leader_id)
            .ok_or(crate::LeaderError::Unassigned(leader_id))?;
        if !assignment.role.grants(permission) {
            return Err(crate::LeaderError::MissingPermission {
                actor: crate::Actor::Leader(leader_id).as_key(),
                needed: permission,
            });
        }
        if spend_cap.is_some_and(|cap| !cap.is_finite() || cap < 0.0) {
            return Err(crate::LeaderError::Parse(
                "spend cap must be finite and >= 0".into(),
            ));
        }
        self.delegations
            .retain(|row| !(row.leader_id == leader_id && row.permission == permission));
        self.delegations.push(crate::Delegation {
            leader_id,
            permission,
            spend_cap,
            spent: 0.0,
            period_start: now,
        });
        self.push_event(
            now,
            format!("delegated {} to leader #{leader_id}", permission.as_key()),
        );
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Other,
            actor: crate::LEADER_PLAYER.into(),
            action: "delegate".into(),
            objects: vec![format!("leader:{leader_id}"), permission.as_key().into()],
            outcome: "delegated".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn revoke_delegation(
        &mut self,
        leader_id: u64,
        permission: Option<crate::Permission>,
        now_unix: u64,
    ) -> Result<(), crate::LeaderError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        self.authorize(crate::Actor::Player, crate::Permission::Delegate, None, 0.0)?;
        if self.leader(leader_id).is_none() {
            return Err(crate::LeaderError::UnknownLeader(leader_id));
        }
        let before = self.delegations.len();
        match permission {
            Some(perm) => self
                .delegations
                .retain(|row| !(row.leader_id == leader_id && row.permission == perm)),
            None => self.delegations.retain(|row| row.leader_id != leader_id),
        }
        if self.delegations.len() == before {
            return Err(crate::LeaderError::Unassigned(leader_id));
        }
        self.push_event(now, format!("revoked delegation on leader #{leader_id}"));
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Other,
            actor: crate::LEADER_PLAYER.into(),
            action: "revoke".into(),
            objects: vec![format!("leader:{leader_id}")],
            outcome: "revoked".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    /// Queue an upgrade on the home planet.
    pub fn enqueue_build(&mut self, kind: BuildingKind, now_unix: u64) -> Result<(), BuildError> {
        self.enqueue_build_as(self.home, kind, now_unix, crate::Actor::Player)
    }

    /// Queue an upgrade. Resources are paid from that planet; only the head runs.
    pub fn enqueue_build_on(
        &mut self,
        coord: Coord,
        kind: BuildingKind,
        now_unix: u64,
    ) -> Result<(), BuildError> {
        self.enqueue_build_as(coord, kind, now_unix, crate::Actor::Player)
    }

    pub fn enqueue_build_as(
        &mut self,
        coord: Coord,
        kind: BuildingKind,
        now_unix: u64,
        actor: crate::Actor,
    ) -> Result<(), BuildError> {
        self.catch_up(now_unix);
        let buildings = *self.buildings_at(coord).ok_or(BuildError::UnknownPlanet)?;
        let queue: Vec<BuildJob> = if coord == self.home {
            self.build_queue.clone()
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| c.build_queue.clone())
                .ok_or(BuildError::UnknownPlanet)?
        };
        let from_level = projected_on(&buildings, &queue, kind, queue.len());
        let cost = self.config.buildings.get(kind).cost(from_level);
        let spend = cost.iron + cost.silicon + cost.deuterium;
        self.authorize(actor, crate::Permission::QueueBuild, Some(coord), spend)
            .map_err(|err| BuildError::PermissionDenied(err.to_string()))?;
        for prerequisite in &self.config.buildings.get(kind).tech_prerequisites {
            let actual = self.tech_level(&prerequisite.id);
            if actual < prerequisite.level {
                return Err(BuildError::MissingTechnology {
                    id: prerequisite.id.clone(),
                    required: prerequisite.level,
                    actual,
                });
            }
        }
        let cap = self.build_queue_cap();
        let resources = if coord == self.home {
            self.resources
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .ok_or(BuildError::UnknownPlanet)?
                .resources
        };
        Self::check_build_job_parts(kind, &buildings, &queue, &resources, cap, &self.config)?;
        let amount = self.value_resources(cost);
        let centers = crate::finance::build_centers(coord);
        let pending = crate::PendingKind::Build {
            coord,
            building: kind,
        };
        if let Err(id) = self.hold_spend(&centers, amount, pending, actor) {
            return Err(BuildError::NeedsApproval { id });
        }
        let now = self.last_tick_unix;
        let construction = self
            .species
            .as_ref()
            .map(|run| run.construction)
            .unwrap_or(1.0);
        if coord == self.home {
            let buildings = self.buildings;
            let robotics = buildings.robotics_factory;
            Self::push_build_job_parts(
                kind,
                now,
                BuildTarget {
                    buildings,
                    resources: &mut self.resources,
                    queue: &mut self.build_queue,
                    robotics,
                    cap,
                },
                &self.config,
                construction,
            )?;
        } else {
            let idx = self
                .colonies
                .iter()
                .position(|c| c.coord == coord)
                .ok_or(BuildError::UnknownPlanet)?;
            let buildings = self.colonies[idx].buildings;
            let robotics = buildings.robotics_factory;
            let colony = &mut self.colonies[idx];
            Self::push_build_job_parts(
                kind,
                now,
                BuildTarget {
                    buildings,
                    resources: &mut colony.resources,
                    queue: &mut colony.build_queue,
                    robotics,
                    cap,
                },
                &self.config,
                construction,
            )?;
        }
        self.charge_delegation(actor, crate::Permission::QueueBuild, spend);
        self.commit_spend(
            &centers,
            amount,
            &crate::PendingKind::Build {
                coord,
                building: kind,
            },
            actor,
        );
        if coord == self.home {
            if let Some(job) = self.build_queue.last_mut() {
                job.credits = amount;
            }
        } else if let Some(colony) = self.colonies.iter_mut().find(|c| c.coord == coord)
            && let Some(job) = colony.build_queue.last_mut()
        {
            job.credits = amount;
        }
        self.history_queue_build(coord, kind, now);
        Ok(())
    }

    fn check_build_job_parts(
        kind: BuildingKind,
        buildings: &Buildings,
        queue: &[BuildJob],
        resources: &Resources,
        cap: usize,
        config: &BalanceConfig,
    ) -> Result<(), BuildError> {
        if queue.len() >= cap {
            return Err(BuildError::QueueFull { cap });
        }
        let prereqs = config.buildings.get(kind).prerequisites.clone();
        for &(req, required) in &prereqs {
            let actual = projected_on(buildings, queue, req, queue.len());
            if actual < required {
                return Err(BuildError::MissingPrerequisite {
                    building: req,
                    required,
                    actual,
                });
            }
        }
        let from_level = projected_on(buildings, queue, kind, queue.len());
        let cost = config.buildings.get(kind).cost(from_level);
        if !resources.can_afford(cost) {
            return Err(BuildError::InsufficientResources {
                missing: resources.missing(cost),
                cost,
            });
        }
        Ok(())
    }

    fn push_build_job_parts(
        kind: BuildingKind,
        now: u64,
        target: BuildTarget<'_>,
        config: &BalanceConfig,
        construction: f64,
    ) -> Result<(), BuildError> {
        Self::check_build_job_parts(
            kind,
            &target.buildings,
            target.queue,
            target.resources,
            target.cap,
            config,
        )?;
        let from_level = projected_on(&target.buildings, target.queue, kind, target.queue.len());
        let cost = config.buildings.get(kind).cost(from_level);
        *target.resources = target.resources.saturating_sub(cost);
        target.queue.push(BuildJob {
            building: kind,
            from_level,
            paid: cost,
            credits: 0.0,
            started_at: None,
            completes_at: None,
        });
        if target.queue.len() == 1 {
            start_queue_head(target.queue, target.robotics, config, now, construction);
        }
        Ok(())
    }

    /// Queue the same upgrade on every selected planet. Logistics tech is required.
    /// Planets that cannot pay are reported; the others still enqueue.
    pub fn enqueue_build_many(
        &mut self,
        kind: BuildingKind,
        coords: &[Coord],
        now_unix: u64,
    ) -> Vec<(Coord, Result<(), BuildError>)> {
        self.catch_up(now_unix);
        if self.modifier(ModifierKind::Logistics) < 1.0 {
            return coords
                .iter()
                .map(|&c| (c, Err(BuildError::NeedLogistics)))
                .collect();
        }
        coords
            .iter()
            .map(|&c| (c, self.enqueue_build_on(c, kind, now_unix)))
            .collect()
    }

    /// Cancel a queue slot on the home planet and refund its paid cost in full.
    pub fn cancel_build(&mut self, index: usize, now_unix: u64) -> Result<Resources, BuildError> {
        self.cancel_build_on(self.home, index, now_unix)
    }

    pub fn cancel_build_on(
        &mut self,
        coord: Coord,
        index: usize,
        now_unix: u64,
    ) -> Result<Resources, BuildError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if coord == self.home {
            let len = self.build_queue.len();
            if index >= len {
                return Err(BuildError::EmptySlot { index, len });
            }
            let kind = self.build_queue[index].building;
            let deque_idx = self
                .build_queue
                .iter()
                .take(index)
                .filter(|job| job.building == kind)
                .count();
            self.history
                .forget_cause_at(&format!("build:{coord}:{}", kind.as_key()), deque_idx);
            let job = self.build_queue.remove(index);
            self.resources = self.resources.saturating_add(job.paid);
            self.refund_spend(
                &crate::finance::build_centers(coord),
                self.refund_credits(job.credits, job.paid),
            );
            self.reprice_queue();
            if index == 0 {
                if let Some(head) = self.build_queue.first_mut() {
                    head.started_at = None;
                    head.completes_at = None;
                }
                self.start_head(now);
            }
            return Ok(job.paid);
        }
        let idx = self
            .colonies
            .iter()
            .position(|c| c.coord == coord)
            .ok_or(BuildError::UnknownPlanet)?;
        let len = self.colonies[idx].build_queue.len();
        if index >= len {
            return Err(BuildError::EmptySlot { index, len });
        }
        let kind = self.colonies[idx].build_queue[index].building;
        let deque_idx = self.colonies[idx]
            .build_queue
            .iter()
            .take(index)
            .filter(|job| job.building == kind)
            .count();
        self.history
            .forget_cause_at(&format!("build:{coord}:{}", kind.as_key()), deque_idx);
        let job = self.colonies[idx].build_queue.remove(index);
        self.colonies[idx].resources = self.colonies[idx].resources.saturating_add(job.paid);
        self.refund_spend(
            &crate::finance::build_centers(coord),
            self.refund_credits(job.credits, job.paid),
        );
        self.reprice_colony_queue(idx);
        if index == 0 {
            if let Some(head) = self.colonies[idx].build_queue.first_mut() {
                head.started_at = None;
                head.completes_at = None;
            }
            self.start_colony_head(idx, now);
        }
        Ok(job.paid)
    }

    /// Start the single research slot. Fails if another tech is already running.
    pub fn start_research(&mut self, tech: &str, now_unix: u64) -> Result<(), ResearchError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if let Some(current) = &self.research {
            return Err(ResearchError::Busy {
                current: current.tech.clone(),
            });
        }
        let from_level = self.tech_level(tech);
        let (cost, duration, gaps) = {
            let spec = self
                .config
                .techs
                .get(tech)
                .ok_or_else(|| ResearchError::UnknownTech {
                    id: tech.to_string(),
                })?;
            (
                spec.cost(from_level),
                ((spec.duration_secs(from_level) as f64) / self.research_speed())
                    .ceil()
                    .max(1.0) as u64,
                self.research_prereq_gaps(spec),
            )
        };
        if !gaps.is_empty() {
            return Err(ResearchError::MissingPrerequisites { gaps });
        }
        match self.tech_node_state(tech) {
            TechNodeState::Hidden => {
                return Err(ResearchError::Hidden {
                    id: tech.to_string(),
                });
            }
            TechNodeState::Excluded => {
                let group = self
                    .config
                    .techs
                    .get(tech)
                    .and_then(|s| s.exclusive_group.clone())
                    .unwrap_or_default();
                return Err(ResearchError::Excluded {
                    id: tech.to_string(),
                    group,
                });
            }
            _ => {}
        }
        if self
            .config
            .techs
            .get(tech)
            .and_then(|s| s.max_level)
            .is_some_and(|m| from_level >= m)
        {
            return Err(ResearchError::Complete {
                id: tech.to_string(),
            });
        }
        if !self.resources.can_afford(cost) {
            return Err(ResearchError::InsufficientResources {
                missing: self.resources.missing(cost),
                cost,
            });
        }
        let amount = self.value_resources(cost);
        let centers = crate::finance::research_centers();
        let pending = crate::PendingKind::Research {
            tech: tech.to_string(),
        };
        if let Err(id) = self.hold_spend(&centers, amount, pending, crate::Actor::Player) {
            return Err(ResearchError::NeedsApproval { id });
        }
        self.resources = self.resources.saturating_sub(cost);
        if let Some(group) = self
            .config
            .techs
            .get(tech)
            .and_then(|s| s.exclusive_group.clone())
        {
            self.exclusive_picks.insert(group, tech.to_string());
        }
        self.research = Some(ResearchJob {
            tech: tech.to_string(),
            from_level,
            paid: cost,
            credits: amount,
            started_at: now,
            completes_at: now.saturating_add(duration),
        });
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Research,
            actor: "player".into(),
            action: "start".into(),
            objects: vec![tech.to_string()],
            inputs: cost.brief(),
            outcome: "started".into(),
            cause_key: Some(format!("research:{tech}")),
            ..crate::HistoryDraft::default()
        });
        self.commit_spend(
            &centers,
            amount,
            &crate::PendingKind::Research {
                tech: tech.to_string(),
            },
            crate::Actor::Player,
        );
        Ok(())
    }

    pub fn cancel_research(&mut self, now_unix: u64) -> Result<Resources, ResearchError> {
        self.catch_up(now_unix);
        let job = self.research.take().ok_or(ResearchError::Idle)?;
        self.history.forget_cause(&format!("research:{}", job.tech));
        self.resources = self.resources.saturating_add(job.paid);
        self.refund_spend(
            &crate::finance::research_centers(),
            self.refund_credits(job.credits, job.paid),
        );
        Ok(job.paid)
    }

    /// Pay for a batch and append it to the shipyard queue. Each remaining ship
    /// in the head batch completes on its own timestamp.
    pub fn enqueue_ships(
        &mut self,
        kind: ShipKind,
        count: u32,
        now_unix: u64,
    ) -> Result<(), ShipError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if count == 0 {
            return Err(ShipError::ZeroCount);
        }
        let spec = self.config.ships.get(kind);
        let yard = self.buildings.shipyard;
        if yard < spec.shipyard {
            return Err(ShipError::NeedShipyard {
                required: spec.shipyard,
                actual: yard,
            });
        }
        let gaps = self.missing_ship_prereqs(kind);
        if !gaps.is_empty() {
            return Err(ShipError::MissingPrerequisites { gaps });
        }
        let cap = self.config.max_ship_queue;
        if self.ship_queue.len() >= cap {
            return Err(ShipError::QueueFull { cap });
        }
        let unit = spec.unit_cost();
        let n = f64::from(count);
        let cost = Resources::from_parts(unit.iron * n, unit.silicon * n, unit.deuterium * n);
        if !self.resources.can_afford(cost) {
            return Err(ShipError::InsufficientResources {
                missing: self.resources.missing(cost),
                cost,
            });
        }
        let amount = self.value_resources(cost);
        let centers = crate::finance::military_centers();
        let pending = crate::PendingKind::Ships { kind, count };
        if let Err(id) = self.hold_spend(&centers, amount, pending, crate::Actor::Player) {
            return Err(ShipError::NeedsApproval { id });
        }
        self.resources = self.resources.saturating_sub(cost);
        self.ship_queue.push(ShipJob {
            ship: Some(kind),
            defence: None,
            remaining: count,
            unit_paid: unit,
            credits: amount,
            started_at: None,
            completes_at: None,
        });
        if self.ship_queue.len() == 1 {
            self.start_ship_head(now);
        }
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Ship,
            actor: "player".into(),
            action: "queue".into(),
            objects: vec![kind.as_key().into()],
            inputs: format!("{count}"),
            outcome: "queued".into(),
            cause_key: Some(format!("ship:{}", kind.as_key())),
            mass: f64::from(count),
            ..crate::HistoryDraft::default()
        });
        self.commit_spend(
            &centers,
            amount,
            &crate::PendingKind::Ships { kind, count },
            crate::Actor::Player,
        );
        Ok(())
    }

    /// Pay for a defence batch and append it to the same shipyard queue.
    pub fn enqueue_defence(
        &mut self,
        kind: DefenceKind,
        count: u32,
        now_unix: u64,
    ) -> Result<(), ShipError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if count == 0 {
            return Err(ShipError::ZeroCount);
        }
        let spec = self.config.defences.get(kind);
        let yard = self.buildings.shipyard;
        if yard < spec.shipyard {
            return Err(ShipError::NeedShipyard {
                required: spec.shipyard,
                actual: yard,
            });
        }
        let cap = self.config.max_ship_queue;
        if self.ship_queue.len() >= cap {
            return Err(ShipError::QueueFull { cap });
        }
        let unit = spec.unit_cost();
        let n = f64::from(count);
        let cost = Resources::from_parts(unit.iron * n, unit.silicon * n, unit.deuterium * n);
        if !self.resources.can_afford(cost) {
            return Err(ShipError::InsufficientResources {
                missing: self.resources.missing(cost),
                cost,
            });
        }
        let amount = self.value_resources(cost);
        let centers = crate::finance::military_centers();
        let pending = crate::PendingKind::Defence { kind, count };
        if let Err(id) = self.hold_spend(&centers, amount, pending, crate::Actor::Player) {
            return Err(ShipError::NeedsApproval { id });
        }
        self.resources = self.resources.saturating_sub(cost);
        self.ship_queue.push(ShipJob {
            ship: None,
            defence: Some(kind),
            remaining: count,
            unit_paid: unit,
            credits: amount,
            started_at: None,
            completes_at: None,
        });
        if self.ship_queue.len() == 1 {
            self.start_ship_head(now);
        }
        self.commit_spend(
            &centers,
            amount,
            &crate::PendingKind::Defence { kind, count },
            crate::Actor::Player,
        );
        Ok(())
    }

    pub fn defence_cost(&self, kind: DefenceKind, count: u32) -> Resources {
        let unit = self.config.defences.get(kind).unit_cost();
        let n = f64::from(count);
        Resources::from_parts(unit.iron * n, unit.silicon * n, unit.deuterium * n)
    }

    pub fn cancel_ships(&mut self, index: usize, now_unix: u64) -> Result<Resources, ShipError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let len = self.ship_queue.len();
        if index >= len {
            return Err(ShipError::EmptySlot { index, len });
        }
        if let Some(kind) = self.ship_queue[index].ship {
            let deque_idx = self
                .ship_queue
                .iter()
                .take(index)
                .filter(|job| job.ship == Some(kind))
                .count();
            self.history
                .forget_cause_at(&format!("ship:{}", kind.as_key()), deque_idx);
        }
        let job = self.ship_queue.remove(index);
        let n = f64::from(job.remaining);
        let refund = Resources::from_parts(
            job.unit_paid.iron * n,
            job.unit_paid.silicon * n,
            job.unit_paid.deuterium * n,
        );
        self.resources = self.resources.saturating_add(refund);
        self.refund_spend(
            &crate::finance::military_centers(),
            self.refund_credits(job.credits, refund),
        );
        if index == 0 {
            self.start_ship_head(now);
        }
        Ok(refund)
    }

    pub fn ship_cost(&self, kind: ShipKind, count: u32) -> Resources {
        let unit = self.config.ships.get(kind).unit_cost();
        let n = f64::from(count);
        let mult = self
            .species
            .as_ref()
            .map(|run| run.ship_cost)
            .unwrap_or(1.0);
        Resources::from_parts(
            unit.iron * n * mult,
            unit.silicon * n * mult,
            unit.deuterium * n * mult,
        )
    }

    /// Ships queue at home. A rare shipyard-world bonus on any owned planet
    /// speeds that shared queue.
    pub fn ship_time_secs(&self, kind: ShipKind) -> u64 {
        let speed = self
            .owned_coords()
            .into_iter()
            .filter_map(|coord| self.rare_bonus_at(coord).map(|row| row.shipyard_speed))
            .fold(0.0_f64, f64::max);
        ship_duration(self.config.ships.get(kind).duration_secs(), speed)
    }

    /// Duration if this world were the shipyard, including its rare speed bonus.
    pub fn ship_time_secs_on(&self, coord: Coord, kind: ShipKind) -> u64 {
        let speed = self
            .rare_bonus_at(coord)
            .map(|row| row.shipyard_speed)
            .unwrap_or(0.0);
        ship_duration(self.config.ships.get(kind).duration_secs(), speed)
    }

    pub fn fleet_slots(&self) -> u32 {
        self.config
            .flight
            .base_fleet_slots
            .saturating_add(self.modifier(ModifierKind::FleetSlots).floor().max(0.0) as u32)
    }

    pub fn flight_time_and_fuel(
        &self,
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
    ) -> Result<(u64, f64), FleetError> {
        self.flight_from(self.home, dest, ships, speed_percent)
    }

    pub(crate) fn flight_from(
        &self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        speed_percent: u8,
    ) -> Result<(u64, f64), FleetError> {
        if ships.is_empty() {
            return Err(FleetError::EmptyFleet);
        }
        if dest == origin {
            return Err(FleetError::SameLocation);
        }
        if !(10..=100).contains(&speed_percent) {
            return Err(FleetError::SpeedPercent { got: speed_percent });
        }
        self.check_frontier_flight(origin, dest)?;
        let dist = distance(origin, dest, &self.config.flight);
        let mut slowest = f64::MAX;
        let mut units_fuel = 0.0;
        for kind in ShipKind::ALL {
            let n = ships.get(kind);
            if n == 0 {
                continue;
            }
            let stats = self.ship_stats(kind);
            slowest = slowest.min(stats.speed);
            units_fuel += f64::from(n) * stats.fuel;
        }
        Ok((
            flight_secs(dist, slowest, speed_percent),
            fuel_cost(units_fuel, dist, speed_percent, &self.config.flight),
        ))
    }

    /// Detach ships from the planet, pay deuterium, and start outbound.
    pub fn launch_fleet(
        &mut self,
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
        hold_secs: u64,
        now_unix: u64,
    ) -> Result<(), FleetError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let (outbound_secs, fuel) = self.flight_time_and_fuel(ships, dest, speed_percent)?;
        for kind in ShipKind::ALL {
            let need = ships.get(kind);
            let have = self.fleet.get(kind);
            if have < need {
                return Err(FleetError::MissingShips { kind, need, have });
            }
        }
        let cap = self.fleet_slots();
        let used = self.missions.len() as u32;
        if used >= cap {
            return Err(FleetError::NoSlots { used, cap });
        }
        if self.resources.deuterium + 1e-9 < fuel {
            return Err(FleetError::InsufficientFuel {
                missing: (fuel - self.resources.deuterium).max(0.0),
                cost: fuel,
            });
        }
        self.resources.deuterium = (self.resources.deuterium - fuel).max(0.0);
        for kind in ShipKind::ALL {
            self.fleet.saturating_sub(kind, ships.get(kind));
        }
        let id = self.next_mission_id;
        self.next_mission_id = self.next_mission_id.saturating_add(1);
        self.missions.push(Mission {
            id,
            ships,
            origin: self.home,
            dest,
            speed_percent,
            phase: MissionPhase::Outbound,
            launched_at: now,
            outbound_secs,
            hold_secs,
            phase_ends_at: now.saturating_add(outbound_secs),
            fuel_paid: fuel,
            colonize: false,
            expedition_seed: None,
            expedition_applied: false,
            cargo: Resources::ZERO,
            transport: false,
            attack: false,
            invade: false,
            spy: false,
        });
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Fleet,
            actor: "player".into(),
            action: "launch".into(),
            objects: vec![dest.to_string()],
            inputs: format!("fuel {fuel:.0}"),
            outcome: "outbound".into(),
            cause_key: Some(format!("mission:{id}")),
            mass: fuel,
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn expedition_slots(&self) -> u32 {
        self.config.expedition.base_slots.saturating_add(
            self.modifier(ModifierKind::ExpeditionSlots)
                .floor()
                .max(0.0) as u32,
        )
    }

    fn expedition_used(&self) -> u32 {
        self.missions
            .iter()
            .filter(|m| m.expedition_seed.is_some())
            .count() as u32
    }

    pub fn expedition_dest(&self) -> Coord {
        Coord {
            galaxy: self.home.galaxy,
            system: self.home.system,
            position: self.config.expedition.dest_position,
        }
    }

    /// Send a fleet to the expedition slot for `hold_secs`. The outcome seed is stored
    /// on the mission so a reload cannot re-roll it.
    pub fn launch_expedition(
        &mut self,
        ships: Fleet,
        hold_secs: u64,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        let min = self.config.expedition.min_hold_secs;
        let max = self.config.expedition.max_hold_secs;
        if hold_secs < min || hold_secs > max {
            return Err(FleetError::HoldOutOfRange {
                got: hold_secs,
                min,
                max,
            });
        }
        let cap = self.expedition_slots();
        let used = self.expedition_used();
        if used >= cap {
            return Err(FleetError::NoExpeditionSlots { used, cap });
        }
        let dest = self.expedition_dest();
        self.launch_fleet(ships, dest, speed_percent, hold_secs, now_unix)?;
        let id = {
            let Some(mission) = self.missions.last_mut() else {
                return Err(FleetError::EmptyFleet);
            };
            mission.expedition_seed = Some(next_u64(&mut self.rng_seed));
            mission.expedition_applied = false;
            mission.id
        };
        self.note_daily(DailyAction::ExpeditionLaunch, now_unix);
        Ok(id)
    }

    pub fn owned_coords(&self) -> Vec<Coord> {
        let mut out = vec![self.home];
        out.extend(self.colonies.iter().map(|c| c.coord));
        out
    }

    pub fn resources_at(&self, coord: Coord) -> Option<&Resources> {
        if coord == self.home {
            Some(&self.resources)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| &c.resources)
        }
    }

    pub fn buildings_at(&self, coord: Coord) -> Option<&Buildings> {
        if coord == self.home {
            Some(&self.buildings)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| &c.buildings)
        }
    }

    pub fn queue_at(&self, coord: Coord) -> Option<&[BuildJob]> {
        if coord == self.home {
            Some(&self.build_queue)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| c.build_queue.as_slice())
        }
    }

    pub fn build_cost_on(&self, coord: Coord, kind: BuildingKind) -> Resources {
        let buildings = self.buildings_at(coord).unwrap_or(&self.buildings);
        let queue = self.queue_at(coord).unwrap_or(&self.build_queue);
        let from_level = projected_on(buildings, queue, kind, queue.len());
        self.config.buildings.get(kind).cost(from_level)
    }

    pub fn build_time_secs_on(&self, coord: Coord, kind: BuildingKind) -> u64 {
        let buildings = self.buildings_at(coord).unwrap_or(&self.buildings);
        let queue = self.queue_at(coord).unwrap_or(&self.build_queue);
        let from_level = projected_on(buildings, queue, kind, queue.len());
        duration_with_robotics(kind, from_level, buildings.robotics_factory, &self.config)
    }

    pub fn cargo_capacity(&self, ships: Fleet) -> f64 {
        ShipKind::ALL
            .iter()
            .map(|&kind| f64::from(ships.get(kind)) * self.ship_stats(kind).cargo)
            .sum()
    }

    pub(crate) fn cargo_mass(cargo: Resources) -> f64 {
        cargo.total()
    }

    fn check_cargo(cargo: Resources) -> Result<(), FleetError> {
        if cargo.is_finite_nonneg() {
            Ok(())
        } else {
            Err(FleetError::InvalidCargo)
        }
    }

    pub(crate) fn add_fleet_at(&mut self, coord: Coord, ships: Fleet) {
        if coord == self.home {
            for kind in ShipKind::ALL {
                self.fleet.add(kind, ships.get(kind));
            }
            return;
        }
        if let Some(colony) = self.colonies.iter_mut().find(|c| c.coord == coord) {
            for kind in ShipKind::ALL {
                colony.fleet.add(kind, ships.get(kind));
            }
        } else {
            for kind in ShipKind::ALL {
                self.fleet.add(kind, ships.get(kind));
            }
        }
    }

    pub(crate) fn credit_at(&mut self, coord: Coord, found: Resources) {
        if coord == self.home {
            self.credit_capped(found);
            return;
        }
        if self.colonies.iter().any(|c| c.coord == coord) {
            let caps: [(ResourceKind, f64); 6] =
                ResourceKind::ALL.map(|kind| (kind, self.storage_cap_at(coord, kind)));
            if let Some(colony) = self.colonies.iter_mut().find(|c| c.coord == coord) {
                for (kind, cap) in caps {
                    let current = colony.resources.get(kind);
                    colony
                        .resources
                        .set(kind, (current + found.get(kind)).min(cap));
                }
            }
        } else {
            self.credit_capped(found);
        }
    }

    fn take_ships_at(&mut self, coord: Coord, ships: Fleet) -> Result<(), FleetError> {
        let fleet = if coord == self.home {
            &mut self.fleet
        } else {
            &mut self
                .colonies
                .iter_mut()
                .find(|c| c.coord == coord)
                .ok_or(FleetError::UnknownPlanet)?
                .fleet
        };
        for kind in ShipKind::ALL {
            let need = ships.get(kind);
            let have = fleet.get(kind);
            if have < need {
                return Err(FleetError::MissingShips { kind, need, have });
            }
        }
        for kind in ShipKind::ALL {
            fleet.saturating_sub(kind, ships.get(kind));
        }
        Ok(())
    }

    pub(crate) fn resources_at_mut(&mut self, coord: Coord) -> Result<&mut Resources, FleetError> {
        if coord == self.home {
            Ok(&mut self.resources)
        } else {
            self.colonies
                .iter_mut()
                .find(|c| c.coord == coord)
                .map(|c| &mut c.resources)
                .ok_or(FleetError::UnknownPlanet)
        }
    }

    /// Move cargo between owned planets. Fuel is paid from the origin at launch.
    pub fn launch_transport(
        &mut self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        cargo: Resources,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        if origin == dest {
            return Err(FleetError::SameLocation);
        }
        if !self.occupied(dest) {
            return Err(FleetError::NotOwned);
        }
        if !self.occupied(origin) {
            return Err(FleetError::UnknownPlanet);
        }
        Self::check_cargo(cargo)?;
        let cap = self.cargo_capacity(ships);
        let need = Self::cargo_mass(cargo);
        if need > cap + 1e-9 {
            return Err(FleetError::CargoExceedsCapacity { need, cap });
        }
        if cargo_has_mass(cargo) && !self.dest_has_room_for(dest, cargo) {
            return Err(FleetError::DestFull);
        }
        let (outbound_secs, fuel) = self.flight_from(origin, dest, ships, speed_percent)?;
        let needed = cargo.with_fuel(fuel);
        let cargo_missing = {
            let res = self.resources_at_mut(origin)?;
            if !res.can_afford(cargo) {
                Some(res.missing(cargo))
            } else {
                None
            }
        };
        if let Some(missing) = cargo_missing {
            return Err(FleetError::InsufficientCargo { missing });
        }
        let fuel_missing = {
            let res = self.resources_at_mut(origin)?;
            if res.deuterium + 1e-9 < needed.deuterium {
                Some((needed.deuterium - res.deuterium).max(0.0))
            } else {
                None
            }
        };
        if let Some(missing) = fuel_missing {
            return Err(FleetError::InsufficientFuel {
                missing,
                cost: fuel,
            });
        }
        let cap_slots = self.fleet_slots();
        let used = self.missions.len() as u32;
        if used >= cap_slots {
            return Err(FleetError::NoSlots {
                used,
                cap: cap_slots,
            });
        }
        self.take_ships_at(origin, ships)?;
        {
            let res = self.resources_at_mut(origin)?;
            *res = res.saturating_sub(needed);
        }
        let id = self.next_mission_id;
        self.next_mission_id = self.next_mission_id.saturating_add(1);
        let now = self.last_tick_unix;
        self.missions.push(Mission {
            id,
            ships,
            origin,
            dest,
            speed_percent,
            phase: MissionPhase::Outbound,
            launched_at: now,
            outbound_secs,
            hold_secs: 0,
            phase_ends_at: now.saturating_add(outbound_secs),
            fuel_paid: fuel,
            colonize: false,
            expedition_seed: None,
            expedition_applied: false,
            cargo,
            transport: true,
            attack: false,
            invade: false,
            spy: false,
        });
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Fleet,
            actor: "player".into(),
            action: "transport".into(),
            objects: vec![origin.to_string(), dest.to_string()],
            inputs: cargo.brief(),
            outcome: "outbound".into(),
            cause_key: Some(format!("mission:{id}")),
            mass: Self::cargo_mass(cargo),
            ..crate::HistoryDraft::default()
        });
        Ok(id)
    }

    pub fn schedule_route(
        &mut self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        cargo: Resources,
        interval_secs: u64,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        if interval_secs == 0 {
            return Err(FleetError::HoldOutOfRange {
                got: 0,
                min: 1,
                max: u64::MAX,
            });
        }
        if origin == dest {
            return Err(FleetError::SameLocation);
        }
        if !self.occupied(origin) || !self.occupied(dest) {
            return Err(FleetError::NotOwned);
        }
        if ships.is_empty() {
            return Err(FleetError::EmptyFleet);
        }
        Self::check_cargo(cargo)?;
        let cap = self.cargo_capacity(ships);
        let need = Self::cargo_mass(cargo);
        if need > cap + 1e-9 {
            return Err(FleetError::CargoExceedsCapacity { need, cap });
        }
        let id = self.next_route_id;
        self.next_route_id = self.next_route_id.saturating_add(1);
        self.routes.push(TradeRoute {
            id,
            origin,
            dest,
            ships,
            cargo,
            speed_percent: 100,
            interval_secs,
            next_at: self.last_tick_unix,
        });
        Ok(id)
    }

    pub fn cancel_route(&mut self, id: u64) -> Result<(), FleetError> {
        let idx = self
            .routes
            .iter()
            .position(|r| r.id == id)
            .ok_or(FleetError::UnknownMission { id })?;
        self.routes.remove(idx);
        Ok(())
    }

    fn fire_routes_at(&mut self, done_at: u64) {
        let ids: Vec<u64> = self
            .routes
            .iter()
            .filter(|r| r.next_at == done_at)
            .map(|r| r.id)
            .collect();
        for id in ids {
            let Some(index) = self.routes.iter().position(|r| r.id == id) else {
                continue;
            };
            let route = self.routes[index].clone();
            self.routes[index].next_at = done_at.saturating_add(route.interval_secs.max(1));
            let cargo = self.adapt_route_cargo(&route);
            match self.launch_transport(
                route.origin,
                route.dest,
                route.ships,
                cargo,
                route.speed_percent,
                done_at,
            ) {
                Ok(_) => {}
                Err(err) => self.note_stall(done_at, format!("route {id}: skipped ({err})")),
            }
        }
    }

    /// Send one colony ship. Settlement is attempted on Action; failure returns the ship.
    pub fn launch_colony(
        &mut self,
        dest: Coord,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        if self.fleet.get(ShipKind::ColonyShip) == 0 {
            return Err(FleetError::NeedColonyShip);
        }
        let mut ships = Fleet::default();
        ships.add(ShipKind::ColonyShip, 1);
        self.launch_fleet(ships, dest, speed_percent, 0, now_unix)?;
        let Some(mission) = self.missions.last_mut() else {
            return Err(FleetError::EmptyFleet);
        };
        mission.colonize = true;
        Ok(mission.id)
    }

    /// Send a fleet to fight an outpost's orbital defences.
    pub fn launch_attack(
        &mut self,
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        if self.outpost(dest).is_none() {
            return Err(FleetError::UnknownOutpost);
        }
        self.launch_fleet(ships, dest, speed_percent, 0, now_unix)?;
        let Some(mission) = self.missions.last_mut() else {
            return Err(FleetError::EmptyFleet);
        };
        mission.attack = true;
        Ok(mission.id)
    }

    /// Land troop transports. Refused while any orbital defences remain.
    pub fn launch_invasion(
        &mut self,
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        let Some(outpost) = self.outpost(dest) else {
            return Err(FleetError::UnknownOutpost);
        };
        if outpost.defences.total() > 0 {
            return Err(FleetError::DefencesRemain);
        }
        if ships.get(ShipKind::TroopTransport) == 0 {
            return Err(FleetError::NeedTroopTransport);
        }
        let used = 1 + self.colonies.len() as u32;
        let cap = self.colony_cap();
        if used >= cap {
            return Err(FleetError::ColonyCap { used, cap });
        }
        self.launch_fleet(ships, dest, speed_percent, 0, now_unix)?;
        let Some(mission) = self.missions.last_mut() else {
            return Err(FleetError::EmptyFleet);
        };
        mission.invade = true;
        Ok(mission.id)
    }

    /// Send probes to an outpost. Arrival writes a report; detection may destroy probes.
    pub fn launch_spy(
        &mut self,
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        if self.outpost(dest).is_none() {
            return Err(FleetError::UnknownOutpost);
        }
        if ships.get(ShipKind::EspionageProbe) == 0 {
            return Err(FleetError::NeedEspionageProbe);
        }
        self.launch_fleet(ships, dest, speed_percent, 0, now_unix)?;
        let Some(mission) = self.missions.last_mut() else {
            return Err(FleetError::EmptyFleet);
        };
        mission.spy = true;
        Ok(mission.id)
    }

    pub fn latest_report(&self, coord: Coord) -> Option<&SpyReport> {
        self.spy_reports
            .iter()
            .filter(|r| r.target == coord && r.delivered(self.last_tick_unix))
            .max_by_key(|r| (r.observed_at, r.id))
    }

    pub fn reports_for(&self, coord: Coord) -> Vec<&SpyReport> {
        self.spy_reports
            .iter()
            .filter(|r| r.target == coord && r.delivered(self.last_tick_unix))
            .collect()
    }

    pub fn outpost(&self, coord: Coord) -> Option<&Outpost> {
        self.outposts.iter().find(|o| o.coord == coord)
    }

    pub fn rare_bonus_at(&self, coord: Coord) -> Option<&RarePlanetBonus> {
        let id = if coord == self.home {
            return None;
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .and_then(|c| c.rare.as_deref())?
        };
        self.config.outposts.rare.iter().find(|row| row.id == id)
    }

    pub fn planet_kind(&self, coord: Coord) -> Option<PlanetType> {
        if coord == self.home {
            Some(self.home_kind)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| c.kind)
        }
    }

    pub fn planet_size(&self, coord: Coord) -> Option<u8> {
        if coord == self.home {
            Some(self.home_size)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| c.size)
        }
    }

    /// Deposit richness for a owned planet, from type, galaxy seed and coord.
    pub fn deposits_at(&self, coord: Coord) -> BTreeMap<String, f64> {
        let Some(kind) = self.planet_kind(coord) else {
            return BTreeMap::new();
        };
        self.chain_deposits(kind, self.galaxy_seed, coord)
    }

    pub fn colony_cap(&self) -> u32 {
        1 + self.modifier(ModifierKind::ColonySlots).floor().max(0.0) as u32
    }

    fn expedition_astro(&self) -> u32 {
        self.tech_level("astrophysics")
            .saturating_add(self.modifier(ModifierKind::Espionage).floor().max(0.0) as u32)
    }

    /// Begin a wonder. One at a time; stages are paid with [`Self::contribute_wonder`].
    pub fn start_wonder(&mut self, id: &str, now_unix: u64) -> Result<(), WonderError> {
        self.catch_up(now_unix);
        if let Some(job) = &self.wonder {
            return Err(WonderError::Busy { id: job.id.clone() });
        }
        if self.completed_wonders.iter().any(|done| done == id) {
            return Err(WonderError::AlreadyComplete { id: id.into() });
        }
        let spec = self
            .config
            .wonders
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| WonderError::Unknown { id: id.into() })?;
        if spec.stages.is_empty() {
            return Err(WonderError::EmptyStages { id: id.into() });
        }
        for prereq in &spec.prereq_techs {
            let actual = self.tech_level(&prereq.id);
            if actual < prereq.level {
                return Err(WonderError::MissingTech {
                    id: prereq.id.clone(),
                    required: prereq.level,
                    actual,
                });
            }
        }
        let stage0 = spec.stage(0).ok_or_else(|| WonderError::EmptyStages {
            id: spec.id.clone(),
        })?;
        self.wonder_tier_ok(stage0)?;
        self.wonder = Some(WonderJob {
            id: spec.id.clone(),
            stage: 0,
            contributed: Resources::ZERO,
            completes_at: None,
        });
        Ok(())
    }

    /// Move stockpile from `coord` onto the current stage. Starts the stage timer when paid.
    pub fn contribute_wonder(
        &mut self,
        coord: Coord,
        now_unix: u64,
    ) -> Result<Resources, WonderError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let (job_id, stage_idx, contributed) = match &self.wonder {
            Some(job) => (job.id.clone(), job.stage, job.contributed),
            None => return Err(WonderError::NothingToContribute),
        };
        let spec = self
            .config
            .wonders
            .iter()
            .find(|s| s.id == job_id)
            .ok_or_else(|| WonderError::Unknown { id: job_id.clone() })?;
        let stage = spec
            .stage(stage_idx)
            .ok_or_else(|| WonderError::EmptyStages { id: job_id.clone() })?;
        self.wonder_tier_ok(stage)?;
        let cost = stage.cost;
        let duration = stage.duration_secs;
        let remaining = Resources::from_parts(
            (cost.iron - contributed.iron).max(0.0),
            (cost.silicon - contributed.silicon).max(0.0),
            (cost.deuterium - contributed.deuterium).max(0.0),
        );
        let planet = self
            .resources_at_mut(coord)
            .map_err(|_| WonderError::UnknownPlanet)?;
        let take = Resources::from_parts(
            planet.iron.min(remaining.iron),
            planet.silicon.min(remaining.silicon),
            planet.deuterium.min(remaining.deuterium),
        );
        if take.iron <= 0.0 && take.silicon <= 0.0 && take.deuterium <= 0.0 {
            return Err(WonderError::NothingToContribute);
        }
        *planet = planet.saturating_sub(take);
        let job = self
            .wonder
            .as_mut()
            .ok_or(WonderError::NothingToContribute)?;
        job.contributed = job.contributed.saturating_add(take);
        if job.contributed.iron + 1e-9 >= cost.iron
            && job.contributed.silicon + 1e-9 >= cost.silicon
            && job.contributed.deuterium + 1e-9 >= cost.deuterium
            && job.completes_at.is_none()
        {
            job.completes_at = Some(now.saturating_add(duration));
        }
        Ok(take)
    }

    pub fn wonder_remaining(&self) -> Option<Resources> {
        let job = self.wonder.as_ref()?;
        let spec = self.config.wonders.iter().find(|s| s.id == job.id)?;
        let stage = spec.stage(job.stage)?;
        Some(Resources::from_parts(
            (stage.cost.iron - job.contributed.iron).max(0.0),
            (stage.cost.silicon - job.contributed.silicon).max(0.0),
            (stage.cost.deuterium - job.contributed.deuterium).max(0.0),
        ))
    }

    /// Seconds until the current stage finishes at current income, or the running timer.
    pub fn wonder_eta_secs(&self) -> Option<u64> {
        let job = self.wonder.as_ref()?;
        if let Some(at) = job.completes_at {
            return Some(at.saturating_sub(self.last_tick_unix));
        }
        let remaining = self.wonder_remaining()?;
        let mut secs = 0_u64;
        for kind in ResourceKind::ALL {
            let need = remaining.get(kind);
            if need <= 1e-9 {
                continue;
            }
            let rate = self.empire_production_per_hour(kind);
            if rate <= 1e-9 {
                return None;
            }
            let wait = (need / rate * 3600.0).ceil() as u64;
            secs = secs.max(wait);
        }
        Some(secs)
    }

    pub(crate) fn empire_production_per_hour(&self, kind: ResourceKind) -> f64 {
        self.production_per_hour(kind)
            + self
                .colonies
                .iter()
                .map(|colony| self.colony_production_per_hour(colony, kind))
                .sum::<f64>()
    }

    fn colony_production_per_hour(&self, colony: &Colony, kind: ResourceKind) -> f64 {
        let bonus = self.config.planet_types.get(colony.kind);
        let rare = self.rare_bonus_for(colony);
        let frontier = self.frontier_production_bonus(colony.coord);
        let energy_t = self.tech_modifier(ModifierKind::EnergyOutput);
        let produced = self
            .config
            .solar_plant
            .energy_output(colony.buildings.solar_plant)
            * self.config.orbits.solar_flux(colony.coord)
            * (1.0 + energy_t + bonus.energy + rare.energy).max(0.0)
            + self.reactor_output_at(colony.coord);
        let consumed = self.config.energy_draw(&colony.buildings);
        let energy = if consumed <= 0.0 {
            1.0
        } else {
            (produced / consumed).clamp(0.0, 1.0)
        };
        let factor = energy * self.unrest_for(colony.coord) * self.emission_factor(colony.coord);
        let boost = self.prod_boost_mult();
        let staff = self.governor_factor(colony.coord);
        if self.config.chains.enabled() {
            return self.chain_net_per_hour(
                ChainSite {
                    buildings: colony.buildings,
                    planet: colony.kind,
                    coord: colony.coord,
                    extra_iron: rare.iron + frontier,
                    extra_silicon: rare.silicon + frontier,
                    extra_deut: rare.deuterium + frontier,
                    energy_factor: factor,
                    boost,
                    resources: &colony.resources,
                    goods: &colony.goods,
                },
                kind,
            ) * staff;
        }
        let (raw, output, planet_bonus) = match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => (
                self.config
                    .metal_mine
                    .production_per_hour(colony.buildings.metal_mine),
                ModifierKind::MetalOutput,
                bonus.iron + rare.iron + frontier,
            ),
            ResourceKind::Silicon => (
                self.config
                    .crystal_mine
                    .production_per_hour(colony.buildings.crystal_mine),
                ModifierKind::CrystalOutput,
                bonus.silicon + rare.silicon + frontier,
            ),
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => (
                self.config
                    .deuterium_synthesizer
                    .production_per_hour(colony.buildings.deuterium_synthesizer),
                ModifierKind::DeuteriumOutput,
                bonus.deuterium + rare.deuterium + frontier,
            ),
        };
        raw * (1.0 + self.tech_modifier(output) + planet_bonus) * factor * boost * staff
    }

    fn rare_bonus_for(&self, colony: &Colony) -> RarePlanetBonus {
        colony
            .rare
            .as_ref()
            .and_then(|id| {
                self.config
                    .outposts
                    .rare
                    .iter()
                    .find(|row| row.id == *id)
                    .cloned()
            })
            .unwrap_or(RarePlanetBonus {
                id: String::new(),
                name: String::new(),
                weight: 0,
                iron: 0.0,
                silicon: 0.0,
                deuterium: 0.0,
                energy: 0.0,
                shipyard_speed: 0.0,
                fields: 0,
            })
    }

    fn complete_wonder_stage(&mut self, now: u64) {
        let Some(mut job) = self.wonder.take() else {
            return;
        };
        let Some(spec) = self.config.wonders.iter().find(|s| s.id == job.id) else {
            return;
        };
        if job.stage + 1 >= spec.stages.len() {
            self.push_event(now, format!("{} complete", spec.name));
            self.completed_wonders.push(job.id);
        } else {
            job.stage += 1;
            job.contributed = Resources::ZERO;
            job.completes_at = None;
            self.push_event(now, format!("{} stage {} complete", spec.name, job.stage));
            self.wonder = Some(job);
        }
    }

    fn occupied(&self, coord: Coord) -> bool {
        coord == self.home
            || self.colonies.iter().any(|c| c.coord == coord)
            || self.outposts.iter().any(|o| o.coord == coord)
    }

    /// Turn a fleet around. Outbound recall uses elapsed time as the return.
    /// `id` is the mission's stable id, so a catch-up that finishes another
    /// fleet cannot retarget this one.
    pub fn recall_fleet(&mut self, id: u64, now_unix: u64) -> Result<(), FleetError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let mission = self
            .missions
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(FleetError::UnknownMission { id })?;
        match mission.phase {
            MissionPhase::Return => return Err(FleetError::AlreadyReturning),
            MissionPhase::Outbound => {
                let elapsed = now.saturating_sub(mission.launched_at);
                mission.phase = MissionPhase::Return;
                mission.phase_ends_at = now.saturating_add(elapsed);
            }
            MissionPhase::Action => {
                let ret = mission.outbound_secs;
                mission.phase = MissionPhase::Return;
                mission.phase_ends_at = now.saturating_add(ret);
            }
        }
        Ok(())
    }

    pub fn missing_ship_prereqs(&self, kind: ShipKind) -> Vec<PrereqGap> {
        let spec = self.config.ships.get(kind);
        spec.prereq_techs
            .iter()
            .filter_map(|pre| {
                let actual = self.tech_level(&pre.id);
                (actual < pre.level).then_some(PrereqGap::Tech {
                    id: pre.id.clone(),
                    required: pre.level,
                    actual,
                })
            })
            .collect()
    }

    /// Config stats scaled by weapons, shields, armour, and the ship's drive.
    pub fn ship_stats(&self, kind: ShipKind) -> crate::ShipStats {
        let spec = self.config.ships.get(kind);
        let attack = spec.attack * (1.0 + self.modifier(ModifierKind::WeaponDamage));
        let shield = spec.shield * (1.0 + self.modifier(ModifierKind::ShieldStrength));
        let hull = spec.hull * (1.0 + self.modifier(ModifierKind::Armour));
        let drive_bonus = self
            .config
            .techs
            .get(&spec.drive)
            .map(|tech| {
                let level = f64::from(self.tech_level(&tech.id));
                tech.modifiers
                    .iter()
                    .filter(|m| m.kind == ModifierKind::DriveSpeed)
                    .map(|m| m.per_level * level)
                    .sum::<f64>()
            })
            .unwrap_or(0.0);
        crate::ShipStats {
            attack,
            shield,
            hull,
            cargo: spec.cargo,
            speed: spec.speed
                * (1.0 + drive_bonus + self.wonder_modifier(ModifierKind::DriveSpeed)),
            fuel: spec.fuel,
        }
    }

    pub fn tech_level(&self, id: &str) -> u32 {
        self.tech_levels.get(id).copied().unwrap_or(0)
    }

    pub fn ancient_unlocked(&self) -> bool {
        (self.prestige.runs.len() as u32) >= self.config.techs.ancient_prestige_runs
            && self.config.techs.ancient_prestige_runs > 0
    }

    pub fn keystone_complete(&self, branch: TechBranch) -> bool {
        self.config
            .techs
            .keystone(branch)
            .is_some_and(|spec| self.tech_level(&spec.id) > 0)
    }

    pub fn tech_node_state(&self, id: &str) -> TechNodeState {
        let Some(spec) = self.config.techs.get(id) else {
            return TechNodeState::Locked;
        };
        if spec.hidden && spec.branch == TechBranch::Ancient && !self.ancient_unlocked() {
            return TechNodeState::Hidden;
        }
        if self.research.as_ref().is_some_and(|job| job.tech == id) {
            return TechNodeState::InProgress;
        }
        if let Some(group) = spec.exclusive_group.as_deref()
            && let Some(picked) = self.exclusive_picks.get(group)
            && picked != id
        {
            return TechNodeState::Excluded;
        }
        let level = self.tech_level(id);
        if level > 0 {
            return TechNodeState::Researched;
        }
        if self.research_prereq_gaps(spec).is_empty() {
            TechNodeState::Available
        } else {
            TechNodeState::Locked
        }
    }

    /// Fill exclusive picks from researched members when a v20 save has none.
    pub fn infer_exclusive_picks(&mut self) {
        for spec in self.config.techs.iter() {
            let Some(group) = spec.exclusive_group.as_deref() else {
                continue;
            };
            if self.exclusive_picks.contains_key(group) {
                continue;
            }
            if self.tech_level(&spec.id) > 0 {
                self.exclusive_picks
                    .insert(group.to_string(), spec.id.clone());
            }
        }
    }

    /// Build queue length including purchased extra slots.
    pub fn build_queue_cap(&self) -> usize {
        let extra = self
            .species
            .as_ref()
            .map(|run| run.queue_slots)
            .unwrap_or(0);
        self.config
            .max_build_queue
            .saturating_add(self.prestige.queue_slots(&self.config.prestige) as usize)
            .saturating_add(extra as usize)
    }

    fn apply_prestige_start(&mut self) {
        let (metal, crystal, deuterium) = self.prestige.starting_bonus(&self.config.prestige);
        self.resources.iron += metal;
        self.resources.silicon += crystal;
        self.resources.deuterium += deuterium;
        if !self.prestige.runs.is_empty() {
            let starting = self.config.techs.starting_techs.clone();
            for id in starting {
                let level = self.tech_levels.entry(id).or_insert(0);
                *level = (*level).max(1);
            }
            self.infer_exclusive_picks();
        }
    }

    fn building_levels(&self) -> u32 {
        let home: u32 = BuildingKind::ALL
            .iter()
            .map(|&k| self.buildings.get(k))
            .sum();
        let colonies: u32 = self
            .colonies
            .iter()
            .map(|c| {
                BuildingKind::ALL
                    .iter()
                    .map(|&k| c.buildings.get(k))
                    .sum::<u32>()
            })
            .sum();
        home + colonies
    }

    fn research_levels(&self) -> u32 {
        self.tech_levels.values().copied().sum()
    }

    /// Idle-game score used by the prestige formula in config.
    pub fn prestige_score(&self) -> u32 {
        self.config.prestige.run_score(
            self.building_levels(),
            self.research_levels(),
            self.colonies.len() as u32,
            self.completed_wonders.len() as u32,
        )
    }

    pub fn can_prestige(&self) -> bool {
        self.victory.is_some()
            || self
                .config
                .prestige
                .can_reset(self.completed_wonders.len() as u32, self.prestige_score())
    }

    /// Copy meta progress onto a fresh galaxy and apply starting bonuses.
    pub fn inherit_prestige(&mut self, mut prestige: Prestige) {
        let next_index = if prestige.runs.is_empty() && prestige.lifetime == 0 {
            1
        } else {
            prestige.current.index.saturating_add(1).max(1)
        };
        prestige.current = RunStats::new(next_index, self.last_tick_unix);
        self.resources = self.config.starting_resources;
        self.prestige = prestige;
        self.apply_prestige_start();
    }

    fn prestige_earned_for(&self, score: u32) -> u32 {
        let per = self.config.prestige.points_per_currency.max(1);
        (score / per).max(1)
    }

    pub fn prestige_preview(&self) -> PrestigePreview {
        let score = self.prestige_score();
        let eligible = self.can_prestige();
        let earned = if eligible {
            self.prestige_earned_for(score)
        } else {
            0
        };
        let mut run = self.prestige.current.clone();
        run.score = score;
        run.prestige_earned = earned;
        run.wonders = self.completed_wonders.len() as u32;
        run.colonies = self.colonies.len() as u32;
        run.building_levels = self.building_levels();
        run.research_levels = self.research_levels();
        PrestigePreview {
            eligible,
            score,
            earned,
            currency_after: self.prestige.currency.saturating_add(earned),
            kept: PRESTIGE_KEPT,
            lost: PRESTIGE_LOST,
            run,
        }
    }

    pub fn buy_prestige_bonus(&mut self, id: &str, now_unix: u64) -> Result<(), PrestigeError> {
        self.catch_up(now_unix);
        let spec = self
            .config
            .prestige
            .bonus(id)
            .cloned()
            .ok_or_else(|| PrestigeError::UnknownBonus { id: id.to_string() })?;
        if self.prestige.currency < spec.cost {
            return Err(PrestigeError::CannotAfford {
                cost: spec.cost,
                have: self.prestige.currency,
            });
        }
        let old_speed = self.research_speed();
        self.prestige.currency -= spec.cost;
        *self.prestige.owned.entry(spec.id).or_insert(0) += 1;
        self.rescale_research(self.last_tick_unix, old_speed, self.research_speed());
        Ok(())
    }

    /// Wipe the run, keep meta progress, start a new galaxy from a derived seed.
    pub fn prestige_reset(&mut self, now_unix: u64) -> Result<PrestigePreview, PrestigeError> {
        self.catch_up(now_unix);
        let preview = self.prestige_preview();
        if !preview.eligible {
            return Err(PrestigeError::NotEligible {
                score: preview.score,
                min_score: self.config.prestige.min_score,
            });
        }
        let now = now_unix.max(self.last_tick_unix);
        let mut closed = preview.run.clone();
        closed.ended_at = Some(now);
        self.prestige.currency = self.prestige.currency.saturating_add(preview.earned);
        self.prestige.lifetime = self.prestige.lifetime.saturating_add(preview.earned);
        self.prestige.runs.push(closed);
        let next_index = self.prestige.current.index.saturating_add(1);
        self.prestige.current = RunStats::new(next_index, now);

        let config = self.config.clone();
        let prestige = self.prestige.clone();
        let achievements = self.achievements.clone();
        let season = self.season.clone();
        let rival_settings = self.rivals.settings();
        let deterministic = self.deterministic;
        let next_seed = self
            .galaxy_seed
            .wrapping_add(u64::from(next_index))
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);

        let mut next = Self::with_config(now, config);
        next.prestige = prestige;
        next.achievements = achievements;
        next.deterministic = deterministic;
        next.galaxy_seed = next_seed;
        next.rng_seed = next_seed;
        next.generator_version = crate::GALAXY_GENERATOR_VERSION;
        let (home_kind, home_size) = planet_profile(next_seed, Coord::HOME, next.generator_version);
        next.home_kind = home_kind;
        next.home_size = home_size;
        next.outposts = generate_outposts(
            next_seed,
            now,
            &next.config.outposts,
            &[Coord::HOME],
            next.generator_version,
        );
        next.seed_frontier();
        next.victory = None;
        next.allied_since = None;
        next.season = season;
        next.species = self.species.clone();
        if let Some(run) = next.species.as_mut() {
            run.brew_until = None;
            if let Some(ship) = run.flagship.as_mut() {
                ship.coord = crate::Coord::HOME;
                ship.arrives_at = None;
                ship.anchored = false;
            }
        }
        next.seed_pirate_home(now);
        if rival_settings.enabled {
            let _ = next.spawn_rivals(&crate::RivalCatalog::default(), rival_settings);
        }
        next.resources = next.config.starting_resources;
        next.apply_prestige_start();
        next.daily = DailyBoard::default();
        next.sync_daily(now);
        if !deterministic {
            next.tutorial = crate::TutorialBoard::start();
        }
        next.push_event(
            now,
            format!(
                "prestige: run {} score {}. earned {}. new galaxy",
                preview.run.index, preview.score, preview.earned
            ),
        );
        next.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Prestige,
            actor: "player".into(),
            action: "reset".into(),
            objects: vec![format!("run {}", preview.run.index)],
            outcome: format!("earned {}", preview.earned),
            ..crate::HistoryDraft::default()
        });
        next.evaluate_achievements(now);
        *self = next;
        Ok(preview)
    }

    /// Tech modifiers plus the home planet's type bonus.
    pub fn modifier(&self, kind: ModifierKind) -> f64 {
        self.modifier_for(self.home_kind, kind)
    }

    /// Tech modifiers plus this planet type's production bonus.
    pub fn modifier_for(&self, planet: PlanetType, kind: ModifierKind) -> f64 {
        self.tech_modifier(kind) + self.planet_bonus(planet, kind)
    }

    pub(crate) fn tech_modifier(&self, kind: ModifierKind) -> f64 {
        self.tech_modifier_range(kind, self.last_tick_unix, self.last_tick_unix)
    }

    fn tech_modifier_range(&self, kind: ModifierKind, from: u64, to: u64) -> f64 {
        let techs = self
            .config
            .techs
            .iter()
            .map(|spec| {
                let level = f64::from(self.tech_level(&spec.id));
                spec.modifiers
                    .iter()
                    .filter(|m| m.kind == kind)
                    .map(|m| m.per_level * level)
                    .sum::<f64>()
            })
            .sum::<f64>();
        techs
            + self.wonder_modifier(kind)
            + self.prestige_output(kind)
            + self.diplomacy_modifier(kind)
            + self.species_modifier(kind, from)
            + self
                .laws
                .modifier_range(kind, &self.config.laws, from, to, self.home)
    }

    fn species_modifier(&self, kind: ModifierKind, from: u64) -> f64 {
        let Some(run) = self.species.as_ref() else {
            return 0.0;
        };
        match kind {
            ModifierKind::Espionage => (run.espionage - 1.0).max(0.0),
            ModifierKind::MetalOutput if run.brew_until.is_some_and(|until| until > from) => 0.2,
            _ => 0.0,
        }
    }

    fn diplomacy_modifier(&self, kind: ModifierKind) -> f64 {
        if !self.config.diplomacy.enabled {
            return 0.0;
        }
        self.config
            .diplomacy
            .iter()
            .map(|faction| {
                let standing = self.diplomacy.standing(&faction.id);
                faction
                    .tier(standing)
                    .map(|tier| tier.modifier(kind))
                    .unwrap_or(0.0)
            })
            .sum()
    }

    fn prestige_output(&self, kind: ModifierKind) -> f64 {
        match kind {
            ModifierKind::MetalOutput
            | ModifierKind::CrystalOutput
            | ModifierKind::DeuteriumOutput => {
                self.prestige.production_bonus(&self.config.prestige)
            }
            _ => 0.0,
        }
    }

    fn wonder_modifier(&self, kind: ModifierKind) -> f64 {
        self.completed_wonders
            .iter()
            .filter_map(|id| self.config.wonders.iter().find(|spec| spec.id == *id))
            .flat_map(|spec| spec.modifiers.iter())
            .filter(|m| m.kind == kind)
            .map(|m| m.value)
            .sum()
    }

    fn planet_bonus(&self, planet: PlanetType, kind: ModifierKind) -> f64 {
        let bonus = self.config.planet_types.get(planet);
        match kind {
            ModifierKind::MetalOutput => bonus.iron,
            ModifierKind::CrystalOutput => bonus.silicon,
            ModifierKind::DeuteriumOutput => bonus.deuterium,
            ModifierKind::EnergyOutput => bonus.energy,
            _ => 0.0,
        }
    }

    pub fn missing_research_prereqs(&self, tech: &str) -> Result<Vec<PrereqGap>, ResearchError> {
        let spec = self
            .config
            .techs
            .get(tech)
            .ok_or_else(|| ResearchError::UnknownTech {
                id: tech.to_string(),
            })?;
        Ok(self.research_prereq_gaps(spec))
    }

    fn research_prereq_gaps(&self, spec: &crate::TechSpec) -> Vec<PrereqGap> {
        let mut gaps = Vec::new();
        for pre in &spec.prereq_buildings {
            let actual = self.buildings.get(pre.building);
            if actual < pre.level {
                gaps.push(PrereqGap::Building {
                    building: pre.building,
                    required: pre.level,
                    actual,
                });
            }
        }
        for pre in &spec.prereq_techs {
            let actual = self.tech_level(&pre.id);
            if actual < pre.level {
                gaps.push(PrereqGap::Tech {
                    id: pre.id.clone(),
                    required: pre.level,
                    actual,
                });
            }
        }
        gaps
    }

    pub fn build_cost(&self, kind: BuildingKind) -> Resources {
        self.config
            .buildings
            .get(kind)
            .cost(self.projected_level(kind))
    }

    pub fn build_time_secs(&self, kind: BuildingKind) -> u64 {
        self.duration_secs(kind, self.projected_level(kind))
    }

    fn projected_level(&self, kind: BuildingKind) -> u32 {
        self.projected_level_before(kind, self.build_queue.len())
    }

    fn projected_level_before(&self, kind: BuildingKind, index: usize) -> u32 {
        self.buildings.get(kind)
            + self.build_queue[..index]
                .iter()
                .filter(|job| job.building == kind)
                .count() as u32
    }

    fn prereqs_ok_at(&self, index: usize) -> bool {
        let kind = self.build_queue[index].building;
        let prereqs = self.config.buildings.get(kind).prerequisites.clone();
        prereqs
            .iter()
            .all(|&(req, required)| self.projected_level_before(req, index) >= required)
    }

    /// After a cancellation, drop jobs whose prereqs no longer hold and
    /// recompute `from_level` / paid cost for the rest.
    fn reprice_queue(&mut self) {
        let centers = crate::finance::build_centers(self.home);
        let mut i = 0;
        while i < self.build_queue.len() {
            if !self.prereqs_ok_at(i) {
                let job = self.build_queue.remove(i);
                self.resources = self.resources.saturating_add(job.paid);
                self.refund_spend(&centers, self.refund_credits(job.credits, job.paid));
                continue;
            }
            let kind = self.build_queue[i].building;
            let from_level = self.projected_level_before(kind, i);
            let new_cost = self.config.buildings.get(kind).cost(from_level);
            let old_paid = self.build_queue[i].paid;
            let old_credits = self.build_queue[i].credits;
            self.resources = self.resources.saturating_add(old_paid);
            if !self.resources.can_afford(new_cost) {
                self.build_queue.remove(i);
                self.refund_spend(&centers, self.refund_credits(old_credits, old_paid));
                continue;
            }
            self.resources = self.resources.saturating_sub(new_cost);
            self.build_queue[i].from_level = from_level;
            self.build_queue[i].paid = new_cost;
            self.build_queue[i].credits =
                self.replace_spend(&centers, old_credits, old_paid, new_cost);
            i += 1;
        }
    }

    fn duration_secs(&self, kind: BuildingKind, from_level: u32) -> u64 {
        let base = duration_with_robotics(
            kind,
            from_level,
            self.buildings.robotics_factory,
            &self.config,
        );
        let mult = self
            .species
            .as_ref()
            .map(|run| run.construction)
            .unwrap_or(1.0);
        ((base as f64) * mult).ceil().max(1.0) as u64
    }

    fn start_head(&mut self, now_unix: u64) {
        let robotics = self.buildings.robotics_factory;
        let construction = self
            .species
            .as_ref()
            .map(|run| run.construction)
            .unwrap_or(1.0);
        start_queue_head(
            &mut self.build_queue,
            robotics,
            &self.config,
            now_unix.max(self.last_tick_unix),
            construction,
        );
    }

    fn start_colony_head(&mut self, index: usize, now_unix: u64) {
        let robotics = self.colonies[index].buildings.robotics_factory;
        let construction = self
            .species
            .as_ref()
            .map(|run| run.construction)
            .unwrap_or(1.0);
        start_queue_head(
            &mut self.colonies[index].build_queue,
            robotics,
            &self.config,
            now_unix.max(self.last_tick_unix),
            construction,
        );
    }

    fn reprice_colony_queue(&mut self, index: usize) {
        let mut i = 0;
        while i < self.colonies[index].build_queue.len() {
            let kind = self.colonies[index].build_queue[i].building;
            let prereqs = self.config.buildings.get(kind).prerequisites.clone();
            let ok = prereqs.iter().all(|&(req, required)| {
                projected_on(
                    &self.colonies[index].buildings,
                    &self.colonies[index].build_queue,
                    req,
                    i,
                ) >= required
            });
            if !ok {
                let job = self.colonies[index].build_queue.remove(i);
                self.colonies[index].resources =
                    self.colonies[index].resources.saturating_add(job.paid);
                let coord = self.colonies[index].coord;
                self.refund_spend(
                    &crate::finance::build_centers(coord),
                    self.refund_credits(job.credits, job.paid),
                );
                continue;
            }
            let from_level = projected_on(
                &self.colonies[index].buildings,
                &self.colonies[index].build_queue,
                kind,
                i,
            );
            let new_cost = self.config.buildings.get(kind).cost(from_level);
            let old_paid = self.colonies[index].build_queue[i].paid;
            let old_credits = self.colonies[index].build_queue[i].credits;
            self.colonies[index].resources =
                self.colonies[index].resources.saturating_add(old_paid);
            let coord = self.colonies[index].coord;
            let centers = crate::finance::build_centers(coord);
            if !self.colonies[index].resources.can_afford(new_cost) {
                self.colonies[index].build_queue.remove(i);
                self.refund_spend(&centers, self.refund_credits(old_credits, old_paid));
                continue;
            }
            self.colonies[index].resources =
                self.colonies[index].resources.saturating_sub(new_cost);
            self.colonies[index].build_queue[i].from_level = from_level;
            self.colonies[index].build_queue[i].paid = new_cost;
            self.colonies[index].build_queue[i].credits =
                self.replace_spend(&centers, old_credits, old_paid, new_cost);
            i += 1;
        }
    }

    fn start_ship_head(&mut self, now_unix: u64) {
        let Some(job) = self.ship_queue.first() else {
            return;
        };
        if job.completes_at.is_some() {
            return;
        }
        let duration = self.yard_time_secs(job);
        let now = now_unix.max(self.last_tick_unix);
        let job = &mut self.ship_queue[0];
        job.started_at = Some(now);
        job.completes_at = Some(now.saturating_add(duration));
    }

    fn advance_missions_at(&mut self, done_at: u64) {
        loop {
            let Some(index) = self
                .missions
                .iter()
                .position(|m| m.phase_ends_at == done_at)
            else {
                return;
            };
            match self.missions[index].phase {
                MissionPhase::Outbound => {
                    let hold = self.missions[index].hold_secs;
                    self.missions[index].phase = MissionPhase::Action;
                    self.missions[index].phase_ends_at = done_at.saturating_add(hold);
                    if self.apply_expedition_at(index, done_at) {
                        continue;
                    }
                }
                MissionPhase::Action => {
                    if self.try_colonize_at(index, done_at) {
                        continue;
                    }
                    if self.try_attack_at(index, done_at) {
                        continue;
                    }
                    if self.try_invade_at(index, done_at) {
                        continue;
                    }
                    if self.try_spy_at(index, done_at) {
                        continue;
                    }
                    if self.missions[index].transport {
                        let dest = self.missions[index].dest;
                        let cargo = self.missions[index].cargo;
                        let accepted = self.accept_up_to_cap(dest, cargo);
                        let leftover = cargo.saturating_sub(accepted);
                        self.missions[index].cargo = leftover;
                        if cargo_has_mass(accepted) {
                            self.note_delivery(
                                done_at,
                                format!("delivered {} to {dest}", format_cargo(accepted)),
                            );
                        }
                        if cargo_has_mass(leftover) {
                            self.note_stall(
                                done_at,
                                format!("stall: bounced cargo at {dest} (storage full)"),
                            );
                        }
                    }
                    let ret = self.missions[index].outbound_secs;
                    self.missions[index].phase = MissionPhase::Return;
                    self.missions[index].phase_ends_at = done_at.saturating_add(ret);
                }
                MissionPhase::Return => {
                    let mission = self.missions.remove(index);
                    self.add_fleet_at(mission.origin, mission.ships);
                    if mission.transport {
                        self.credit_at(mission.origin, mission.cargo);
                    }
                }
            }
        }
    }

    /// Apply a pending expedition when Action starts. Returns true if the mission was removed.
    fn apply_expedition_at(&mut self, index: usize, done_at: u64) -> bool {
        let Some(seed) = self.missions[index].expedition_seed else {
            return false;
        };
        if self.missions[index].expedition_applied {
            return false;
        }
        self.missions[index].expedition_applied = true;
        let ships = self.missions[index].ships;
        let hold = self.missions[index].hold_secs;
        let astro = self.expedition_astro();
        let result = roll_expedition(
            seed,
            &ships,
            astro,
            hold,
            &self.config.expedition,
            &self.config.ships,
        );
        match result {
            ExpeditionResult::Nothing => {
                self.push_event(done_at, "expedition: nothing found".into());
                false
            }
            ExpeditionResult::Resources(found) => {
                self.credit_capped(found);
                self.push_event(done_at, format!("expedition: found {}", found.brief()));
                false
            }
            ExpeditionResult::Ships(found) => {
                for kind in ShipKind::ALL {
                    self.missions[index].ships.add(kind, found.get(kind));
                }
                let n = found.get(self.config.expedition.ship_kind);
                self.push_event(
                    done_at,
                    format!("expedition: found {n} {}", self.config.expedition.ship_kind),
                );
                false
            }
            ExpeditionResult::Delay { extra_secs } => {
                self.missions[index].phase_ends_at = self.missions[index]
                    .phase_ends_at
                    .saturating_add(extra_secs);
                self.push_event(done_at, format!("expedition: delayed {extra_secs}s"));
                false
            }
            ExpeditionResult::EarlyReturn => {
                let ret = self.missions[index].outbound_secs;
                self.missions[index].phase = MissionPhase::Return;
                self.missions[index].phase_ends_at = done_at.saturating_add(ret);
                self.push_event(done_at, "expedition: returning early".into());
                false
            }
            ExpeditionResult::Pirates { enemy } => {
                let mut fight_seed = seed.wrapping_add(0xA5A5_A5A5_A5A5_A5A5);
                let report = self.resolve_player_attack(ships, enemy, &mut fight_seed);
                self.push_combat_log(done_at, report.clone());
                let dest = self.missions[index].dest;
                self.note_fight_debris(done_at, dest, &report);
                self.missions[index].ships = report.attacker_after;
                let survivors: u32 = ShipKind::ALL
                    .iter()
                    .map(|&k| self.missions[index].ships.get(k))
                    .sum();
                self.push_event(
                    done_at,
                    format!(
                        "expedition: pirates. {} rounds. survivors {survivors}",
                        report.rounds
                    ),
                );
                if self.missions[index].ships.is_empty() {
                    self.missions.remove(index);
                    return true;
                }
                let ret = self.missions[index].outbound_secs;
                self.missions[index].phase = MissionPhase::Return;
                self.missions[index].phase_ends_at = done_at.saturating_add(ret);
                false
            }
            ExpeditionResult::Loss => {
                self.push_event(done_at, "expedition: fleet lost".into());
                self.missions.remove(index);
                true
            }
        }
    }

    fn credit_capped(&mut self, found: Resources) {
        for kind in ResourceKind::ALL {
            let cap = self.storage_cap(kind);
            let current = self.resources.get(kind);
            self.resources
                .set(kind, (current + found.get(kind)).min(cap));
        }
    }

    /// Found a colony at Action, or leave the mission to return.
    fn try_colonize_at(&mut self, index: usize, _done_at: u64) -> bool {
        if !self.missions[index].colonize {
            return false;
        }
        let dest = self.missions[index].dest;
        let planets = 1 + self.colonies.len() as u32;
        let no_ship = self.missions[index].ships.get(ShipKind::ColonyShip) == 0;
        if self.occupied(dest) || planets >= self.colony_cap() || no_ship {
            return false;
        }
        let mut ships = self.missions.remove(index).ships;
        ships.saturating_sub(ShipKind::ColonyShip, 1);
        let (kind, size, rare) = if let Some(world) = self.frontier.world_at(dest).cloned() {
            (world.kind, world.size, world.rare)
        } else {
            let (kind, size) = planet_profile(self.galaxy_seed, dest, self.generator_version);
            (kind, size, None)
        };
        self.colonies.push(Colony {
            coord: dest,
            kind,
            size,
            resources: Resources::ZERO,
            buildings: Buildings::default(),
            fleet: ships,
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare,
            goods: BTreeMap::new(),
        });
        true
    }

    fn rebuild_outposts_at(&mut self, done_at: u64) {
        let interval = self.config.outposts.rebuild_secs;
        let progress = self.fighter_equivalent();
        let cfg = self.config.outposts.clone();
        let def_mult = self.config.victory.homeworld_defence_mult.max(1.0);
        let gar_mult = self.config.victory.homeworld_garrison_mult.max(1.0);
        let victory_on = self.config.victory.enabled;
        for outpost in &mut self.outposts {
            if outpost.next_rebuild_at(interval) == done_at {
                let mut defences = scaled_defence(progress, &cfg);
                let mut garrison = scaled_garrison(progress, &cfg);
                if outpost.homeworld && victory_on {
                    let n = ((defences.get(ShipKind::LightFighter) as f64) * def_mult)
                        .ceil()
                        .max(1.0) as u32;
                    defences = Fleet::default();
                    defences.add(ShipKind::LightFighter, n);
                    garrison = ((garrison as f64) * gar_mult).ceil().max(1.0) as u32;
                }
                outpost.defences = defences;
                outpost.garrison = garrison;
                outpost.last_rebuild_at = done_at;
            }
        }
    }

    fn fighter_equivalent(&self) -> f64 {
        let stats = self.ship_stats(ShipKind::LightFighter);
        let unit = (stats.attack + stats.hull).max(1.0);
        self.fleet_combat_points() / unit
    }

    /// Write a spy report on arrival. Returns true when every ship was lost.
    fn try_spy_at(&mut self, index: usize, done_at: u64) -> bool {
        if !self.missions[index].spy {
            return false;
        }
        let dest = self.missions[index].dest;
        let probes = self.missions[index].ships.get(ShipKind::EspionageProbe);
        let own = self.modifier(ModifierKind::Espionage);
        let outcome = resolve_spy(probes, own, &mut self.rng_seed, &self.config.espionage);
        let show_resources = reveals(&outcome.tier, "resources", &self.config.espionage);
        let show_fleet = reveals(&outcome.tier, "fleet", &self.config.espionage);
        let show_defences = reveals(&outcome.tier, "defences", &self.config.espionage);
        let show_garrison = reveals(&outcome.tier, "garrison", &self.config.espionage);
        // Outposts have no parked visiting fleet; orbital ships live in `defences`.
        let (kind, size, defences, garrison) = match self.outpost(dest) {
            Some(outpost) => (
                Some(outpost.kind),
                Some(outpost.size),
                outpost.defences,
                Some(outpost.garrison),
            ),
            None => (None, None, Fleet::default(), None),
        };
        let delay = light_delay_secs(self.home, dest, &self.config.orbits);
        let quality = 1.0 + own;
        let range = light_range_au(self.home, dest, &self.config.orbits);
        let error = sensor_error(range, quality, &self.config.espionage);
        let deceived = outcome.detected;
        let decoy = self.config.espionage.decoy_bias;
        let resources = show_resources.then_some(noisy_resources(
            Resources::ZERO,
            error,
            deceived,
            decoy,
            &mut self.rng_seed,
        ));
        let fleet = show_fleet.then_some(noisy_fleet(
            Fleet::default(),
            error,
            deceived,
            decoy,
            &mut self.rng_seed,
        ));
        let defences = show_defences.then_some(noisy_fleet(
            defences,
            error,
            deceived,
            decoy,
            &mut self.rng_seed,
        ));
        let garrison = if show_garrison {
            garrison.map(|n| {
                crate::intel::apply_count_error(
                    n,
                    error,
                    signed_unit(&mut self.rng_seed),
                    deceived,
                    decoy,
                )
            })
        } else {
            None
        };
        let confidence = intel_confidence(error, 0, outcome.detected, &self.config.espionage);
        let id = self.next_report_id;
        self.next_report_id = self.next_report_id.saturating_add(1);
        self.spy_reports.push(SpyReport {
            id,
            target: dest,
            created_at: done_at,
            probes_sent: probes,
            probes_lost: outcome.probes_lost,
            detected: outcome.detected,
            score: outcome.score,
            tier: outcome.tier.clone(),
            resources,
            fleet,
            defences,
            garrison,
            kind,
            size,
            formation: if show_fleet {
                Some(crate::Formation::Neutral.as_key().into())
            } else {
                None
            },
            stance: if show_fleet {
                Some(crate::Stance::Neutral.as_key().into())
            } else {
                None
            },
            source: "probe".into(),
            observed_at: done_at,
            arrives_at: done_at.saturating_add(delay),
            confidence,
            deceived,
        });
        self.missions[index]
            .ships
            .saturating_sub(ShipKind::EspionageProbe, outcome.probes_lost);
        if self.missions[index].ships.is_empty() {
            self.missions.remove(index);
            return true;
        }
        false
    }

    /// Fight outpost defences. Returns true when the attacking fleet is gone.
    fn try_attack_at(&mut self, index: usize, done_at: u64) -> bool {
        if !self.missions[index].attack {
            return false;
        }
        let dest = self.missions[index].dest;
        let Some(outpost_idx) = self.outposts.iter().position(|o| o.coord == dest) else {
            return false;
        };
        let ships = self.missions[index].ships;
        let defences = self.outposts[outpost_idx].defences;
        let mut rng = self.rng_seed;
        let report = self.resolve_player_attack(ships, defences, &mut rng);
        self.rng_seed = rng;
        self.outposts[outpost_idx].defences = report.defender_after;
        self.missions[index].ships = report.attacker_after;
        self.push_combat_log(done_at, report.clone());
        self.note_fight_debris(done_at, dest, &report);
        self.push_event(done_at, format_outpost_attack(&report, dest));
        if self.missions[index].ships.is_empty() {
            self.missions.remove(index);
            return true;
        }
        false
    }

    /// Land troops. Returns true when the outpost becomes a colony (mission consumed).
    fn try_invade_at(&mut self, index: usize, done_at: u64) -> bool {
        if !self.missions[index].invade {
            return false;
        }
        let dest = self.missions[index].dest;
        let Some(outpost_idx) = self.outposts.iter().position(|o| o.coord == dest) else {
            return false;
        };
        if self.outposts[outpost_idx].defences.total() > 0 {
            self.push_event(
                done_at,
                format!("outpost {dest}: invade refused (defences remain)"),
            );
            return false;
        }
        let used = 1 + self.colonies.len() as u32;
        let cap = self.colony_cap();
        if used >= cap {
            self.push_event(
                done_at,
                format!("outpost {dest}: invade refused (colony cap {used}/{cap})"),
            );
            return false;
        }
        let troops = self.missions[index]
            .ships
            .get(ShipKind::TroopTransport)
            .saturating_mul(self.config.outposts.troop_strength);
        let garrison = self.outposts[outpost_idx].garrison;
        let mult = self.config.defences.tactical_ground_mult;
        let seed = self.rng_seed;
        let ground = resolve_ground(
            scale_ground(troops, mult),
            scale_ground(garrison, mult),
            self.config.outposts.variance,
            &mut self.rng_seed,
        );
        let mut report = CombatReport {
            rounds: 0,
            attacker_before: self.missions[index].ships,
            defender_before: Fleet::default(),
            attacker_after: self.missions[index].ships,
            defender_after: Fleet::default(),
            winner: ground.winner,
            ground: Some(ground.clone()),
            seed: 0,
            frames: Vec::new(),
            layered: false,
        };
        let landed = self.missions[index].ships.get(ShipKind::TroopTransport);
        self.missions[index]
            .ships
            .saturating_sub(ShipKind::TroopTransport, landed);
        report.attacker_after = self.missions[index].ships;
        report = report.with_seed_and_endpoints(seed);
        self.push_combat_log(done_at, report.clone());
        self.note_fight_debris(done_at, dest, &report);
        if ground.winner != CombatWinner::Attacker {
            self.push_event(done_at, format_outpost_invade(&report, dest));
            return false;
        }
        self.apply_faction_delta("pirates", |f| f.conquest_delta, "conquest", done_at);
        self.outposts_conquered = self.outposts_conquered.saturating_add(1);
        self.push_event(done_at, format_outpost_invade(&report, dest));
        let outpost = self.outposts.remove(outpost_idx);
        let ships = self.missions.remove(index).ships;
        self.colonies.push(Colony {
            coord: outpost.coord,
            kind: outpost.kind,
            size: outpost.size,
            resources: Resources::ZERO,
            buildings: Buildings::default(),
            fleet: ships,
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: outpost.rare,
            goods: BTreeMap::new(),
        });
        self.evaluate_achievements(done_at);
        self.maybe_declare_victory(done_at);
        self.evaluate_ladder(done_at);
        self.evaluate_frontier(done_at);
        true
    }

    /// Seconds until the next raid when it has been announced, else `None`.
    pub fn raid_countdown_secs(&self) -> Option<u64> {
        if !self.config.raid.enabled {
            return None;
        }
        let lands = self.next_raid_at?;
        if self.last_tick_unix + self.config.raid.min_warning_secs < lands {
            return None;
        }
        Some(lands.saturating_sub(self.last_tick_unix))
    }

    /// Run a raid now. The bridge and the scheduler share this path.
    /// Clears a pending schedule first so `catch_up` does not also resolve it.
    pub fn trigger_raid(&mut self, now_unix: u64) -> CombatReport {
        self.next_raid_at = None;
        self.catch_up(now_unix);
        self.resolve_raid_at(self.last_tick_unix)
    }

    fn resolve_raid_at(&mut self, done_at: u64) -> CombatReport {
        self.finish_pending_tactical();
        if self.interactive_tactics && self.defences.is_empty() {
            self.begin_home_raid_pending(done_at);
            if let Some(pending) = &self.pending_tactical {
                return CombatReport {
                    rounds: 0,
                    attacker_before: pending.attacker,
                    defender_before: pending.defender,
                    attacker_after: pending.attacker,
                    defender_after: pending.defender,
                    winner: CombatWinner::Draw,
                    ground: None,
                    seed: pending.seed,
                    frames: Vec::new(),
                    layered: false,
                };
            }
        }
        self.resolve_raid_now(done_at)
    }

    fn begin_home_raid_pending(&mut self, done_at: u64) {
        let pirates = self.pirate_fleet();
        self.next_raid_at = None;
        self.pending_tactical = Some(crate::PendingTactical {
            seed: self.rng_seed,
            attacker: pirates,
            defender: self.fleet,
            reserves: self.reserves,
            atk_plan: crate::TacticalPlan::default(),
            def_plan: self.player_plan(),
            actions: Vec::new(),
            next_at: done_at.saturating_add(1),
            queued: None,
        });
    }

    /// Hold, retreat, or commit reserves for the current round. Ignored when idle.
    pub fn queue_tactical_action(&mut self, action: RoundAction) {
        let _ = self.queue_tactical_action_as(action, crate::Actor::Player);
    }

    pub fn queue_tactical_action_as(
        &mut self,
        action: RoundAction,
        actor: crate::Actor,
    ) -> Result<(), crate::LeaderError> {
        self.authorize(actor, crate::Permission::AcceptBattle, None, 0.0)?;
        if let Some(pending) = &mut self.pending_tactical {
            pending.queued = Some(action);
        }
        Ok(())
    }

    /// Apply queued or auto-defaults until `now`, or until a live pause.
    pub fn progress_tactical(&mut self, now: u64) {
        loop {
            let Some(pending) = self.pending_tactical.clone() else {
                return;
            };
            if pending.next_at > now {
                return;
            }
            let action = pending.queued.unwrap_or_else(|| {
                if pending.def_plan.formation == crate::Formation::Neutral
                    && pending.def_plan.stance == crate::Stance::Neutral
                {
                    return RoundAction::Hold;
                }
                let peek = self.peek_tactical(&pending);
                let round = pending.actions.len() as u8;
                crate::auto_action(
                    pending.defender,
                    peek.defender_after,
                    pending.reserves,
                    self.fleet_tactics,
                    pending.def_plan.stance,
                    round.max(1),
                    &self.config.tactics,
                )
            });
            if let Some(slot) = &mut self.pending_tactical {
                slot.queued = None;
                slot.actions.push(action);
            }
            let report = self.peek_tactical(&self.pending_tactical.clone().expect("pending"));
            let finished = report.winner != CombatWinner::Draw
                || report.rounds >= self.config.tactics.max_rounds
                || action == RoundAction::Retreat
                || report.attacker_after.is_empty()
                || report.defender_after.is_empty();
            if finished {
                self.finish_home_raid(now.max(pending.next_at), report);
                return;
            }
            if let Some(slot) = &mut self.pending_tactical {
                slot.next_at = slot.next_at.saturating_add(1);
            }
            if self.interactive_tactics {
                return;
            }
        }
    }

    /// Resolve any in-progress fight with auto-defaults. Battles never block a save.
    pub fn finish_pending_tactical(&mut self) {
        if self.pending_tactical.is_none() {
            return;
        }
        let interactive = self.interactive_tactics;
        self.interactive_tactics = false;
        let until = self
            .pending_tactical
            .as_ref()
            .map(|p| {
                p.next_at
                    .saturating_add(u64::from(self.config.tactics.max_rounds) + 1)
            })
            .unwrap_or(self.last_tick_unix);
        self.progress_tactical(until);
        self.interactive_tactics = interactive;
    }

    pub fn set_formation(&mut self, key: &str) -> Result<(), crate::TacticsError> {
        let formation = crate::Formation::from_key(key)
            .ok_or_else(|| crate::TacticsError::Unknown(format!("formation {key}")))?;
        self.fleet_tactics.formation = formation;
        Ok(())
    }

    pub fn set_stance(&mut self, key: &str) -> Result<(), crate::TacticsError> {
        let stance = crate::Stance::from_key(key)
            .ok_or_else(|| crate::TacticsError::Unknown(format!("stance {key}")))?;
        self.fleet_tactics.stance = stance;
        Ok(())
    }

    pub fn set_retreat_below(&mut self, value: f64) -> Result<(), crate::TacticsError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(crate::TacticsError::Unknown("retreat must be 0..=1".into()));
        }
        self.fleet_tactics.retreat_below = value;
        Ok(())
    }

    fn player_plan(&self) -> crate::TacticalPlan {
        crate::TacticalPlan {
            formation: self.fleet_tactics.formation,
            stance: self.fleet_tactics.stance,
            terrain: crate::Terrain::Open,
        }
    }

    fn home_admiral(&self) -> Option<&crate::Admiral> {
        let id = self.home_admiral_id?;
        self.admirals.iter().find(|a| a.id == id)
    }

    fn grant_admiral_xp(&mut self, winner: CombatWinner) {
        let Some(id) = self.home_admiral_id else {
            return;
        };
        let mut xp = self.config.tactics.xp_per_battle;
        if winner == CombatWinner::Defender {
            xp = xp.saturating_add(self.config.tactics.xp_on_win);
        }
        let cap = self.config.tactics.xp_trait_cap;
        let next_trait = {
            let Some(adm) = self.admirals.iter().find(|a| a.id == id) else {
                return;
            };
            let new_xp = adm.xp.saturating_add(xp).min(cap);
            self.config.tactics.next_trait(new_xp, &adm.trait_id)
        };
        if let Some(adm) = self.admirals.iter_mut().find(|a| a.id == id) {
            adm.xp = adm.xp.saturating_add(xp).min(cap);
            adm.trait_id = next_trait.clone();
        }
        if let Some(leader) = self.leaders.iter_mut().find(|row| row.id == id) {
            leader.xp = leader.xp.saturating_add(xp).min(cap);
            leader.trait_id = next_trait;
            leader.skill_command = leader.skill_command.saturating_add(1);
        }
    }

    fn resolve_player_attack(
        &self,
        attacker: Fleet,
        defender: Fleet,
        rng: &mut u64,
    ) -> CombatReport {
        let atk_stats = self.player_combat_stats();
        let def_stats = stats_from_catalog(&self.config.ships);
        let rf = rapid_fire_table(&self.config.ships);
        crate::resolve_tactical(
            attacker,
            defender,
            &atk_stats,
            &def_stats,
            &rf,
            rng,
            &self.config.tactics,
            self.player_plan(),
            crate::TacticalPlan::default(),
            crate::FleetTactics::default(),
            self.home_admiral(),
            None,
            &[],
            Fleet::default(),
        )
    }

    fn peek_tactical(&self, pending: &crate::PendingTactical) -> CombatReport {
        let atk_stats = stats_from_catalog(&self.config.ships);
        let def_stats = self.player_combat_stats();
        let rf = rapid_fire_table(&self.config.ships);
        let mut rng = pending.seed;
        crate::resolve_tactical(
            pending.attacker,
            pending.defender,
            &atk_stats,
            &def_stats,
            &rf,
            &mut rng,
            &self.config.tactics,
            pending.atk_plan,
            pending.def_plan,
            self.fleet_tactics,
            None,
            self.home_admiral(),
            &pending.actions,
            pending.reserves,
        )
    }

    fn resolve_raid_now(&mut self, done_at: u64) -> CombatReport {
        let pirates = self.pirate_fleet();
        let atk_stats = stats_from_catalog(&self.config.ships);
        let def_stats = self.player_combat_stats();
        let rf = rapid_fire_table(&self.config.ships);
        if self.defences.is_empty() {
            let mut rng = self.rng_seed;
            let report = crate::resolve_tactical(
                pirates,
                self.fleet,
                &atk_stats,
                &def_stats,
                &rf,
                &mut rng,
                &self.config.tactics,
                crate::TacticalPlan::default(),
                self.player_plan(),
                self.fleet_tactics,
                None,
                self.home_admiral(),
                &[],
                self.reserves,
            );
            self.rng_seed = rng;
            self.finish_home_raid(done_at, report)
        } else {
            let parked = self.fleet;
            let hangar = def_stats[ShipKind::LightFighter.index()];
            let seed = self.rng_seed;
            let battle = resolve_planet_battle(
                pirates,
                parked,
                self.defences,
                &atk_stats,
                &def_stats,
                hangar,
                &self.config.defences,
                self.config.outposts.troop_strength,
                self.config.raid.max_rounds,
                &rf,
                &mut self.rng_seed,
            );
            self.fleet = battle.fleet_after;
            self.defences = battle.defences_after;
            let plunder = self.take_raid_plunder(battle.winner);
            self.apply_raid_standing(battle.winner, done_at);
            if battle.winner != CombatWinner::Attacker {
                self.raids_survived = self.raids_survived.saturating_add(1);
            }
            self.push_event(
                done_at,
                format_layered_raid(pirates, parked, &battle, plunder),
            );
            let report = CombatReport {
                rounds: battle.orbital_rounds,
                attacker_before: pirates,
                defender_before: parked,
                attacker_after: battle.attacker_after,
                defender_after: battle.fleet_after,
                winner: battle.winner,
                ground: battle.ground.clone(),
                seed: 0,
                frames: Vec::new(),
                layered: false,
            }
            .with_seed_and_endpoints(seed);
            self.schedule_next_raid(done_at, report)
        }
    }

    fn finish_home_raid(&mut self, done_at: u64, report: CombatReport) -> CombatReport {
        if let Some(pending) = self.pending_tactical.take() {
            let mut rng = pending.seed;
            let _ = crate::resolve_tactical(
                pending.attacker,
                pending.defender,
                &stats_from_catalog(&self.config.ships),
                &self.player_combat_stats(),
                &rapid_fire_table(&self.config.ships),
                &mut rng,
                &self.config.tactics,
                pending.atk_plan,
                pending.def_plan,
                self.fleet_tactics,
                None,
                self.home_admiral(),
                &pending.actions,
                pending.reserves,
            );
            self.rng_seed = rng;
        }
        self.fleet = report.defender_after;
        let plunder = self.take_raid_plunder(report.winner);
        self.apply_raid_standing(report.winner, done_at);
        self.note_raid_incident(report.winner, done_at);
        if report.winner != CombatWinner::Attacker {
            self.raids_survived = self.raids_survived.saturating_add(1);
        }
        self.grant_admiral_xp(report.winner);
        self.push_event(done_at, format_raid_report(&report, plunder));
        self.schedule_next_raid(done_at, report)
    }

    fn schedule_next_raid(&mut self, done_at: u64, report: CombatReport) -> CombatReport {
        self.next_raid_at = self
            .raid_interval_secs()
            .map(|wait| done_at.saturating_add(wait));
        self.push_combat_log(done_at, report.clone());
        self.note_fight_debris(done_at, self.home, &report);
        if report.winner != CombatWinner::Attacker {
            self.note_daily(DailyAction::RaidWon, done_at);
        }
        self.evaluate_achievements(done_at);
        report
    }

    fn push_combat_log(&mut self, at: u64, report: CombatReport) {
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Combat,
            actor: "simulation".into(),
            action: "fight".into(),
            objects: vec![report.winner.as_key().into()],
            outcome: format!("{} rounds", report.rounds),
            ..crate::HistoryDraft::default()
        });
        self.combat_logs.push(crate::CombatLog { at, report });
        let extra = self
            .combat_logs
            .len()
            .saturating_sub(crate::MAX_COMBAT_LOGS);
        if extra > 0 {
            self.combat_logs.drain(..extra);
        }
    }

    fn take_raid_plunder(&mut self, winner: CombatWinner) -> Resources {
        if winner != CombatWinner::Attacker {
            return Resources::ZERO;
        }
        let taken = plunder_from(
            self.resources,
            self.config.raid.plunder_share,
            self.config.raid.plunder_cap,
        );
        self.resources = self.resources.saturating_sub(taken);
        taken
    }

    fn apply_raid_standing(&mut self, winner: CombatWinner, done_at: u64) {
        let delta_fn: fn(&crate::FactionSpec) -> f64 = if winner == CombatWinner::Defender {
            |f| f.raid_win_delta
        } else {
            |f| f.raid_loss_delta
        };
        self.apply_faction_delta("pirates", delta_fn, "raid", done_at);
    }

    fn pirate_fleet(&self) -> Fleet {
        let points =
            self.fleet_combat_points() + self.defences.combat_points(&self.config.defences);
        let scaled = (points * self.config.raid.strength_factor).floor().max(0.0) as u32;
        let signature = self
            .waste_home()
            .raid_ships(self.config.waste.raid_signature_per_overflow);
        let n = scaled
            .max(self.config.raid.min_ships)
            .saturating_add(signature);
        let mut fleet = Fleet::default();
        if n > 0 {
            fleet.add(ShipKind::LightFighter, n);
        }
        fleet
    }

    fn fleet_combat_points(&self) -> f64 {
        ShipKind::ALL
            .iter()
            .map(|&kind| {
                let stats = self.ship_stats(kind);
                f64::from(self.fleet.get(kind)) * (stats.attack + stats.hull)
            })
            .sum()
    }

    pub(crate) fn player_combat_stats(&self) -> [ShipStats; crate::SHIP_KINDS] {
        let mut out = [ShipStats {
            attack: 0.0,
            shield: 0.0,
            hull: 0.0,
            cargo: 0.0,
            speed: 0.0,
            fuel: 0.0,
        }; crate::SHIP_KINDS];
        for kind in ShipKind::ALL {
            out[kind.index()] = self.ship_stats(kind);
        }
        out
    }

    pub(crate) fn push_event(&mut self, at: u64, text: String) {
        self.events.push(GameEvent { at, text });
        self.trim_event_log();
    }

    /// Cap delivered lines at `EVENT_LOG_CAP`. Pending (future `at`) spy lines
    /// stay until the clock reaches them, then they compete with the rest.
    fn trim_event_log(&mut self) {
        let now = self.last_tick_unix;
        let delivered = self.events.iter().filter(|event| event.at <= now).count();
        if delivered <= EVENT_LOG_CAP {
            return;
        }
        let drop = delivered - EVENT_LOG_CAP;
        let mut removed = 0;
        self.events.retain(|event| {
            if removed < drop && event.at <= now {
                removed += 1;
                false
            } else {
                true
            }
        });
    }

    fn next_intel_at(&self) -> Option<u64> {
        self.spy_reports
            .iter()
            .map(|report| report.arrives_at)
            .filter(|&at| at > self.last_tick_unix)
            .min()
    }

    /// Write the spy log line (and a detection leak) when light-time elapses.
    fn deliver_spy_reports_at(&mut self, now: u64) {
        let due: Vec<SpyReport> = self
            .spy_reports
            .iter()
            .filter(|report| report.arrives_at == now)
            .cloned()
            .collect();
        for report in due {
            let text = spy_event_text(&report);
            if self
                .events
                .iter()
                .any(|event| event.at == now && event.text == text)
            {
                continue;
            }
            self.maybe_leak_spy(now, report.detected);
            self.push_event(now, text);
        }
    }

    pub fn record_history(&mut self, draft: crate::HistoryDraft) -> u64 {
        let waste = self.waste_home();
        let snapshot = crate::EmpireSnapshot {
            at: draft.at,
            event_id: 0,
            iron: self.resources.iron,
            silicon: self.resources.silicon,
            deuterium: self.resources.deuterium,
            energy: self.energy_produced(),
            heat: waste.heat_load,
            home_kind: self.home_kind.as_key().to_string(),
            building_levels: crate::BuildingKind::ALL
                .iter()
                .map(|kind| self.buildings.get(*kind))
                .sum(),
        };
        self.history.record(draft, snapshot)
    }

    fn history_queue_build(&mut self, coord: Coord, kind: BuildingKind, at: u64) {
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Build,
            actor: "player".into(),
            action: "queue".into(),
            objects: vec![coord.to_string(), kind.as_key().into()],
            outcome: "queued".into(),
            cause_key: Some(format!("build:{coord}:{}", kind.as_key())),
            ..crate::HistoryDraft::default()
        });
    }

    fn history_complete_build(&mut self, coord: Coord, kind: BuildingKind, at: u64) {
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Build,
            actor: "simulation".into(),
            action: "complete".into(),
            objects: vec![coord.to_string(), kind.as_key().into()],
            outcome: format!(
                "lv {}",
                self.buildings_at(coord).map(|b| b.get(kind)).unwrap_or(0)
            ),
            parent_key: Some(format!("build:{coord}:{}", kind.as_key())),
            ..crate::HistoryDraft::default()
        });
    }

    fn next_logistics_at(&self) -> Option<u64> {
        let mut soonest: Option<u64> = None;
        let push = |soonest: &mut Option<u64>, secs: f64| {
            if secs.is_finite() && secs > 1e-9 {
                let wait = secs.ceil().max(1.0) as u64;
                let at = self.last_tick_unix.saturating_add(wait);
                *soonest = Some(soonest.map_or(at, |s| s.min(at)));
            }
        };
        for coord in self.owned_coords() {
            for kind in ResourceKind::ALL {
                let rate = self.production_at(coord, kind);
                let have = self.resources_at(coord).map(|r| r.get(kind)).unwrap_or(0.0);
                let cap = self.storage_cap_at(coord, kind);
                if rate > 1e-12 && have + 1e-9 < cap {
                    push(&mut soonest, (cap - have) / rate * 3600.0);
                }
            }
            if let Some(secs) = self.next_reactor_event_secs(coord) {
                push(&mut soonest, secs);
            }
            for secs in self.chain_input_empty_secs(coord) {
                push(&mut soonest, secs);
            }
        }
        soonest.filter(|&t| t > self.last_tick_unix)
    }

    /// Unconstrained recipe rates so shared inputs empty when all consumers
    /// would drain them together, matching second-by-second catch-up.
    fn chain_input_empty_secs(&self, coord: Coord) -> Vec<f64> {
        if !self.config.chains.enabled() {
            return Vec::new();
        }
        let Some(buildings) = self.buildings_at(coord).copied() else {
            return Vec::new();
        };
        let Some(kind) = self.planet_kind(coord) else {
            return Vec::new();
        };
        let Some(resources) = self.resources_at(coord).copied() else {
            return Vec::new();
        };
        let goods = if coord == self.home {
            self.goods.clone()
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| c.goods.clone())
                .unwrap_or_default()
        };
        let extra = self.site_extra(coord);
        let now = self.last_tick_unix;
        let throttle = self.chain_throttle_at(coord);
        if throttle <= 0.0 {
            return Vec::new();
        }
        let deposits = self.chain_deposits(kind, self.galaxy_seed, coord);
        let mut consume: BTreeMap<String, f64> = BTreeMap::new();
        let mut produce: BTreeMap<String, f64> = BTreeMap::new();
        for recipe in &self.config.chains.recipes {
            if buildings.get(recipe.building) == 0 {
                continue;
            }
            let level_scale = self.recipe_building_scale(
                buildings,
                kind,
                extra,
                recipe.building,
                (now, now),
                coord,
            );
            if level_scale <= 0.0 {
                continue;
            }
            let deposit = recipe
                .deposit
                .as_ref()
                .map(|id| deposits.get(id).copied().unwrap_or(0.0))
                .unwrap_or(1.0);
            if recipe.deposit.is_some() && deposit <= 0.0 {
                continue;
            }
            let mut scale = level_scale * throttle * deposit;
            if scale <= 0.0 {
                continue;
            }
            for (id, qty) in &recipe.outputs {
                if *qty <= 0.0 {
                    continue;
                }
                let Some(resource) = component_kind(id) else {
                    continue;
                };
                let cap = self.storage_cap_at(coord, resource);
                if resources.get(resource) + 1e-9 >= cap {
                    scale = 0.0;
                    break;
                }
            }
            if scale <= 0.0 {
                continue;
            }
            for (id, qty) in &recipe.inputs {
                if *qty > 0.0 {
                    *consume.entry(id.clone()).or_insert(0.0) += *qty * scale;
                }
            }
            for (id, qty) in &recipe.outputs {
                if *qty > 0.0 {
                    *produce.entry(id.clone()).or_insert(0.0) += *qty * scale;
                }
            }
        }
        let mut out = Vec::new();
        for (id, used) in consume {
            let made = produce.get(&id).copied().unwrap_or(0.0);
            if used <= made + 1e-12 {
                continue;
            }
            let have = available(&resources, &goods, &id);
            if have > 1e-9 {
                out.push(have / (used - made) * 3600.0);
            }
        }
        out
    }

    fn chain_throttle_at(&self, coord: Coord) -> f64 {
        if self.buildings_at(coord).is_none() {
            return 0.0;
        }
        let energy = if coord == self.home {
            self.energy_factor()
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|colony| {
                    let bonus = self.config.planet_types.get(colony.kind);
                    let rare = self.rare_bonus_for(colony);
                    let produced = self
                        .config
                        .solar_plant
                        .energy_output(colony.buildings.solar_plant)
                        * self.config.orbits.solar_flux(colony.coord)
                        * (1.0
                            + self.tech_modifier(ModifierKind::EnergyOutput)
                            + bonus.energy
                            + rare.energy)
                            .max(0.0)
                        + self.reactor_output_at(colony.coord);
                    let consumed = self.config.energy_draw(&colony.buildings);
                    if consumed <= 0.0 {
                        1.0
                    } else {
                        (produced / consumed).clamp(0.0, 1.0)
                    }
                })
                .unwrap_or(1.0)
        };
        let unrest = if coord == self.home {
            self.unrest_factor()
        } else {
            self.unrest_for(coord)
        };
        energy.max(0.0)
            * unrest.max(0.0)
            * self.emission_factor(coord).max(0.0)
            * self.prod_boost_mult().max(0.0)
    }

    pub(crate) fn storage_cap_at(&self, coord: Coord, kind: ResourceKind) -> f64 {
        let Some(buildings) = self.buildings_at(coord) else {
            return 0.0;
        };
        match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => {
                self.config.metal_storage.capacity(buildings.metal_storage)
            }
            ResourceKind::Silicon => self
                .config
                .crystal_storage
                .capacity(buildings.crystal_storage),
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => self
                .config
                .deuterium_storage
                .capacity(buildings.deuterium_storage),
        }
    }

    fn room_at(&self, coord: Coord, kind: ResourceKind) -> f64 {
        let have = self.resources_at(coord).map(|r| r.get(kind)).unwrap_or(0.0);
        (self.storage_cap_at(coord, kind) - have).max(0.0)
    }

    fn dest_has_room_for(&self, dest: Coord, cargo: Resources) -> bool {
        ResourceKind::ALL
            .iter()
            .any(|&kind| cargo.get(kind) > 1e-9 && self.room_at(dest, kind) > 1e-9)
    }

    fn accept_up_to_cap(&mut self, coord: Coord, cargo: Resources) -> Resources {
        let mut accepted = Resources::ZERO;
        for kind in ResourceKind::ALL {
            let take = cargo.get(kind).min(self.room_at(coord, kind));
            accepted.set(kind, take);
        }
        if cargo_has_mass(accepted) {
            self.credit_at(coord, accepted);
        }
        accepted
    }

    fn note_logistics_stalls(&mut self, at: u64) {
        let report = self.production_report();
        let mut lines = Vec::new();
        for step in &report.steps {
            let line = match &step.limited_by {
                LimitingFactor::FullBuffer { good } => {
                    format!("stall: full storage ({good}) at {}", step.planet)
                }
                LimitingFactor::Input { good } => {
                    format!("stall: missing {good} at {}", step.planet)
                }
                LimitingFactor::Energy => {
                    format!("stall: energy deficit at {}", step.planet)
                }
                LimitingFactor::Heat => {
                    format!("stall: heat overflow at {}", step.planet)
                }
                LimitingFactor::Pollution => {
                    format!("stall: pollution overflow at {}", step.planet)
                }
                LimitingFactor::Transport { good, from, .. } => {
                    format!("stall: awaiting {good} from {from} at {}", step.planet)
                }
                LimitingFactor::None => continue,
            };
            lines.push(line);
        }
        if report.steps.is_empty() {
            match &report.limited_by {
                LimitingFactor::Energy => {
                    lines.push(format!("stall: energy deficit at {}", self.home));
                }
                LimitingFactor::FullBuffer { good } => {
                    lines.push(format!("stall: full storage ({good}) at {}", self.home));
                }
                LimitingFactor::Input { good } => {
                    lines.push(format!("stall: missing {good} at {}", self.home));
                }
                _ => {}
            }
        }
        for line in lines {
            self.note_stall(at, line);
        }
    }

    pub(crate) fn note_stall(&mut self, at: u64, line: String) {
        if self.logistics.stalls.iter().any(|s| s == &line) {
            return;
        }
        push_logistics_line(&mut self.logistics.stalls, line.clone());
        self.push_event(at, line);
    }

    fn note_delivery(&mut self, _at: u64, line: String) {
        push_logistics_line(&mut self.logistics.deliveries, line);
    }

    fn note_completed(&mut self, _at: u64, line: String) {
        push_logistics_line(&mut self.logistics.completed, line);
    }

    /// Next timeout or first eligible offer after `last_tick_unix`.
    fn next_story_at(&self) -> Option<u64> {
        if self.config.story.is_empty() {
            return None;
        }
        let at = if let Some(pending) = &self.story_pending {
            pending.timeout_at
        } else {
            self.story_next_offer_at()?
        };
        if at > self.last_tick_unix {
            Some(at)
        } else {
            Some(self.last_tick_unix.saturating_add(1))
        }
    }

    /// Earliest time an event can be offered with the current fighter count.
    fn story_next_offer_at(&self) -> Option<u64> {
        let started = self.prestige.current.started_at;
        let gate = if self.last_story_at == 0 {
            started.saturating_add(self.config.story.first_delay_secs)
        } else {
            self.last_story_at
                .saturating_add(self.config.story.cooldown_secs)
        };
        let fighters = self.fleet.get(ShipKind::LightFighter);
        let mut soonest: Option<u64> = None;
        for event in self.config.story.iter() {
            if fighters < event.min_fighters {
                continue;
            }
            let ready = gate.max(started.saturating_add(event.min_elapsed_secs));
            soonest = Some(match soonest {
                Some(t) => t.min(ready),
                None => ready,
            });
        }
        soonest
    }

    /// Apply a timed-out default, then offer at most one pending event.
    fn resolve_story(&mut self, now: u64) {
        if let Some(pending) = self.story_pending.clone()
            && now >= pending.timeout_at
        {
            let choice = self
                .config
                .story
                .get(&pending.event_id)
                .map(|e| e.timeout_choice.clone());
            if let Some(choice) = choice {
                let _ = self.apply_story_choice(&pending.event_id, &choice, pending.timeout_at);
            } else {
                self.story_pending = None;
            }
        }
        if self.story_pending.is_some() {
            return;
        }
        self.offer_story(now);
    }

    fn offer_story(&mut self, now: u64) {
        let started = self.prestige.current.started_at;
        if now < started.saturating_add(self.config.story.first_delay_secs) {
            return;
        }
        if self.last_story_at > 0
            && now
                < self
                    .last_story_at
                    .saturating_add(self.config.story.cooldown_secs)
        {
            return;
        }
        let fighters = self.fleet.get(ShipKind::LightFighter);
        let elapsed = now.saturating_sub(started);
        let eligible: Vec<(String, u32, u64)> = self
            .config
            .story
            .iter()
            .filter(|e| elapsed >= e.min_elapsed_secs && fighters >= e.min_fighters)
            .map(|e| (e.id.clone(), e.weight, e.timeout_secs))
            .collect();
        if eligible.is_empty() {
            return;
        }
        let total: u64 = eligible.iter().map(|(_, w, _)| u64::from(*w)).sum();
        if total == 0 {
            return;
        }
        let mut pick = next_u64(&mut self.rng_seed) % total;
        let mut chosen = &eligible[0];
        for row in &eligible {
            let w = u64::from(row.1);
            if pick < w {
                chosen = row;
                break;
            }
            pick -= w;
        }
        self.story_pending = Some(crate::PendingStory {
            event_id: chosen.0.clone(),
            offered_at: now,
            timeout_at: now.saturating_add(chosen.2),
        });
    }

    /// Resolve the pending event with `choice_id`. Offline timeouts use the event's default.
    pub fn choose_story(
        &mut self,
        choice_id: &str,
        now_unix: u64,
    ) -> Result<(), crate::StoryError> {
        self.catch_up(now_unix);
        let pending = self
            .story_pending
            .clone()
            .ok_or(crate::StoryError::NoPending)?;
        self.apply_story_choice(&pending.event_id, choice_id, self.last_tick_unix)
    }

    fn next_market_at(&self) -> Option<u64> {
        if !self.config.market.enabled {
            return None;
        }
        let due = if self.last_market_at == 0 {
            self.prestige
                .current
                .started_at
                .saturating_add(self.config.market.tick_secs)
        } else {
            self.last_market_at
                .saturating_add(self.config.market.tick_secs)
        };
        if due > self.last_tick_unix {
            Some(due)
        } else {
            Some(self.last_tick_unix.saturating_add(1))
        }
    }

    pub(crate) fn tick_market(&mut self, at: u64) {
        if !self.config.market.enabled {
            return;
        }
        self.market.tick(&self.config.market, &mut self.rng_seed);
        self.last_market_at = at;
        self.on_market_tick(at);
    }

    pub fn quote_market(
        &self,
        pay: ResourceKind,
        recv: ResourceKind,
        amount: f64,
    ) -> Result<crate::TradeQuote, crate::MarketError> {
        if !self.config.market.enabled {
            return Err(crate::MarketError::Disabled);
        }
        let mut cfg = self.config.market.clone();
        cfg.fee = self.effective_market_fee();
        self.market.quote(pay, recv, amount, &cfg)
    }

    /// Spend `amount` of `pay` for `recv` at the current quote, then move the mid.
    pub fn trade_market(
        &mut self,
        pay: ResourceKind,
        recv: ResourceKind,
        amount: f64,
        now_unix: u64,
    ) -> Result<crate::TradeQuote, crate::MarketError> {
        self.catch_up(now_unix);
        if !self.config.market.enabled {
            return Err(crate::MarketError::Disabled);
        }
        let mut cfg = self.config.market.clone();
        cfg.fee = self.effective_market_fee();
        let quote = self.market.quote(pay, recv, amount, &cfg)?;
        let have = self.resources.get(pay);
        if have + 1e-9 < quote.pay_amount {
            return Err(crate::MarketError::Insufficient {
                missing: quote.pay_amount - have,
                kind: pay,
            });
        }
        if quote.pay_amount > self.config.market.cargo_free {
            let have_cargo = self.cargo_capacity(self.fleet);
            if have_cargo + 1e-9 < quote.pay_amount {
                return Err(crate::MarketError::NeedCargo {
                    need: quote.pay_amount,
                    have: have_cargo,
                });
            }
        }
        self.resources.set(pay, (have - quote.pay_amount).max(0.0));
        let got = self.resources.get(recv);
        let cap = self.storage_cap(recv);
        let stored = (got + quote.recv_amount).min(cap).max(got);
        let added = (stored - got).max(0.0);
        self.resources.set(recv, stored);
        self.market.apply_trade_impact(&quote, &self.config.market);
        self.apply_faction_delta("traders", |f| f.trade_delta, "trade", self.last_tick_unix);
        self.note_daily(DailyAction::Trade, self.last_tick_unix);
        self.push_event(
            self.last_tick_unix,
            format!(
                "market: {:.0} {} -> {:.1} {}",
                quote.pay_amount,
                pay.label(),
                quote.recv_amount,
                recv.label()
            ),
        );
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: crate::HistoryKind::Trade,
            actor: "player".into(),
            action: "trade".into(),
            objects: vec![pay.as_key().into(), recv.as_key().into()],
            inputs: format!("{:.0}", quote.pay_amount),
            outcome: format!("{:.1} {}", quote.recv_amount, recv.label()),
            mass: quote.pay_amount,
            ..crate::HistoryDraft::default()
        });
        let mut paid = Resources::ZERO;
        paid.set(pay, quote.pay_amount);
        let mut received = Resources::ZERO;
        received.set(recv, added);
        let net = self.value_resources(received) - self.value_resources(paid);
        if net > 0.0 {
            self.credit_income(net, crate::finance::IncomeKind::Trade);
        } else if net < 0.0 {
            self.finance.treasury.cash += net;
            self.finance.income.trade += net;
            self.kpis.period_trade += net;
        }
        Ok(quote)
    }

    fn next_diplomacy_at(&self) -> Option<u64> {
        if !self.config.diplomacy.enabled {
            return None;
        }
        let due = if self.last_diplomacy_at == 0 {
            self.prestige
                .current
                .started_at
                .saturating_add(self.config.diplomacy.tick_secs)
        } else {
            self.last_diplomacy_at
                .saturating_add(self.config.diplomacy.tick_secs)
        };
        if due > self.last_tick_unix {
            Some(due)
        } else {
            Some(self.last_tick_unix.saturating_add(1))
        }
    }

    fn tick_diplomacy(&mut self, at: u64) {
        if !self.config.diplomacy.enabled {
            return;
        }
        let decay = self.config.diplomacy.decay;
        let ids: Vec<String> = self.config.diplomacy.iter().map(|f| f.id.clone()).collect();
        for id in ids {
            let cur = self.diplomacy.standing(&id);
            let next = cur + decay * (0.0 - cur);
            self.diplomacy.set_standing(&id, self.clamp_standing(next));
        }
        self.last_diplomacy_at = at;
        self.sync_raid_schedule_after_standing(at);
        self.maybe_declare_victory(at);
        self.evaluate_ladder(at);
        self.evaluate_frontier(at);
    }

    fn clamp_standing(&self, value: f64) -> f64 {
        value
            .max(self.config.diplomacy.min_standing)
            .min(self.config.diplomacy.max_standing)
    }

    /// Market fee after the traders' current tier overlay.
    pub fn effective_market_fee(&self) -> f64 {
        let mut fee = self.config.market.fee;
        if self.config.diplomacy.enabled
            && let Some(traders) = self.config.diplomacy.get("traders")
            && let Some(tier) = traders.tier(self.diplomacy.standing("traders"))
        {
            fee += tier.market_fee;
        }
        fee += self.laws.effect(
            crate::LawEffectKind::MarketFee,
            &self.config.laws,
            self.last_tick_unix,
            self.home,
        );
        fee.clamp(0.0, 0.95)
    }

    fn raid_interval_secs(&self) -> Option<u64> {
        if !self.config.raid.enabled {
            return None;
        }
        let base = self.config.raid.interval_secs.max(1);
        if self.config.diplomacy.enabled
            && let Some(pirates) = self.config.diplomacy.get("pirates")
            && let Some(tier) = pirates.tier(self.diplomacy.standing("pirates"))
        {
            if tier.raid_delay_mult == 0.0 {
                return None;
            }
            return Some(((base as f64) * tier.raid_delay_mult).round().max(1.0) as u64);
        }
        Some(base)
    }

    pub(crate) fn apply_faction_delta(
        &mut self,
        id: &str,
        delta: fn(&crate::FactionSpec) -> f64,
        reason: &str,
        at: u64,
    ) {
        let Some(spec) = self.config.diplomacy.get(id) else {
            return;
        };
        let amount = delta(spec);
        self.adjust_standing(id, amount, reason, at);
    }

    fn adjust_standing(&mut self, id: &str, delta: f64, reason: &str, at: u64) {
        if !self.config.diplomacy.enabled || delta == 0.0 || !delta.is_finite() {
            return;
        }
        let Some(spec) = self.config.diplomacy.get(id).cloned() else {
            return;
        };
        let before = self.diplomacy.standing(id);
        let after = self.clamp_standing(before + delta);
        if (after - before).abs() < 1e-12 {
            return;
        }
        let old_tier = spec.tier(before).map(|t| t.id.clone());
        let new_tier = spec.tier(after).map(|t| t.id.clone());
        self.diplomacy.set_standing(id, after);
        self.diplomacy.log.push(crate::StandingChange {
            at,
            faction: id.to_string(),
            delta: after - before,
            reason: reason.to_string(),
        });
        let cap = self.config.diplomacy.history_len.max(1);
        if self.diplomacy.log.len() > cap {
            let drop = self.diplomacy.log.len() - cap;
            self.diplomacy.log.drain(..drop);
        }
        self.push_event(
            at,
            format!(
                "standing: {} {before:.0} -> {after:.0} ({reason})",
                spec.name
            ),
        );
        if old_tier != new_tier
            && let Some(tier) = spec.tier(after)
            && tier.schedule_raid
            && self.config.raid.enabled
        {
            let soon = at.saturating_add(60);
            self.next_raid_at = Some(match self.next_raid_at {
                Some(existing) => existing.min(soon),
                None => soon,
            });
        }
        self.sync_raid_schedule_after_standing(at);
        self.maybe_declare_victory(at);
        self.evaluate_ladder(at);
        self.evaluate_frontier(at);
    }

    pub(crate) fn sync_raid_schedule_after_standing(&mut self, at: u64) {
        if !self.config.raid.enabled {
            return;
        }
        match (self.next_raid_at, self.raid_interval_secs()) {
            (_, None) => self.next_raid_at = None,
            (None, Some(wait)) => {
                self.next_raid_at = Some(at.saturating_add(wait));
            }
            _ => {}
        }
    }

    /// Pay configured tribute to raise standing with `faction`.
    pub fn pay_tribute(
        &mut self,
        faction: &str,
        now_unix: u64,
    ) -> Result<(), crate::DiplomacyError> {
        self.catch_up(now_unix);
        if !self.config.diplomacy.enabled {
            return Err(crate::DiplomacyError::Disabled);
        }
        let spec = self
            .config
            .diplomacy
            .get(faction)
            .cloned()
            .ok_or_else(|| crate::DiplomacyError::UnknownFaction(faction.to_string()))?;
        if spec.tribute_metal <= 0.0 || spec.tribute_delta == 0.0 {
            return Err(crate::DiplomacyError::NoTribute(spec.name));
        }
        let before = self.diplomacy.standing(&spec.id);
        let after = self.clamp_standing(before + spec.tribute_delta);
        if (after - before).abs() < 1e-12 {
            return Err(crate::DiplomacyError::NoStandingChange(spec.name));
        }
        if self.resources.iron + 1e-9 < spec.tribute_metal {
            return Err(crate::DiplomacyError::Insufficient {
                missing: spec.tribute_metal - self.resources.iron,
            });
        }
        self.resources.iron -= spec.tribute_metal;
        self.adjust_standing(&spec.id, spec.tribute_delta, "tribute", self.last_tick_unix);
        Ok(())
    }

    fn apply_story_choice(
        &mut self,
        event_id: &str,
        choice_id: &str,
        at: u64,
    ) -> Result<(), crate::StoryError> {
        let event = self
            .config
            .story
            .get(event_id)
            .ok_or_else(|| crate::StoryError::UnknownChoice(event_id.to_string()))?;
        let choice = event
            .choice(choice_id)
            .ok_or_else(|| crate::StoryError::UnknownChoice(choice_id.to_string()))?
            .clone();
        let title = event.title.clone();
        self.resources.iron = (self.resources.iron + choice.iron).max(0.0);
        self.resources.silicon = (self.resources.silicon + choice.silicon).max(0.0);
        self.resources.deuterium = (self.resources.deuterium + choice.deuterium).max(0.0);
        for (kind, delta) in choice.ships {
            if delta >= 0 {
                self.fleet.add(kind, delta as u32);
            } else {
                self.fleet.saturating_sub(kind, delta.unsigned_abs());
            }
        }
        if choice.schedule_raid && self.config.raid.enabled && self.raid_interval_secs().is_some() {
            let soon = at.saturating_add(60);
            self.next_raid_at = Some(match self.next_raid_at {
                Some(existing) => existing.min(soon),
                None => soon,
            });
        }
        if self.config.market.enabled {
            self.market
                .shock(ResourceKind::Iron, choice.metal_price, &self.config.market);
            self.market.shock(
                ResourceKind::Silicon,
                choice.crystal_price,
                &self.config.market,
            );
            self.market.shock(
                ResourceKind::Deuterium,
                choice.deuterium_price,
                &self.config.market,
            );
        }
        for (faction, delta) in choice.standing {
            self.adjust_standing(&faction, delta, "story", at);
        }
        self.last_story_at = at;
        self.story_pending = None;
        self.push_event(at, format!("story: {title} → {}", choice.label));
        Ok(())
    }

    fn complete_one_ship(&mut self, done_at: u64) {
        let Some(job) = self.ship_queue.first() else {
            return;
        };
        let duration = self.yard_time_secs(job);
        let ship = job.ship;
        let defence = job.defence;
        if let Some(kind) = defence {
            self.defences.add(kind, 1);
        } else if let Some(kind) = ship {
            self.fleet.add(kind, 1);
        }
        let job = &mut self.ship_queue[0];
        let n = job.remaining;
        if n > 0 {
            job.credits *= f64::from(n.saturating_sub(1)) / f64::from(n);
        }
        job.remaining = n.saturating_sub(1);
        if job.remaining == 0 {
            let label = defence
                .map(|kind| kind.to_string())
                .or_else(|| ship.map(|kind| kind.to_string()))
                .unwrap_or_else(|| "yard job".into());
            self.ship_queue.remove(0);
            self.start_ship_head(done_at);
            self.note_completed(done_at, label);
            if let Some(kind) = ship {
                self.record_history(crate::HistoryDraft {
                    at: done_at,
                    kind: crate::HistoryKind::Ship,
                    actor: "simulation".into(),
                    action: "complete".into(),
                    objects: vec![kind.as_key().into()],
                    outcome: "built".into(),
                    parent_key: Some(format!("ship:{}", kind.as_key())),
                    ..crate::HistoryDraft::default()
                });
            }
            return;
        }
        job.started_at = Some(done_at);
        job.completes_at = Some(done_at.saturating_add(duration));
    }

    fn yard_time_secs(&self, job: &ShipJob) -> u64 {
        if let Some(kind) = job.defence {
            let speed = self
                .owned_coords()
                .into_iter()
                .filter_map(|coord| self.rare_bonus_at(coord).map(|row| row.shipyard_speed))
                .fold(0.0_f64, f64::max);
            ship_duration(self.config.defences.get(kind).duration_secs(), speed)
        } else if let Some(kind) = job.ship {
            self.ship_time_secs(kind)
        } else {
            1
        }
    }

    fn produce_until(&mut self, now_unix: u64) {
        if now_unix <= self.last_tick_unix {
            return;
        }
        // Telemetry is not a world-event split (that blows the 7-day catch-up
        // budget). Apply samples between production segments instead.
        if self.maintenance.next_at <= self.last_tick_unix {
            self.apply_maintenance_at(self.last_tick_unix);
        }
        let mut guard = 0u32;
        while self.last_tick_unix < now_unix {
            let next = self.maintenance.next_at;
            let step_to = if next > self.last_tick_unix && next < now_unix {
                next
            } else {
                now_unix
            };
            self.produce_segment(step_to);
            if step_to < now_unix {
                self.apply_maintenance_at(step_to);
            }
            guard += 1;
            if guard > 10_000 {
                if self.last_tick_unix < now_unix {
                    self.produce_segment(now_unix);
                }
                break;
            }
        }
    }

    fn produce_segment(&mut self, now_unix: u64) {
        if now_unix <= self.last_tick_unix {
            return;
        }
        let from = self.last_tick_unix;
        let hours = (now_unix - from) as f64 / 3600.0;
        let before = self.resources;
        let colony_before: Vec<Resources> = self.colonies.iter().map(|c| c.resources).collect();
        let reactor_power: Vec<_> = self
            .owned_coords()
            .into_iter()
            .map(|coord| (coord, self.reactor_output_for_hours(coord, hours)))
            .collect();
        let factor = self.reactor_production_factor(self.home, hours)
            * self.emission_factor_range(self.home, from, now_unix);
        let boost = self.prod_boost_mult();
        let home_staff = self.governor_factor(self.home);
        if self.config.chains.enabled() {
            self.produce_chains(hours, factor, boost, from, now_unix);
        } else {
            for kind in ResourceKind::ALL {
                let rate = self.base_production_range(kind, from, now_unix)
                    * factor
                    * boost
                    * home_staff
                    * self.mine_health_factor(kind);
                let cap = self.storage_cap(kind);
                let current = self.resources.get(kind);
                self.resources
                    .set(kind, add_capped(current, rate, hours, cap));
            }
            let config = self.config.clone();
            let metal_t = self.tech_modifier_range(ModifierKind::MetalOutput, from, now_unix);
            let crystal_t = self.tech_modifier_range(ModifierKind::CrystalOutput, from, now_unix);
            let deut_t = self.tech_modifier_range(ModifierKind::DeuteriumOutput, from, now_unix);
            let colony_factors: Vec<_> = self
                .colonies
                .iter()
                .map(|colony| {
                    (
                        colony.coord,
                        self.reactor_production_factor(colony.coord, hours),
                    )
                })
                .collect();
            let colony_emission: Vec<f64> = self
                .colonies
                .iter()
                .map(|c| self.emission_factor_range(c.coord, from, now_unix))
                .collect();
            let colony_staff: Vec<f64> = self
                .colonies
                .iter()
                .map(|c| self.governor_factor(c.coord))
                .collect();
            let colony_health: Vec<(f64, f64, f64)> = self
                .colonies
                .iter()
                .map(|c| {
                    (
                        self.building_health(c.coord, crate::BuildingKind::MetalMine),
                        self.building_health(c.coord, crate::BuildingKind::CrystalMine),
                        self.building_health(c.coord, crate::BuildingKind::DeuteriumSynthesizer),
                    )
                })
                .collect();
            for (((colony, emission), staff), health) in self
                .colonies
                .iter_mut()
                .zip(colony_emission)
                .zip(colony_staff)
                .zip(colony_health)
            {
                let bonus = config.planet_types.get(colony.kind);
                let factor = colony_factors
                    .iter()
                    .find(|(coord, _)| *coord == colony.coord)
                    .map(|(_, factor)| *factor)
                    .unwrap_or(0.0)
                    * emission;
                let metal_rate = config
                    .metal_mine
                    .production_per_hour(colony.buildings.metal_mine)
                    * (1.0 + metal_t + bonus.iron)
                    * factor
                    * boost
                    * staff
                    * health.0;
                let crystal_rate = config
                    .crystal_mine
                    .production_per_hour(colony.buildings.crystal_mine)
                    * (1.0 + crystal_t + bonus.silicon)
                    * factor
                    * boost
                    * staff
                    * health.1;
                let deut_rate = config
                    .deuterium_synthesizer
                    .production_per_hour(colony.buildings.deuterium_synthesizer)
                    * (1.0 + deut_t + bonus.deuterium)
                    * factor
                    * boost
                    * staff
                    * health.2;
                let metal_cap = config
                    .metal_storage
                    .capacity(colony.buildings.metal_storage);
                let crystal_cap = config
                    .crystal_storage
                    .capacity(colony.buildings.crystal_storage);
                let deut_cap = config
                    .deuterium_storage
                    .capacity(colony.buildings.deuterium_storage);
                colony.resources.iron =
                    add_capped(colony.resources.iron, metal_rate, hours, metal_cap);
                colony.resources.silicon =
                    add_capped(colony.resources.silicon, crystal_rate, hours, crystal_cap);
                colony.resources.deuterium =
                    add_capped(colony.resources.deuterium, deut_rate, hours, deut_cap);
            }
        }
        let mut produced = Resources::ZERO;
        for kind in ResourceKind::ALL {
            let mut added = (self.resources.get(kind) - before.get(kind)).max(0.0);
            for (colony, prior) in self.colonies.iter().zip(&colony_before) {
                added += (colony.resources.get(kind) - prior.get(kind)).max(0.0);
            }
            produced.set(kind, added);
        }
        self.collect_law_tax(hours, from, now_unix);
        self.accrue_production_income(hours, produced);
        self.tick_flagship(now_unix);
        self.last_tick_unix = now_unix;
        for (coord, power) in reactor_power {
            if power <= 0.0 {
                continue;
            }
            let burn = power * hours * self.config.fission_reactor.reactor_fuel_per_energy_hour;
            let goods = if coord == self.home {
                &mut self.goods
            } else {
                let Some(colony) = self.colonies.iter_mut().find(|c| c.coord == coord) else {
                    continue;
                };
                &mut colony.goods
            };
            let fuel = goods.entry("reactor_fuel".into()).or_default();
            *fuel = (*fuel - burn).max(0.0);
            let empty = *fuel <= 1e-12;
            let spent = goods.entry("spent_fuel".into()).or_default();
            *spent += burn;
            let spent = *spent;
            let level = self
                .buildings_at(coord)
                .map(|b| b.fission_reactor)
                .unwrap_or(0);
            if (empty && self.reactor_output_at(coord) <= 1e-9)
                || spent + 1e-12 >= self.config.fission_reactor.pool_capacity(level)
            {
                self.note_reactor_shutdown(coord, now_unix);
            }
        }
        self.last_tick_unix = now_unix;
        self.trim_event_log();
        self.sync_daily(now_unix);
    }

    fn produce_chains(&mut self, hours: f64, home_factor: f64, boost: f64, from: u64, to: u64) {
        let chains = self.config.chains.clone();
        let home_deposits = self.chain_deposits(self.home_kind, self.galaxy_seed, self.home);
        let home_buildings = self.buildings;
        let home_kind = self.home_kind;
        let metal_cap = self.storage_cap(ResourceKind::Iron);
        let crystal_cap = self.storage_cap(ResourceKind::Silicon);
        let deut_cap = self.storage_cap(ResourceKind::Deuterium);
        let home_metal = self.recipe_building_scale(
            home_buildings,
            home_kind,
            (0.0, 0.0, 0.0),
            BuildingKind::MetalMine,
            (from, to),
            self.home,
        );
        let home_crystal = self.recipe_building_scale(
            home_buildings,
            home_kind,
            (0.0, 0.0, 0.0),
            BuildingKind::CrystalMine,
            (from, to),
            self.home,
        );
        let home_deut = self.recipe_building_scale(
            home_buildings,
            home_kind,
            (0.0, 0.0, 0.0),
            BuildingKind::DeuteriumSynthesizer,
            (from, to),
            self.home,
        );
        let home_yard = home_buildings.shipyard as f64;
        let home_staff = self.governor_factor(self.home);
        let home_fabrication = self.recipe_building_scale(
            home_buildings,
            home_kind,
            (0.0, 0.0, 0.0),
            BuildingKind::FissionReactor,
            (from, to),
            self.home,
        );
        apply_recipes(
            RecipeRun {
                config: &chains,
                buildings: home_buildings,
                deposits: &home_deposits,
                hours: hours * home_staff,
                energy_factor: home_factor,
                boost,
            },
            |kind| match kind {
                BuildingKind::MetalMine => home_metal,
                BuildingKind::FissionReactor => home_fabrication,
                BuildingKind::CrystalMine => home_crystal,
                BuildingKind::DeuteriumSynthesizer => home_deut,
                BuildingKind::Shipyard => home_yard,
                _ => 0.0,
            },
            &mut self.resources,
            &mut self.goods,
            |kind| match kind {
                ResourceKind::Iron | ResourceKind::RareEarths => metal_cap,
                ResourceKind::Silicon => crystal_cap,
                ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                    deut_cap
                }
            },
        );

        let n = self.colonies.len();
        for i in 0..n {
            let snapshot = {
                let colony = &self.colonies[i];
                let rare = self.rare_bonus_for(colony);
                let factor = self.reactor_production_factor(colony.coord, hours)
                    * self.emission_factor_range(colony.coord, from, to);
                (
                    colony.coord,
                    colony.kind,
                    colony.buildings,
                    rare.iron + self.frontier_production_bonus(colony.coord),
                    rare.silicon + self.frontier_production_bonus(colony.coord),
                    rare.deuterium + self.frontier_production_bonus(colony.coord),
                    factor,
                    self.config
                        .metal_storage
                        .capacity(colony.buildings.metal_storage),
                    self.config
                        .crystal_storage
                        .capacity(colony.buildings.crystal_storage),
                    self.config
                        .deuterium_storage
                        .capacity(colony.buildings.deuterium_storage),
                )
            };
            let (
                coord,
                kind,
                buildings,
                rare_m,
                rare_c,
                rare_d,
                factor,
                metal_cap,
                crystal_cap,
                deut_cap,
            ) = snapshot;
            let deposits = self.chain_deposits(kind, self.galaxy_seed, coord);
            let metal_s = self.recipe_building_scale(
                buildings,
                kind,
                (rare_m, rare_c, rare_d),
                BuildingKind::MetalMine,
                (from, to),
                coord,
            );
            let crystal_s = self.recipe_building_scale(
                buildings,
                kind,
                (rare_m, rare_c, rare_d),
                BuildingKind::CrystalMine,
                (from, to),
                coord,
            );
            let deut_s = self.recipe_building_scale(
                buildings,
                kind,
                (rare_m, rare_c, rare_d),
                BuildingKind::DeuteriumSynthesizer,
                (from, to),
                coord,
            );
            let yard_s = buildings.shipyard as f64;
            let staff = self.governor_factor(coord);
            let fabrication_s = self.recipe_building_scale(
                buildings,
                kind,
                (rare_m, rare_c, rare_d),
                BuildingKind::FissionReactor,
                (from, to),
                coord,
            );
            let colony = &mut self.colonies[i];
            apply_recipes(
                RecipeRun {
                    config: &chains,
                    buildings,
                    deposits: &deposits,
                    hours: hours * staff,
                    energy_factor: factor,
                    boost,
                },
                |b| match b {
                    BuildingKind::MetalMine => metal_s,
                    BuildingKind::FissionReactor => fabrication_s,
                    BuildingKind::CrystalMine => crystal_s,
                    BuildingKind::DeuteriumSynthesizer => deut_s,
                    BuildingKind::Shipyard => yard_s,
                    _ => 0.0,
                },
                &mut colony.resources,
                &mut colony.goods,
                |res| match res {
                    ResourceKind::Iron | ResourceKind::RareEarths => metal_cap,
                    ResourceKind::Silicon => crystal_cap,
                    ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                        deut_cap
                    }
                },
            );
        }
    }

    pub(crate) fn chain_deposits(
        &self,
        kind: crate::PlanetType,
        galaxy_seed: u64,
        coord: Coord,
    ) -> std::collections::BTreeMap<String, f64> {
        let bonus = self.config.planet_types.get(kind);
        let mut deposits = self.config.chains.deposits_for(kind, galaxy_seed, coord);
        for (id, value) in deposits.iter_mut() {
            *value *= bonus.chain_deposit_factor(id);
        }
        deposits
    }

    pub(crate) fn recipe_building_scale(
        &self,
        buildings: Buildings,
        planet: PlanetType,
        extra: (f64, f64, f64),
        building: BuildingKind,
        span: (u64, u64),
        coord: Coord,
    ) -> f64 {
        let (from, to) = span;
        let (extra_iron, extra_silicon, extra_deut) = extra;
        let scale = match building {
            BuildingKind::MetalMine => {
                self.config
                    .metal_mine
                    .production_per_hour(buildings.metal_mine)
                    * (1.0
                        + self.tech_modifier_range(ModifierKind::MetalOutput, from, to)
                        + self.planet_bonus(planet, ModifierKind::MetalOutput)
                        + extra_iron)
            }
            BuildingKind::CrystalMine => {
                self.config
                    .crystal_mine
                    .production_per_hour(buildings.crystal_mine)
                    * (1.0
                        + self.tech_modifier_range(ModifierKind::CrystalOutput, from, to)
                        + self.planet_bonus(planet, ModifierKind::CrystalOutput)
                        + extra_silicon)
            }
            BuildingKind::DeuteriumSynthesizer => {
                self.config
                    .deuterium_synthesizer
                    .production_per_hour(buildings.deuterium_synthesizer)
                    * (1.0
                        + self.tech_modifier_range(ModifierKind::DeuteriumOutput, from, to)
                        + self.planet_bonus(planet, ModifierKind::DeuteriumOutput)
                        + extra_deut)
            }
            BuildingKind::FissionReactor => {
                self.recipe_building_scale(
                    buildings,
                    planet,
                    extra,
                    BuildingKind::MetalMine,
                    span,
                    coord,
                ) * f64::from(buildings.fission_reactor)
            }
            BuildingKind::Shipyard => buildings.shipyard as f64,
            _ => 0.0,
        };
        scale * self.building_health(coord, building)
    }

    pub fn energy_produced(&self) -> f64 {
        self.config
            .solar_plant
            .energy_output(self.buildings.solar_plant)
            * self.config.orbits.solar_flux(self.home)
            * (1.0 + self.modifier(ModifierKind::EnergyOutput)).max(0.0)
            + self.reactor_output_at(self.home)
    }

    /// Current total electrical output for an owned planet, including orbit and local modifiers.
    pub fn energy_produced_at(&self, coord: Coord) -> Option<f64> {
        Some(self.site_energy_produced(coord, *self.buildings_at(coord)?, self.planet_kind(coord)?))
    }

    pub fn energy_consumed(&self) -> f64 {
        self.config.energy_draw(&self.buildings) + self.automation_control_energy()
    }

    pub fn waste_home(&self) -> crate::WasteReport {
        self.waste_at(self.home)
            .unwrap_or_else(|| self.waste_for(self.buildings, self.home_kind))
    }

    pub fn waste_at(&self, coord: Coord) -> Option<crate::WasteReport> {
        let mut report = self.base_waste_at(coord)?;
        if self.buildings_at(coord)?.fission_reactor == 0
            && !self.reactor_shutdowns.contains_key(&coord)
        {
            return Some(report);
        }
        report.heat_load += self.reactor_output_at(coord)
            * self.config.fission_reactor.waste_heat_per_energy
            + self.reactor_decay_heat_at(coord);
        Some(report)
    }

    pub(crate) fn base_waste_at(&self, coord: Coord) -> Option<crate::WasteReport> {
        Some(self.waste_for(*self.buildings_at(coord)?, self.planet_kind(coord)?))
    }

    fn waste_for(&self, buildings: Buildings, planet: PlanetType) -> crate::WasteReport {
        crate::waste::report(
            &self.config.waste,
            &self.config.chains.recipes,
            self.config.industry_energy(&buildings),
            buildings,
            planet,
            self.tech_modifier(ModifierKind::HeatSink),
            self.tech_modifier(ModifierKind::PollutionSink),
        )
    }

    pub fn unrest_factor(&self) -> f64 {
        self.unrest_for(self.home)
    }

    /// Production throttle from an emission-cap law. `1.0` when no cap is in force.
    pub fn emission_factor(&self, coord: Coord) -> f64 {
        self.emission_factor_range(coord, self.last_tick_unix, self.last_tick_unix)
    }

    fn emission_factor_range(&self, coord: Coord, from: u64, to: u64) -> f64 {
        let Some(report) = self.waste_at(coord) else {
            return 1.0;
        };
        self.laws.emission_factor_range(
            &self.config.laws,
            from,
            to,
            coord,
            report.pollution_load,
            report.pollution_sink,
        )
    }

    fn law_metrics_for(&self, jurisdiction: crate::Jurisdiction) -> crate::LawMetrics {
        match jurisdiction {
            crate::Jurisdiction::Empire => {
                let waste = self.waste_home();
                crate::LawMetrics {
                    output: ResourceKind::ALL
                        .iter()
                        .map(|kind| self.empire_production_per_hour(*kind))
                        .sum(),
                    pollution: waste.pollution_load * self.emission_factor(self.home),
                    pollution_load: waste.pollution_load,
                    pollution_sink: waste.pollution_sink,
                    research: self.research_speed(),
                }
            }
            crate::Jurisdiction::Planet(coord) => {
                let Some(waste) = self.waste_at(coord) else {
                    return crate::LawMetrics {
                        output: 0.0,
                        pollution: 0.0,
                        pollution_load: 0.0,
                        pollution_sink: 0.0,
                        research: 0.0,
                    };
                };
                let output = ResourceKind::ALL
                    .iter()
                    .map(|kind| self.production_at(coord, *kind))
                    .sum();
                crate::LawMetrics {
                    output,
                    pollution: waste.pollution_load * self.emission_factor(coord),
                    pollution_load: waste.pollution_load,
                    pollution_sink: waste.pollution_sink,
                    research: if coord == self.home {
                        self.research_speed()
                    } else {
                        0.0
                    },
                }
            }
        }
    }

    pub(crate) fn production_at(&self, coord: Coord, kind: ResourceKind) -> f64 {
        if coord == self.home {
            self.production_per_hour(kind)
        } else {
            self.colonies
                .iter()
                .find(|colony| colony.coord == coord)
                .map(|colony| self.colony_production_per_hour(colony, kind))
                .unwrap_or(0.0)
        }
    }

    fn next_law_at(&self) -> Option<u64> {
        self.laws
            .next_event_at(&self.config.laws, self.last_tick_unix)
    }

    fn apply_laws_at(&mut self, now: u64) {
        let by_id: Vec<(u64, crate::LawMetrics)> = self
            .laws
            .laws
            .iter()
            .map(|law| (law.id, self.law_metrics_for(law.jurisdiction)))
            .collect();
        let old_speed = self.research_speed();
        let logs = self.laws.apply_clock(&self.config.laws, now, |law| {
            by_id
                .iter()
                .find(|(id, _)| *id == law.id)
                .map(|(_, metrics)| *metrics)
                .unwrap_or(crate::LawMetrics {
                    output: 0.0,
                    pollution: 0.0,
                    pollution_load: 0.0,
                    pollution_sink: 0.0,
                    research: 0.0,
                })
        });
        let new_speed = self.research_speed();
        self.rescale_research(now, old_speed, new_speed);
        for (at, text) in logs {
            self.push_event(at, text);
        }
    }

    fn collect_law_tax(&mut self, hours: f64, from: u64, to: u64) {
        let rate = self.laws.effect_range(
            crate::LawEffectKind::TaxMetalPerHour,
            &self.config.laws,
            from,
            to,
            self.home,
        );
        if rate <= 0.0 || hours <= 0.0 {
            return;
        }
        let take = (rate * hours).max(0.0).min(self.resources.iron.max(0.0));
        self.resources.iron -= take;
        if take <= 0.0 {
            return;
        }
        let tax_ids: Vec<u64> = self
            .laws
            .laws
            .iter()
            .filter(|law| {
                matches!(
                    law.status,
                    crate::LawStatus::Enacted | crate::LawStatus::RepealProposed
                ) && self
                    .config
                    .laws
                    .get(&law.template_id)
                    .is_some_and(|spec| spec.has_effect(crate::LawEffectKind::TaxMetalPerHour))
            })
            .map(|law| law.id)
            .collect();
        for id in tax_ids {
            if let Some(law) = self.laws.get_mut(id) {
                law.revenue_accrued += take;
            }
        }
        let mut taxed = Resources::ZERO;
        taxed.iron = take;
        let amount = self.value_resources(taxed);
        self.credit_income(amount, crate::finance::IncomeKind::Tax);
    }

    /// Propose a law. `effective_in` is seconds from now.
    pub fn propose_law(
        &mut self,
        template_id: &str,
        param: f64,
        jurisdiction: crate::Jurisdiction,
        effective_in: u64,
        now_unix: u64,
    ) -> Result<u64, crate::LawError> {
        self.propose_law_as(
            template_id,
            param,
            jurisdiction,
            effective_in,
            now_unix,
            crate::Actor::Player,
        )
    }

    pub fn propose_law_as(
        &mut self,
        template_id: &str,
        param: f64,
        jurisdiction: crate::Jurisdiction,
        effective_in: u64,
        now_unix: u64,
        actor: crate::Actor,
    ) -> Result<u64, crate::LawError> {
        self.catch_up(now_unix);
        self.authorize(actor, crate::Permission::ProposeLaw, None, 0.0)
            .map_err(|err| crate::LawError::PermissionDenied(err.to_string()))?;
        let now = self.last_tick_unix;
        if let crate::Jurisdiction::Planet(coord) = jurisdiction
            && self.buildings_at(coord).is_none()
        {
            return Err(crate::LawError::UnownedWorld);
        }
        let id = self.laws.propose(
            &self.config.laws,
            template_id,
            param,
            jurisdiction,
            now.saturating_add(effective_in),
            now,
        )?;
        self.push_event(now, format!("proposed law {template_id} #{id}"));
        Ok(id)
    }

    pub fn approve_law(&mut self, id: u64, now_unix: u64) -> Result<(), crate::LawError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        self.laws.approve(id, now)?;
        self.push_event(now, format!("approved law #{id}"));
        self.apply_laws_at(now);
        self.apply_automation_at(now);
        Ok(())
    }

    pub fn repeal_law(
        &mut self,
        id: u64,
        effective_in: u64,
        now_unix: u64,
    ) -> Result<(), crate::LawError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        self.laws
            .propose_repeal(id, now.saturating_add(effective_in), now)?;
        self.push_event(now, format!("proposed repeal of law #{id}"));
        Ok(())
    }

    fn unrest_for(&self, coord: Coord) -> f64 {
        self.waste_at(coord)
            .map(|report| report.unrest_factor(self.config.waste.unrest_per_overflow))
            .unwrap_or(1.0)
    }

    /// Produced minus consumed. Negative when mines draw more than the plant supplies.
    pub fn energy_balance(&self) -> f64 {
        self.energy_produced() - self.energy_consumed()
    }

    /// Scales mine output when energy is short. Always in `[0, 1]`.
    fn mine_health_factor(&self, kind: ResourceKind) -> f64 {
        let building = match kind {
            ResourceKind::Iron => crate::BuildingKind::MetalMine,
            ResourceKind::Silicon => crate::BuildingKind::CrystalMine,
            ResourceKind::Deuterium => crate::BuildingKind::DeuteriumSynthesizer,
            _ => return 1.0,
        };
        self.building_health(self.home, building)
    }

    pub fn energy_factor(&self) -> f64 {
        self.energy_factor_for(self.energy_produced())
    }

    pub(crate) fn energy_factor_for(&self, produced: f64) -> f64 {
        let consumed = self.energy_consumed();
        let base = if consumed <= 0.0 {
            1.0
        } else {
            (produced / consumed).clamp(0.0, 1.0)
        };
        let Some(run) = self.species.as_ref() else {
            return base;
        };
        let mut factor = base.powf(run.energy_curve.max(0.05));
        if run.energy_surplus > 0.0 && consumed > 0.0 && produced > consumed {
            factor *= 1.0 + run.energy_surplus * (produced / consumed - 1.0).min(2.0);
        }
        factor
    }

    pub fn production_per_hour(&self, kind: ResourceKind) -> f64 {
        let factor = self.energy_factor() * self.unrest_factor() * self.emission_factor(self.home);
        let boost = self.prod_boost_mult();
        let staff = self.governor_factor(self.home);
        if self.config.chains.enabled() {
            self.chain_net_per_hour(
                ChainSite {
                    buildings: self.buildings,
                    planet: self.home_kind,
                    coord: self.home,
                    extra_iron: 0.0,
                    extra_silicon: 0.0,
                    extra_deut: 0.0,
                    energy_factor: factor,
                    boost,
                    resources: &self.resources,
                    goods: &self.goods,
                },
                kind,
            ) * staff
        } else {
            self.base_production_per_hour(kind)
                * factor
                * boost
                * staff
                * self.mine_health_factor(kind)
        }
    }

    fn chain_net_per_hour(&self, site: ChainSite<'_>, kind: ResourceKind) -> f64 {
        let mut stock = *site.resources;
        let mut bag = site.goods.clone();
        let before = stock.get(kind);
        let deposits = self.chain_deposits(site.planet, self.galaxy_seed, site.coord);
        let now = self.last_tick_unix;
        let extra = (site.extra_iron, site.extra_silicon, site.extra_deut);
        let metal_s = self.recipe_building_scale(
            site.buildings,
            site.planet,
            extra,
            BuildingKind::MetalMine,
            (now, now),
            site.coord,
        );
        let crystal_s = self.recipe_building_scale(
            site.buildings,
            site.planet,
            extra,
            BuildingKind::CrystalMine,
            (now, now),
            site.coord,
        );
        let deut_s = self.recipe_building_scale(
            site.buildings,
            site.planet,
            extra,
            BuildingKind::DeuteriumSynthesizer,
            (now, now),
            site.coord,
        );
        let yard_s = site.buildings.shipyard as f64;
        let fabrication_s = self.recipe_building_scale(
            site.buildings,
            site.planet,
            extra,
            BuildingKind::FissionReactor,
            (now, now),
            site.coord,
        );
        apply_recipes(
            RecipeRun {
                config: &self.config.chains,
                buildings: site.buildings,
                deposits: &deposits,
                hours: 1.0,
                energy_factor: site.energy_factor,
                boost: site.boost,
            },
            |b| match b {
                BuildingKind::MetalMine => metal_s,
                BuildingKind::FissionReactor => fabrication_s,
                BuildingKind::CrystalMine => crystal_s,
                BuildingKind::DeuteriumSynthesizer => deut_s,
                BuildingKind::Shipyard => yard_s,
                _ => 0.0,
            },
            &mut stock,
            &mut bag,
            |_| f64::MAX,
        );
        stock.get(kind) - before
    }

    pub(crate) fn prod_boost_mult(&self) -> f64 {
        match self.activity.prod_boost_until {
            Some(until) if self.last_tick_unix < until => 1.0 + self.activity.prod_boost_factor,
            _ => 1.0,
        }
    }

    fn research_speed(&self) -> f64 {
        let activity = match self.activity.research_boost_until {
            Some(until) if self.last_tick_unix < until => {
                (1.0 + self.activity.research_boost_factor).max(1.0)
            }
            _ => 1.0,
        };
        self.compose_research_speed(activity)
    }

    fn compose_research_speed(&self, activity: f64) -> f64 {
        let species = self
            .species
            .as_ref()
            .map(|run| 1.0 / run.research.max(0.05))
            .unwrap_or(1.0);
        activity.max(1.0)
            * (1.0 + self.prestige.research_bonus(&self.config.prestige)).max(1.0)
            * (1.0
                + self.laws.effect(
                    crate::LawEffectKind::ResearchSpeed,
                    &self.config.laws,
                    self.last_tick_unix,
                    self.home,
                ))
            .max(0.1)
            * species
    }

    fn rescale_research(&mut self, at: u64, old_speed: f64, new_speed: f64) {
        if (old_speed - new_speed).abs() < 1e-12 {
            return;
        }
        let Some(job) = self.research.as_mut() else {
            return;
        };
        let remaining = job.completes_at.saturating_sub(at) as f64;
        let base = (remaining * old_speed).max(1.0);
        let new_rem = (base / new_speed.max(1e-9)).ceil().max(1.0) as u64;
        job.completes_at = at.saturating_add(new_rem);
    }

    fn grant_payout(&mut self, at: u64, hours: f64) {
        if hours <= 0.0 {
            return;
        }
        for kind in ResourceKind::ALL {
            let rate = self.production_per_hour(kind);
            let cap = self.storage_cap(kind);
            let current = self.resources.get(kind);
            self.resources
                .set(kind, add_capped(current, rate, hours, cap));
        }
        self.push_event(
            at,
            format!("activity: merged PR paid {hours}h of production"),
        );
    }

    fn set_prod_boost(&mut self, at: u64, factor: f64, secs: u64) {
        if factor <= 0.0 || secs == 0 {
            return;
        }
        self.activity.prod_boost_factor = factor;
        self.activity.prod_boost_until = Some(at.saturating_add(secs));
        self.push_event(
            at,
            format!("activity: production +{:.0}% for {secs}s", factor * 100.0),
        );
    }

    fn set_research_boost(&mut self, at: u64, factor: f64, secs: u64) {
        if factor <= 0.0 || secs == 0 {
            return;
        }
        let old = self.research_speed();
        self.activity.research_boost_factor = factor;
        self.activity.research_boost_until = Some(at.saturating_add(secs));
        self.rescale_research(at, old, self.research_speed());
        self.push_event(
            at,
            format!("activity: research +{:.0}% for {secs}s", factor * 100.0),
        );
    }

    pub fn storage_cap(&self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => self
                .config
                .metal_storage
                .capacity(self.buildings.metal_storage),
            ResourceKind::Silicon => self
                .config
                .crystal_storage
                .capacity(self.buildings.crystal_storage),
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => self
                .config
                .deuterium_storage
                .capacity(self.buildings.deuterium_storage),
        }
    }

    fn base_production_per_hour(&self, kind: ResourceKind) -> f64 {
        self.base_production_range(kind, self.last_tick_unix, self.last_tick_unix)
    }

    fn base_production_range(&self, kind: ResourceKind, from: u64, to: u64) -> f64 {
        let raw = match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => self
                .config
                .metal_mine
                .production_per_hour(self.buildings.metal_mine),
            ResourceKind::Silicon => self
                .config
                .crystal_mine
                .production_per_hour(self.buildings.crystal_mine),
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => self
                .config
                .deuterium_synthesizer
                .production_per_hour(self.buildings.deuterium_synthesizer),
        };
        let output = match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => ModifierKind::MetalOutput,
            ResourceKind::Silicon => ModifierKind::CrystalOutput,
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                ModifierKind::DeuteriumOutput
            }
        };
        raw * (1.0
            + self.tech_modifier_range(output, from, to)
            + self.planet_bonus(self.home_kind, output))
    }
}

fn fleet_label(fleet: Fleet) -> String {
    let parts: Vec<String> = ShipKind::ALL
        .iter()
        .filter_map(|&kind| {
            let n = fleet.get(kind);
            (n > 0).then(|| format!("{n} {kind}"))
        })
        .collect();
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

fn fleet_losses(before: Fleet, after: Fleet) -> Fleet {
    let mut lost = Fleet::default();
    for kind in ShipKind::ALL {
        lost.add(kind, before.get(kind).saturating_sub(after.get(kind)));
    }
    lost
}

fn spy_event_text(report: &SpyReport) -> String {
    let detected = if report.detected { "yes" } else { "no" };
    format!(
        "outpost {}: spy. score {:.0} {}. detected {detected}. lost {}/{}",
        report.target, report.score, report.tier, report.probes_lost, report.probes_sent
    )
}

fn format_outpost_attack(report: &crate::CombatReport, dest: Coord) -> String {
    format!(
        "outpost {dest}: attack. {} rounds. lost {}. defences lost {}. winner {}",
        report.rounds,
        fleet_label(fleet_losses(report.attacker_before, report.attacker_after)),
        fleet_label(fleet_losses(report.defender_before, report.defender_after)),
        report.winner.as_key()
    )
}

fn format_outpost_invade(report: &crate::CombatReport, dest: Coord) -> String {
    match &report.ground {
        Some(ground) => format!(
            "outpost {dest}: invade. troops {} vs garrison {}. rolls {}/{}. winner {}",
            ground.attacker_strength,
            ground.defender_strength,
            ground.attacker_roll,
            ground.defender_roll,
            ground.winner.as_key()
        ),
        None => format!("outpost {dest}: invade. winner {}", report.winner.as_key()),
    }
}

fn format_raid_report(report: &crate::CombatReport, plunder: Resources) -> String {
    format!(
        "raid: pirates {} vs {}. {} rounds. lost {}. pirates lost {}. plunder {}. winner {}",
        fleet_label(report.attacker_before),
        fleet_label(report.defender_before),
        report.rounds,
        fleet_label(fleet_losses(report.defender_before, report.defender_after)),
        fleet_label(fleet_losses(report.attacker_before, report.attacker_after)),
        plunder.brief(),
        report.winner.as_key(),
    )
}

fn format_layered_raid(
    pirates: Fleet,
    parked: Fleet,
    battle: &PlanetBattle,
    plunder: Resources,
) -> String {
    let intercept = if battle.intercept_in > 0 {
        format!(
            "intercept {}/{}. ",
            battle.intercept_down, battle.intercept_in
        )
    } else {
        String::new()
    };
    let dome = if battle.dome_collapsed {
        format!("dome collapsed absorbed {:.0}", battle.dome_absorbed)
    } else if battle.dome_absorbed > 0.0 {
        format!("dome held absorbed {:.0}", battle.dome_absorbed)
    } else {
        "dome none".into()
    };
    let ground = match &battle.ground {
        Some(g) => format!(
            "ground troops {} vs garrison {} winner {}",
            g.attacker_strength,
            g.defender_strength,
            g.winner.as_key()
        ),
        None => "ground none".into(),
    };
    format!(
        "raid: pirates {} vs {}. {intercept}orbital stations {}->{} hangar {}->{} {} rounds. {dome}. batteries {}->{}. {ground}. lost {}. pirates lost {}. rebuilt {}. winner {}. plunder {}",
        fleet_label(pirates),
        fleet_label(parked),
        battle.stations_before,
        battle.stations_after,
        battle.hangar_before,
        battle.hangar_after,
        battle.orbital_rounds,
        battle.batteries_before,
        battle.batteries_after,
        fleet_label(fleet_losses(parked, battle.fleet_after)),
        fleet_label(fleet_losses(pirates, battle.attacker_after)),
        battle.rebuilt,
        battle.winner.as_key(),
        plunder.brief()
    )
}

fn scale_ground(strength: u32, mult: f64) -> u32 {
    if strength == 0 {
        0
    } else {
        ((f64::from(strength) * mult).floor() as u32).max(1)
    }
}

fn ship_duration(base_secs: u64, shipyard_speed: f64) -> u64 {
    let base = base_secs as f64;
    (base / (1.0 + shipyard_speed).max(1.0)).ceil().max(1.0) as u64
}

fn min_time(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Integrate a constant rate for `hours`, stopping exactly at `cap`.
/// Never lowers `current` (over-cap stockpiles and zero/negative rates stay put).
fn projected_on(
    buildings: &Buildings,
    queue: &[BuildJob],
    kind: BuildingKind,
    index: usize,
) -> u32 {
    buildings.get(kind)
        + queue[..index]
            .iter()
            .filter(|job| job.building == kind)
            .count() as u32
}

fn duration_with_robotics(
    kind: BuildingKind,
    from_level: u32,
    robotics: u32,
    config: &BalanceConfig,
) -> u64 {
    let raw = config.buildings.get(kind).raw_seconds(from_level);
    let divisor = 1.0 + f64::from(robotics) * config.robotics_speed_per_level;
    let secs = if divisor > 0.0 { raw / divisor } else { raw };
    secs.ceil().max(1.0) as u64
}

fn start_queue_head(
    queue: &mut [BuildJob],
    robotics: u32,
    config: &BalanceConfig,
    now: u64,
    construction: f64,
) {
    let Some(job) = queue.first() else {
        return;
    };
    if job.completes_at.is_some() {
        return;
    }
    let base = duration_with_robotics(job.building, job.from_level, robotics, config);
    let duration = ((base as f64) * construction.max(0.05)).ceil().max(1.0) as u64;
    let job = &mut queue[0];
    job.started_at = Some(now);
    job.completes_at = Some(now.saturating_add(duration));
}

fn add_capped(current: f64, rate_per_hour: f64, hours: f64, cap: f64) -> f64 {
    if hours <= 0.0 || rate_per_hour <= 0.0 || current >= cap {
        current
    } else {
        (current + rate_per_hour * hours).min(cap)
    }
}

fn cargo_has_mass(cargo: Resources) -> bool {
    ResourceKind::ALL.iter().any(|&kind| cargo.get(kind) > 1e-9)
}

fn format_cargo(cargo: Resources) -> String {
    let mut parts = Vec::new();
    for kind in ResourceKind::ALL {
        let n = cargo.get(kind);
        if n > 1e-9 {
            parts.push(format!("{:.0} {}", n, kind.as_key()));
        }
    }
    if parts.is_empty() {
        "nothing".into()
    } else {
        parts.join(", ")
    }
}

fn push_logistics_line(buf: &mut Vec<String>, line: String) {
    buf.push(line);
    if buf.len() > LOGISTICS_LOG_CAP {
        let drop = buf.len() - LOGISTICS_LOG_CAP;
        buf.drain(..drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::TechTree;
    use crate::buildings::{BuildError, BuildingKind};
    use crate::clock::{Clock, FakeClock};
    use crate::fleets::{Coord, FleetError, MissionPhase};
    use crate::research::{ModifierKind, PrereqGap, ResearchError};
    use crate::ships::{ShipError, ShipKind};

    const EPS: f64 = 1e-6;

    fn close(a: f64, b: f64) {
        assert!(
            (a - b).abs() < EPS,
            "left={a} right={b} delta={}",
            (a - b).abs()
        );
    }

    /// Config with round numbers so catch-up tests read the spec fields, not defaults.
    fn test_config() -> BalanceConfig {
        BalanceConfig {
            metal_mine: crate::ProducerSpec {
                base_per_hour: 100.0,
                growth: 2.0,
                base_energy: 10.0,
                energy_growth: 1.0,
            },
            crystal_mine: crate::ProducerSpec {
                base_per_hour: 50.0,
                growth: 2.0,
                base_energy: 10.0,
                energy_growth: 1.0,
            },
            deuterium_synthesizer: crate::ProducerSpec {
                base_per_hour: 20.0,
                growth: 2.0,
                base_energy: 20.0,
                energy_growth: 1.0,
            },
            solar_plant: crate::EnergyProducerSpec {
                base_energy: 20.0,
                growth: 1.0,
            },
            fission_reactor: crate::ReactorConfig::default(),
            metal_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            crystal_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            deuterium_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            starting_resources: Resources::from_parts(0.0, 0.0, 0.0),
            starting_buildings: Buildings {
                metal_mine: 1,
                crystal_mine: 1,
                deuterium_synthesizer: 0,
                solar_plant: 1,
                metal_storage: 0,
                crystal_storage: 0,
                deuterium_storage: 0,
                robotics_factory: 0,
                research_lab: 0,
                shipyard: 0,
                radiator: 0,
                scrubber: 0,
                fission_reactor: 0,
            },
            buildings: crate::BuildingCatalog::default(),
            max_build_queue: 5,
            robotics_speed_per_level: 1.0,
            techs: TechTree::empty(),
            ships: crate::ShipCatalog::default(),
            max_ship_queue: 5,
            flight: crate::FlightConfig::default(),
            raid: crate::RaidConfig::inactive(),
            planet_types: crate::PlanetTypeCatalog {
                rocky: crate::PlanetTypeBonus::ZERO,
                ice: crate::PlanetTypeBonus::ZERO,
                gas_giant: crate::PlanetTypeBonus::ZERO,
            },
            activity: crate::ActivityEffectsConfig::default(),
            expedition: crate::ExpeditionConfig::default(),
            pacing: crate::PacingTargets::default(),
            wonders: Vec::new(),
            outposts: crate::OutpostConfig::default(),
            prestige: crate::PrestigeConfig::default(),
            chains: crate::ChainConfig::default(),
            story: crate::StoryCatalog::empty(),
            market: crate::MarketConfig::inactive(),
            diplomacy: crate::DiplomacyConfig::inactive(),
            espionage: crate::EspionageConfig::default(),
            defences: crate::DefenceConfig::default(),
            waste: crate::WasteConfig::default(),
            laws: crate::LawConfig::default(),
            achievements: crate::AchievementConfig::empty(),
            daily: crate::DailyConfig::empty(),
            moons: crate::MoonConfig::inactive(),
            tactics: crate::TacticsConfig::default(),
            leaders: {
                let mut leaders = crate::LeaderConfig::default();
                leaders.empty_governor = 0.0;
                leaders.empty_admiral = 0.0;
                leaders
            },
            orbits: crate::OrbitConfig::default(),
            victory: crate::VictoryConfig::inactive(),
            ladder: crate::LadderConfig::default(),
            frontier: crate::FrontierConfig::default(),
            self_play: crate::SelfPlayConfig::default(),
        }
    }

    fn catch_up_via_clock(state: &mut GameState, clock: &FakeClock) {
        state.catch_up(clock.now_unix());
    }

    #[test]
    fn production_and_energy_factor_come_from_config() {
        let cfg = test_config();
        let state = GameState::with_config(0, cfg.clone());
        // Solar 20, mines 10+10, factor 1. Metal 100/h, crystal 50/h from spec fields.
        close(state.energy_produced(), cfg.solar_plant.base_energy);
        close(
            state.energy_consumed(),
            cfg.metal_mine.base_energy + cfg.crystal_mine.base_energy,
        );
        close(state.energy_factor(), 1.0);
        close(
            state.production_per_hour(ResourceKind::Iron),
            cfg.metal_mine.base_per_hour,
        );
        close(
            state.production_per_hour(ResourceKind::Silicon),
            cfg.crystal_mine.base_per_hour,
        );
        close(state.production_per_hour(ResourceKind::Deuterium), 0.0);
        close(
            state.storage_cap(ResourceKind::Iron),
            cfg.metal_storage.base,
        );
    }

    #[test]
    fn energy_shortfall_scales_all_mine_output() {
        let mut cfg = test_config();
        cfg.solar_plant.base_energy = 10.0; // need 20 → factor 0.5
        let mut state = GameState::with_config(0, cfg.clone());
        close(state.energy_factor(), 0.5);
        close(state.energy_balance(), -10.0);
        state.catch_up(3600);
        close(state.resources.iron, cfg.metal_mine.base_per_hour * 0.5);
        close(
            state.resources.silicon,
            cfg.crystal_mine.base_per_hour * 0.5,
        );
        close(state.resources.deuterium, 0.0);
    }

    #[test]
    fn waste_overflow_slows_production_and_grows_pirate_fleets() {
        let mut cfg = crate::BalanceConfig::default();
        cfg.raid.min_ships = 1;
        cfg.raid.strength_factor = 0.0;
        let mut raw = GameState::with_config(0, cfg.clone());
        raw.buildings.metal_mine = 18;
        raw.buildings.crystal_mine = 18;
        raw.buildings.deuterium_synthesizer = 12;
        raw.buildings.solar_plant = 24;
        assert!(raw.waste_home().overflowing());
        assert!(raw.unrest_factor() < 1.0);
        let quiet = raw.production_per_hour(ResourceKind::Iron);
        let pirates = raw.pirate_fleet().get(crate::ShipKind::LightFighter);
        assert!(pirates > 1, "{pirates}");

        let mut ok = raw.clone();
        ok.buildings.radiator = 5;
        ok.buildings.scrubber = 3;
        assert!(!ok.waste_home().overflowing());
        close(ok.unrest_factor(), 1.0);
        assert!(ok.production_per_hour(ResourceKind::Iron) > quiet);
        assert_eq!(ok.pirate_fleet().get(crate::ShipKind::LightFighter), 1);
    }

    #[test]
    fn production_chains_stop_when_an_input_runs_out() {
        let mut cfg = test_config();
        let mut deposits = BTreeMap::new();
        deposits.insert(
            PlanetType::Rocky,
            BTreeMap::from([("iron_ore".into(), 0.0)]),
        );
        cfg.chains = crate::ChainConfig {
            goods: vec![
                crate::GoodSpec {
                    id: "iron_ore".into(),
                    kind: crate::GoodKind::Raw,
                },
                crate::GoodSpec {
                    id: "steel".into(),
                    kind: crate::GoodKind::Refined,
                },
                crate::GoodSpec {
                    id: "frames".into(),
                    kind: crate::GoodKind::Component,
                },
            ],
            recipes: vec![
                crate::Recipe {
                    id: "extract".into(),
                    building: BuildingKind::MetalMine,
                    deposit: Some("iron_ore".into()),
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::from([("iron_ore".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
                crate::Recipe {
                    id: "smelt".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("iron_ore".into(), 1.0)]),
                    outputs: BTreeMap::from([("steel".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
                crate::Recipe {
                    id: "forge".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("steel".into(), 1.0)]),
                    outputs: BTreeMap::from([("frames".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
            ],
            deposits,
            deposit_jitter: 0.0,
        };
        cfg.starting_buildings.crystal_mine = 0;
        let mut state = GameState::with_config(0, cfg);
        state.goods.insert("iron_ore".into(), 5.0);
        close(state.production_per_hour(ResourceKind::Iron), 5.0);
        state.catch_up(3600);
        close(state.resources.iron, 5.0);
        close(state.goods.get("iron_ore").copied().unwrap_or(0.0), 0.0);
        close(state.goods.get("steel").copied().unwrap_or(0.0), 0.0);
        state.catch_up(7200);
        close(state.resources.iron, 5.0);
    }

    #[test]
    fn bundled_chains_fill_component_stockpiles() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(0, cfg);
        let start = state.resources.iron;
        state.catch_up(3600);
        assert!(
            state.resources.iron > start,
            "frames {} start {start}",
            state.resources.iron
        );
        assert!(
            state.goods.values().any(|v| *v > 0.0),
            "intermediates {:?}",
            state.goods
        );
    }

    #[test]
    fn zero_energy_stops_mines() {
        let mut cfg = test_config();
        cfg.starting_buildings.solar_plant = 0;
        let mut state = GameState::with_config(0, cfg);
        close(state.energy_factor(), 0.0);
        state.catch_up(3600 * 10);
        close(state.resources.iron, 0.0);
        close(state.resources.silicon, 0.0);
    }

    #[test]
    fn higher_mine_level_uses_growth_from_config() {
        let mut cfg = test_config();
        cfg.starting_buildings.metal_mine = 3; // 100 * 2^2 = 400
        cfg.solar_plant.base_energy = 1_000.0; // no shortfall
        let mut state = GameState::with_config(0, cfg.clone());
        close(
            state.production_per_hour(ResourceKind::Iron),
            cfg.metal_mine.base_per_hour * cfg.metal_mine.growth.powi(2),
        );
        state.catch_up(3600);
        close(
            state.resources.iron,
            cfg.metal_mine.base_per_hour * cfg.metal_mine.growth.powi(2),
        );
    }

    #[test]
    fn catch_up_over_gap_equals_step_by_step() {
        let mut one_shot = GameState::with_config(0, test_config());
        one_shot.catch_up(7200);

        let mut stepped = GameState::with_config(0, test_config());
        for t in 1..=7200 {
            stepped.catch_up(t);
        }

        close(one_shot.resources.iron, stepped.resources.iron);
        close(one_shot.resources.silicon, stepped.resources.silicon);
        close(one_shot.resources.deuterium, stepped.resources.deuterium);
        assert_eq!(one_shot.last_tick_unix, 7200);
        assert_eq!(stepped.last_tick_unix, 7200);
    }

    #[test]
    fn colony_production_rate_applies_planet_emission_cap() {
        let mut state = GameState::new(0);
        let coord = Coord {
            galaxy: 1,
            system: 1,
            position: 4,
        };
        state.colonies.push(Colony {
            coord,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::ZERO,
            buildings: Buildings {
                metal_mine: 18,
                solar_plant: 24,
                ..Buildings::default()
            },
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
        let before = state.colony_production_per_hour(&state.colonies[0], ResourceKind::Iron);
        let id = state
            .propose_law(
                "emission_cap",
                0.2,
                crate::Jurisdiction::Planet(coord),
                0,
                0,
            )
            .unwrap();
        state.approve_law(id, 0).unwrap();
        state.catch_up(3_600);
        let after = state.colony_production_per_hour(&state.colonies[0], ResourceKind::Iron);
        assert!(
            after < before,
            "colony rate {after} should drop from {before}"
        );
        let samples = state.laws.latest_samples(id);
        let pollution = samples
            .iter()
            .find(|s| s.metric == crate::LawMetric::Pollution)
            .expect("colony pollution sample");
        let colony_waste = state.waste_at(coord).expect("colony waste");
        let allowed = colony_waste.pollution_sink * 0.2;
        let expected = if colony_waste.pollution_load <= allowed {
            colony_waste.pollution_load
        } else {
            allowed
        };
        assert!(
            (pollution.expected - expected).abs() < 1e-6,
            "sample {pollution:?} should use colony sink {}, load {}",
            colony_waste.pollution_sink,
            colony_waste.pollution_load
        );
        let home_waste = state.waste_home();
        assert!(
            (home_waste.pollution_sink - colony_waste.pollution_sink).abs() > 1e-6
                || (home_waste.pollution_load - colony_waste.pollution_load).abs() > 1e-6,
            "home and colony waste should differ so the sample is not accidentally home"
        );
    }

    #[test]
    fn catch_up_uses_injected_clock() {
        let clock = FakeClock::new(1_000);
        let mut state = GameState::with_config(clock.now_unix(), test_config());
        clock.advance(3600);
        catch_up_via_clock(&mut state, &clock);
        close(state.resources.iron, 100.0);
        assert_eq!(state.last_tick_unix, 4_600);
    }

    #[test]
    fn storage_cap_stops_accumulation_and_partial_tick_is_exact() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 150.0;
        cfg.starting_resources.iron = 100.0;
        // 100/h, 50 room → hits cap in 0.5 h = 1800 s.
        let mut one_shot = GameState::with_config(0, cfg.clone());
        one_shot.catch_up(3600);
        close(one_shot.resources.iron, 150.0);

        let mut stepped = GameState::with_config(0, cfg.clone());
        for t in 1..=3600 {
            stepped.catch_up(t);
        }
        close(stepped.resources.iron, 150.0);
        close(one_shot.resources.iron, stepped.resources.iron);

        // Exactly at the crossing instant.
        let mut at_cap_time = GameState::with_config(0, cfg);
        at_cap_time.catch_up(1800);
        close(at_cap_time.resources.iron, 150.0);
    }

    #[test]
    fn over_cap_stockpile_is_not_drained() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 50.0;
        cfg.starting_resources.iron = 80.0;
        let mut state = GameState::with_config(0, cfg);
        state.catch_up(3600);
        close(state.resources.iron, 80.0);
        let mut chained = test_config();
        chained.metal_storage.base = 50.0;
        chained.starting_resources.iron = 80.0;
        chained.chains = crate::ChainConfig {
            goods: vec![crate::GoodSpec {
                id: "frames".into(),
                kind: crate::GoodKind::Component,
            }],
            recipes: vec![crate::Recipe {
                id: "forge".into(),
                building: BuildingKind::MetalMine,
                deposit: None,
                inputs: BTreeMap::new(),
                outputs: BTreeMap::from([("frames".into(), 1.0)]),
                energy: 0.0,
                heat: 0.0,
                pollution: 0.0,
            }],
            deposits: BTreeMap::new(),
            deposit_jitter: 0.0,
        };
        let mut state = GameState::with_config(0, chained);
        state.catch_up(3600);
        close(state.resources.iron, 80.0);
    }

    #[test]
    fn full_frames_stockpile_does_not_consume_steel() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 50.0;
        cfg.starting_resources.iron = 50.0;
        cfg.starting_buildings.crystal_mine = 0;
        let mut deposits = BTreeMap::new();
        deposits.insert(
            PlanetType::Rocky,
            BTreeMap::from([("iron_ore".into(), 0.0)]),
        );
        cfg.chains = crate::ChainConfig {
            goods: vec![
                crate::GoodSpec {
                    id: "iron_ore".into(),
                    kind: crate::GoodKind::Raw,
                },
                crate::GoodSpec {
                    id: "steel".into(),
                    kind: crate::GoodKind::Refined,
                },
                crate::GoodSpec {
                    id: "frames".into(),
                    kind: crate::GoodKind::Component,
                },
            ],
            recipes: vec![
                crate::Recipe {
                    id: "extract".into(),
                    building: BuildingKind::MetalMine,
                    deposit: Some("iron_ore".into()),
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::from([("iron_ore".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
                crate::Recipe {
                    id: "smelt".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("iron_ore".into(), 1.0)]),
                    outputs: BTreeMap::from([("steel".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
                crate::Recipe {
                    id: "forge".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("steel".into(), 1.0)]),
                    outputs: BTreeMap::from([("frames".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
            ],
            deposits,
            deposit_jitter: 0.0,
        };
        let mut state = GameState::with_config(0, cfg);
        state.goods.insert("steel".into(), 20.0);
        state.catch_up(3600);
        close(state.resources.iron, 50.0);
        close(state.goods.get("steel").copied().unwrap_or(0.0), 20.0);
    }

    #[test]
    fn clock_going_backwards_does_not_lose_resources() {
        let mut state = GameState::with_config(100, test_config());
        state.catch_up(3600);
        let metal = state.resources.iron;
        let crystal = state.resources.silicon;
        let last = state.last_tick_unix;
        state.catch_up(50);
        close(state.resources.iron, metal);
        close(state.resources.silicon, crystal);
        assert_eq!(state.last_tick_unix, last);
    }

    #[test]
    fn default_starter_has_full_energy_for_starting_mines() {
        let state = GameState::new(0);
        assert!(state.energy_factor() >= 1.0 - EPS);
        assert!(state.production_per_hour(ResourceKind::Iron) > 0.0);
        assert!(state.production_per_hour(ResourceKind::Silicon) > 0.0);
    }

    #[test]
    fn catch_up_equals_step_by_step_over_random_gaps() {
        let mut seed = 0xC0FFEE_u64;
        for trial in 0..40 {
            let gap = 1 + splitmix(&mut seed) % (3 * 24 * 3600);
            let metal_level = (splitmix(&mut seed) % 6) as u32;
            let solar_level = 1 + (splitmix(&mut seed) % 8) as u32;
            let fill = (splitmix(&mut seed) % 1000) as f64;

            let mut cfg = test_config();
            cfg.starting_buildings.metal_mine = metal_level;
            cfg.starting_buildings.crystal_mine = metal_level.min(4);
            cfg.starting_buildings.solar_plant = solar_level;
            cfg.starting_resources.iron = fill;
            cfg.starting_resources.silicon = fill / 2.0;

            let mut one_shot = GameState::with_config(0, cfg.clone());
            one_shot.catch_up(gap);

            let mut stepped = GameState::with_config(0, cfg);
            let step = if gap > 7200 { 60 } else { 1 };
            let mut t = 0u64;
            while t + step < gap {
                t += step;
                stepped.catch_up(t);
            }
            stepped.catch_up(gap);

            assert_eq!(one_shot.last_tick_unix, gap, "trial {trial}");
            close(one_shot.resources.iron, stepped.resources.iron);
            close(one_shot.resources.silicon, stepped.resources.silicon);
            close(one_shot.resources.deuterium, stepped.resources.deuterium);
            assert!(
                one_shot.resources.iron <= one_shot.storage_cap(ResourceKind::Iron) + EPS
                    || fill > one_shot.storage_cap(ResourceKind::Iron),
                "cap violated trial {trial}"
            );
        }
    }

    #[test]
    fn catch_up_routes_chains_queues_match_step_by_step() {
        let mut seed = 0xA11CE_u64;
        for trial in 0..20 {
            let gap = 1 + splitmix(&mut seed) % (2 * 24 * 3600);
            let mut cfg = logistics_config();
            cfg.chains = crate::ChainConfig {
                goods: vec![crate::GoodSpec {
                    id: "steel".into(),
                    kind: crate::GoodKind::Refined,
                }],
                recipes: vec![crate::Recipe {
                    id: "forge".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("steel".into(), 1.0)]),
                    outputs: BTreeMap::from([("frames".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                }],
                deposits: BTreeMap::new(),
                deposit_jitter: 0.0,
            };
            cfg.buildings.metal_mine = cheap_spec(1_800.0);
            cfg.starting_resources.iron = 20_000.0;
            cfg.starting_resources.deuterium = 500.0;
            cfg.starting_resources.silicon = 5_000.0;

            let mut one_shot = GameState::with_config(0, cfg.clone());
            plant_colony(&mut one_shot, neighbor());
            one_shot.fleet.add(ShipKind::SmallCargo, 2);
            one_shot.goods.insert("steel".into(), 40.0);
            one_shot
                .schedule_route(
                    one_shot.home,
                    neighbor(),
                    cargo_fleet(),
                    Resources::from_parts(50.0, 0.0, 0.0),
                    30,
                    0,
                )
                .unwrap();
            one_shot.enqueue_build(BuildingKind::MetalMine, 0).unwrap();

            let mut stepped = GameState::with_config(0, cfg);
            plant_colony(&mut stepped, neighbor());
            stepped.fleet.add(ShipKind::SmallCargo, 2);
            stepped.goods.insert("steel".into(), 40.0);
            stepped
                .schedule_route(
                    stepped.home,
                    neighbor(),
                    cargo_fleet(),
                    Resources::from_parts(50.0, 0.0, 0.0),
                    30,
                    0,
                )
                .unwrap();
            stepped.enqueue_build(BuildingKind::MetalMine, 0).unwrap();

            one_shot.catch_up(gap);
            let step = if gap > 7200 { 60 } else { 1 };
            let mut t = 0u64;
            while t + step < gap {
                t += step;
                stepped.catch_up(t);
            }
            stepped.catch_up(gap);

            assert_eq!(one_shot.last_tick_unix, gap, "trial {trial}");
            close(one_shot.resources.iron, stepped.resources.iron);
            close(one_shot.resources.silicon, stepped.resources.silicon);
            close(one_shot.resources.deuterium, stepped.resources.deuterium);
            close(
                one_shot.colonies[0].resources.iron,
                stepped.colonies[0].resources.iron,
            );
            close(
                one_shot.goods.get("steel").copied().unwrap_or(0.0),
                stepped.goods.get("steel").copied().unwrap_or(0.0),
            );
            assert_eq!(one_shot.buildings.metal_mine, stepped.buildings.metal_mine);
            assert_eq!(one_shot.missions.len(), stepped.missions.len());
            assert_eq!(
                one_shot.routes[0].next_at, stepped.routes[0].next_at,
                "trial {trial}"
            );
            assert_eq!(
                one_shot.fleet.get(ShipKind::SmallCargo),
                stepped.fleet.get(ShipKind::SmallCargo)
            );
        }
    }

    #[test]
    fn catch_up_seven_days_on_a_large_empire_finishes_under_one_second() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 50_000_000.0;
        cfg.crystal_storage.base = 50_000_000.0;
        cfg.deuterium_storage.base = 50_000_000.0;
        cfg.starting_buildings.metal_mine = 18;
        cfg.starting_buildings.crystal_mine = 16;
        cfg.starting_buildings.solar_plant = 24;
        cfg.starting_buildings.robotics_factory = 8;
        cfg.starting_buildings.shipyard = 6;
        cfg.starting_resources = Resources::from_parts(1_000_000.0, 1_000_000.0, 50_000.0);
        cfg.flight.base_fleet_slots = 16;
        let mut state = GameState::with_config(0, cfg);
        for pos in 2..=8u16 {
            let coord = Coord {
                galaxy: 1,
                system: 1,
                position: pos,
            };
            plant_colony(&mut state, coord);
            let colony = state.colonies.last_mut().unwrap();
            colony.buildings.metal_mine = 12;
            colony.buildings.crystal_mine = 10;
            colony.buildings.solar_plant = 16;
            colony.resources = Resources::from_parts(10_000.0, 10_000.0, 1_000.0);
        }
        state.fleet.add(ShipKind::SmallCargo, 12);
        for i in 0..4 {
            let dest = state.colonies[i].coord;
            state
                .schedule_route(
                    state.home,
                    dest,
                    cargo_fleet(),
                    Resources::from_parts(80.0, 0.0, 0.0),
                    3_600,
                    0,
                )
                .unwrap();
        }
        let started = std::time::Instant::now();
        state.catch_up(7 * 24 * 3600);
        let elapsed = started.elapsed();
        let budget = if cfg!(debug_assertions) {
            std::time::Duration::from_millis(4_000)
        } else {
            std::time::Duration::from_secs(1)
        };
        assert!(
            elapsed < budget,
            "7-day catch-up took {elapsed:?} (budget {budget:?})"
        );
        assert_eq!(state.last_tick_unix, 7 * 24 * 3600);
    }

    #[test]
    fn storage_full_pauses_at_fill_instant_and_notes_a_stall() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 150.0;
        cfg.starting_resources.iron = 100.0;
        let mut state = GameState::with_config(0, cfg);
        state.catch_up(1800);
        close(state.resources.iron, 150.0);
        assert!(
            state
                .logistics
                .stalls
                .iter()
                .any(|s| s.contains("full storage") && s.contains("iron")),
            "{:?}",
            state.logistics.stalls
        );
        state.catch_up(3600);
        close(state.resources.iron, 150.0);
    }

    #[test]
    fn missing_chain_input_pauses_then_resumes() {
        let mut cfg = test_config();
        cfg.starting_buildings.crystal_mine = 0;
        cfg.chains = crate::ChainConfig {
            goods: vec![crate::GoodSpec {
                id: "steel".into(),
                kind: crate::GoodKind::Refined,
            }],
            recipes: vec![crate::Recipe {
                id: "forge".into(),
                building: BuildingKind::MetalMine,
                deposit: None,
                inputs: BTreeMap::from([("steel".into(), 1.0)]),
                outputs: BTreeMap::from([("frames".into(), 1.0)]),
                energy: 0.0,
                heat: 0.0,
                pollution: 0.0,
            }],
            deposits: BTreeMap::new(),
            deposit_jitter: 0.0,
        };
        let mut state = GameState::with_config(0, cfg);
        state.goods.insert("steel".into(), 50.0);
        state.catch_up(1800);
        close(state.goods.get("steel").copied().unwrap_or(0.0), 0.0);
        close(state.resources.iron, 50.0);
        assert!(
            state
                .logistics
                .stalls
                .iter()
                .any(|s| s.contains("missing steel")),
            "{:?}",
            state.logistics.stalls
        );
        state.goods.insert("steel".into(), 50.0);
        state.catch_up(3600);
        close(state.goods.get("steel").copied().unwrap_or(0.0), 0.0);
        close(state.resources.iron, 100.0);
    }

    #[test]
    fn catch_up_shared_chain_inputs_match_step_by_step() {
        let mut cfg = test_config();
        cfg.starting_buildings.crystal_mine = 1;
        cfg.starting_buildings.metal_mine = 1;
        cfg.crystal_mine.base_per_hour = 100.0;
        cfg.chains = crate::ChainConfig {
            goods: vec![crate::GoodSpec {
                id: "steel".into(),
                kind: crate::GoodKind::Refined,
            }],
            recipes: vec![
                crate::Recipe {
                    id: "forge".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("steel".into(), 1.0)]),
                    outputs: BTreeMap::from([("frames".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
                crate::Recipe {
                    id: "circuit".into(),
                    building: BuildingKind::CrystalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("steel".into(), 1.0)]),
                    outputs: BTreeMap::from([("electronics".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
            ],
            deposits: BTreeMap::new(),
            deposit_jitter: 0.0,
        };
        let mut one_shot = GameState::with_config(0, cfg.clone());
        one_shot.goods.insert("steel".into(), 100.0);
        one_shot.catch_up(3600);

        let mut stepped = GameState::with_config(0, cfg);
        stepped.goods.insert("steel".into(), 100.0);
        for t in 1..=3600 {
            stepped.catch_up(t);
        }

        close(one_shot.goods.get("steel").copied().unwrap_or(0.0), 0.0);
        close(stepped.goods.get("steel").copied().unwrap_or(0.0), 0.0);
        close(one_shot.resources.iron, stepped.resources.iron);
        close(one_shot.resources.silicon, stepped.resources.silicon);
        close(one_shot.resources.iron, 50.0);
        close(one_shot.resources.silicon, 50.0);
        assert!(
            one_shot
                .logistics
                .stalls
                .iter()
                .any(|s| s.contains("missing steel")),
            "{:?}",
            one_shot.logistics.stalls
        );
    }

    #[test]
    fn energy_deficit_pauses_then_resumes_when_solar_completes() {
        let mut cfg = queue_config();
        cfg.starting_buildings.solar_plant = 0;
        cfg.starting_buildings.metal_mine = 1;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_resources.iron = 1_000.0;
        let mut state = GameState::with_config(0, cfg);
        assert!(state.energy_factor() < 1e-12);
        state.enqueue_build(BuildingKind::SolarPlant, 0).unwrap();
        state.catch_up(1_800);
        close(state.resources.iron, 1_000.0);
        assert!(
            state
                .logistics
                .stalls
                .iter()
                .any(|s| s.contains("energy deficit")),
            "{:?}",
            state.logistics.stalls
        );
        state.catch_up(7_200);
        assert_eq!(state.buildings.solar_plant, 1);
        assert!(state.energy_factor() > 0.99);
        // Wear from the first telemetry period slightly reduces the resumed hour.
        assert!(
            (state.resources.iron - 1_100.0).abs() < 0.05,
            "iron {}",
            state.resources.iron
        );
        assert!(
            state
                .logistics
                .completed
                .iter()
                .any(|s| s.contains("solar plant")),
            "{:?}",
            state.logistics.completed
        );
    }

    fn splitmix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn cheap_spec(seconds: f64) -> crate::BuildingSpec {
        crate::BuildingSpec {
            metal_base: 0.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: seconds,
            time_growth: 1.0,
            prerequisites: vec![],
            tech_prerequisites: vec![],
        }
    }

    fn queue_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.metal_storage.base = 100_000.0;
        cfg.crystal_storage.base = 100_000.0;
        cfg.deuterium_storage.base = 100_000.0;
        cfg.starting_resources = Resources::from_parts(1_000.0, 1_000.0, 1_000.0);
        cfg.buildings.metal_mine = cheap_spec(1_800.0);
        cfg.buildings.solar_plant = cheap_spec(3_600.0);
        cfg.buildings.robotics_factory = cheap_spec(100.0);
        cfg.buildings.shipyard = crate::BuildingSpec {
            metal_base: 10.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: 100.0,
            time_growth: 1.0,
            prerequisites: vec![(BuildingKind::RoboticsFactory, 1)],
            tech_prerequisites: vec![],
        };
        cfg.buildings.metal_storage = crate::BuildingSpec {
            metal_base: 50.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: 10.0,
            time_growth: 1.0,
            prerequisites: vec![],
            tech_prerequisites: vec![],
        };
        cfg.max_build_queue = 5;
        cfg.robotics_speed_per_level = 1.0;
        cfg
    }

    #[test]
    fn mine_finishing_mid_gap_changes_production_from_completion_time() {
        let cfg = queue_config();
        let mut one_shot = GameState::with_config(0, cfg.clone());
        one_shot.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        assert_eq!(one_shot.buildings.metal_mine, 1);
        assert_eq!(one_shot.build_queue[0].completes_at, Some(1_800));

        one_shot.catch_up(3_600);
        assert_eq!(one_shot.buildings.metal_mine, 2);
        assert!(one_shot.build_queue.is_empty());
        // 0.5 h at 100/h then 0.5 h at 200/h, paid 0.
        close(one_shot.resources.iron, 1_000.0 + 50.0 + 100.0);

        let mut stepped = GameState::with_config(0, cfg);
        stepped.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        for t in 1..=3_600 {
            stepped.catch_up(t);
        }
        close(one_shot.resources.iron, stepped.resources.iron);
        assert_eq!(stepped.buildings.metal_mine, 2);
    }

    #[test]
    fn queue_caps_at_five_and_only_head_has_a_timer() {
        let mut state = GameState::with_config(0, queue_config());
        for _ in 0..5 {
            state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        }
        let err = state
            .enqueue_build(BuildingKind::MetalStorage, 0)
            .unwrap_err();
        assert_eq!(err, BuildError::QueueFull { cap: 5 });
        assert!(err.to_string().contains("full"));
        assert_eq!(state.build_queue.len(), 5);
        assert!(state.build_queue[0].completes_at.is_some());
        for job in &state.build_queue[1..] {
            assert!(job.completes_at.is_none());
            assert_eq!(job.building, BuildingKind::MetalStorage);
        }
    }

    #[test]
    fn insufficient_resources_report_missing_amounts() {
        let mut cfg = queue_config();
        cfg.starting_resources = Resources::ZERO;
        cfg.buildings.metal_storage.metal_base = 40.0;
        let mut state = GameState::with_config(0, cfg);
        let err = state
            .enqueue_build(BuildingKind::MetalStorage, 0)
            .unwrap_err();
        match &err {
            BuildError::InsufficientResources { missing, cost } => {
                close(missing.iron, 40.0);
                close(cost.iron, 40.0);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(err.to_string().contains("missing metal 40"));
        assert!(state.build_queue.is_empty());
    }

    #[test]
    fn missing_prerequisite_is_rejected_with_building_and_level() {
        let mut state = GameState::with_config(0, queue_config());
        let err = state.enqueue_build(BuildingKind::Shipyard, 0).unwrap_err();
        assert_eq!(
            err,
            BuildError::MissingPrerequisite {
                building: BuildingKind::RoboticsFactory,
                required: 1,
                actual: 0,
            }
        );
        assert!(err.to_string().contains("robotics factory level 1"));
    }

    #[test]
    fn robotics_level_shortens_the_next_job_when_it_becomes_head() {
        let mut cfg = queue_config();
        cfg.buildings.metal_mine = cheap_spec(100.0);
        let mut state = GameState::with_config(0, cfg);
        state
            .enqueue_build(BuildingKind::RoboticsFactory, 0)
            .unwrap();
        state.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        assert_eq!(state.build_queue[0].completes_at, Some(100));
        assert!(state.build_queue[1].completes_at.is_none());

        state.catch_up(100);
        assert_eq!(state.buildings.robotics_factory, 1);
        assert_eq!(state.build_queue.len(), 1);
        // 100 s / (1 + 1) = 50 s, starting at t=100.
        assert_eq!(state.build_queue[0].completes_at, Some(150));
        assert_eq!(state.build_queue[0].building, BuildingKind::MetalMine);
    }

    #[test]
    fn cancel_refunds_paid_resources_in_full() {
        let mut cfg = queue_config();
        cfg.buildings.metal_storage.metal_base = 80.0;
        cfg.starting_resources.iron = 100.0;
        let mut state = GameState::with_config(0, cfg);
        state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        close(state.resources.iron, 20.0);
        let refund = state.cancel_build(0, 0).unwrap();
        close(refund.iron, 80.0);
        close(state.resources.iron, 100.0);
        assert!(state.build_queue.is_empty());
        assert_eq!(state.buildings.metal_storage, 0);
    }

    #[test]
    fn canceling_an_earlier_job_reprojects_later_from_levels() {
        let mut cfg = queue_config();
        cfg.buildings.metal_storage.metal_base = 10.0;
        cfg.buildings.metal_storage.cost_growth = 2.0;
        let mut state = GameState::with_config(0, cfg);
        for _ in 0..3 {
            state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        }
        assert_eq!(
            state
                .build_queue
                .iter()
                .map(|j| j.from_level)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        state.cancel_build(0, 0).unwrap();
        assert_eq!(
            state
                .build_queue
                .iter()
                .map(|j| j.from_level)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        close(state.build_queue[0].paid.iron, 10.0);
        close(state.build_queue[1].paid.iron, 20.0);
        assert_eq!(state.build_queue[0].completes_at, Some(10));
    }

    #[test]
    fn canceling_a_queued_prerequisite_drops_the_dependent_job() {
        let mut cfg = queue_config();
        cfg.buildings.robotics_factory.metal_base = 5.0;
        cfg.buildings.shipyard.metal_base = 7.0;
        cfg.starting_resources.iron = 100.0;
        let mut state = GameState::with_config(0, cfg);
        state
            .enqueue_build(BuildingKind::RoboticsFactory, 0)
            .unwrap();
        state.enqueue_build(BuildingKind::Shipyard, 0).unwrap();
        assert_eq!(state.build_queue.len(), 2);
        close(state.resources.iron, 88.0);
        state.cancel_build(0, 0).unwrap();
        assert!(state.build_queue.is_empty());
        close(state.resources.iron, 100.0);
        assert_eq!(state.buildings.robotics_factory, 0);
        assert_eq!(state.buildings.shipyard, 0);
    }

    #[test]
    fn enqueue_after_advance_ignores_a_stale_timestamp() {
        let mut state = GameState::with_config(0, queue_config());
        state.catch_up(100);
        state.enqueue_build(BuildingKind::MetalMine, 50).unwrap();
        assert_eq!(state.last_tick_unix, 100);
        assert_eq!(state.build_queue[0].started_at, Some(100));
        assert_eq!(state.build_queue[0].completes_at, Some(1_900));
    }

    fn research_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.starting_resources = Resources::from_parts(10_000.0, 10_000.0, 10_000.0);
        cfg.starting_buildings.research_lab = 1;
        cfg.techs = TechTree::parse(
            r#"
[[tech]]
id = "energy"
name = "Energy"
metal = 0.0
crystal = 10.0
deuterium = 0.0
cost_growth = 2.0
base_seconds = 1800.0
time_growth = 1.0
prereq_buildings = [{ building = "research_lab", level = 1 }]
modifiers = []

[[tech]]
id = "weapons"
name = "Weapons"
metal = 10.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 60.0
time_growth = 1.0
prereq_buildings = [{ building = "research_lab", level = 1 }]
modifiers = [{ kind = "weapon_damage", per_level = 0.1 }]

[[tech]]
id = "laser"
name = "Laser"
metal = 10.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 60.0
time_growth = 1.0
prereq_techs = [{ id = "energy", level = 2 }]
modifiers = []
"#,
        )
        .unwrap();
        cfg
    }

    #[test]
    fn research_completes_during_offline_catch_up() {
        let mut one_shot = GameState::with_config(0, research_config());
        one_shot.start_research("energy", 0).unwrap();
        assert_eq!(one_shot.research.as_ref().unwrap().completes_at, 1_800);
        one_shot.catch_up(3_600);
        assert_eq!(one_shot.tech_level("energy"), 1);
        assert!(one_shot.research.is_none());

        let mut stepped = GameState::with_config(0, research_config());
        stepped.start_research("energy", 0).unwrap();
        for t in 1..=3_600 {
            stepped.catch_up(t);
        }
        assert_eq!(stepped.tech_level("energy"), 1);
        close(one_shot.resources.silicon, stepped.resources.silicon);
    }

    #[test]
    fn modifier_lookup_scales_with_tech_level() {
        let mut state = GameState::with_config(0, research_config());
        close(state.modifier(ModifierKind::WeaponDamage), 0.0);
        state.start_research("weapons", 0).unwrap();
        state.catch_up(60);
        close(state.modifier(ModifierKind::WeaponDamage), 0.1);
        state.start_research("weapons", 60).unwrap();
        state.catch_up(120);
        close(state.modifier(ModifierKind::WeaponDamage), 0.2);
    }

    #[test]
    fn unavailable_tech_lists_missing_prerequisites() {
        let state = GameState::with_config(0, research_config());
        let gaps = state.missing_research_prereqs("laser").unwrap();
        assert_eq!(
            gaps,
            vec![PrereqGap::Tech {
                id: "energy".into(),
                required: 2,
                actual: 0,
            }]
        );
        let err = GameState::with_config(0, research_config())
            .start_research("laser", 0)
            .unwrap_err();
        match err {
            ResearchError::MissingPrerequisites { gaps } => {
                assert!(gaps[0].to_string().contains("energy level 2"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn second_research_is_rejected_while_busy() {
        let mut state = GameState::with_config(0, research_config());
        state.start_research("energy", 0).unwrap();
        let err = state.start_research("weapons", 0).unwrap_err();
        assert_eq!(
            err,
            ResearchError::Busy {
                current: "energy".into(),
            }
        );
    }

    #[test]
    fn exclusive_choice_excludes_the_other_and_survives_cancel() {
        let mut state = GameState::new(0);
        state.buildings.research_lab = 1;
        state.tech_levels.insert("energy".into(), 1);
        state.resources.iron = 10_000.0;
        state.resources.silicon = 10_000.0;
        state.resources.deuterium = 10_000.0;
        assert_eq!(
            state.tech_node_state("deep_core"),
            crate::TechNodeState::Available
        );
        state.start_research("deep_core", 0).unwrap();
        assert_eq!(
            state
                .exclusive_picks
                .get("mining_method")
                .map(String::as_str),
            Some("deep_core")
        );
        assert_eq!(
            state.tech_node_state("orbital_harvest"),
            crate::TechNodeState::Excluded
        );
        state.cancel_research(0).unwrap();
        assert_eq!(
            state
                .exclusive_picks
                .get("mining_method")
                .map(String::as_str),
            Some("deep_core")
        );
        let err = state.start_research("orbital_harvest", 0).unwrap_err();
        match err {
            ResearchError::Excluded { id, group } => {
                assert_eq!(id, "orbital_harvest");
                assert_eq!(group, "mining_method");
            }
            other => panic!("unexpected {other:?}"),
        }
        state.start_research("deep_core", 0).unwrap();
        state.catch_up(state.research.as_ref().unwrap().completes_at);
        assert!(state.tech_level("deep_core") >= 1);
        assert!(!state.keystone_complete(crate::TechBranch::Industry));
    }

    #[test]
    fn ancient_tech_stays_hidden_until_prestige_and_starting_techs_survive_reset() {
        let mut state = GameState::new(0);
        state.buildings.research_lab = 1;
        assert_eq!(
            state.tech_node_state("ancient_drive"),
            crate::TechNodeState::Hidden
        );
        let err = state.start_research("ancient_drive", 0).unwrap_err();
        match err {
            ResearchError::Hidden { id } => assert_eq!(id, "ancient_drive"),
            other => panic!("unexpected {other:?}"),
        }
        state.prestige.runs.push(crate::RunStats::new(1, 0));
        state.prestige.currency = 10;
        state.apply_prestige_start();
        assert_eq!(state.tech_level("energy"), 1);
        assert_eq!(
            state.tech_node_state("ancient_drive"),
            crate::TechNodeState::Available
        );
    }

    fn yard_state(now: u64) -> GameState {
        let mut state = GameState::new(now);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        state.resources.deuterium = 100_000.0;
        state
    }

    #[test]
    fn ship_stats_scale_with_research_modifiers() {
        let mut state = GameState::new(0);
        let base = state.ship_stats(ShipKind::LightFighter);
        let spec = state.config.ships.get(ShipKind::LightFighter);
        close(base.attack, spec.attack);
        close(base.speed, spec.speed);
        state.tech_levels.insert("weapons".into(), 2);
        state.tech_levels.insert("shields".into(), 1);
        state.tech_levels.insert("armour".into(), 3);
        state.tech_levels.insert("combustion_drive".into(), 4);
        let scaled = state.ship_stats(ShipKind::LightFighter);
        close(scaled.attack, spec.attack * 1.2);
        close(scaled.shield, spec.shield * 1.1);
        close(scaled.hull, spec.hull * 1.3);
        close(scaled.speed, spec.speed * 1.4);
        close(scaled.cargo, spec.cargo);
        close(scaled.fuel, spec.fuel);
    }

    #[test]
    fn locked_ship_lists_missing_techs() {
        let mut state = GameState::new(0);
        state.buildings.shipyard = 5;
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        state.resources.deuterium = 100_000.0;
        let gaps = state.missing_ship_prereqs(ShipKind::Cruiser);
        assert!(
            gaps.iter().any(|g| matches!(
                g,
                PrereqGap::Tech {
                    id,
                    required: 4,
                    ..
                } if id == "impulse_drive"
            )),
            "{gaps:?}"
        );
        let err = state.enqueue_ships(ShipKind::Cruiser, 1, 0).unwrap_err();
        match err {
            ShipError::MissingPrerequisites { gaps } => {
                assert!(!gaps.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn ship_batch_completes_one_ship_at_a_time_across_an_offline_gap() {
        let mut gap = yard_state(0);
        gap.enqueue_ships(ShipKind::LightFighter, 5, 0).unwrap();
        let unit = gap.ship_time_secs(ShipKind::LightFighter);
        gap.catch_up(unit * 2);
        assert_eq!(gap.fleet.get(ShipKind::LightFighter), 2);
        assert_eq!(gap.ship_queue[0].remaining, 3);

        let mut step = yard_state(0);
        step.enqueue_ships(ShipKind::LightFighter, 5, 0).unwrap();
        for t in 1..=unit * 2 {
            step.catch_up(t);
        }
        assert_eq!(step.fleet, gap.fleet);
        assert_eq!(step.ship_queue, gap.ship_queue);
        for kind in ResourceKind::ALL {
            close(step.resources.get(kind), gap.resources.get(kind));
        }
        assert_eq!(step.last_tick_unix, gap.last_tick_unix);
    }

    #[test]
    fn unaffordable_ship_batch_reports_missing_amounts() {
        let mut state = GameState::new(0);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources = Resources::ZERO;
        let err = state
            .enqueue_ships(ShipKind::LightFighter, 2, 0)
            .unwrap_err();
        match err {
            ShipError::InsufficientResources { missing, cost } => {
                assert!(missing.iron > 0.0);
                close(
                    cost.iron,
                    state.config.ships.get(ShipKind::LightFighter).iron * 2.0,
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn shipyard_is_required_to_queue_ships() {
        let mut state = GameState::new(0);
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        let err = state.enqueue_ships(ShipKind::SmallCargo, 1, 0).unwrap_err();
        assert_eq!(
            err,
            ShipError::NeedShipyard {
                required: 2,
                actual: 0,
            }
        );
    }

    fn scout_fleet() -> Fleet {
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 2);
        ships
    }

    fn far_dest() -> Coord {
        Coord {
            galaxy: 1,
            system: 200,
            position: 1,
        }
    }

    fn flying_state(now: u64) -> GameState {
        let mut state = GameState::new(now);
        state.fleet.add(ShipKind::LightFighter, 4);
        state.resources.deuterium = 10_000.0;
        state
    }

    #[test]
    fn recall_mid_outbound_returns_in_elapsed_time() {
        let mut state = flying_state(0);
        state
            .launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        let id = state.missions[0].id;
        let outbound = state.missions[0].outbound_secs;
        assert!(outbound > 1);
        state.recall_fleet(id, 1).unwrap();
        assert_eq!(state.missions[0].phase, MissionPhase::Return);
        assert_eq!(state.missions[0].phase_ends_at, 2);
        state.catch_up(2);
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::LightFighter), 4);
    }

    #[test]
    fn arrival_and_return_resolve_across_an_offline_gap() {
        let mut gap = flying_state(0);
        gap.launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        let outbound = gap.missions[0].outbound_secs;
        gap.catch_up(outbound * 2);
        assert!(gap.missions.is_empty());
        assert_eq!(gap.fleet.get(ShipKind::LightFighter), 4);

        let mut step = flying_state(0);
        step.launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        for t in 1..=outbound * 2 {
            step.catch_up(t);
        }
        assert_eq!(step.fleet, gap.fleet);
        assert_eq!(step.missions, gap.missions);
        assert_eq!(step.resources.deuterium, gap.resources.deuterium);
        assert_eq!(step.last_tick_unix, gap.last_tick_unix);
    }

    #[test]
    fn fleet_slots_follow_computer_technology() {
        let mut state = flying_state(0);
        assert_eq!(state.fleet_slots(), 1);
        state
            .launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        let err = state
            .launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap_err();
        assert_eq!(err, FleetError::NoSlots { used: 1, cap: 1 });
        state.tech_levels.insert("computer".into(), 1);
        assert_eq!(state.fleet_slots(), 2);
        state
            .launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        assert_eq!(state.missions.len(), 2);
    }

    #[test]
    fn disabled_raids_do_not_resolve_a_stale_schedule() {
        let mut state = GameState::with_config(0, test_config());
        state.next_raid_at = Some(10);
        state.resources.iron = 500.0;
        state.catch_up(100);
        assert!(state.events.is_empty());
        close(state.resources.iron, 500.0 + 100.0 * 100.0 / 3600.0);
        assert_eq!(state.next_raid_at, Some(10));
        assert_eq!(state.last_tick_unix, 100);
    }

    #[test]
    fn starter_survives_the_first_raid() {
        let mut state = GameState::new(0);
        assert_eq!(state.next_raid_at, Some(3600));
        assert_eq!(state.raid_countdown_secs(), Some(3600));
        state.catch_up(3599);
        assert!(state.events.is_empty());
        assert_eq!(state.fleet.total(), 0);
        state.catch_up(3600);
        assert!(state.events.iter().any(|e| e.text.contains("raid:")));
        assert!(
            state.events.iter().any(|e| e.text.contains("plunder")),
            "{:?}",
            state.events
        );
        assert!(state.resources.iron > 0.0);
        assert!(state.resources.silicon > 0.0);
        assert_eq!(state.next_raid_at, Some(3600 + 8 * 3600));
    }

    #[test]
    fn trigger_raid_uses_the_same_resolver_as_the_scheduler() {
        let mut bridged = GameState::new(0);
        let report = bridged.trigger_raid(0);
        assert_eq!(report.winner, CombatWinner::Attacker);
        assert!(bridged.events.iter().any(|e| e.text.contains("lost")));
        assert!(
            bridged
                .events
                .iter()
                .any(|e| e.text.contains("pirates lost")),
            "{:?}",
            bridged.events
        );
        assert_eq!(bridged.next_raid_at, Some(8 * 3600));
        let mut again = GameState::new(0);
        again.trigger_raid(0);
        assert_eq!(again.fleet, bridged.fleet);
        assert_eq!(again.events, bridged.events);
        assert_eq!(again.rng_seed, bridged.rng_seed);
    }

    #[test]
    fn raid_log_replays_to_the_same_result() {
        let mut state = GameState::new(0);
        state.fleet.add(ShipKind::LightFighter, 8);
        let report = state.trigger_raid(0);
        let log = state.combat_logs.last().expect("logged");
        assert_eq!(log.report.winner, report.winner);
        let last = log.report.frames.last().expect("frames");
        assert_eq!(last.attacker, report.attacker_after);
        assert_eq!(last.defender, report.defender_after);
        let atk = stats_from_catalog(&state.config.ships);
        let def = state.player_combat_stats();
        let replayed = CombatReport::replay(
            log.report.attacker_before,
            log.report.defender_before,
            &atk,
            &def,
            state.config.raid.max_rounds,
            &rapid_fire_table(&state.config.ships),
            log.report.seed,
        );
        assert_eq!(replayed.winner, report.winner);
        assert_eq!(replayed.attacker_after, report.attacker_after);
        assert_eq!(replayed.defender_after, report.defender_after);
        assert!(!report.layered);
    }

    #[test]
    fn layered_raid_replays_from_stored_frames() {
        let mut state = GameState::new(0);
        state.defences.add(DefenceKind::LightCannon, 6);
        state.defences.add(DefenceKind::OrbitalStation, 1);
        state.defences.add(DefenceKind::ShieldDome, 1);
        state.defences.add(DefenceKind::Garrison, 4);
        state.fleet.add(ShipKind::LightFighter, 2);
        let report = state.trigger_raid(0);
        assert!(report.layered);
        let last = report.frames.last().expect("frames");
        assert_eq!(last.attacker, report.attacker_after);
        assert_eq!(last.defender, report.defender_after);
        assert_eq!(
            report.frames.first().map(|f| f.attacker),
            Some(report.attacker_before)
        );
    }

    #[test]
    fn trigger_raid_at_the_scheduled_instant_resolves_once() {
        let mut state = GameState::new(0);
        state.trigger_raid(3600);
        assert_eq!(state.events[0].at, 3600);
        assert!(state.events.iter().any(|e| e.text.contains("raid:")));
        assert_eq!(state.next_raid_at, Some(3600 + 8 * 3600));
    }

    #[test]
    fn defences_complete_through_the_shipyard_queue() {
        let mut cfg = test_config();
        cfg.starting_buildings.shipyard = 1;
        cfg.starting_resources.iron = 10_000.0;
        let mut state = GameState::with_config(0, cfg);
        state
            .enqueue_defence(DefenceKind::LightCannon, 2, 0)
            .unwrap();
        assert_eq!(state.ship_queue[0].defence, Some(DefenceKind::LightCannon));
        let first = state.ship_queue[0].completes_at.unwrap();
        state.catch_up(first);
        assert_eq!(state.defences.get(DefenceKind::LightCannon), 1);
        let second = state.ship_queue[0].completes_at.unwrap();
        state.catch_up(second);
        assert_eq!(state.defences.get(DefenceKind::LightCannon), 2);
        assert!(state.ship_queue.is_empty());
    }

    #[test]
    fn raid_with_defences_is_deterministic_and_reports_layers() {
        let mut a = GameState::new(0);
        a.defences.add(DefenceKind::LightCannon, 6);
        a.defences.add(DefenceKind::OrbitalStation, 1);
        a.defences.add(DefenceKind::ShieldDome, 1);
        a.defences.add(DefenceKind::Garrison, 4);
        a.fleet.add(ShipKind::LightFighter, 2);
        let mut b = a.clone();
        let r1 = a.trigger_raid(0);
        let r2 = b.trigger_raid(0);
        assert_eq!(r1, r2);
        assert_eq!(a.defences, b.defences);
        assert_eq!(a.fleet, b.fleet);
        let text = a
            .events
            .iter()
            .find(|e| e.text.contains("raid:"))
            .expect("raid report")
            .text
            .as_str();
        assert!(text.contains("orbital stations"), "{text}");
        assert!(text.contains("dome"), "{text}");
        assert!(text.contains("batteries"), "{text}");
        assert!(text.contains("rebuilt"), "{text}");
        assert!(text.contains("winner"), "{text}");
        assert!(text.contains("lost"), "{text}");
        assert!(text.contains("pirates lost"), "{text}");
    }

    #[test]
    fn layered_raid_does_not_plunder_while_the_fleet_still_stands() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.strength_factor = 0.0;
        cfg.raid.min_ships = 1;
        let mut state = GameState::with_config(0, cfg);
        state.resources.iron = 1_000.0;
        state.fleet.add(ShipKind::Battleship, 8);
        state.defences.add(DefenceKind::LightCannon, 1);
        let before = state.resources.iron;
        let report = state.trigger_raid(0);
        assert_ne!(report.winner, CombatWinner::Attacker);
        assert!(!report.defender_after.is_empty());
        assert_eq!(state.resources.iron, before);
        let text = state
            .events
            .iter()
            .find(|e| e.text.contains("raid:"))
            .expect("raid report")
            .text
            .as_str();
        assert!(text.contains("plunder Fe0"), "{text}");
    }

    #[test]
    fn resource_brief_uses_material_abbreviations() {
        assert_eq!(
            Resources::from_parts(40.0, 30.0, 5.0).brief(),
            "Fe40 Si30 D5"
        );
        let extras = Resources {
            iron: 10.0,
            silicon: 0.0,
            rare_earths: 3.0,
            water_ice: 4.0,
            helium_3: 0.0,
            deuterium: 1.0,
        };
        assert_eq!(extras.brief(), "Fe10 Si0 REE3 H2O4 D1");
    }

    #[test]
    fn raid_strength_grows_with_planet_defences() {
        let mut bare = GameState::new(0);
        bare.fleet.add(ShipKind::LightFighter, 10);
        let n_bare = bare.trigger_raid(0).attacker_before.total();
        let mut armed = GameState::new(0);
        armed.fleet.add(ShipKind::LightFighter, 10);
        armed.defences.add(DefenceKind::LightCannon, 20);
        let n_armed = armed.trigger_raid(0).attacker_before.total();
        assert!(n_armed > n_bare, "armed={n_armed} bare={n_bare}");
    }

    #[test]
    fn raid_during_offline_gap_matches_step_by_step() {
        let cfg = BalanceConfig {
            planet_types: crate::PlanetTypeCatalog {
                rocky: crate::PlanetTypeBonus::ZERO,
                ice: crate::PlanetTypeBonus::ZERO,
                gas_giant: crate::PlanetTypeBonus::ZERO,
            },
            ..BalanceConfig::default()
        };
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.catch_up(7200);
        let mut step = GameState::with_config(0, cfg);
        for t in 1..=7200 {
            step.catch_up(t);
        }
        close(gap.resources.iron, step.resources.iron);
        close(gap.resources.silicon, step.resources.silicon);
        assert_eq!(gap.fleet, step.fleet);
        assert_eq!(gap.events, step.events);
        assert_eq!(gap.rng_seed, step.rng_seed);
        assert_eq!(gap.next_raid_at, step.next_raid_at);
        assert_eq!(gap.last_tick_unix, 7200);
    }

    #[test]
    fn no_raid_lands_before_configured_first_delay() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.first_delay_secs = 7200;
        cfg.raid.min_warning_secs = 1800;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(100, cfg);
        assert_eq!(state.next_raid_at, Some(7300));
        assert_eq!(state.raid_countdown_secs(), None);
        state.catch_up(5500);
        assert!(state.events.is_empty());
        assert_eq!(state.raid_countdown_secs(), Some(1800));
        state.catch_up(7300);
        assert!(state.events.iter().any(|e| e.text.contains("raid:")));
        assert_eq!(state.events[0].at, 7300);
    }

    #[test]
    fn plunder_cap_is_applied_on_a_lost_raid() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.first_delay_secs = 10;
        cfg.raid.min_warning_secs = 10;
        cfg.raid.plunder_share = 1.0;
        cfg.raid.plunder_cap = Resources::from_parts(40.0, 30.0, 5.0);
        cfg.starting_resources = Resources::from_parts(500.0, 500.0, 100.0);
        cfg.starting_buildings.metal_mine = 0;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_buildings.solar_plant = 0;
        let mut state = GameState::with_config(0, cfg);
        state.catch_up(10);
        close(state.resources.iron, 460.0);
        close(state.resources.silicon, 470.0);
        close(state.resources.deuterium, 95.0);
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text.contains("plunder Fe40 Si30 D5")),
            "{:?}",
            state.events
        );
    }

    fn colony_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.techs = TechTree::parse(
            r#"
[[tech]]
id = "colonization"
name = "Colonization"
metal = 1000.0
crystal = 4000.0
deuterium = 2000.0
cost_growth = 2.0
base_seconds = 600.0
time_growth = 2.0
prereq_buildings = []
prereq_techs = []
modifiers = [{ kind = "colony_slots", per_level = 1.0 }]
"#,
        )
        .expect("test colonization tech");
        cfg
    }

    fn colony_ready(now: u64) -> GameState {
        let mut state = GameState::with_config(now, colony_config());
        state.resources.deuterium = 10_000.0;
        state.fleet.add(ShipKind::ColonyShip, 1);
        state
    }

    fn empty_slot() -> Coord {
        Coord {
            galaxy: 1,
            system: 1,
            position: 14,
        }
    }

    #[test]
    fn colony_ship_and_colonization_tech_are_in_config() {
        let mut state = GameState::new(0);
        let spec = state.config.ships.get(ShipKind::ColonyShip);
        assert_eq!(spec.kind, ShipKind::ColonyShip);
        assert!(spec.shipyard >= 1);
        let tech = state.config.techs.get("colonization").unwrap();
        assert!(
            tech.modifiers
                .iter()
                .any(|m| m.kind == ModifierKind::ColonySlots && m.per_level == 1.0)
        );
        assert_eq!(state.colony_cap(), 1);
        state.tech_levels.insert("colonization".into(), 2);
        assert_eq!(state.colony_cap(), 3);
        let mut empty = GameState::with_config(0, colony_config());
        empty.resources.deuterium = 10_000.0;
        assert_eq!(
            empty.launch_colony(empty_slot(), 100, 0),
            Err(FleetError::NeedColonyShip)
        );
    }

    #[test]
    fn over_cap_or_occupied_returns_the_colony_ship() {
        let mut state = colony_ready(0);
        assert_eq!(state.colony_cap(), 1);
        state.launch_colony(empty_slot(), 100, 0).unwrap();
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(outbound * 2);
        assert!(state.colonies.is_empty());
        assert_eq!(state.fleet.get(ShipKind::ColonyShip), 1);
        assert!(state.missions.is_empty());

        state.tech_levels.insert("colonization".into(), 1);
        let now = state.last_tick_unix;
        state.launch_colony(empty_slot(), 100, now).unwrap();
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(now + outbound * 2);
        assert_eq!(state.colonies.len(), 1);
        assert_eq!(state.fleet.get(ShipKind::ColonyShip), 0);

        state.fleet.add(ShipKind::ColonyShip, 1);
        state.launch_colony(empty_slot(), 100, 0).unwrap();
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(state.last_tick_unix + outbound * 2);
        assert_eq!(state.colonies.len(), 1);
        assert_eq!(state.fleet.get(ShipKind::ColonyShip), 1);
    }

    #[test]
    fn same_shareable_seed_replays_the_galaxy() {
        let cfg = test_config();
        let seed = 0xA11A_5EED_00C0_FFEE;
        let a = GameState::new_live_galaxy(10, cfg.clone(), seed);
        let b = GameState::new_live_galaxy(10, cfg, seed);
        assert_eq!(a.home_kind, b.home_kind);
        assert_eq!(a.home_size, b.home_size);
        assert_eq!(a.outposts, b.outposts);
        assert_eq!(a.generator_version, crate::GALAXY_GENERATOR_VERSION);
        assert_eq!(
            crate::parse_galaxy_seed(&crate::encode_galaxy_seed(seed)).unwrap(),
            Some(seed)
        );
    }

    #[test]
    fn planet_type_and_size_follow_the_galaxy_seed() {
        let mut a = colony_ready(0);
        a.galaxy_seed = 11;
        a.tech_levels.insert("colonization".into(), 1);
        a.launch_colony(empty_slot(), 100, 0).unwrap();
        let outbound = a.missions[0].outbound_secs;
        a.catch_up(outbound * 2);
        let expected = planet_profile(11, empty_slot(), a.generator_version);
        assert_eq!(a.colonies[0].kind, expected.0);
        assert_eq!(a.colonies[0].size, expected.1);
        let mut b = colony_ready(0);
        b.galaxy_seed = 11;
        b.tech_levels.insert("colonization".into(), 1);
        b.launch_colony(empty_slot(), 100, 0).unwrap();
        b.catch_up(outbound * 2);
        assert_eq!(a.colonies[0].kind, b.colonies[0].kind);
        assert_eq!(a.colonies[0].size, b.colonies[0].size);
    }

    #[test]
    fn type_bonuses_go_through_modifier_lookup() {
        let mut cfg = test_config();
        cfg.planet_types.gas_giant.deuterium = 0.5;
        cfg.planet_types.gas_giant.energy = 0.0;
        let state = GameState::with_config(0, cfg);
        close(
            state.modifier_for(PlanetType::GasGiant, ModifierKind::DeuteriumOutput),
            0.5,
        );
        close(
            state.modifier_for(PlanetType::Rocky, ModifierKind::DeuteriumOutput),
            0.0,
        );
    }

    #[test]
    fn offline_arrival_founds_the_colony_at_the_landing_time() {
        let mut gap = colony_ready(0);
        gap.tech_levels.insert("colonization".into(), 1);
        gap.launch_colony(empty_slot(), 100, 0).unwrap();
        let outbound = gap.missions[0].outbound_secs;
        gap.catch_up(outbound * 10);
        assert_eq!(gap.colonies.len(), 1);
        assert_eq!(gap.colonies[0].coord, empty_slot());

        let mut step = colony_ready(0);
        step.tech_levels.insert("colonization".into(), 1);
        step.launch_colony(empty_slot(), 100, 0).unwrap();
        for t in 1..=outbound * 10 {
            step.catch_up(t);
        }
        assert_eq!(step.colonies, gap.colonies);
        assert_eq!(step.fleet, gap.fleet);
        assert_eq!(step.last_tick_unix, gap.last_tick_unix);
    }

    fn activity_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.activity.enabled = true;
        cfg.activity.pr_merged.cooldown_secs = 60;
        cfg.activity.pr_merged.daily_cap = 2;
        cfg.activity.pr_merged.payout_hours = 1.0;
        cfg.activity.ci_green.cooldown_secs = 0;
        cfg.activity.ci_green.daily_cap = 10;
        cfg.activity.ci_green.boost_factor = 1.0;
        cfg.activity.ci_green.boost_secs = 1800;
        cfg.activity.ci_red.cooldown_secs = 0;
        cfg.activity.ci_red.daily_cap = 10;
        cfg.activity.agent_long.min_duration_secs = 100;
        cfg.activity.agent_long.boost_factor = 1.0;
        cfg.activity.agent_long.boost_secs = 3_600;
        cfg.activity.agent_long.cooldown_secs = 0;
        cfg.activity.agent_stuck.min_duration_secs = 50;
        cfg.raid = crate::RaidConfig::inactive();
        cfg
    }

    #[test]
    fn merged_pr_pays_an_hour_of_production_at_the_event_timestamp() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg.clone());
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 1800 }, 10_000),
            ActivityApply::Applied
        );
        close(state.resources.iron, 50.0 + 100.0);
        close(state.resources.silicon, 25.0 + 50.0);
        assert_eq!(state.last_tick_unix, 1800);
        assert!(state.events.iter().any(|e| e.text.contains("merged PR")));
    }

    #[test]
    fn historical_events_use_their_own_timestamps_for_cooldowns() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 100 }, 3600),
            ActivityApply::Applied
        );
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 160 }, 3600),
            ActivityApply::Applied
        );
        assert_eq!(state.last_tick_unix, 160);
    }

    #[test]
    fn events_before_last_tick_are_skipped() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        state.catch_up(3600);
        let metal = state.resources.iron;
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 100 }, 4000),
            ActivityApply::Skipped
        );
        close(state.resources.iron, metal);
        assert_eq!(state.last_tick_unix, 3600);
    }

    #[test]
    fn activity_caps_and_cooldowns_drop_a_burst() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        let mut applied = 0;
        for i in 0..10 {
            if state.apply_activity(ActivityPulse::PrMerged { ts: i }, 10_000)
                == ActivityApply::Applied
            {
                applied += 1;
            }
        }
        assert_eq!(applied, 1);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 60 }, 10_000),
            ActivityApply::Applied
        );
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 120 }, 10_000),
            ActivityApply::Skipped
        );
    }

    #[test]
    fn disabled_mapping_is_a_no_op() {
        let mut cfg = activity_config();
        cfg.activity.pr_merged.enabled = false;
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 10 }, 10_000),
            ActivityApply::Skipped
        );
        close(state.resources.iron, 0.0);
    }

    #[test]
    fn production_boost_splits_catch_up_at_expiry() {
        let cfg = activity_config();
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.apply_activity(
            ActivityPulse::WorkflowFinished {
                ts: 0,
                success: true,
            },
            10_000,
        );
        gap.catch_up(3600);
        let mut step = GameState::with_config(0, cfg);
        step.apply_activity(
            ActivityPulse::WorkflowFinished {
                ts: 0,
                success: true,
            },
            10_000,
        );
        for t in 1..=3600 {
            step.catch_up(t);
        }
        close(gap.resources.iron, step.resources.iron);
        close(gap.resources.silicon, step.resources.silicon);
        // 1800s at 2x then 1800s at 1x: metal 100/h -> 50+50 = 100?
        // 0.5h * 200 + 0.5h * 100 = 100+50 = 150
        close(gap.resources.iron, 150.0);
    }

    #[test]
    fn long_agent_run_shortens_research() {
        let mut cfg = research_config();
        cfg.activity = activity_config().activity;
        let mut state = GameState::with_config(0, cfg);
        state.start_research("energy", 0).unwrap();
        let before = state.research.as_ref().unwrap().completes_at;
        assert_eq!(
            state.apply_activity(
                ActivityPulse::AgentFinished {
                    ts: 0,
                    duration_secs: 100,
                },
                10_000,
            ),
            ActivityApply::Applied
        );
        let after = state.research.as_ref().unwrap().completes_at;
        assert!(after < before);
        assert_eq!(
            state.apply_activity(
                ActivityPulse::AgentFinished {
                    ts: 1,
                    duration_secs: 10,
                },
                10_000,
            ),
            ActivityApply::Skipped
        );
    }

    #[test]
    fn red_ci_does_not_consume_limits_when_raids_are_off() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(
                ActivityPulse::WorkflowFinished {
                    ts: 10,
                    success: false,
                },
                10_000,
            ),
            ActivityApply::Skipped
        );
        assert!(state.activity.last_applied[ActivityChannel::CiRed.index()].is_none());
    }

    #[test]
    fn red_ci_uses_the_raid_api() {
        let mut cfg = activity_config();
        cfg.raid = crate::RaidConfig::default_active();
        cfg.raid.first_delay_secs = 10_000;
        let mut state = GameState::with_config(0, cfg);
        let events_before = state.events.len();
        assert_eq!(
            state.apply_activity(
                ActivityPulse::WorkflowFinished {
                    ts: 10,
                    success: false,
                },
                10_000,
            ),
            ActivityApply::Applied
        );
        assert!(state.events.len() > events_before);
        assert!(state.events.iter().any(|e| e.text.starts_with("raid:")));
    }

    #[test]
    fn older_day_events_do_not_reset_today_caps() {
        let mut cfg = activity_config();
        cfg.activity.pr_merged.cooldown_secs = 0;
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 100 }, 86_400 * 2),
            ActivityApply::Applied
        );
        let today = 86_400;
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: today }, today + 10),
            ActivityApply::Applied
        );
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: today + 60 }, today + 70),
            ActivityApply::Applied
        );
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: today + 120 }, today + 130),
            ActivityApply::Skipped
        );
    }

    #[test]
    fn future_activity_timestamps_are_skipped() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 50 }, 10),
            ActivityApply::Skipped
        );
        assert_eq!(state.last_tick_unix, 0);
        close(state.resources.iron, 0.0);
    }

    fn expedition_state(now: u64) -> GameState {
        let mut cfg = test_config();
        cfg.expedition.min_hold_secs = 10;
        cfg.expedition.max_hold_secs = 10_000;
        cfg.expedition.delay_secs = 50;
        cfg.raid = crate::RaidConfig::inactive();
        cfg.metal_storage.base = 1_000_000.0;
        cfg.crystal_storage.base = 1_000_000.0;
        cfg.deuterium_storage.base = 1_000_000.0;
        let mut state = GameState::with_config(now, cfg);
        state.fleet.add(ShipKind::LightFighter, 10);
        state.resources.deuterium = 10_000.0;
        state
    }

    #[test]
    fn expedition_result_lands_in_the_event_log() {
        let mut state = expedition_state(0);
        state.launch_expedition(scout_fleet(), 10, 100, 0).unwrap();
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(outbound);
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text.starts_with("expedition:")),
            "{:?}",
            state.events
        );
    }

    #[test]
    fn reloading_the_save_does_not_change_a_pending_expedition() {
        let mut original = expedition_state(0);
        original
            .launch_expedition(scout_fleet(), 10, 100, 0)
            .unwrap();
        let mut conn = crate::open_save(std::path::Path::new(":memory:")).unwrap();
        crate::save_state(&mut conn, &original).unwrap();
        let mut loaded = crate::load_state(&conn, original.config.clone()).unwrap();
        assert_eq!(
            loaded.missions[0].expedition_seed,
            original.missions[0].expedition_seed
        );
        let outbound = original.missions[0].outbound_secs;
        let hold = original.missions[0].hold_secs;
        original.rng_seed = 0xDEAD_BEEF_DEAD_BEEF;
        original.catch_up(outbound + hold + outbound);
        loaded.catch_up(outbound + hold + outbound);
        assert_eq!(original.events, loaded.events);
        assert_eq!(original.fleet, loaded.fleet);
        close(original.resources.iron, loaded.resources.iron);
        close(original.resources.silicon, loaded.resources.silicon);
        close(original.resources.deuterium, loaded.resources.deuterium);
    }

    #[test]
    fn expedition_slots_follow_astrophysics() {
        let mut state = expedition_state(0);
        assert_eq!(state.expedition_slots(), 1);
        state.launch_expedition(scout_fleet(), 10, 100, 0).unwrap();
        let err = state
            .launch_expedition(scout_fleet(), 10, 100, 0)
            .unwrap_err();
        assert_eq!(err, FleetError::NoExpeditionSlots { used: 1, cap: 1 });
        state.tech_levels.insert("astrophysics".into(), 1);
        state.config.flight.base_fleet_slots = 2;
        state.config.techs = crate::TechTree::parse(
            r#"
[[tech]]
id = "astrophysics"
name = "Astrophysics"
metal = 1.0
crystal = 1.0
deuterium = 1.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_buildings = []
prereq_techs = []
modifiers = [{ kind = "expedition_slots", per_level = 1.0 }]
"#,
        )
        .unwrap();
        assert_eq!(state.expedition_slots(), 2);
        state.launch_expedition(scout_fleet(), 10, 100, 0).unwrap();
        assert_eq!(
            state
                .missions
                .iter()
                .filter(|m| m.expedition_seed.is_some())
                .count(),
            2
        );
    }

    fn neighbor() -> Coord {
        Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        }
    }

    fn plant_colony(state: &mut GameState, coord: Coord) {
        state.colonies.push(Colony {
            coord,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::from_parts(100_000.0, 100_000.0, 100_000.0),
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
    }

    fn cargo_fleet() -> Fleet {
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        ships
    }

    fn logistics_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.techs = TechTree::parse(
            r#"
[[tech]]
id = "logistics"
name = "Logistics"
metal = 1.0
crystal = 1.0
deuterium = 1.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_buildings = []
prereq_techs = []
modifiers = [{ kind = "logistics", per_level = 1.0 }]
"#,
        )
        .expect("test logistics tech");
        cfg.flight.base_fleet_slots = 4;
        cfg.metal_storage.base = 1_000_000.0;
        cfg.crystal_storage.base = 1_000_000.0;
        cfg.deuterium_storage.base = 1_000_000.0;
        cfg
    }

    #[test]
    fn colony_and_home_queues_advance_independently() {
        let mut state = GameState::with_config(0, test_config());
        state.resources = Resources::from_parts(100_000.0, 100_000.0, 100_000.0);
        plant_colony(&mut state, neighbor());
        state.colonies[0].buildings.robotics_factory = 1;
        state
            .enqueue_build_on(state.home, BuildingKind::MetalMine, 0)
            .unwrap();
        state
            .enqueue_build_on(neighbor(), BuildingKind::MetalMine, 0)
            .unwrap();
        let home_done = state.build_queue[0].completes_at.unwrap();
        let colony_done = state.colonies[0].build_queue[0].completes_at.unwrap();
        assert!(colony_done < home_done);
        state.catch_up(colony_done);
        assert_eq!(state.colonies[0].buildings.metal_mine, 1);
        assert!(state.colonies[0].build_queue.is_empty());
        assert_eq!(state.buildings.metal_mine, 1);
        assert_eq!(state.build_queue.len(), 1);
        state.catch_up(home_done);
        assert_eq!(state.buildings.metal_mine, 2);
        assert!(state.build_queue.is_empty());
    }

    #[test]
    fn transport_rejects_cargo_over_capacity_and_pays_fuel() {
        let mut state = GameState::with_config(0, logistics_config());
        state.resources.iron = 10_000.0;
        state.resources.silicon = 0.0;
        state.resources.deuterium = 50.0;
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 1);
        let over = Resources::from_parts(5_001.0, 0.0, 0.0);
        let err = state
            .launch_transport(state.home, neighbor(), cargo_fleet(), over, 100, 0)
            .unwrap_err();
        match err {
            FleetError::CargoExceedsCapacity { need, cap } => {
                close(need, 5_001.0);
                close(cap, state.cargo_capacity(cargo_fleet()));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
        close(state.resources.deuterium, 50.0);

        state.resources.iron = 50.0;
        let short_metal = Resources::from_parts(200.0, 0.0, 0.0);
        match state.launch_transport(state.home, neighbor(), cargo_fleet(), short_metal, 100, 0) {
            Err(FleetError::InsufficientCargo { missing }) => close(missing.iron, 150.0),
            other => panic!("{other:?}"),
        }
        state.resources.iron = 10_000.0;

        let cargo = Resources::from_parts(100.0, 0.0, 0.0);
        let id = state
            .launch_transport(state.home, neighbor(), cargo_fleet(), cargo, 100, 0)
            .unwrap();
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 0);
        close(state.resources.iron, 9_900.0);
        let fuel = state.missions[0].fuel_paid;
        assert!(fuel > 0.0);
        close(state.resources.deuterium, 50.0 - fuel);
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(outbound);
        close(state.colonies[0].resources.iron, 100_100.0);
        assert!(state.missions[0].transport);
        assert_eq!(state.missions[0].id, id);
        state.catch_up(outbound * 2);
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
    }

    #[test]
    fn recalling_an_outbound_transport_returns_its_cargo() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.iron = 10_000.0;
        state.resources.deuterium = 50.0;
        let cargo = Resources::from_parts(100.0, 0.0, 0.0);
        let id = state
            .launch_transport(state.home, neighbor(), cargo_fleet(), cargo, 100, 0)
            .unwrap();
        let fuel = state.missions[0].fuel_paid;
        let metal_rate = state.production_per_hour(ResourceKind::Iron);
        state.recall_fleet(id, 0).unwrap();
        state.catch_up(1);
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
        close(state.resources.iron, 10_000.0 + metal_rate / 3600.0);
        close(state.resources.deuterium, 50.0 - fuel);
        close(state.colonies[0].resources.iron, 100_000.0);
    }

    #[test]
    fn negative_or_non_finite_cargo_is_rejected() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 2);
        state.resources.iron = 10_000.0;
        state.resources.deuterium = 50.0;
        let bad = Resources::from_parts(-10.0, 0.0, 0.0);
        assert_eq!(
            state.launch_transport(state.home, neighbor(), cargo_fleet(), bad, 100, 0),
            Err(FleetError::InvalidCargo)
        );
        close(state.resources.iron, 10_000.0);
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 2);
        assert_eq!(
            state.schedule_route(state.home, neighbor(), cargo_fleet(), bad, 10, 0),
            Err(FleetError::InvalidCargo)
        );
        let over = Resources::from_parts(5_001.0, 0.0, 0.0);
        match state.schedule_route(state.home, neighbor(), cargo_fleet(), over, 10, 0) {
            Err(FleetError::CargoExceedsCapacity { .. }) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(
            state.schedule_route(
                state.home,
                neighbor(),
                Fleet::default(),
                Resources::ZERO,
                10,
                0
            ),
            Err(FleetError::EmptyFleet)
        );
        assert!(state.routes.is_empty());
    }

    #[test]
    fn repeating_route_launches_when_cargo_and_ships_are_ready() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.iron = 10_000.0;
        state.resources.deuterium = 50.0;
        let cargo = Resources::from_parts(100.0, 0.0, 0.0);
        let id = state
            .schedule_route(state.home, neighbor(), cargo_fleet(), cargo, 10, 0)
            .unwrap();
        let metal_rate = state.production_per_hour(ResourceKind::Iron);
        state.catch_up(1);
        assert_eq!(state.missions.len(), 1);
        assert!(state.missions[0].transport);
        close(state.resources.iron, 10_000.0 - 100.0 + metal_rate / 3600.0);
        close(state.colonies[0].resources.iron, 100_100.0);
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 0);
        assert_eq!(state.routes[0].id, id);
        assert_eq!(state.routes[0].next_at, 10);
    }

    #[test]
    fn repeating_route_skips_when_cargo_is_short_and_can_cancel() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.iron = 10.0;
        state.resources.deuterium = 50.0;
        let cargo = Resources::from_parts(100.0, 0.0, 0.0);
        let id = state
            .schedule_route(state.home, neighbor(), cargo_fleet(), cargo, 10, 0)
            .unwrap();
        state.catch_up(1);
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text.contains(&format!("route {id}: skipped"))),
            "{:?}",
            state.events
        );
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
        state.cancel_route(id).unwrap();
        let events = state.events.len();
        state.catch_up(100);
        assert_eq!(state.events.len(), events);
        assert!(state.routes.is_empty());
    }

    #[test]
    fn repeating_route_skips_when_destination_storage_is_full() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        let cap = state.storage_cap_at(neighbor(), ResourceKind::Iron);
        state.colonies[0].resources.iron = cap;
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.iron = 10_000.0;
        state.resources.deuterium = 50.0;
        let cargo = Resources::from_parts(100.0, 0.0, 0.0);
        let id = state
            .schedule_route(state.home, neighbor(), cargo_fleet(), cargo, 10, 0)
            .unwrap();
        state.catch_up(1);
        assert!(
            state
                .logistics
                .stalls
                .iter()
                .any(|s| s.contains(&format!("route {id}: skipped")) && s.contains("storage")),
            "{:?}",
            state.logistics.stalls
        );
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
        close(state.colonies[0].resources.iron, cap);
    }

    #[test]
    fn transport_bounces_overflow_when_dest_has_no_room() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        let cap = state.storage_cap_at(neighbor(), ResourceKind::Iron);
        state.colonies[0].resources.iron = cap - 1.0;
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.iron = 10_000.0;
        state.resources.deuterium = 50.0;
        let cargo = Resources::from_parts(100.0, 0.0, 0.0);
        state
            .launch_transport(state.home, neighbor(), cargo_fleet(), cargo, 100, 0)
            .unwrap();
        state.catch_up(1);
        close(state.colonies[0].resources.iron, cap);
        assert!(
            state
                .logistics
                .stalls
                .iter()
                .any(|s| s.contains("bounced cargo")),
            "{:?}",
            state.logistics.stalls
        );
        assert!(
            state
                .logistics
                .deliveries
                .iter()
                .any(|s| s.contains("delivered")),
            "{:?}",
            state.logistics.deliveries
        );
        assert_eq!(state.missions.len(), 1);
        close(state.missions[0].cargo.iron, 99.0);
    }

    #[test]
    fn enqueue_build_many_needs_logistics_and_reports_unaffordable() {
        let mut state = GameState::with_config(0, logistics_config());
        state.resources = Resources::from_parts(100_000.0, 100_000.0, 100_000.0);
        plant_colony(&mut state, neighbor());
        state.colonies[0].resources = Resources::ZERO;
        let coords = [state.home, neighbor()];
        let blocked = state.enqueue_build_many(BuildingKind::MetalMine, &coords, 0);
        assert!(
            blocked
                .iter()
                .all(|(_, r)| matches!(r, Err(BuildError::NeedLogistics)))
        );
        assert!(state.build_queue.is_empty());
        assert!(state.colonies[0].build_queue.is_empty());

        state.tech_levels.insert("logistics".into(), 1);
        let results = state.enqueue_build_many(BuildingKind::MetalMine, &coords, 0);
        assert!(results[0].1.is_ok(), "{:?}", results[0].1);
        assert!(
            matches!(results[1].1, Err(BuildError::InsufficientResources { .. })),
            "{:?}",
            results[1].1
        );
        assert_eq!(state.build_queue.len(), 1);
        assert!(state.colonies[0].build_queue.is_empty());
    }

    fn wonder_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.starting_resources = Resources::from_parts(1_000.0, 1_000.0, 1_000.0);
        cfg.wonders = vec![crate::WonderSpec {
            id: "test_ring".into(),
            name: "Test ring".into(),
            prereq_techs: vec![crate::TechPrereq {
                id: "energy".into(),
                level: 1,
            }],
            stages: vec![
                crate::WonderStage {
                    cost: Resources::from_parts(100.0, 40.0, 10.0),
                    duration_secs: 10,
                    require_tier: None,
                },
                crate::WonderStage {
                    cost: Resources::from_parts(200.0, 80.0, 20.0),
                    duration_secs: 20,
                    require_tier: None,
                },
            ],
            modifiers: vec![crate::WonderModifier {
                kind: ModifierKind::EnergyOutput,
                value: 0.25,
            }],
        }];
        cfg
    }

    #[test]
    fn start_wonder_needs_research_and_blocks_a_second_job() {
        let mut state = GameState::with_config(0, wonder_config());
        assert!(matches!(
            state.start_wonder("missing", 0),
            Err(WonderError::Unknown { .. })
        ));
        assert!(matches!(
            state.start_wonder("test_ring", 0),
            Err(WonderError::MissingTech {
                required: 1,
                actual: 0,
                ..
            })
        ));
        state.tech_levels.insert("energy".into(), 1);
        state.start_wonder("test_ring", 0).unwrap();
        assert!(matches!(
            state.start_wonder("test_ring", 0),
            Err(WonderError::Busy { .. })
        ));
    }

    #[test]
    fn several_planets_pay_a_stage_and_offline_catch_up_matches_step_by_step() {
        let mut gap = GameState::with_config(0, wonder_config());
        gap.tech_levels.insert("energy".into(), 1);
        plant_colony(&mut gap, neighbor());
        gap.resources = Resources::from_parts(60.0, 20.0, 4.0);
        gap.colonies[0].resources = Resources::from_parts(60.0, 20.0, 6.0);
        gap.start_wonder("test_ring", 0).unwrap();
        let from_home = gap.contribute_wonder(gap.home, 0).unwrap();
        close(from_home.iron, 60.0);
        close(from_home.silicon, 20.0);
        close(from_home.deuterium, 4.0);
        let from_colony = gap.contribute_wonder(neighbor(), 0).unwrap();
        close(from_colony.iron, 40.0);
        close(from_colony.silicon, 20.0);
        close(from_colony.deuterium, 6.0);
        assert_eq!(gap.wonder.as_ref().unwrap().completes_at, Some(10));
        close(gap.resources.iron, 0.0);
        close(gap.colonies[0].resources.iron, 20.0);

        let mut step = gap.clone();
        gap.catch_up(10);
        for t in 1..=10 {
            step.catch_up(t);
        }
        assert_eq!(gap.wonder.as_ref().unwrap().stage, 1);
        assert_eq!(step.wonder.as_ref().unwrap().stage, 1);
        assert_eq!(gap.wonder.as_ref().unwrap().completes_at, None);
        assert_eq!(gap.last_tick_unix, step.last_tick_unix);
        close(gap.resources.iron, step.resources.iron);
        close(
            gap.colonies[0].resources.iron,
            step.colonies[0].resources.iron,
        );
        assert!(
            gap.events
                .iter()
                .any(|e| e.text.contains("stage 1 complete"))
        );
    }

    #[test]
    fn completed_wonder_feeds_modifier_lookup() {
        let mut state = GameState::with_config(0, wonder_config());
        state.tech_levels.insert("energy".into(), 1);
        state.resources = Resources::from_parts(400.0, 200.0, 50.0);
        let before = state.modifier(ModifierKind::EnergyOutput);
        state.start_wonder("test_ring", 0).unwrap();
        state.contribute_wonder(state.home, 0).unwrap();
        state.catch_up(10);
        state.contribute_wonder(state.home, 10).unwrap();
        state.catch_up(30);
        assert!(state.wonder.is_none());
        assert_eq!(state.completed_wonders, vec!["test_ring".to_string()]);
        close(state.modifier(ModifierKind::EnergyOutput), before + 0.25);
        close(
            state.energy_produced(),
            state
                .config
                .solar_plant
                .energy_output(state.buildings.solar_plant)
                * (1.0 + before + 0.25),
        );
        assert!(matches!(
            state.start_wonder("test_ring", 30),
            Err(WonderError::AlreadyComplete { .. })
        ));
    }

    #[test]
    fn wonder_eta_includes_colony_income() {
        let mut cfg = wonder_config();
        cfg.starting_buildings.metal_mine = 0;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_buildings.solar_plant = 0;
        let mut state = GameState::with_config(0, cfg);
        state.tech_levels.insert("energy".into(), 1);
        plant_colony(&mut state, neighbor());
        state.colonies[0].buildings.metal_mine = 1;
        state.colonies[0].buildings.crystal_mine = 1;
        state.colonies[0].buildings.solar_plant = 1;
        state.resources = Resources::from_parts(50.0, 20.0, 10.0);
        state.start_wonder("test_ring", 0).unwrap();
        state.contribute_wonder(state.home, 0).unwrap();
        close(state.production_per_hour(ResourceKind::Iron), 0.0);
        assert!(
            state.wonder_eta_secs().is_some(),
            "colony mines should fund the unpaid remainder"
        );
    }

    #[test]
    fn observatory_espionage_feeds_expedition_astro() {
        let mut cfg = wonder_config();
        cfg.wonders[0].modifiers = vec![crate::WonderModifier {
            kind: ModifierKind::Espionage,
            value: 2.0,
        }];
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(state.expedition_astro(), 0);
        state.completed_wonders.push("test_ring".into());
        assert_eq!(state.expedition_astro(), 2);
    }

    #[test]
    fn wonder_drive_speed_applies_to_ship_stats() {
        let mut cfg = wonder_config();
        cfg.wonders[0].modifiers = vec![crate::WonderModifier {
            kind: ModifierKind::DriveSpeed,
            value: 0.20,
        }];
        let mut state = GameState::with_config(0, cfg);
        let before = state.ship_stats(ShipKind::LightFighter).speed;
        state.completed_wonders.push("test_ring".into());
        close(
            state.ship_stats(ShipKind::LightFighter).speed,
            before * 1.20,
        );
    }

    fn outpost_config() -> BalanceConfig {
        let mut cfg = colony_config();
        cfg.flight.base_fleet_slots = 4;
        cfg.outposts.count = 3;
        cfg.outposts.min_ships = 1;
        cfg.outposts.min_garrison = 2;
        cfg.outposts.strength_factor = 0.4;
        cfg.outposts.garrison_factor = 0.5;
        cfg.outposts.rebuild_secs = 86_400;
        cfg.outposts.variance = 0.0;
        cfg.outposts.troop_strength = 5;
        cfg.outposts.rare_chance = 0.0;
        cfg
    }

    fn fighters(n: u32) -> Fleet {
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, n);
        ships
    }

    fn troops(n: u32) -> Fleet {
        let mut ships = Fleet::default();
        ships.add(ShipKind::TroopTransport, n);
        ships
    }

    fn fly_home(state: &mut GameState) {
        if state.missions.is_empty() {
            return;
        }
        let until = state.missions[0]
            .phase_ends_at
            .saturating_add(state.missions[0].outbound_secs);
        state.catch_up(until);
        assert!(state.missions.is_empty());
    }

    fn deliver_intel(state: &mut GameState) {
        let until = state
            .spy_reports
            .iter()
            .map(|r| r.arrives_at)
            .max()
            .unwrap_or(state.last_tick_unix);
        if until > state.last_tick_unix {
            state.catch_up(until);
        }
    }

    #[test]
    fn invasion_is_refused_while_defences_remain() {
        let mut state = GameState::with_config(0, outpost_config());
        state.resources.deuterium = 10_000.0;
        let dest = state.outposts[0].coord;
        assert!(state.outpost(dest).unwrap().defences.total() > 0);
        state.fleet.add(ShipKind::TroopTransport, 2);
        assert_eq!(
            state.launch_invasion(troops(2), dest, 100, 0),
            Err(FleetError::DefencesRemain)
        );
        assert_eq!(state.colonies.len(), 0);
        assert!(state.outpost(dest).is_some());
    }

    fn probes(n: u32) -> Fleet {
        let mut ships = Fleet::default();
        ships.add(ShipKind::EspionageProbe, n);
        ships
    }

    #[test]
    fn spy_report_tiers_follow_probes_and_research() {
        let mut cfg = outpost_config();
        cfg.espionage.destroy_chance = 0.0;
        cfg.espionage.detect_base = 0.0;
        cfg.espionage.detect_per_probe = 0.0;
        cfg.espionage.error_base = 0.0;
        cfg.espionage.error_per_au = 0.0;
        cfg.espionage.decoy_bias = 0.0;
        cfg.techs = TechTree::parse(
            r#"
[[tech]]
id = "espionage"
name = "Espionage"
metal = 1.0
crystal = 1.0
deuterium = 0.0
cost_growth = 2.0
base_seconds = 1.0
time_growth = 2.0
prereq_buildings = []
prereq_techs = []
modifiers = [{ kind = "espionage", per_level = 1.0 }]
"#,
        )
        .expect("espionage tech");
        let mut low = GameState::with_config(0, cfg.clone());
        low.resources.deuterium = 10_000.0;
        let dest = low.outposts[0].coord;
        low.fleet.add(ShipKind::EspionageProbe, 1);
        low.launch_spy(probes(1), dest, 100, 0).unwrap();
        let arrives = low.missions[0].phase_ends_at;
        low.catch_up(arrives);
        let delay = crate::light_delay_secs(low.home, dest, &low.config.orbits);
        if delay > 0 {
            assert!(low.latest_report(dest).is_none());
            low.catch_up(arrives.saturating_add(delay));
        }
        let one = low.latest_report(dest).expect("report");
        assert_eq!(one.tier, "activity");
        assert!(one.resources.is_none());
        assert!(one.garrison.is_none());
        assert!(one.formation.is_none());
        assert!(one.stance.is_none());
        assert_eq!(one.age_secs(low.last_tick_unix), delay);
        fly_home(&mut low);
        let aged = low.latest_report(dest).expect("report");
        assert!(aged.age_secs(low.last_tick_unix) > 0);

        let mut high = GameState::with_config(0, cfg);
        high.resources.deuterium = 10_000.0;
        high.tech_levels.insert("espionage".into(), 4);
        let dest = high.outposts[0].coord;
        let garrison = high.outposts[0].garrison;
        let defences = high.outposts[0].defences;
        high.fleet.add(ShipKind::EspionageProbe, 1);
        high.launch_spy(probes(1), dest, 100, 0).unwrap();
        fly_home(&mut high);
        let delay = crate::light_delay_secs(high.home, dest, &high.config.orbits);
        if delay > 0 {
            assert!(
                high.visible_events(crate::Actor::Player)
                    .iter()
                    .all(|e| !e.text.contains("spy")),
                "{:?}",
                high.visible_events(crate::Actor::Player)
            );
        }
        deliver_intel(&mut high);
        let full = high.latest_report(dest).expect("report");
        assert_eq!(full.tier, "garrison");
        assert_eq!(full.resources, Some(Resources::ZERO));
        assert_eq!(full.fleet, Some(Fleet::default()));
        assert_eq!(full.defences, Some(defences));
        assert_eq!(full.garrison, Some(garrison));
        assert_eq!(full.formation.as_deref(), Some("neutral"));
        assert_eq!(full.stance.as_deref(), Some("neutral"));
        assert!(
            high.events.iter().any(|e| e.text.contains("spy")
                && e.text.contains("detected")
                && e.text.contains("lost")),
            "{:?}",
            high.events
        );
    }

    #[test]
    fn spy_detection_is_deterministic_for_a_seed() {
        let cfg = outpost_config();
        let mut a = GameState::with_seed(0, cfg.clone(), 99);
        let mut b = GameState::with_seed(0, cfg, 99);
        a.resources.deuterium = 10_000.0;
        b.resources.deuterium = 10_000.0;
        let dest = a.outposts[0].coord;
        a.fleet.add(ShipKind::EspionageProbe, 6);
        b.fleet.add(ShipKind::EspionageProbe, 6);
        a.launch_spy(probes(6), dest, 100, 0).unwrap();
        b.launch_spy(probes(6), dest, 100, 0).unwrap();
        fly_home(&mut a);
        fly_home(&mut b);
        deliver_intel(&mut a);
        deliver_intel(&mut b);
        let ra = a.latest_report(dest).unwrap();
        let rb = b.latest_report(dest).unwrap();
        assert_eq!(ra.detected, rb.detected);
        assert_eq!(ra.probes_lost, rb.probes_lost);
        assert_eq!(ra.tier, rb.tier);
        assert_eq!(ra.score, rb.score);
        if !ra.detected {
            assert_eq!(ra.probes_lost, 0);
        }
        assert!(ra.probes_lost <= 6);
    }

    #[test]
    fn pending_spy_event_survives_the_event_log_cap() {
        let mut cfg = outpost_config();
        cfg.espionage.destroy_chance = 0.0;
        cfg.espionage.detect_base = 0.0;
        cfg.espionage.detect_per_probe = 0.0;
        let mut state = GameState::with_config(0, cfg);
        state.resources.deuterium = 10_000.0;
        let dest = state.outposts[0].coord;
        let delay = crate::light_delay_secs(state.home, dest, &state.config.orbits);
        assert!(delay > 0);
        state.fleet.add(ShipKind::EspionageProbe, 1);
        state.launch_spy(probes(1), dest, 100, 0).unwrap();
        fly_home(&mut state);
        assert!(state.latest_report(dest).is_none());
        assert!(
            state.events.iter().all(|event| !event.text.contains("spy")),
            "{:?}",
            state.events
        );
        for i in 0..40 {
            state.push_event(state.last_tick_unix, format!("noise {i}"));
        }
        deliver_intel(&mut state);
        assert!(
            state
                .visible_events(crate::Actor::Player)
                .iter()
                .any(|event| event.text.contains("spy")),
            "{:?}",
            state.visible_events(crate::Actor::Player)
        );
    }

    #[test]
    fn spy_reports_wait_for_light_delay_and_keep_conflicts() {
        let mut cfg = outpost_config();
        cfg.espionage.destroy_chance = 0.0;
        cfg.espionage.detect_base = 0.0;
        cfg.espionage.detect_per_probe = 0.0;
        cfg.espionage.error_base = 0.0;
        cfg.espionage.error_per_au = 0.0;
        let mut state = GameState::with_config(0, cfg);
        let dest = state.outposts[0].coord;
        let delay = crate::light_delay_secs(state.home, dest, &state.config.orbits);
        state.spy_reports.push(SpyReport {
            id: 1,
            target: dest,
            created_at: 10,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 4.0,
            tier: "garrison".into(),
            resources: None,
            fleet: None,
            defences: None,
            garrison: Some(10),
            kind: None,
            size: None,
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 10,
            arrives_at: 10 + delay,
            confidence: 0.9,
            deceived: false,
        });
        state.last_tick_unix = 10;
        assert!(state.latest_report(dest).is_none());
        state.last_tick_unix = 10 + delay;
        assert_eq!(state.latest_report(dest).unwrap().garrison, Some(10));
        state.spy_reports.push(SpyReport {
            id: 2,
            target: dest,
            created_at: 20,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 4.0,
            tier: "garrison".into(),
            resources: None,
            fleet: None,
            defences: None,
            garrison: Some(40),
            kind: None,
            size: None,
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 20,
            arrives_at: 20,
            confidence: 0.8,
            deceived: false,
        });
        state.last_tick_unix = state.last_tick_unix.max(20);
        let facts = state.intel_facts_for(crate::Actor::Player, state.last_tick_unix);
        let garrison = facts
            .iter()
            .find(|f| f.field == "garrison")
            .expect("garrison fact");
        assert_eq!(garrison.observations.len(), 2);
        let values: Vec<_> = garrison
            .observations
            .iter()
            .map(|o| match o.value {
                crate::IntelValue::Count(n) => n,
                _ => panic!("expected count"),
            })
            .collect();
        assert!(values.contains(&10));
        assert!(values.contains(&40));
        let chain = state.provenance_for(
            crate::Actor::Player,
            &dest.to_string(),
            "garrison",
            state.last_tick_unix,
        );
        assert_eq!(chain.len(), 2);
        assert!(chain.iter().all(|o| o.source == "probe"));
    }

    #[test]
    fn detected_spy_wave_is_deceived() {
        let mut cfg = outpost_config();
        cfg.espionage.destroy_chance = 0.0;
        cfg.espionage.detect_base = 1.0;
        cfg.espionage.detect_per_probe = 0.0;
        cfg.espionage.detect_per_level = 0.0;
        cfg.espionage.error_base = 0.0;
        cfg.espionage.error_per_au = 0.0;
        cfg.espionage.decoy_bias = 0.35;
        cfg.techs = TechTree::parse(
            r#"
[[tech]]
id = "espionage"
name = "Espionage"
metal = 1.0
crystal = 1.0
deuterium = 0.0
cost_growth = 2.0
base_seconds = 1.0
time_growth = 2.0
prereq_buildings = []
prereq_techs = []
modifiers = [{ kind = "espionage", per_level = 1.0 }]
"#,
        )
        .expect("espionage tech");
        let mut state = GameState::with_config(0, cfg);
        state.resources.deuterium = 10_000.0;
        state.tech_levels.insert("espionage".into(), 4);
        let dest = state.outposts[0].coord;
        let truth = state.outposts[0].garrison;
        state.fleet.add(ShipKind::EspionageProbe, 1);
        state.launch_spy(probes(1), dest, 100, 0).unwrap();
        fly_home(&mut state);
        deliver_intel(&mut state);
        let report = state.latest_report(dest).expect("report");
        assert!(report.detected);
        assert!(report.deceived);
        assert_eq!(report.tier, "garrison");
        assert_ne!(report.garrison, Some(truth));
        let json_like = format!("{:?}", report);
        assert!(json_like.contains("garrison"));
    }

    #[test]
    fn spy_refuses_without_probes() {
        let mut state = GameState::with_config(0, outpost_config());
        state.resources.deuterium = 10_000.0;
        let dest = state.outposts[0].coord;
        state.fleet.add(ShipKind::LightFighter, 1);
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 1);
        assert_eq!(
            state.launch_spy(ships, dest, 100, 0),
            Err(FleetError::NeedEspionageProbe)
        );
    }

    #[test]
    fn ground_combat_is_deterministic_and_lands_in_the_report() {
        let mut cfg = outpost_config();
        cfg.outposts.variance = 0.2;
        let mut a = GameState::with_seed(0, cfg.clone(), 42);
        let mut b = GameState::with_seed(0, cfg, 42);
        a.resources.deuterium = 10_000.0;
        b.resources.deuterium = 10_000.0;
        a.tech_levels.insert("colonization".into(), 1);
        b.tech_levels.insert("colonization".into(), 1);
        let dest = a.outposts[0].coord;
        assert_eq!(dest, b.outposts[0].coord);
        a.outposts[0].defences = Fleet::default();
        b.outposts[0].defences = Fleet::default();
        a.outposts[0].garrison = 4;
        b.outposts[0].garrison = 4;
        a.fleet.add(ShipKind::TroopTransport, 3);
        b.fleet.add(ShipKind::TroopTransport, 3);
        a.launch_invasion(troops(3), dest, 100, 0).unwrap();
        b.launch_invasion(troops(3), dest, 100, 0).unwrap();
        fly_home(&mut a);
        fly_home(&mut b);
        let report_a = a
            .events
            .iter()
            .find(|e| e.text.contains("invade"))
            .expect("invade report");
        let report_b = b
            .events
            .iter()
            .find(|e| e.text.contains("invade"))
            .expect("invade report");
        assert_eq!(report_a.text, report_b.text);
        assert!(
            report_a.text.contains("troops 15 vs garrison 4"),
            "{}",
            report_a.text
        );
        assert!(report_a.text.contains("rolls"), "{}", report_a.text);
        assert!(report_a.text.contains("winner"), "{}", report_a.text);
    }

    #[test]
    fn conquered_outpost_counts_against_the_colony_cap() {
        let mut state = GameState::with_config(0, outpost_config());
        state.resources.deuterium = 10_000.0;
        let dest = state.outposts[0].coord;
        state.outposts[0].defences = Fleet::default();
        state.fleet.add(ShipKind::TroopTransport, 2);
        assert_eq!(state.colony_cap(), 1);
        assert_eq!(
            state.launch_invasion(troops(2), dest, 100, 0),
            Err(FleetError::ColonyCap { used: 1, cap: 1 })
        );
        state.tech_levels.insert("colonization".into(), 1);
        assert_eq!(state.colony_cap(), 2);
        state.launch_invasion(troops(2), dest, 100, 0).unwrap();
        fly_home(&mut state);
        assert_eq!(state.colonies.len(), 1);
        assert_eq!(state.colonies[0].coord, dest);
        assert!(state.outpost(dest).is_none());
        assert_eq!(1 + state.colonies.len() as u32, state.colony_cap());
        let dest2 = state.outposts[0].coord;
        state.outposts[0].defences = Fleet::default();
        state.fleet.add(ShipKind::TroopTransport, 2);
        assert_eq!(
            state.launch_invasion(troops(2), dest2, 100, 0),
            Err(FleetError::ColonyCap { used: 2, cap: 2 })
        );
    }

    #[test]
    fn rare_bonus_comes_from_config() {
        let mut state = GameState::with_config(0, outpost_config());
        state.resources.deuterium = 10_000.0;
        state.tech_levels.insert("colonization".into(), 1);
        let dest = state.outposts[0].coord;
        state.outposts[0].defences = Fleet::default();
        state.outposts[0].rare = Some("rich_vein".into());
        state.fleet.add(ShipKind::TroopTransport, 2);
        state.launch_invasion(troops(2), dest, 100, 0).unwrap();
        fly_home(&mut state);
        assert_eq!(state.colonies[0].rare.as_deref(), Some("rich_vein"));
        let bonus = state.rare_bonus_at(dest).expect("rare bonus");
        assert_eq!(bonus.id, "rich_vein");
        assert!((bonus.iron - 0.25).abs() < 1e-9);
        assert!(bonus.summary().contains("iron +25%"), "{}", bonus.summary());
        let base = outpost_config()
            .ships
            .get(ShipKind::LightFighter)
            .duration_secs();
        let faster_cfg = outpost_config();
        let mut world = GameState::with_config(0, faster_cfg);
        world.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 140,
            resources: Resources::ZERO,
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: Some("shipyard_world".into()),
            goods: BTreeMap::new(),
        });
        let sped = world.ship_time_secs(ShipKind::LightFighter);
        assert!(sped < base, "sped={sped} base={base}");
        assert_eq!(sped, world.ship_time_secs_on(dest, ShipKind::LightFighter));
    }

    #[test]
    fn outpost_strength_scales_with_player_progress_and_early_outposts_are_winnable() {
        let mut cfg = outpost_config();
        cfg.outposts.rebuild_secs = 10;
        let mut state = GameState::with_config(0, cfg);
        state.resources.deuterium = 10_000.0;
        state.tech_levels.insert("colonization".into(), 1);
        let dest = state.outposts[0].coord;
        let start_ships = state
            .outpost(dest)
            .unwrap()
            .defences
            .get(ShipKind::LightFighter);
        assert_eq!(start_ships, 1);
        state.fleet.add(ShipKind::LightFighter, 8);
        state.launch_attack(fighters(8), dest, 100, 0).unwrap();
        fly_home(&mut state);
        assert_eq!(
            state.outpost(dest).unwrap().defences.total(),
            0,
            "early outpost should fall to a small fighter wing"
        );
        state.fleet.add(ShipKind::LightFighter, 200);
        state.catch_up(10);
        let rebuilt = state
            .outpost(dest)
            .unwrap()
            .defences
            .get(ShipKind::LightFighter);
        assert!(
            rebuilt > start_ships,
            "rebuilt={rebuilt} start={start_ships}"
        );
    }

    fn prestige_test_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.prestige.min_score = 4;
        cfg.prestige.points_per_currency = 4;
        cfg.prestige.colony_weight = 8;
        cfg.prestige.wonder_weight = 40;
        cfg
    }

    #[test]
    fn prestige_is_refused_until_the_configured_score() {
        let mut cfg = prestige_test_config();
        cfg.prestige.min_score = 50;
        let mut state = GameState::with_config(0, cfg);
        assert!(state.prestige_score() < 50);
        let err = state.prestige_reset(0).unwrap_err();
        assert!(
            matches!(err, crate::PrestigeError::NotEligible { .. }),
            "{err}"
        );
        state.completed_wonders.push("dyson_ring".into());
        assert!(state.can_prestige());
        let before = state.building_levels();
        state.resources.iron = 10_000.0;
        state.resources.silicon = 10_000.0;
        state.enqueue_build(BuildingKind::SolarPlant, 0).unwrap();
        state.prestige_reset(10_000).unwrap();
        assert!(state.build_queue.is_empty());
        assert!(state.completed_wonders.is_empty());
        assert_eq!(state.prestige.current.index, 2);
        assert_eq!(state.prestige.runs.len(), 1);
        assert!(
            state.prestige.runs[0].building_levels > before,
            "reset should count buildings that finished during catch-up"
        );
    }

    #[test]
    fn prestige_bonuses_survive_reset_and_run_state_does_not() {
        let mut state = GameState::with_config(0, prestige_test_config());
        state.resources.iron = 9_000.0;
        state.buildings.metal_mine = 6;
        state.tech_levels.insert("energy".into(), 3);
        state.fleet.add(ShipKind::LightFighter, 4);
        state.completed_wonders.push("observatory".into());
        state.prestige.currency = 5;
        state.buy_prestige_bonus("mine_focus", 0).unwrap();
        state.buy_prestige_bonus("starter_cache", 0).unwrap();
        let owned = state.prestige.owned.clone();
        let preview = state.prestige_preview();
        assert!(preview.eligible);
        assert!(preview.kept.contains(&"purchased bonuses"));
        assert!(preview.kept.contains(&"achievements"));
        assert!(preview.lost.contains(&"resources"));
        let score = preview.score;
        state.prestige_reset(20).unwrap();
        assert_eq!(state.prestige.owned, owned);
        assert!(state.prestige.currency >= 1);
        assert_eq!(state.buildings.metal_mine, 1);
        assert!(state.tech_levels.is_empty());
        assert_eq!(state.fleet.total(), 0);
        assert!(state.completed_wonders.is_empty());
        assert!(state.resources.iron > state.config.starting_resources.iron);
        assert_eq!(state.prestige.runs[0].score, score);
        assert_eq!(state.prestige.runs[0].ended_at, Some(20));
        assert_eq!(state.prestige.current.index, 2);
        let with_bonus = state.production_per_hour(ResourceKind::Iron);
        let mut plain = GameState::with_config(20, prestige_test_config());
        plain.resources = state.resources;
        assert!(
            with_bonus > plain.production_per_hour(ResourceKind::Iron),
            "bonus {with_bonus} plain {}",
            plain.production_per_hour(ResourceKind::Iron)
        );
    }

    #[test]
    fn inherit_prestige_applies_starting_cache_to_a_new_galaxy() {
        let mut source = GameState::with_config(0, prestige_test_config());
        source.prestige.currency = 1;
        source.buy_prestige_bonus("starter_cache", 0).unwrap();
        source.prestige.lifetime = 4;
        source.prestige.runs.push(crate::RunStats::new(1, 0));
        let mut next = GameState::new_live_galaxy(10, prestige_test_config(), 7);
        let before = next.resources.iron;
        next.inherit_prestige(source.prestige.clone());
        assert!(next.resources.iron > before);
        assert_eq!(next.prestige.owned_count("starter_cache"), 1);
        assert_eq!(next.prestige.current.index, 2);
        assert_eq!(next.prestige.lifetime, 4);
    }

    #[test]
    fn extra_queue_slot_comes_from_a_purchased_bonus() {
        let mut state = GameState::with_config(0, prestige_test_config());
        let base = state.build_queue_cap();
        state.prestige.currency = 3;
        state.buy_prestige_bonus("extra_queue", 0).unwrap();
        assert_eq!(state.build_queue_cap(), base + 1);
    }

    #[test]
    fn research_boost_composes_with_lab_focus() {
        let mut cfg = research_config();
        cfg.activity = activity_config().activity;
        cfg.activity.agent_long.boost_secs = 100;
        let mut state = GameState::with_config(0, cfg);
        state.prestige.currency = 2;
        state.buy_prestige_bonus("lab_focus", 0).unwrap();
        state.start_research("energy", 0).unwrap();
        // 1800s / 1.08 lab-focus, ceiled.
        assert_eq!(state.research.as_ref().unwrap().completes_at, 1_667);
        assert_eq!(
            state.apply_activity(
                ActivityPulse::AgentFinished {
                    ts: 0,
                    duration_secs: 100,
                },
                10_000,
            ),
            ActivityApply::Applied
        );
        // remaining 1667 * 1.08 / (1.08 * 2) = 833.5, ceiled.
        assert_eq!(state.research.as_ref().unwrap().completes_at, 834);
        state.catch_up(100);
        // remaining 734 * 2.16 / 1.08 = 1468; completes at 100 + 1468.
        assert_eq!(state.research.as_ref().unwrap().completes_at, 1_568);
        assert!(state.tech_level("energy") == 0);
        state.catch_up(1_568);
        assert_eq!(state.tech_level("energy"), 1);
    }

    #[test]
    fn buying_lab_focus_rescales_an_open_research_job() {
        let mut state = GameState::with_config(0, research_config());
        state.start_research("energy", 0).unwrap();
        assert_eq!(state.research.as_ref().unwrap().completes_at, 1_800);
        state.prestige.currency = 2;
        state.buy_prestige_bonus("lab_focus", 0).unwrap();
        // 1800 * 1.0 / 1.08 = 1666.66, ceiled.
        assert_eq!(state.research.as_ref().unwrap().completes_at, 1_667);
    }

    #[test]
    fn mine_focus_raises_colony_metal_over_an_hour() {
        fn with_colony(bonus: bool) -> GameState {
            let mut state = GameState::with_config(0, prestige_test_config());
            state.buildings.metal_mine = 0;
            state.buildings.crystal_mine = 0;
            state.resources = Resources::ZERO;
            if bonus {
                state.prestige.currency = 2;
                state.buy_prestige_bonus("mine_focus", 0).unwrap();
            }
            state.colonies.push(Colony {
                coord: Coord {
                    galaxy: 1,
                    system: 1,
                    position: 2,
                },
                kind: PlanetType::Rocky,
                size: 100,
                resources: Resources::ZERO,
                buildings: Buildings {
                    metal_mine: 1,
                    crystal_mine: 0,
                    deuterium_synthesizer: 0,
                    solar_plant: 1,
                    metal_storage: 0,
                    crystal_storage: 0,
                    deuterium_storage: 0,
                    robotics_factory: 0,
                    research_lab: 0,
                    shipyard: 0,
                    radiator: 0,
                    scrubber: 0,
                    fission_reactor: 0,
                },
                fleet: Fleet::default(),
                defences: crate::DefenceGrid::default(),
                build_queue: Vec::new(),
                rare: None,
                goods: BTreeMap::new(),
            });
            state
        }
        let mut plain = with_colony(false);
        let mut boosted = with_colony(true);
        plain.catch_up(3600);
        boosted.catch_up(3600);
        assert!(
            boosted.colonies[0].resources.iron > plain.colonies[0].resources.iron,
            "boosted {} plain {}",
            boosted.colonies[0].resources.iron,
            plain.colonies[0].resources.iron
        );
    }

    #[test]
    fn prestige_formula_reads_weights_from_config() {
        let mut cfg = prestige_test_config();
        cfg.prestige.colony_weight = 5;
        cfg.prestige.wonder_weight = 11;
        let mut state = GameState::with_config(0, cfg);
        let base = state.prestige_score();
        state.colonies.push(Colony {
            coord: Coord {
                galaxy: 1,
                system: 1,
                position: 2,
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
        state.completed_wonders.push("stellar_gate".into());
        assert_eq!(state.prestige_score(), base + 5 + 11);
    }

    fn story_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.starting_buildings.metal_mine = 0;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_buildings.solar_plant = 0;
        cfg.story = crate::StoryCatalog::parse(
            r#"
first_delay_secs = 10
cooldown_secs = 50
[[event]]
id = "alpha"
title = "Alpha"
text = "First pick."
weight = 1
timeout_secs = 20
timeout_choice = "leave"
[[event.choices]]
id = "take"
label = "Take it"
metal = 40.0
deuterium_price = -0.12
standing = { traders = 6.0, pirates = -8.0 }
[[event.choices]]
id = "leave"
label = "Leave it"
[[event]]
id = "beta"
title = "Beta"
text = "Needs fighters."
weight = 1
timeout_secs = 20
timeout_choice = "pass"
min_fighters = 4
[[event.choices]]
id = "hire"
label = "Hire"
ships = { light_fighter = 1 }
[[event.choices]]
id = "pass"
label = "Pass"
"#,
        )
        .expect("story test catalog");
        cfg
    }

    #[test]
    fn same_seed_offers_the_same_story_after_the_first_delay() {
        let cfg = story_config();
        let mut a = GameState::with_config(0, cfg.clone());
        a.rng_seed = 42;
        let mut b = GameState::with_config(0, cfg);
        b.rng_seed = 42;
        a.catch_up(10);
        b.catch_up(10);
        let pending = a.story_pending.clone().expect("offered");
        assert_eq!(pending.event_id, "alpha");
        assert_eq!(pending.offered_at, 10);
        assert_eq!(pending.timeout_at, 30);
        assert_eq!(a.story_pending, b.story_pending);
        assert_eq!(a.rng_seed, b.rng_seed);
    }

    #[test]
    fn ignored_story_applies_the_timeout_choice() {
        let mut state = GameState::with_config(0, story_config());
        state.rng_seed = 1;
        state.catch_up(10);
        assert_eq!(
            state.story_pending.as_ref().map(|p| p.event_id.as_str()),
            Some("alpha")
        );
        state.catch_up(30);
        assert!(state.story_pending.is_none());
        close(state.resources.iron, 0.0);
        assert_eq!(state.last_story_at, 30);
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text == "story: Alpha → Leave it"),
            "{:?}",
            state.events
        );
    }

    #[test]
    fn choose_story_applies_the_selected_effect() {
        let mut state = GameState::with_config(0, story_config());
        state.rng_seed = 1;
        state.catch_up(10);
        state.choose_story("take", 12).unwrap();
        close(state.resources.iron, 40.0);
        assert!(state.story_pending.is_none());
        assert_eq!(state.last_story_at, 12);
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text == "story: Alpha → Take it"),
            "{:?}",
            state.events
        );
    }

    #[test]
    fn story_offer_and_timeout_match_step_by_step_catch_up() {
        let cfg = story_config();
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.rng_seed = 7;
        gap.catch_up(45);
        let mut step = GameState::with_config(0, cfg);
        step.rng_seed = 7;
        for t in 1..=45 {
            step.catch_up(t);
        }
        assert_eq!(gap.story_pending, step.story_pending);
        assert_eq!(gap.last_story_at, step.last_story_at);
        assert_eq!(gap.events, step.events);
        assert_eq!(gap.rng_seed, step.rng_seed);
        assert_eq!(gap.resources, step.resources);
    }

    #[test]
    fn overdue_first_story_offers_on_the_next_tick_matching_step_by_step() {
        let cfg = story_config();
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.rng_seed = 9;
        gap.last_tick_unix = 20;
        let mut step = GameState::with_config(0, cfg);
        step.rng_seed = 9;
        step.last_tick_unix = 20;
        gap.catch_up(40);
        for t in 21..=40 {
            step.catch_up(t);
        }
        let pending = gap.story_pending.clone().expect("offered");
        assert_eq!(pending.offered_at, 21);
        assert_eq!(gap.story_pending, step.story_pending);
        assert_eq!(gap.rng_seed, step.rng_seed);
        assert_eq!(gap.last_story_at, step.last_story_at);
    }

    #[test]
    fn prestige_reset_clears_a_pending_story() {
        let mut state = GameState::with_config(0, story_config());
        state.rng_seed = 1;
        state.catch_up(10);
        assert!(state.story_pending.is_some());
        let config = state.config.clone();
        let mut next = GameState::with_config(50, config);
        next.prestige = state.prestige.clone();
        assert!(next.story_pending.is_none());
        assert_eq!(next.last_story_at, 0);
    }

    fn market_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.market = crate::MarketConfig {
            enabled: true,
            tick_secs: 10,
            ..crate::MarketConfig::default_active()
        };
        cfg
    }

    #[test]
    fn market_catch_up_gap_matches_step_by_step() {
        let cfg = market_config();
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.rng_seed = 11;
        gap.catch_up(100);
        let mut step = GameState::with_config(0, cfg);
        step.rng_seed = 11;
        for t in 1..=100 {
            step.catch_up(t);
        }
        assert_eq!(gap.market, step.market);
        assert_eq!(gap.last_market_at, step.last_market_at);
        assert_eq!(gap.rng_seed, step.rng_seed);
        assert!(gap.last_market_at > 0);
        for kind in ResourceKind::ALL {
            let mean = gap.config.market.mean(kind);
            let p = gap.market.price(kind);
            assert!(
                p >= mean * gap.config.market.min_mult - 1e-9,
                "{kind:?} {p}"
            );
            assert!(
                p <= mean * gap.config.market.max_mult + 1e-9,
                "{kind:?} {p}"
            );
        }
    }

    #[test]
    fn market_buy_then_sell_back_loses() {
        let mut cfg = market_config();
        cfg.market.tick_secs = 10_000;
        cfg.market.cargo_free = 1_000.0;
        let mut state = GameState::with_config(0, cfg);
        state.resources.iron = 500.0;
        let buy = state
            .trade_market(ResourceKind::Iron, ResourceKind::Silicon, 100.0, 0)
            .unwrap();
        let back = state
            .trade_market(
                ResourceKind::Silicon,
                ResourceKind::Iron,
                buy.recv_amount,
                0,
            )
            .unwrap();
        assert!(
            back.recv_amount < 100.0,
            "round trip {} -> {}",
            100.0,
            back.recv_amount
        );
        assert!(state.resources.iron < 500.0);
    }

    #[test]
    fn market_large_trade_needs_cargo() {
        let mut cfg = market_config();
        cfg.market.cargo_free = 50.0;
        cfg.market.tick_secs = 10_000;
        let mut state = GameState::with_config(0, cfg);
        state.resources.iron = 200.0;
        let err = state
            .trade_market(ResourceKind::Iron, ResourceKind::Silicon, 100.0, 0)
            .unwrap_err();
        assert!(
            matches!(err, crate::MarketError::NeedCargo { need, have } if (need - 100.0).abs() < 1e-9 && have == 0.0),
            "{err:?}"
        );
        state.fleet.add(ShipKind::SmallCargo, 1);
        state
            .trade_market(ResourceKind::Iron, ResourceKind::Silicon, 100.0, 0)
            .unwrap();
        close(state.resources.iron, 100.0);
    }

    #[test]
    fn story_choice_shocks_market_when_enabled() {
        let mut cfg = story_config();
        cfg.market = crate::MarketConfig::default_active();
        cfg.market.tick_secs = 10_000;
        let mut state = GameState::with_config(0, cfg);
        state.rng_seed = 1;
        let before = state.market.deuterium;
        state.catch_up(10);
        state.choose_story("take", 12).unwrap();
        assert!(
            state.market.deuterium < before,
            "before {before} after {}",
            state.market.deuterium
        );
    }

    fn diplomacy_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.diplomacy = crate::DiplomacyConfig::default();
        cfg.diplomacy.tick_secs = 10;
        cfg
    }

    #[test]
    fn diplomacy_decay_gap_matches_step_by_step() {
        let cfg = diplomacy_config();
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.diplomacy.set_standing("traders", 20.0);
        gap.diplomacy.set_standing("pirates", -40.0);
        let mut step = GameState::with_config(0, cfg);
        step.diplomacy.set_standing("traders", 20.0);
        step.diplomacy.set_standing("pirates", -40.0);
        gap.catch_up(100);
        for t in 1..=100 {
            step.catch_up(t);
        }
        assert_eq!(gap.diplomacy, step.diplomacy);
        assert_eq!(gap.last_diplomacy_at, step.last_diplomacy_at);
        assert!(gap.diplomacy.standing("traders") < 20.0);
        assert!(gap.diplomacy.standing("traders") > 0.0);
        assert!(gap.diplomacy.standing("pirates") > -40.0);
        assert!(gap.diplomacy.standing("pirates") < 0.0);
        assert_eq!(gap.events, step.events);
        assert!(
            gap.events.iter().all(|e| !e.text.starts_with("standing:")),
            "{:?}",
            gap.events
        );
    }

    #[test]
    fn overdue_first_diplomacy_tick_matches_step_by_step() {
        let cfg = diplomacy_config();
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.last_tick_unix = 20;
        gap.diplomacy.set_standing("traders", 20.0);
        let mut step = GameState::with_config(0, cfg);
        step.last_tick_unix = 20;
        step.diplomacy.set_standing("traders", 20.0);
        gap.catch_up(40);
        for t in 21..=40 {
            step.catch_up(t);
        }
        assert_eq!(gap.diplomacy, step.diplomacy);
        assert_eq!(gap.last_diplomacy_at, step.last_diplomacy_at);
        assert_eq!(gap.last_diplomacy_at, 31);
    }

    #[test]
    fn traders_tier_sets_market_fee_and_output_modifiers() {
        let mut cfg = diplomacy_config();
        cfg.market = crate::MarketConfig {
            enabled: true,
            tick_secs: 10_000,
            cargo_free: 1_000.0,
            ..crate::MarketConfig::default_active()
        };
        let mut state = GameState::with_config(0, cfg);
        close(state.modifier(ModifierKind::MetalOutput), 0.0);
        close(state.effective_market_fee(), 0.04);
        state.diplomacy.set_standing("traders", 20.0);
        close(state.modifier(ModifierKind::MetalOutput), 0.05);
        close(state.effective_market_fee(), 0.02);
        state.diplomacy.set_standing("traders", 50.0);
        close(state.modifier(ModifierKind::MetalOutput), 0.10);
        close(state.modifier(ModifierKind::CrystalOutput), 0.05);
        close(state.effective_market_fee(), 0.01);
        state.diplomacy.set_standing("traders", -100.0);
        close(state.modifier(ModifierKind::MetalOutput), 0.0);
        close(state.effective_market_fee(), 0.08);
    }

    #[test]
    fn pirate_tiers_change_raid_delay_and_pact_stops_raids() {
        let mut cfg = diplomacy_config();
        cfg.raid.enabled = true;
        cfg.raid.interval_secs = 100;
        cfg.raid.first_delay_secs = 100;
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(state.raid_interval_secs(), Some(100));
        state.diplomacy.set_standing("pirates", -100.0);
        assert_eq!(state.raid_interval_secs(), Some(50));
        state.diplomacy.set_standing("pirates", 25.0);
        assert_eq!(state.raid_interval_secs(), Some(200));
        state.diplomacy.set_standing("pirates", 60.0);
        assert_eq!(state.raid_interval_secs(), None);
        state.sync_raid_schedule_after_standing(0);
        assert_eq!(state.next_raid_at, None);
        state.diplomacy.set_standing("pirates", 0.0);
        state.sync_raid_schedule_after_standing(10);
        assert_eq!(state.next_raid_at, Some(110));
    }

    #[test]
    fn ancient_tiers_apply_modifiers_and_awakened_schedules_a_raid() {
        let mut cfg = diplomacy_config();
        cfg.raid.enabled = true;
        cfg.raid.first_delay_secs = 10_000;
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(state.next_raid_at, Some(10_000));
        close(state.modifier(ModifierKind::WeaponDamage), 0.0);
        state.adjust_standing("ancients", -50.0, "story", 0);
        close(state.modifier(ModifierKind::WeaponDamage), -0.05);
        assert_eq!(state.next_raid_at, Some(60));
        state.diplomacy.set_standing("ancients", 50.0);
        close(state.modifier(ModifierKind::Espionage), 2.0);
        close(state.modifier(ModifierKind::EnergyOutput), 0.10);
    }

    #[test]
    fn market_trade_changes_trader_standing() {
        let mut cfg = diplomacy_config();
        cfg.market = crate::MarketConfig {
            enabled: true,
            tick_secs: 10_000,
            cargo_free: 1_000.0,
            ..crate::MarketConfig::default_active()
        };
        let mut state = GameState::with_config(0, cfg);
        state.resources.iron = 500.0;
        state
            .trade_market(ResourceKind::Iron, ResourceKind::Silicon, 100.0, 0)
            .unwrap();
        close(state.diplomacy.standing("traders"), 1.0);
        assert!(
            state
                .diplomacy
                .log
                .iter()
                .any(|c| c.faction == "traders" && c.reason == "trade"),
            "{:?}",
            state.diplomacy.log
        );
    }

    #[test]
    fn raid_outcome_changes_pirate_standing() {
        let mut cfg = diplomacy_config();
        cfg.raid.enabled = true;
        cfg.raid.min_ships = 1;
        cfg.raid.strength_factor = 0.0;
        let mut loss = GameState::with_config(0, cfg.clone());
        loss.trigger_raid(0);
        close(loss.diplomacy.standing("pirates"), -4.0);
        let mut win = GameState::with_config(0, cfg);
        win.fleet.add(ShipKind::Battleship, 20);
        win.trigger_raid(0);
        close(win.diplomacy.standing("pirates"), -8.0);
        assert!(
            win.diplomacy
                .log
                .iter()
                .any(|c| c.faction == "pirates" && c.reason == "raid"),
            "{:?}",
            win.diplomacy.log
        );
    }

    #[test]
    fn conquering_an_outpost_changes_pirate_standing() {
        let mut cfg = outpost_config();
        cfg.diplomacy = crate::DiplomacyConfig::default();
        cfg.diplomacy.tick_secs = 10_000;
        let mut state = GameState::with_config(0, cfg);
        state.resources.deuterium = 10_000.0;
        state.tech_levels.insert("colonization".into(), 1);
        let dest = state.outposts[0].coord;
        state.outposts[0].defences = Fleet::default();
        state.fleet.add(ShipKind::TroopTransport, 2);
        state.launch_invasion(troops(2), dest, 100, 0).unwrap();
        fly_home(&mut state);
        close(state.diplomacy.standing("pirates"), -15.0);
        assert!(
            state.diplomacy.log.iter().any(|c| c.reason == "conquest"),
            "{:?}",
            state.diplomacy.log
        );
    }

    #[test]
    fn story_choice_changes_faction_standing() {
        let mut cfg = story_config();
        cfg.diplomacy = crate::DiplomacyConfig::default();
        let mut state = GameState::with_config(0, cfg);
        state.rng_seed = 1;
        state.catch_up(10);
        state.choose_story("take", 12).unwrap();
        close(state.diplomacy.standing("traders"), 6.0);
        close(state.diplomacy.standing("pirates"), -8.0);
        assert!(
            state.diplomacy.log.iter().any(|c| c.reason == "story"),
            "{:?}",
            state.diplomacy.log
        );
    }

    #[test]
    fn tribute_pays_metal_for_pirate_standing() {
        let mut state = GameState::with_config(0, diplomacy_config());
        state.resources.iron = 250.0;
        state.pay_tribute("pirates", 0).unwrap();
        close(state.resources.iron, 50.0);
        close(state.diplomacy.standing("pirates"), 12.0);
        let err = state.pay_tribute("traders", 0).unwrap_err();
        assert!(matches!(err, crate::DiplomacyError::NoTribute(_)), "{err}");
        state.resources.iron = 0.0;
        let err = state.pay_tribute("pirates", 0).unwrap_err();
        assert!(
            matches!(err, crate::DiplomacyError::Insufficient { .. }),
            "{err}"
        );
        state.resources.iron = 500.0;
        state.diplomacy.set_standing("pirates", 100.0);
        let err = state.pay_tribute("pirates", 0).unwrap_err();
        assert!(
            matches!(err, crate::DiplomacyError::NoStandingChange(_)),
            "{err}"
        );
        close(state.resources.iron, 500.0);
        close(state.diplomacy.standing("pirates"), 100.0);
    }

    #[test]
    fn story_schedule_raid_respects_pirate_pact() {
        let mut cfg = story_config();
        cfg.diplomacy = crate::DiplomacyConfig::default();
        cfg.raid.enabled = true;
        cfg.raid.first_delay_secs = 10_000;
        cfg.story = crate::StoryCatalog::parse(
            r#"
first_delay_secs = 10
cooldown_secs = 50
[[event]]
id = "alpha"
title = "Alpha"
text = "First pick."
weight = 1
timeout_secs = 20
timeout_choice = "leave"
[[event.choices]]
id = "take"
label = "Take it"
schedule_raid = true
[[event.choices]]
id = "leave"
label = "Leave it"
"#,
        )
        .expect("story pact catalog");
        let mut state = GameState::with_config(0, cfg);
        state.rng_seed = 1;
        state.diplomacy.set_standing("pirates", 60.0);
        state.sync_raid_schedule_after_standing(0);
        assert_eq!(state.next_raid_at, None);
        state.catch_up(10);
        state.choose_story("take", 12).unwrap();
        assert_eq!(state.next_raid_at, None);
    }

    #[test]
    fn interactive_raid_auto_finishes_without_blocking() {
        let mut cfg = test_config();
        cfg.raid.enabled = true;
        cfg.raid.first_delay_secs = 10;
        let mut state = GameState::with_config(0, cfg);
        state.fleet.add(ShipKind::LightFighter, 4);
        state.interactive_tactics = true;
        state.next_raid_at = Some(10);
        state.catch_up(10);
        assert!(state.pending_tactical.is_some());
        state.finish_pending_tactical();
        assert!(state.pending_tactical.is_none());
        assert!(state.combat_logs.last().is_some());
        assert!(state.admirals[0].xp > 0);
    }

    #[test]
    fn queued_retreat_ends_a_live_fight() {
        let mut cfg = test_config();
        cfg.raid.enabled = true;
        let mut state = GameState::with_config(0, cfg);
        state.fleet.add(ShipKind::LightFighter, 8);
        state.interactive_tactics = true;
        state.next_raid_at = Some(5);
        state.catch_up(5);
        assert!(state.pending_tactical.is_some());
        state.queue_tactical_action(RoundAction::Retreat);
        state.progress_tactical(6);
        assert!(state.pending_tactical.is_none());
        assert_eq!(
            state.combat_logs.last().unwrap().report.winner,
            CombatWinner::Draw
        );
    }

    #[test]
    fn history_time_travel_matches_recorded_snapshot() {
        let mut state = GameState::with_config(0, test_config());
        state.resources = Resources::from_parts(1_000.0, 1_000.0, 1_000.0);
        state.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        let queued = state
            .history
            .events
            .iter()
            .find(|e| e.action == "queue")
            .cloned()
            .expect("queued build");
        let snap = state
            .history
            .as_of(queued.at)
            .cloned()
            .expect("snapshot at queue");
        assert_eq!(snap.iron, state.resources.iron);
        assert_eq!(snap.event_id, queued.id);
        let done_at = state.build_queue[0].completes_at.expect("timer");
        state.catch_up(done_at);
        let complete = state
            .history
            .events
            .iter()
            .find(|e| e.action == "complete")
            .cloned()
            .expect("completed build");
        assert_eq!(complete.parent_ids, vec![queued.id]);
        let lineage = state.history.lineage(complete.id);
        assert_eq!(lineage.len(), 2);
        assert_eq!(lineage[0].id, queued.id);
        let past = state.history.as_of(queued.at).expect("past glance");
        assert_eq!(past.building_levels, snap.building_levels);
        assert_eq!(past.iron, snap.iron);
        let now = state.history.as_of(complete.at).expect("now glance");
        assert!(now.building_levels > past.building_levels);
        let hits = state.history.search(&crate::HistoryQuery {
            kind: Some(crate::HistoryKind::Build),
            object: Some("metal_mine".into()),
            ..crate::HistoryQuery::default()
        });
        assert!(hits.len() >= 2);
    }

    #[test]
    fn leader_permissions_delegation_and_spend_cap() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 50_000.0;
        let admiral = 1;
        let governor = 2;
        let denied = state.enqueue_build_as(
            Coord::HOME,
            BuildingKind::MetalMine,
            1_000,
            crate::Actor::Leader(admiral),
        );
        assert!(matches!(denied, Err(BuildError::PermissionDenied(_))));
        let still = state.enqueue_build_as(
            Coord::HOME,
            BuildingKind::MetalMine,
            1_000,
            crate::Actor::Leader(governor),
        );
        assert!(matches!(still, Err(BuildError::PermissionDenied(_))));
        assert!(
            state
                .delegate_leader(
                    governor,
                    crate::Permission::QueueBuild,
                    Some(f64::NAN),
                    1_000
                )
                .is_err()
        );
        assert!(
            state
                .delegate_leader(governor, crate::Permission::QueueBuild, Some(-1.0), 1_000)
                .is_err()
        );
        state
            .delegate_leader(governor, crate::Permission::QueueBuild, Some(80.0), 1_000)
            .unwrap();
        let capped = state.enqueue_build_as(
            Coord::HOME,
            BuildingKind::MetalMine,
            1_000,
            crate::Actor::Leader(governor),
        );
        assert!(
            matches!(capped, Err(BuildError::PermissionDenied(ref msg)) if msg.contains("spend cap")),
            "{capped:?}"
        );
        state
            .revoke_delegation(governor, Some(crate::Permission::QueueBuild), 1_000)
            .unwrap();
        state
            .delegate_leader(governor, crate::Permission::QueueBuild, None, 1_000)
            .unwrap();
        let broke = state.resources.iron;
        state.resources.iron = 0.0;
        let unaffordable = state.enqueue_build_as(
            Coord::HOME,
            BuildingKind::MetalMine,
            1_000,
            crate::Actor::Leader(governor),
        );
        assert!(matches!(
            unaffordable,
            Err(BuildError::InsufficientResources { .. })
        ));
        assert_eq!(
            state
                .delegations
                .iter()
                .find(|row| row.leader_id == governor)
                .map(|row| row.spent),
            Some(0.0)
        );
        state.resources.iron = broke;
        state
            .enqueue_build_as(
                Coord::HOME,
                BuildingKind::MetalMine,
                1_000,
                crate::Actor::Leader(governor),
            )
            .unwrap();
        assert_eq!(state.build_queue.len(), 1);
        state
            .enqueue_build(BuildingKind::CrystalMine, 1_000)
            .unwrap();
        assert_eq!(state.build_queue.len(), 2);
    }

    #[test]
    fn governor_trait_raises_output_inside_energy_budget() {
        let mut state = GameState::new(1_000);
        let baseline = state.production_per_hour(ResourceKind::Iron);
        if let Some(leader) = state.leaders.iter_mut().find(|row| row.id == 2) {
            leader.trait_id = "logistics".into();
        }
        let boosted = state.production_per_hour(ResourceKind::Iron);
        assert!(boosted > baseline);
        assert!(boosted / baseline < 1.05);
        state.assignments.retain(|row| row.leader_id != 2);
        let empty = state.production_per_hour(ResourceKind::Iron);
        assert!(empty < baseline);

        let mut governed = GameState::new(2_000);
        governed.resources.iron = 0.0;
        let mut vacant = GameState::new(2_000);
        vacant.resources.iron = 0.0;
        vacant.assignments.retain(|row| row.leader_id != 2);
        governed.catch_up(2_000 + 3_600);
        vacant.catch_up(2_000 + 3_600);
        assert!(governed.resources.iron > vacant.resources.iron);
    }

    #[test]
    fn reassignment_delay_and_retirement() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 5_000.0;
        state.resources.silicon = 5_000.0;
        let id = state.recruit_leader("Nara", "reformer", 1_000).unwrap();
        state
            .assign_leader(
                id,
                crate::RoleKind::Minister,
                crate::AssignmentTarget::Empire,
                1_000,
            )
            .unwrap();
        assert!(state.assignment_of(id).unwrap().ready(1_000));
        state
            .assign_leader(
                id,
                crate::RoleKind::Governor,
                crate::AssignmentTarget::Planet(Coord::HOME),
                1_000,
            )
            .unwrap();
        let ready_at = state.assignment_of(id).unwrap().ready_at;
        assert!(ready_at > 1_000);
        assert!(matches!(
            state.enqueue_build_as(
                Coord::HOME,
                BuildingKind::MetalMine,
                1_000,
                crate::Actor::Leader(id)
            ),
            Err(BuildError::PermissionDenied(_))
        ));
        if let Some(leader) = state.leaders.iter_mut().find(|row| row.id == 1) {
            leader.lifespan_secs = 10;
        }
        state.catch_up(1_020);
        assert!(state.leader(1).unwrap().retired);
        assert!(state.home_admiral_id.is_none());
    }

    #[test]
    fn minister_needs_delegation_to_propose() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 5_000.0;
        state.resources.silicon = 5_000.0;
        let id = state.recruit_leader("Iri", "reformer", 1_000).unwrap();
        state
            .assign_leader(
                id,
                crate::RoleKind::Minister,
                crate::AssignmentTarget::Empire,
                1_000,
            )
            .unwrap();
        let template = "trade_tariff".to_string();
        let denied = state.propose_law_as(
            &template,
            0.0,
            crate::Jurisdiction::Empire,
            0,
            1_000,
            crate::Actor::Leader(id),
        );
        assert!(matches!(denied, Err(crate::LawError::PermissionDenied(_))));
        state
            .delegate_leader(id, crate::Permission::ProposeLaw, None, 1_000)
            .unwrap();
        let param = state.config.laws.get(&template).unwrap().default;
        state
            .propose_law_as(
                &template,
                param,
                crate::Jurisdiction::Empire,
                0,
                1_000,
                crate::Actor::Leader(id),
            )
            .unwrap();
    }
}
