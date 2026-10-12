//! Data-driven production, energy, storage and construction. Numbers live in
//! `data/balance.toml`, not in the tick.

use crate::activity::{ActivityChannelConfig, ActivityEffectsConfig};
use crate::buildings::{BuildingKind, Buildings};
use crate::chains::{ChainConfig, FileChains};
use crate::espionage::{EspionageConfig, EspionageTier};
use crate::expedition::{ExpeditionConfig, ExpeditionWeights};
use crate::fleets::FlightConfig;
use crate::outposts::{OutpostConfig, RarePlanetBonus};
use crate::planets::{PlanetTypeBonus, PlanetTypeCatalog};
use crate::prestige::{PrestigeBonusSpec, PrestigeConfig};
use crate::research::{ModifierKind, TechPrereq};
use crate::ships::ShipKind;
use crate::state::Resources;
use crate::waste::WasteConfig;
use crate::wonders::{WonderModifier, WonderSpec, WonderStage};
use serde::Deserialize;
use std::fmt;
use std::path::Path;

const DEFAULT_BALANCE: &str = include_str!("../data/balance.toml");

/// A mine or synthesizer: hourly output and energy draw by level.
#[derive(Debug, Clone, PartialEq)]
pub struct ProducerSpec {
    /// Hourly output at level 1.
    pub base_per_hour: f64,
    /// Multiplier applied once per level above 1.
    pub growth: f64,
    /// Energy draw at level 1.
    pub base_energy: f64,
    /// Multiplier applied once per level above 1 for energy draw.
    pub energy_growth: f64,
}

impl ProducerSpec {
    pub fn production_per_hour(&self, level: u32) -> f64 {
        if level == 0 {
            0.0
        } else {
            self.base_per_hour * self.growth.powi(level as i32 - 1)
        }
    }

    pub fn energy_use(&self, level: u32) -> f64 {
        if level == 0 {
            0.0
        } else {
            self.base_energy * self.energy_growth.powi(level as i32 - 1)
        }
    }
}

/// An energy building (solar plant).
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyProducerSpec {
    /// Energy output at level 1.
    pub base_energy: f64,
    /// Multiplier applied once per level above 1.
    pub growth: f64,
}

impl EnergyProducerSpec {
    pub fn energy_output(&self, level: u32) -> f64 {
        if level == 0 {
            0.0
        } else {
            self.base_energy * self.growth.powi(level as i32 - 1)
        }
    }
}

/// Storage capacity by level. Level 0 still has a starter cap.
#[derive(Debug, Clone, PartialEq)]
pub struct StorageSpec {
    /// Capacity at level 0.
    pub base: f64,
    /// Multiplier applied once per storage level.
    pub growth: f64,
}

impl StorageSpec {
    pub fn capacity(&self, level: u32) -> f64 {
        self.base * self.growth.powi(level as i32)
    }
}

/// Cost and duration to raise a building from level L to L+1.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildingSpec {
    pub metal_base: f64,
    pub crystal_base: f64,
    pub deuterium_base: f64,
    /// Applied as `base * growth.powi(from_level)`.
    pub cost_growth: f64,
    pub base_seconds: f64,
    pub time_growth: f64,
    pub prerequisites: Vec<(BuildingKind, u32)>,
}

impl BuildingSpec {
    pub fn cost(&self, from_level: u32) -> Resources {
        let g = self.cost_growth.powi(from_level as i32);
        Resources {
            metal: self.metal_base * g,
            crystal: self.crystal_base * g,
            deuterium: self.deuterium_base * g,
        }
    }

    pub fn raw_seconds(&self, from_level: u32) -> f64 {
        self.base_seconds * self.time_growth.powi(from_level as i32)
    }
}

fn spec(
    metal: f64,
    crystal: f64,
    deuterium: f64,
    cost_growth: f64,
    base_seconds: f64,
    time_growth: f64,
    prerequisites: Vec<(BuildingKind, u32)>,
) -> BuildingSpec {
    BuildingSpec {
        metal_base: metal,
        crystal_base: crystal,
        deuterium_base: deuterium,
        cost_growth,
        base_seconds,
        time_growth,
        prerequisites,
    }
}

/// Per-building construction numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildingCatalog {
    pub metal_mine: BuildingSpec,
    pub crystal_mine: BuildingSpec,
    pub deuterium_synthesizer: BuildingSpec,
    pub solar_plant: BuildingSpec,
    pub metal_storage: BuildingSpec,
    pub crystal_storage: BuildingSpec,
    pub deuterium_storage: BuildingSpec,
    pub robotics_factory: BuildingSpec,
    pub research_lab: BuildingSpec,
    pub shipyard: BuildingSpec,
    pub radiator: BuildingSpec,
    pub scrubber: BuildingSpec,
}

impl BuildingCatalog {
    pub fn get(&self, kind: BuildingKind) -> &BuildingSpec {
        match kind {
            BuildingKind::MetalMine => &self.metal_mine,
            BuildingKind::CrystalMine => &self.crystal_mine,
            BuildingKind::DeuteriumSynthesizer => &self.deuterium_synthesizer,
            BuildingKind::SolarPlant => &self.solar_plant,
            BuildingKind::MetalStorage => &self.metal_storage,
            BuildingKind::CrystalStorage => &self.crystal_storage,
            BuildingKind::DeuteriumStorage => &self.deuterium_storage,
            BuildingKind::RoboticsFactory => &self.robotics_factory,
            BuildingKind::ResearchLab => &self.research_lab,
            BuildingKind::Shipyard => &self.shipyard,
            BuildingKind::Radiator => &self.radiator,
            BuildingKind::Scrubber => &self.scrubber,
        }
    }
}

impl Default for BuildingCatalog {
    fn default() -> Self {
        Self {
            metal_mine: spec(60.0, 15.0, 0.0, 1.5, 120.0, 1.5, vec![]),
            crystal_mine: spec(48.0, 24.0, 0.0, 1.6, 120.0, 1.6, vec![]),
            deuterium_synthesizer: spec(225.0, 75.0, 0.0, 1.5, 180.0, 1.5, vec![]),
            solar_plant: spec(75.0, 30.0, 0.0, 1.5, 120.0, 1.5, vec![]),
            metal_storage: spec(1_000.0, 0.0, 0.0, 2.0, 60.0, 2.0, vec![]),
            crystal_storage: spec(1_000.0, 500.0, 0.0, 2.0, 60.0, 2.0, vec![]),
            deuterium_storage: spec(1_000.0, 1_000.0, 0.0, 2.0, 60.0, 2.0, vec![]),
            robotics_factory: spec(400.0, 120.0, 200.0, 2.0, 300.0, 2.0, vec![]),
            research_lab: spec(200.0, 400.0, 200.0, 2.0, 300.0, 2.0, vec![]),
            shipyard: spec(
                400.0,
                200.0,
                100.0,
                2.0,
                300.0,
                2.0,
                vec![(BuildingKind::RoboticsFactory, 1)],
            ),
            radiator: spec(
                200.0,
                150.0,
                0.0,
                1.6,
                180.0,
                1.6,
                vec![(BuildingKind::SolarPlant, 1)],
            ),
            scrubber: spec(150.0, 200.0, 50.0, 1.6, 180.0, 1.6, vec![]),
        }
    }
}

/// Pirate raid schedule and combat knobs. Frequency and strength live here.
#[derive(Debug, Clone, PartialEq)]
pub struct RaidConfig {
    /// When false, no raid is scheduled and catch-up skips combat.
    pub enabled: bool,
    /// Seconds between announce and landing. Default 3600.
    pub min_warning_secs: u64,
    /// First raid lands this many seconds after a new game. Default 3600.
    pub first_delay_secs: u64,
    /// Delay from one landing to the next.
    pub interval_secs: u64,
    /// Pirate light-fighter count is `max(min_ships, floor(player_points * this))`.
    pub strength_factor: f64,
    pub min_ships: u32,
    /// Combat rounds cap. Later combat issues keep this resolver and add extras.
    pub max_rounds: u8,
    /// Fraction of stored resources taken on a lost raid, then clamped by `plunder_cap`.
    pub plunder_share: f64,
    pub plunder_cap: Resources,
}

impl RaidConfig {
    /// Production defaults: hourly warning, first landing after one hour, eight-hour gap.
    pub fn default_active() -> Self {
        Self {
            enabled: true,
            min_warning_secs: 3600,
            first_delay_secs: 3600,
            interval_secs: 8 * 3600,
            strength_factor: 0.4,
            min_ships: 1,
            max_rounds: 6,
            plunder_share: 0.5,
            plunder_cap: Resources {
                metal: 10_000.0,
                crystal: 10_000.0,
                deuterium: 5_000.0,
            },
        }
    }

    /// Tests that only exercise production or queues keep raids off.
    pub fn inactive() -> Self {
        Self {
            enabled: false,
            ..Self::default_active()
        }
    }
}

impl Default for RaidConfig {
    fn default() -> Self {
        Self::default_active()
    }
}

/// Documented pacing gates the simulator checks.
#[derive(Debug, Clone, PartialEq)]
pub struct PacingTargets {
    /// First building upgrade must complete within this many seconds.
    pub first_upgrade_secs: u64,
    /// Metal mine levels gained per simulated day under the greedy strategy.
    pub min_metal_mine_per_day: u32,
    /// `metal(with max PR-merge) / metal(activity off)` must stay at or below this.
    pub max_bridge_metal_ratio: f64,
    /// Each branch keystone should complete within this many seconds.
    pub keystone_secs: u64,
}

impl Default for PacingTargets {
    fn default() -> Self {
        Self {
            first_upgrade_secs: 300,
            min_metal_mine_per_day: 1,
            max_bridge_metal_ratio: 8.0,
            keystone_secs: 14 * 86_400,
        }
    }
}

/// Why a balance file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BalanceError {
    Io(String),
    Parse(String),
    Invalid(String),
}

impl fmt::Display for BalanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BalanceError::Io(e) => write!(f, "balance file: {e}"),
            BalanceError::Parse(e) => write!(f, "balance.toml: {e}"),
            BalanceError::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BalanceError {}

/// Tunable economy. [`GameState`](crate::GameState) reads these fields; it does not
/// hard-code production or cap numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct BalanceConfig {
    pub metal_mine: ProducerSpec,
    pub crystal_mine: ProducerSpec,
    pub deuterium_synthesizer: ProducerSpec,
    pub solar_plant: EnergyProducerSpec,
    pub metal_storage: StorageSpec,
    pub crystal_storage: StorageSpec,
    pub deuterium_storage: StorageSpec,
    pub buildings: BuildingCatalog,
    /// Maximum jobs waiting or in progress on one planet.
    pub max_build_queue: usize,
    /// Build time is divided by `1 + robotics_level * this`.
    pub robotics_speed_per_level: f64,
    pub techs: crate::TechTree,
    pub ships: crate::ShipCatalog,
    /// Maximum shipyard batches waiting or in progress on one planet.
    pub max_ship_queue: usize,
    pub flight: crate::FlightConfig,
    pub raid: RaidConfig,
    pub planet_types: crate::PlanetTypeCatalog,
    pub activity: crate::ActivityEffectsConfig,
    pub expedition: crate::ExpeditionConfig,
    pub starting_resources: Resources,
    pub starting_buildings: Buildings,
    pub pacing: PacingTargets,
    pub wonders: Vec<WonderSpec>,
    pub outposts: OutpostConfig,
    pub prestige: PrestigeConfig,
    pub chains: ChainConfig,
    pub story: crate::StoryCatalog,
    pub market: crate::MarketConfig,
    pub diplomacy: crate::DiplomacyConfig,
    pub espionage: EspionageConfig,
    pub defences: crate::DefenceConfig,
    pub waste: WasteConfig,
    pub laws: crate::LawConfig,
    pub achievements: crate::AchievementConfig,
    pub daily: crate::DailyConfig,
    pub moons: crate::MoonConfig,
}

impl BalanceConfig {
    /// Mine plus radiator and scrubber energy draw for a planet's buildings.
    pub fn energy_draw(&self, buildings: &Buildings) -> f64 {
        self.metal_mine.energy_use(buildings.metal_mine)
            + self.crystal_mine.energy_use(buildings.crystal_mine)
            + self
                .deuterium_synthesizer
                .energy_use(buildings.deuterium_synthesizer)
            + self.waste.mitigation_energy(buildings)
    }

    /// Mine energy only. Waste heat and pollution scale from this, not from radiators.
    pub fn industry_energy(&self, buildings: &Buildings) -> f64 {
        self.metal_mine.energy_use(buildings.metal_mine)
            + self.crystal_mine.energy_use(buildings.crystal_mine)
            + self
                .deuterium_synthesizer
                .energy_use(buildings.deuterium_synthesizer)
    }
}

impl BalanceConfig {
    /// Parse a full balance file. Research and ships stay on their bundled tables.
    pub fn parse(toml: &str) -> Result<Self, BalanceError> {
        let file: File = toml::from_str(toml).map_err(|e| BalanceError::Parse(e.to_string()))?;
        file.into_config()
    }

    /// Load and validate an override file. Missing path is an error.
    pub fn load_file(path: impl AsRef<Path>) -> Result<Self, BalanceError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| BalanceError::Io(format!("{}: {e}", path.display())))?;
        Self::parse(&text)
    }
}

impl Default for BalanceConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_BALANCE).expect("bundled balance.toml must be valid")
    }
}

#[derive(Deserialize)]
struct File {
    version: u32,
    pacing: FilePacing,
    metal_mine: FileProducer,
    crystal_mine: FileProducer,
    deuterium_synthesizer: FileProducer,
    solar_plant: FileEnergy,
    metal_storage: FileStorage,
    crystal_storage: FileStorage,
    deuterium_storage: FileStorage,
    buildings: FileBuildings,
    queues: FileQueues,
    starting_resources: FileResources,
    #[serde(default)]
    starting_buildings: FileStartingBuildings,
    raid: FileRaid,
    activity: FileActivity,
    expedition: FileExpedition,
    flight: FileFlight,
    planet_types: FilePlanetTypes,
    #[serde(default)]
    wonders: Vec<FileWonder>,
    #[serde(default)]
    outposts: FileOutposts,
    #[serde(default)]
    prestige: FilePrestige,
    #[serde(default)]
    chains: FileChains,
    #[serde(default)]
    market: FileMarket,
    #[serde(default)]
    espionage: FileEspionage,
    #[serde(default)]
    waste: Option<FileWaste>,
    #[serde(default)]
    daily: Option<FileDaily>,
    #[serde(default)]
    moons: Option<crate::moons::FileMoons>,
}

#[derive(Deserialize)]
struct FileDaily {
    #[serde(default = "default_day_secs")]
    day_secs: u64,
    #[serde(default)]
    reset_offset_secs: u64,
    #[serde(default = "default_daily_slots")]
    slots: u32,
    #[serde(default = "default_streak_cap")]
    streak_cap: u32,
    #[serde(default = "default_streak_metal")]
    streak_metal: f64,
    #[serde(default = "default_streak_crystal")]
    streak_crystal: f64,
    #[serde(default)]
    streak_deuterium: f64,
}

fn default_day_secs() -> u64 {
    86_400
}
fn default_daily_slots() -> u32 {
    3
}
fn default_streak_cap() -> u32 {
    7
}
fn default_streak_metal() -> f64 {
    80.0
}
fn default_streak_crystal() -> f64 {
    40.0
}

#[derive(Deserialize)]
struct FileMarket {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_market_tick")]
    tick_secs: u64,
    #[serde(default = "default_market_reversion")]
    reversion: f64,
    #[serde(default = "default_market_vol")]
    volatility: f64,
    #[serde(default = "default_market_shock_chance")]
    shock_chance: f64,
    #[serde(default = "default_market_shock_size")]
    shock_size: f64,
    #[serde(default = "default_market_fee")]
    fee: f64,
    #[serde(default = "default_market_slip")]
    slippage: f64,
    #[serde(default = "default_market_impact")]
    impact_unit: f64,
    #[serde(default = "default_market_min")]
    min_mult: f64,
    #[serde(default = "default_market_max")]
    max_mult: f64,
    #[serde(default = "default_market_hist")]
    history_len: u32,
    #[serde(default = "default_market_metal")]
    metal_mean: f64,
    #[serde(default = "default_market_crystal")]
    crystal_mean: f64,
    #[serde(default = "default_market_deut")]
    deuterium_mean: f64,
    #[serde(default = "default_market_cargo")]
    cargo_free: f64,
}

fn default_true() -> bool {
    true
}
fn default_market_tick() -> u64 {
    3600
}
fn default_market_reversion() -> f64 {
    0.15
}
fn default_market_vol() -> f64 {
    0.04
}
fn default_market_shock_chance() -> f64 {
    0.05
}
fn default_market_shock_size() -> f64 {
    0.12
}
fn default_market_fee() -> f64 {
    0.04
}
fn default_market_slip() -> f64 {
    0.002
}
fn default_market_impact() -> f64 {
    100.0
}
fn default_market_min() -> f64 {
    0.5
}
fn default_market_max() -> f64 {
    2.0
}
fn default_market_hist() -> u32 {
    24
}
fn default_market_metal() -> f64 {
    1.0
}
fn default_market_crystal() -> f64 {
    2.0
}
fn default_market_deut() -> f64 {
    3.0
}
fn default_market_cargo() -> f64 {
    50.0
}

impl Default for FileMarket {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            tick_secs: default_market_tick(),
            reversion: default_market_reversion(),
            volatility: default_market_vol(),
            shock_chance: default_market_shock_chance(),
            shock_size: default_market_shock_size(),
            fee: default_market_fee(),
            slippage: default_market_slip(),
            impact_unit: default_market_impact(),
            min_mult: default_market_min(),
            max_mult: default_market_max(),
            history_len: default_market_hist(),
            metal_mean: default_market_metal(),
            crystal_mean: default_market_crystal(),
            deuterium_mean: default_market_deut(),
            cargo_free: default_market_cargo(),
        }
    }
}

#[derive(Deserialize)]
struct FileEspionage {
    #[serde(default = "default_spy_target")]
    target_level: f64,
    #[serde(default = "default_spy_probe_weight")]
    probe_weight: f64,
    #[serde(default = "default_spy_level_weight")]
    level_weight: f64,
    #[serde(default = "default_spy_detect_base")]
    detect_base: f64,
    #[serde(default = "default_spy_detect_probe")]
    detect_per_probe: f64,
    #[serde(default = "default_spy_detect_level")]
    detect_per_level: f64,
    #[serde(default = "default_spy_destroy")]
    destroy_chance: f64,
    #[serde(default)]
    tiers: Vec<FileEspionageTier>,
}

#[derive(Deserialize)]
struct FileEspionageTier {
    id: String,
    min_score: f64,
}

#[derive(Deserialize)]
struct FileWaste {
    heat_per_energy: f64,
    pollution_per_energy: f64,
    rocky_heat_sink: f64,
    ice_heat_sink: f64,
    gas_giant_heat_sink: f64,
    rocky_pollution_sink: f64,
    ice_pollution_sink: f64,
    gas_giant_pollution_sink: f64,
    radiator_per_level: f64,
    radiator_base_energy: f64,
    radiator_energy_growth: f64,
    scrubber_per_level: f64,
    scrubber_base_energy: f64,
    scrubber_energy_growth: f64,
    warn_ratio: f64,
    unrest_per_overflow: f64,
    raid_signature_per_overflow: f64,
}

fn default_spy_target() -> f64 {
    1.0
}
fn default_spy_probe_weight() -> f64 {
    1.0
}
fn default_spy_level_weight() -> f64 {
    1.0
}
fn default_spy_detect_base() -> f64 {
    0.05
}
fn default_spy_detect_probe() -> f64 {
    0.08
}
fn default_spy_detect_level() -> f64 {
    0.12
}
fn default_spy_destroy() -> f64 {
    0.5
}

impl Default for FileEspionage {
    fn default() -> Self {
        Self {
            target_level: default_spy_target(),
            probe_weight: default_spy_probe_weight(),
            level_weight: default_spy_level_weight(),
            detect_base: default_spy_detect_base(),
            detect_per_probe: default_spy_detect_probe(),
            detect_per_level: default_spy_detect_level(),
            destroy_chance: default_spy_destroy(),
            tiers: Vec::new(),
        }
    }
}

impl FileEspionage {
    fn into_config(self) -> EspionageConfig {
        let tiers = if self.tiers.is_empty() {
            EspionageConfig::default().tiers
        } else {
            self.tiers
                .into_iter()
                .map(|row| EspionageTier {
                    id: row.id,
                    min_score: row.min_score,
                })
                .collect()
        };
        EspionageConfig {
            target_level: self.target_level,
            probe_weight: self.probe_weight,
            level_weight: self.level_weight,
            detect_base: self.detect_base,
            detect_per_probe: self.detect_per_probe,
            detect_per_level: self.detect_per_level,
            destroy_chance: self.destroy_chance,
            tiers,
        }
    }
}

#[derive(Deserialize)]
struct FileOutposts {
    #[serde(default = "default_outpost_count")]
    count: u32,
    #[serde(default = "default_outpost_min_ships")]
    min_ships: u32,
    #[serde(default = "default_outpost_strength")]
    strength_factor: f64,
    #[serde(default = "default_outpost_min_garrison")]
    min_garrison: u32,
    #[serde(default = "default_outpost_garrison_factor")]
    garrison_factor: f64,
    #[serde(default = "default_outpost_rebuild")]
    rebuild_secs: u64,
    #[serde(default = "default_outpost_variance")]
    variance: f64,
    #[serde(default = "default_outpost_troop_strength")]
    troop_strength: u32,
    #[serde(default = "default_outpost_rare_chance")]
    rare_chance: f64,
    #[serde(default)]
    rare: Vec<FileRarePlanet>,
}

fn default_outpost_count() -> u32 {
    8
}
fn default_outpost_min_ships() -> u32 {
    1
}
fn default_outpost_strength() -> f64 {
    0.4
}
fn default_outpost_min_garrison() -> u32 {
    2
}
fn default_outpost_garrison_factor() -> f64 {
    0.5
}
fn default_outpost_rebuild() -> u64 {
    86_400
}
fn default_outpost_variance() -> f64 {
    0.2
}
fn default_outpost_troop_strength() -> u32 {
    5
}
fn default_outpost_rare_chance() -> f64 {
    0.25
}

#[derive(Deserialize)]
struct FilePrestige {
    #[serde(default = "default_prestige_min_score")]
    min_score: u32,
    #[serde(default = "default_prestige_colony_weight")]
    colony_weight: u32,
    #[serde(default = "default_prestige_wonder_weight")]
    wonder_weight: u32,
    #[serde(default = "default_prestige_points")]
    points_per_currency: u32,
    #[serde(default)]
    bonuses: Vec<FilePrestigeBonus>,
}

fn default_prestige_min_score() -> u32 {
    80
}
fn default_prestige_colony_weight() -> u32 {
    8
}
fn default_prestige_wonder_weight() -> u32 {
    40
}
fn default_prestige_points() -> u32 {
    20
}

impl Default for FilePrestige {
    fn default() -> Self {
        Self {
            min_score: default_prestige_min_score(),
            colony_weight: default_prestige_colony_weight(),
            wonder_weight: default_prestige_wonder_weight(),
            points_per_currency: default_prestige_points(),
            bonuses: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
struct FilePrestigeBonus {
    id: String,
    name: String,
    cost: u32,
    #[serde(default)]
    starting_metal: f64,
    #[serde(default)]
    starting_crystal: f64,
    #[serde(default)]
    starting_deuterium: f64,
    #[serde(default)]
    production: f64,
    #[serde(default)]
    research: f64,
    #[serde(default)]
    queue_slots: u32,
}

impl Default for FileOutposts {
    fn default() -> Self {
        Self {
            count: default_outpost_count(),
            min_ships: default_outpost_min_ships(),
            strength_factor: default_outpost_strength(),
            min_garrison: default_outpost_min_garrison(),
            garrison_factor: default_outpost_garrison_factor(),
            rebuild_secs: default_outpost_rebuild(),
            variance: default_outpost_variance(),
            troop_strength: default_outpost_troop_strength(),
            rare_chance: default_outpost_rare_chance(),
            rare: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
struct FileRarePlanet {
    id: String,
    name: String,
    #[serde(default = "default_rare_weight")]
    weight: u32,
    #[serde(default)]
    metal: f64,
    #[serde(default)]
    crystal: f64,
    #[serde(default)]
    deuterium: f64,
    #[serde(default)]
    energy: f64,
    #[serde(default)]
    shipyard_speed: f64,
    #[serde(default)]
    fields: u8,
}

fn default_rare_weight() -> u32 {
    1
}

#[derive(Deserialize)]
struct FileWonder {
    id: String,
    name: String,
    #[serde(default)]
    prereq_techs: Vec<FileTechPrereq>,
    stages: Vec<FileWonderStage>,
    #[serde(default)]
    modifiers: Vec<FileWonderModifier>,
}

#[derive(Deserialize)]
struct FileTechPrereq {
    id: String,
    level: u32,
}

#[derive(Deserialize)]
struct FileWonderStage {
    metal: f64,
    crystal: f64,
    deuterium: f64,
    duration_secs: u64,
}

#[derive(Deserialize)]
struct FileWonderModifier {
    kind: ModifierKind,
    value: f64,
}

#[derive(Deserialize)]
struct FilePacing {
    first_upgrade_secs: u64,
    min_metal_mine_per_day: u32,
    max_bridge_metal_ratio: f64,
    #[serde(default = "default_keystone_secs")]
    keystone_secs: u64,
}

fn default_keystone_secs() -> u64 {
    14 * 86_400
}

#[derive(Deserialize)]
struct FileProducer {
    base_per_hour: f64,
    growth: f64,
    base_energy: f64,
    energy_growth: f64,
}

#[derive(Deserialize)]
struct FileEnergy {
    base_energy: f64,
    growth: f64,
}

#[derive(Deserialize)]
struct FileStorage {
    base: f64,
    growth: f64,
}

#[derive(Deserialize)]
struct FileBuildings {
    metal_mine: FileBuilding,
    crystal_mine: FileBuilding,
    deuterium_synthesizer: FileBuilding,
    solar_plant: FileBuilding,
    metal_storage: FileBuilding,
    crystal_storage: FileBuilding,
    deuterium_storage: FileBuilding,
    robotics_factory: FileBuilding,
    research_lab: FileBuilding,
    shipyard: FileBuilding,
    radiator: FileBuilding,
    scrubber: FileBuilding,
}

#[derive(Deserialize)]
struct FileBuilding {
    #[serde(alias = "frames")]
    metal_base: f64,
    #[serde(alias = "electronics")]
    crystal_base: f64,
    #[serde(alias = "fuel")]
    deuterium_base: f64,
    cost_growth: f64,
    base_seconds: f64,
    time_growth: f64,
    #[serde(default)]
    prerequisites: Vec<FilePrereq>,
}

#[derive(Deserialize)]
struct FilePrereq {
    building: String,
    level: u32,
}

#[derive(Deserialize)]
struct FileQueues {
    max_build_queue: usize,
    max_ship_queue: usize,
    robotics_speed_per_level: f64,
}

#[derive(Deserialize)]
struct FileResources {
    #[serde(alias = "frames")]
    metal: f64,
    #[serde(alias = "electronics")]
    crystal: f64,
    #[serde(alias = "fuel")]
    deuterium: f64,
}

#[derive(Deserialize, Default)]
struct FileStartingBuildings {
    #[serde(default)]
    metal_mine: u32,
    #[serde(default)]
    crystal_mine: u32,
    #[serde(default)]
    deuterium_synthesizer: u32,
    #[serde(default)]
    solar_plant: u32,
    #[serde(default)]
    metal_storage: u32,
    #[serde(default)]
    crystal_storage: u32,
    #[serde(default)]
    deuterium_storage: u32,
    #[serde(default)]
    robotics_factory: u32,
    #[serde(default)]
    research_lab: u32,
    #[serde(default)]
    shipyard: u32,
    #[serde(default)]
    radiator: u32,
    #[serde(default)]
    scrubber: u32,
}

#[derive(Deserialize)]
struct FileRaid {
    enabled: bool,
    min_warning_secs: u64,
    first_delay_secs: u64,
    interval_secs: u64,
    strength_factor: f64,
    min_ships: u32,
    max_rounds: u8,
    plunder_share: f64,
    plunder_cap: FileResources,
}

#[derive(Deserialize)]
struct FileActivity {
    enabled: bool,
    pr_merged: FileChannel,
    ci_green: FileChannel,
    ci_red: FileChannel,
    agent_long: FileChannel,
    agent_stuck: FileChannel,
}

#[derive(Deserialize)]
struct FileChannel {
    enabled: bool,
    cooldown_secs: u64,
    daily_cap: u32,
    #[serde(default)]
    payout_hours: f64,
    #[serde(default)]
    boost_factor: f64,
    #[serde(default)]
    boost_secs: u64,
    #[serde(default)]
    min_duration_secs: u64,
}

#[derive(Deserialize)]
struct FileExpedition {
    dest_position: u16,
    min_hold_secs: u64,
    max_hold_secs: u64,
    base_slots: u32,
    weights: FileWeights,
    astro_find_per_level: u32,
    hold_find_per_hour: u32,
    danger_points_unit: f64,
    pirate_weight_per_unit: u32,
    metal_per_point: f64,
    crystal_per_point: f64,
    deuterium_per_point: f64,
    ships_per_point: f64,
    ships_max: u32,
    ship_kind: String,
    pirate_per_point: f64,
    pirate_min: u32,
    delay_secs: u64,
}

#[derive(Deserialize)]
struct FileWeights {
    nothing: u32,
    resources: u32,
    ships: u32,
    delay: u32,
    early_return: u32,
    pirates: u32,
    loss: u32,
}

#[derive(Deserialize)]
struct FileFlight {
    galaxy_unit: f64,
    system_base: f64,
    system_unit: f64,
    position_unit: f64,
    min_distance: f64,
    fuel_distance: f64,
    base_fleet_slots: u32,
}

#[derive(Deserialize)]
struct FilePlanetTypes {
    rocky: FileBonus,
    ice: FileBonus,
    gas_giant: FileBonus,
}

#[derive(Deserialize)]
struct FileBonus {
    metal: f64,
    crystal: f64,
    deuterium: f64,
    energy: f64,
}

impl File {
    fn into_config(self) -> Result<BalanceConfig, BalanceError> {
        if self.version != 1 {
            return Err(BalanceError::Invalid(format!(
                "unsupported balance version {} (want 1)",
                self.version
            )));
        }
        let ship_kind = ShipKind::from_key(&self.expedition.ship_kind).ok_or_else(|| {
            BalanceError::Invalid(format!(
                "unknown expedition ship_kind {}",
                self.expedition.ship_kind
            ))
        })?;
        let mut config = BalanceConfig {
            metal_mine: self.metal_mine.into_spec(),
            crystal_mine: self.crystal_mine.into_spec(),
            deuterium_synthesizer: self.deuterium_synthesizer.into_spec(),
            solar_plant: EnergyProducerSpec {
                base_energy: self.solar_plant.base_energy,
                growth: self.solar_plant.growth,
            },
            metal_storage: self.metal_storage.into_spec(),
            crystal_storage: self.crystal_storage.into_spec(),
            deuterium_storage: self.deuterium_storage.into_spec(),
            buildings: BuildingCatalog {
                metal_mine: self.buildings.metal_mine.into_spec()?,
                crystal_mine: self.buildings.crystal_mine.into_spec()?,
                deuterium_synthesizer: self.buildings.deuterium_synthesizer.into_spec()?,
                solar_plant: self.buildings.solar_plant.into_spec()?,
                metal_storage: self.buildings.metal_storage.into_spec()?,
                crystal_storage: self.buildings.crystal_storage.into_spec()?,
                deuterium_storage: self.buildings.deuterium_storage.into_spec()?,
                robotics_factory: self.buildings.robotics_factory.into_spec()?,
                research_lab: self.buildings.research_lab.into_spec()?,
                shipyard: self.buildings.shipyard.into_spec()?,
                radiator: self.buildings.radiator.into_spec()?,
                scrubber: self.buildings.scrubber.into_spec()?,
            },
            max_build_queue: self.queues.max_build_queue,
            robotics_speed_per_level: self.queues.robotics_speed_per_level,
            techs: crate::TechTree::default(),
            ships: crate::ShipCatalog::default(),
            max_ship_queue: self.queues.max_ship_queue,
            flight: FlightConfig {
                galaxy_unit: self.flight.galaxy_unit,
                system_base: self.flight.system_base,
                system_unit: self.flight.system_unit,
                position_unit: self.flight.position_unit,
                min_distance: self.flight.min_distance,
                fuel_distance: self.flight.fuel_distance,
                base_fleet_slots: self.flight.base_fleet_slots,
            },
            raid: RaidConfig {
                enabled: self.raid.enabled,
                min_warning_secs: self.raid.min_warning_secs,
                first_delay_secs: self.raid.first_delay_secs,
                interval_secs: self.raid.interval_secs,
                strength_factor: self.raid.strength_factor,
                min_ships: self.raid.min_ships,
                max_rounds: self.raid.max_rounds,
                plunder_share: self.raid.plunder_share,
                plunder_cap: self.raid.plunder_cap.into_resources(),
            },
            planet_types: PlanetTypeCatalog {
                rocky: self.planet_types.rocky.into_bonus(),
                ice: self.planet_types.ice.into_bonus(),
                gas_giant: self.planet_types.gas_giant.into_bonus(),
            },
            activity: ActivityEffectsConfig {
                enabled: self.activity.enabled,
                pr_merged: self.activity.pr_merged.into_channel(),
                ci_green: self.activity.ci_green.into_channel(),
                ci_red: self.activity.ci_red.into_channel(),
                agent_long: self.activity.agent_long.into_channel(),
                agent_stuck: self.activity.agent_stuck.into_channel(),
            },
            expedition: ExpeditionConfig {
                dest_position: self.expedition.dest_position,
                min_hold_secs: self.expedition.min_hold_secs,
                max_hold_secs: self.expedition.max_hold_secs,
                base_slots: self.expedition.base_slots,
                weights: ExpeditionWeights {
                    nothing: self.expedition.weights.nothing,
                    resources: self.expedition.weights.resources,
                    ships: self.expedition.weights.ships,
                    delay: self.expedition.weights.delay,
                    early_return: self.expedition.weights.early_return,
                    pirates: self.expedition.weights.pirates,
                    loss: self.expedition.weights.loss,
                },
                astro_find_per_level: self.expedition.astro_find_per_level,
                hold_find_per_hour: self.expedition.hold_find_per_hour,
                danger_points_unit: self.expedition.danger_points_unit,
                pirate_weight_per_unit: self.expedition.pirate_weight_per_unit,
                metal_per_point: self.expedition.metal_per_point,
                crystal_per_point: self.expedition.crystal_per_point,
                deuterium_per_point: self.expedition.deuterium_per_point,
                ships_per_point: self.expedition.ships_per_point,
                ships_max: self.expedition.ships_max,
                ship_kind,
                pirate_per_point: self.expedition.pirate_per_point,
                pirate_min: self.expedition.pirate_min,
                delay_secs: self.expedition.delay_secs,
            },
            starting_resources: self.starting_resources.into_resources(),
            starting_buildings: Buildings {
                metal_mine: self.starting_buildings.metal_mine,
                crystal_mine: self.starting_buildings.crystal_mine,
                deuterium_synthesizer: self.starting_buildings.deuterium_synthesizer,
                solar_plant: self.starting_buildings.solar_plant,
                metal_storage: self.starting_buildings.metal_storage,
                crystal_storage: self.starting_buildings.crystal_storage,
                deuterium_storage: self.starting_buildings.deuterium_storage,
                robotics_factory: self.starting_buildings.robotics_factory,
                research_lab: self.starting_buildings.research_lab,
                shipyard: self.starting_buildings.shipyard,
                radiator: self.starting_buildings.radiator,
                scrubber: self.starting_buildings.scrubber,
            },
            pacing: PacingTargets {
                first_upgrade_secs: self.pacing.first_upgrade_secs,
                min_metal_mine_per_day: self.pacing.min_metal_mine_per_day,
                max_bridge_metal_ratio: self.pacing.max_bridge_metal_ratio,
                keystone_secs: self.pacing.keystone_secs,
            },
            outposts: {
                let rare = if self.outposts.rare.is_empty() {
                    OutpostConfig::default().rare
                } else {
                    self.outposts
                        .rare
                        .into_iter()
                        .map(|row| RarePlanetBonus {
                            id: row.id,
                            name: row.name,
                            weight: row.weight,
                            metal: row.metal,
                            crystal: row.crystal,
                            deuterium: row.deuterium,
                            energy: row.energy,
                            shipyard_speed: row.shipyard_speed,
                            fields: row.fields,
                        })
                        .collect()
                };
                OutpostConfig {
                    count: self.outposts.count,
                    min_ships: self.outposts.min_ships,
                    strength_factor: self.outposts.strength_factor,
                    min_garrison: self.outposts.min_garrison,
                    garrison_factor: self.outposts.garrison_factor,
                    rebuild_secs: self.outposts.rebuild_secs,
                    variance: self.outposts.variance,
                    troop_strength: self.outposts.troop_strength,
                    rare_chance: self.outposts.rare_chance,
                    rare,
                }
            },
            wonders: self
                .wonders
                .into_iter()
                .map(|w| WonderSpec {
                    id: w.id,
                    name: w.name,
                    prereq_techs: w
                        .prereq_techs
                        .into_iter()
                        .map(|p| TechPrereq {
                            id: p.id,
                            level: p.level,
                        })
                        .collect(),
                    stages: w
                        .stages
                        .into_iter()
                        .map(|s| WonderStage {
                            cost: Resources {
                                metal: s.metal,
                                crystal: s.crystal,
                                deuterium: s.deuterium,
                            },
                            duration_secs: s.duration_secs,
                        })
                        .collect(),
                    modifiers: w
                        .modifiers
                        .into_iter()
                        .map(|m| WonderModifier {
                            kind: m.kind,
                            value: m.value,
                        })
                        .collect(),
                })
                .collect(),
            prestige: {
                let bonuses = if self.prestige.bonuses.is_empty() {
                    PrestigeConfig::default().bonuses
                } else {
                    self.prestige
                        .bonuses
                        .into_iter()
                        .map(|row| PrestigeBonusSpec {
                            id: row.id,
                            name: row.name,
                            cost: row.cost,
                            starting_metal: row.starting_metal,
                            starting_crystal: row.starting_crystal,
                            starting_deuterium: row.starting_deuterium,
                            production: row.production,
                            research: row.research,
                            queue_slots: row.queue_slots,
                        })
                        .collect()
                };
                PrestigeConfig {
                    min_score: self.prestige.min_score,
                    colony_weight: self.prestige.colony_weight,
                    wonder_weight: self.prestige.wonder_weight,
                    points_per_currency: self.prestige.points_per_currency,
                    bonuses,
                }
            },
            chains: self.chains.into_config().map_err(BalanceError::Invalid)?,
            story: crate::StoryCatalog::default(),
            diplomacy: crate::DiplomacyConfig::default(),
            espionage: self.espionage.into_config(),
            defences: crate::DefenceConfig::default(),
            waste: self.waste.map(FileWaste::into_config).unwrap_or_default(),
            laws: crate::LawConfig::default(),
            achievements: crate::AchievementConfig::default(),
            daily: {
                let mut daily = crate::DailyConfig::default();
                if let Some(file) = self.daily {
                    daily.day_secs = file.day_secs.max(1);
                    daily.reset_offset_secs = file.reset_offset_secs;
                    daily.slots = file.slots.max(1);
                    daily.streak_cap = file.streak_cap.max(1);
                    daily.streak_reward = Resources {
                        metal: file.streak_metal,
                        crystal: file.streak_crystal,
                        deuterium: file.streak_deuterium,
                    };
                }
                daily
            },
            moons: self
                .moons
                .map(crate::moons::FileMoons::into_config)
                .unwrap_or_default(),
            market: crate::MarketConfig {
                enabled: self.market.enabled,
                tick_secs: self.market.tick_secs,
                reversion: self.market.reversion,
                volatility: self.market.volatility,
                shock_chance: self.market.shock_chance,
                shock_size: self.market.shock_size,
                fee: self.market.fee,
                slippage: self.market.slippage,
                impact_unit: self.market.impact_unit,
                min_mult: self.market.min_mult,
                max_mult: self.market.max_mult,
                history_len: self.market.history_len.max(1) as usize,
                metal_mean: self.market.metal_mean,
                crystal_mean: self.market.crystal_mean,
                deuterium_mean: self.market.deuterium_mean,
                cargo_free: self.market.cargo_free,
            },
        };
        let wonders = config.wonders.clone();
        config.achievements.drop_unknown_wonders(&wonders);
        config.validate()?;
        Ok(config)
    }
}

impl FileProducer {
    fn into_spec(self) -> ProducerSpec {
        ProducerSpec {
            base_per_hour: self.base_per_hour,
            growth: self.growth,
            base_energy: self.base_energy,
            energy_growth: self.energy_growth,
        }
    }
}

impl FileStorage {
    fn into_spec(self) -> StorageSpec {
        StorageSpec {
            base: self.base,
            growth: self.growth,
        }
    }
}

impl FileBuilding {
    fn into_spec(self) -> Result<BuildingSpec, BalanceError> {
        let mut prerequisites = Vec::with_capacity(self.prerequisites.len());
        for pre in self.prerequisites {
            let building = BuildingKind::from_key(&pre.building).ok_or_else(|| {
                BalanceError::Invalid(format!("unknown building {}", pre.building))
            })?;
            prerequisites.push((building, pre.level));
        }
        Ok(BuildingSpec {
            metal_base: self.metal_base,
            crystal_base: self.crystal_base,
            deuterium_base: self.deuterium_base,
            cost_growth: self.cost_growth,
            base_seconds: self.base_seconds,
            time_growth: self.time_growth,
            prerequisites,
        })
    }
}

impl FileResources {
    fn into_resources(self) -> Resources {
        Resources {
            metal: self.metal,
            crystal: self.crystal,
            deuterium: self.deuterium,
        }
    }
}

impl FileWaste {
    fn into_config(self) -> WasteConfig {
        WasteConfig {
            heat_per_energy: self.heat_per_energy,
            pollution_per_energy: self.pollution_per_energy,
            rocky_heat_sink: self.rocky_heat_sink,
            ice_heat_sink: self.ice_heat_sink,
            gas_giant_heat_sink: self.gas_giant_heat_sink,
            rocky_pollution_sink: self.rocky_pollution_sink,
            ice_pollution_sink: self.ice_pollution_sink,
            gas_giant_pollution_sink: self.gas_giant_pollution_sink,
            radiator_per_level: self.radiator_per_level,
            radiator_base_energy: self.radiator_base_energy,
            radiator_energy_growth: self.radiator_energy_growth,
            scrubber_per_level: self.scrubber_per_level,
            scrubber_base_energy: self.scrubber_base_energy,
            scrubber_energy_growth: self.scrubber_energy_growth,
            warn_ratio: self.warn_ratio,
            unrest_per_overflow: self.unrest_per_overflow,
            raid_signature_per_overflow: self.raid_signature_per_overflow,
        }
    }
}

impl FileBonus {
    fn into_bonus(self) -> PlanetTypeBonus {
        PlanetTypeBonus {
            metal: self.metal,
            crystal: self.crystal,
            deuterium: self.deuterium,
            energy: self.energy,
        }
    }
}

impl FileChannel {
    fn into_channel(self) -> ActivityChannelConfig {
        ActivityChannelConfig {
            enabled: self.enabled,
            cooldown_secs: self.cooldown_secs,
            daily_cap: self.daily_cap,
            payout_hours: self.payout_hours,
            boost_factor: self.boost_factor,
            boost_secs: self.boost_secs,
            min_duration_secs: self.min_duration_secs,
        }
    }
}

impl BalanceConfig {
    fn validate(&self) -> Result<(), BalanceError> {
        if self.pacing.first_upgrade_secs == 0 {
            return Err(BalanceError::Invalid(
                "pacing.first_upgrade_secs must be > 0".into(),
            ));
        }
        if self.pacing.min_metal_mine_per_day == 0 {
            return Err(BalanceError::Invalid(
                "pacing.min_metal_mine_per_day must be > 0".into(),
            ));
        }
        finite_positive(
            "pacing.max_bridge_metal_ratio",
            self.pacing.max_bridge_metal_ratio,
        )?;
        if self.pacing.max_bridge_metal_ratio < 1.0 {
            return Err(BalanceError::Invalid(
                "pacing.max_bridge_metal_ratio must be >= 1".into(),
            ));
        }
        if self.max_build_queue < 1 {
            return Err(BalanceError::Invalid(
                "queues.max_build_queue must be >= 1".into(),
            ));
        }
        if self.max_ship_queue < 1 {
            return Err(BalanceError::Invalid(
                "queues.max_ship_queue must be >= 1".into(),
            ));
        }
        if self.daily.day_secs < 1 {
            return Err(BalanceError::Invalid("daily.day_secs must be >= 1".into()));
        }
        if self.daily.slots < 1 {
            return Err(BalanceError::Invalid("daily.slots must be >= 1".into()));
        }
        if self.daily.streak_cap < 1 {
            return Err(BalanceError::Invalid(
                "daily.streak_cap must be >= 1".into(),
            ));
        }
        nonneg_finite("daily.streak_metal", self.daily.streak_reward.metal)?;
        nonneg_finite("daily.streak_crystal", self.daily.streak_reward.crystal)?;
        nonneg_finite("daily.streak_deuterium", self.daily.streak_reward.deuterium)?;
        check_producer("metal_mine", &self.metal_mine)?;
        check_producer("crystal_mine", &self.crystal_mine)?;
        check_producer("deuterium_synthesizer", &self.deuterium_synthesizer)?;
        check_energy("solar_plant", &self.solar_plant)?;
        check_storage("metal_storage", &self.metal_storage)?;
        check_storage("crystal_storage", &self.crystal_storage)?;
        check_storage("deuterium_storage", &self.deuterium_storage)?;
        for kind in BuildingKind::ALL {
            check_building(kind.as_key(), self.buildings.get(kind))?;
        }
        nonneg_finite(
            "queues.robotics_speed_per_level",
            self.robotics_speed_per_level,
        )?;
        nonneg_finite("starting_resources.metal", self.starting_resources.metal)?;
        nonneg_finite(
            "starting_resources.crystal",
            self.starting_resources.crystal,
        )?;
        nonneg_finite(
            "starting_resources.deuterium",
            self.starting_resources.deuterium,
        )?;
        self.chains.validate().map_err(BalanceError::Invalid)?;
        let mut seen = std::collections::HashSet::new();
        for spec in &self.wonders {
            if spec.id.is_empty() {
                return Err(BalanceError::Invalid("wonder id must not be empty".into()));
            }
            if !seen.insert(&spec.id) {
                return Err(BalanceError::Invalid(format!(
                    "duplicate wonder id {}",
                    spec.id
                )));
            }
            if spec.stages.is_empty() {
                return Err(BalanceError::Invalid(format!(
                    "wonder {} needs at least one stage",
                    spec.id
                )));
            }
            for (i, stage) in spec.stages.iter().enumerate() {
                if stage.duration_secs == 0 {
                    return Err(BalanceError::Invalid(format!(
                        "wonder {} stage {i} duration_secs must be > 0",
                        spec.id
                    )));
                }
                nonneg_finite(
                    &format!("wonder {} stage {i} metal", spec.id),
                    stage.cost.metal,
                )?;
                nonneg_finite(
                    &format!("wonder {} stage {i} crystal", spec.id),
                    stage.cost.crystal,
                )?;
                nonneg_finite(
                    &format!("wonder {} stage {i} deuterium", spec.id),
                    stage.cost.deuterium,
                )?;
                if stage.cost.metal <= 0.0
                    && stage.cost.crystal <= 0.0
                    && stage.cost.deuterium <= 0.0
                {
                    return Err(BalanceError::Invalid(format!(
                        "wonder {} stage {i} needs a positive cost",
                        spec.id
                    )));
                }
            }
            for prereq in &spec.prereq_techs {
                if self.techs.get(&prereq.id).is_none() {
                    return Err(BalanceError::Invalid(format!(
                        "wonder {} unknown tech {}",
                        spec.id, prereq.id
                    )));
                }
            }
            for modifier in &spec.modifiers {
                if !modifier.value.is_finite() {
                    return Err(BalanceError::Invalid(format!(
                        "wonder {} modifier must be finite",
                        spec.id
                    )));
                }
            }
        }
        if self.outposts.count < 1 {
            return Err(BalanceError::Invalid("outposts.count must be >= 1".into()));
        }
        if self.outposts.min_ships < 1 {
            return Err(BalanceError::Invalid(
                "outposts.min_ships must be >= 1".into(),
            ));
        }
        if self.outposts.min_garrison < 1 {
            return Err(BalanceError::Invalid(
                "outposts.min_garrison must be >= 1".into(),
            ));
        }
        if self.outposts.rebuild_secs < 1 {
            return Err(BalanceError::Invalid(
                "outposts.rebuild_secs must be >= 1".into(),
            ));
        }
        if self.outposts.troop_strength < 1 {
            return Err(BalanceError::Invalid(
                "outposts.troop_strength must be >= 1".into(),
            ));
        }
        nonneg_finite("outposts.strength_factor", self.outposts.strength_factor)?;
        nonneg_finite("outposts.garrison_factor", self.outposts.garrison_factor)?;
        if self.prestige.min_score < 1 {
            return Err(BalanceError::Invalid(
                "prestige.min_score must be >= 1".into(),
            ));
        }
        if self.prestige.points_per_currency < 1 {
            return Err(BalanceError::Invalid(
                "prestige.points_per_currency must be >= 1".into(),
            ));
        }
        let mut seen_bonus = std::collections::HashSet::new();
        if self.prestige.bonuses.is_empty() {
            return Err(BalanceError::Invalid(
                "prestige needs at least one bonus".into(),
            ));
        }
        for spec in &self.prestige.bonuses {
            if spec.id.is_empty() {
                return Err(BalanceError::Invalid(
                    "prestige bonus id must not be empty".into(),
                ));
            }
            if !seen_bonus.insert(&spec.id) {
                return Err(BalanceError::Invalid(format!(
                    "duplicate prestige bonus id {}",
                    spec.id
                )));
            }
            if spec.cost < 1 {
                return Err(BalanceError::Invalid(format!(
                    "prestige bonus {} cost must be >= 1",
                    spec.id
                )));
            }
            nonneg_finite(
                &format!("prestige.{} starting_metal", spec.id),
                spec.starting_metal,
            )?;
            nonneg_finite(
                &format!("prestige.{} starting_crystal", spec.id),
                spec.starting_crystal,
            )?;
            nonneg_finite(
                &format!("prestige.{} starting_deuterium", spec.id),
                spec.starting_deuterium,
            )?;
            nonneg_finite(&format!("prestige.{} production", spec.id), spec.production)?;
            nonneg_finite(&format!("prestige.{} research", spec.id), spec.research)?;
            if spec.production <= 0.0
                && spec.research <= 0.0
                && spec.queue_slots == 0
                && spec.starting_metal <= 0.0
                && spec.starting_crystal <= 0.0
                && spec.starting_deuterium <= 0.0
            {
                return Err(BalanceError::Invalid(format!(
                    "prestige bonus {} needs an effect",
                    spec.id
                )));
            }
        }
        nonneg_finite("outposts.variance", self.outposts.variance)?;
        if self.outposts.variance > 1.0 {
            return Err(BalanceError::Invalid(
                "outposts.variance must be <= 1".into(),
            ));
        }
        nonneg_finite("outposts.rare_chance", self.outposts.rare_chance)?;
        if self.outposts.rare_chance > 1.0 {
            return Err(BalanceError::Invalid(
                "outposts.rare_chance must be <= 1".into(),
            ));
        }
        let mut rare_ids = std::collections::HashSet::new();
        for row in &self.outposts.rare {
            if row.id.is_empty() {
                return Err(BalanceError::Invalid(
                    "outposts.rare id must not be empty".into(),
                ));
            }
            if !rare_ids.insert(&row.id) {
                return Err(BalanceError::Invalid(format!(
                    "duplicate rare planet id {}",
                    row.id
                )));
            }
            nonneg_finite(&format!("outposts.rare {}.metal", row.id), row.metal)?;
            nonneg_finite(&format!("outposts.rare {}.crystal", row.id), row.crystal)?;
            nonneg_finite(
                &format!("outposts.rare {}.deuterium", row.id),
                row.deuterium,
            )?;
            nonneg_finite(&format!("outposts.rare {}.energy", row.id), row.energy)?;
            nonneg_finite(
                &format!("outposts.rare {}.shipyard_speed", row.id),
                row.shipyard_speed,
            )?;
        }
        if self.market.enabled {
            if self.market.tick_secs < 1 {
                return Err(BalanceError::Invalid(
                    "market.tick_secs must be >= 1".into(),
                ));
            }
            if self.market.history_len < 1 {
                return Err(BalanceError::Invalid(
                    "market.history_len must be >= 1".into(),
                ));
            }
            finite_positive("market.reversion", self.market.reversion)?;
            nonneg_finite("market.volatility", self.market.volatility)?;
            nonneg_finite("market.shock_chance", self.market.shock_chance)?;
            if self.market.shock_chance > 1.0 {
                return Err(BalanceError::Invalid(
                    "market.shock_chance must be <= 1".into(),
                ));
            }
            nonneg_finite("market.shock_size", self.market.shock_size)?;
            nonneg_finite("market.fee", self.market.fee)?;
            if self.market.fee >= 1.0 {
                return Err(BalanceError::Invalid("market.fee must be < 1".into()));
            }
            nonneg_finite("market.slippage", self.market.slippage)?;
            finite_positive("market.impact_unit", self.market.impact_unit)?;
            finite_positive("market.min_mult", self.market.min_mult)?;
            finite_positive("market.max_mult", self.market.max_mult)?;
            if self.market.max_mult < self.market.min_mult {
                return Err(BalanceError::Invalid(
                    "market.max_mult must be >= min_mult".into(),
                ));
            }
            finite_positive("market.metal_mean", self.market.metal_mean)?;
            finite_positive("market.crystal_mean", self.market.crystal_mean)?;
            finite_positive("market.deuterium_mean", self.market.deuterium_mean)?;
            nonneg_finite("market.cargo_free", self.market.cargo_free)?;
        }
        if !self.espionage.target_level.is_finite() {
            return Err(BalanceError::Invalid(
                "espionage.target_level must be finite".into(),
            ));
        }
        finite_positive("espionage.probe_weight", self.espionage.probe_weight)?;
        if !self.espionage.level_weight.is_finite() {
            return Err(BalanceError::Invalid(
                "espionage.level_weight must be finite".into(),
            ));
        }
        nonneg_finite("espionage.detect_base", self.espionage.detect_base)?;
        nonneg_finite(
            "espionage.detect_per_probe",
            self.espionage.detect_per_probe,
        )?;
        nonneg_finite(
            "espionage.detect_per_level",
            self.espionage.detect_per_level,
        )?;
        nonneg_finite("espionage.destroy_chance", self.espionage.destroy_chance)?;
        if self.espionage.destroy_chance > 1.0 {
            return Err(BalanceError::Invalid(
                "espionage.destroy_chance must be <= 1".into(),
            ));
        }
        // Empty `[[espionage.tiers]]` uses the built-in activity..garrison bands.
        let mut tier_ids = std::collections::HashSet::new();
        for tier in &self.espionage.tiers {
            if tier.id.is_empty() {
                return Err(BalanceError::Invalid(
                    "espionage.tiers id must not be empty".into(),
                ));
            }
            if !tier_ids.insert(tier.id.as_str()) {
                return Err(BalanceError::Invalid(format!(
                    "duplicate espionage tier {}",
                    tier.id
                )));
            }
            if !tier.min_score.is_finite() {
                return Err(BalanceError::Invalid(format!(
                    "espionage.tiers {} min_score must be finite",
                    tier.id
                )));
            }
        }
        self.waste.validate().map_err(BalanceError::Invalid)?;
        Ok(())
    }
}

fn check_producer(name: &str, spec: &ProducerSpec) -> Result<(), BalanceError> {
    nonneg_finite(&format!("{name}.base_per_hour"), spec.base_per_hour)?;
    growth(&format!("{name}.growth"), spec.growth)?;
    nonneg_finite(&format!("{name}.base_energy"), spec.base_energy)?;
    growth(&format!("{name}.energy_growth"), spec.energy_growth)?;
    Ok(())
}

fn check_energy(name: &str, spec: &EnergyProducerSpec) -> Result<(), BalanceError> {
    nonneg_finite(&format!("{name}.base_energy"), spec.base_energy)?;
    growth(&format!("{name}.growth"), spec.growth)?;
    Ok(())
}

fn check_storage(name: &str, spec: &StorageSpec) -> Result<(), BalanceError> {
    nonneg_finite(&format!("{name}.base"), spec.base)?;
    growth(&format!("{name}.growth"), spec.growth)?;
    Ok(())
}

fn check_building(name: &str, spec: &BuildingSpec) -> Result<(), BalanceError> {
    nonneg_finite(&format!("buildings.{name}.metal_base"), spec.metal_base)?;
    nonneg_finite(&format!("buildings.{name}.crystal_base"), spec.crystal_base)?;
    nonneg_finite(
        &format!("buildings.{name}.deuterium_base"),
        spec.deuterium_base,
    )?;
    growth(&format!("buildings.{name}.cost_growth"), spec.cost_growth)?;
    nonneg_finite(&format!("buildings.{name}.base_seconds"), spec.base_seconds)?;
    growth(&format!("buildings.{name}.time_growth"), spec.time_growth)?;
    Ok(())
}

fn growth(field: &str, value: f64) -> Result<(), BalanceError> {
    finite_positive(field, value)?;
    Ok(())
}

fn finite_positive(field: &str, value: f64) -> Result<(), BalanceError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(BalanceError::Invalid(format!("{field} must be > 0")));
    }
    Ok(())
}

fn nonneg_finite(field: &str, value: f64) -> Result<(), BalanceError> {
    if !value.is_finite() || value < 0.0 {
        return Err(BalanceError::Invalid(format!("{field} must be >= 0")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn bundled_file_matches_previous_defaults() {
        let cfg = BalanceConfig::default();
        assert_eq!(cfg.metal_mine.base_per_hour, 30.0);
        assert_eq!(cfg.metal_mine.growth, 1.5);
        assert_eq!(cfg.crystal_mine.base_per_hour, 20.0);
        assert_eq!(cfg.buildings, BuildingCatalog::default());
        assert_eq!(cfg.max_build_queue, 5);
        assert_eq!(cfg.starting_resources.metal, 500.0);
        assert_eq!(cfg.starting_buildings.metal_mine, 1);
        assert_eq!(cfg.starting_buildings.solar_plant, 1);
        assert_eq!(cfg.raid, RaidConfig::default_active());
        assert_eq!(cfg.activity, ActivityEffectsConfig::default());
        assert_eq!(cfg.expedition, crate::ExpeditionConfig::default());
        assert_eq!(cfg.flight, crate::FlightConfig::default());
        assert_eq!(cfg.planet_types, crate::PlanetTypeCatalog::default());
        assert_eq!(cfg.pacing.first_upgrade_secs, 300);
        assert_eq!(
            cfg.buildings.shipyard.prerequisites,
            vec![(BuildingKind::RoboticsFactory, 1)]
        );
        assert_eq!(cfg.outposts, crate::OutpostConfig::default());
        assert_eq!(cfg.prestige, crate::PrestigeConfig::default());
        assert!(cfg.chains.enabled());
        assert!(cfg.chains.validate().is_ok());
        assert_eq!(cfg.market, crate::MarketConfig::default_active());
        assert_eq!(cfg.diplomacy, crate::DiplomacyConfig::default());
        assert_eq!(cfg.wonders.len(), 3);
        assert_eq!(cfg.wonders[0].id, "dyson_ring");
        assert_eq!(cfg.wonders[0].prereq_techs[0].id, "energy");
        assert_eq!(cfg.wonders[0].prereq_techs[0].level, 8);
    }

    #[test]
    fn rejects_duplicate_wonder_id() {
        let text = format!(
            "{DEFAULT_BALANCE}
[[wonders]]
id = \"dyson_ring\"
name = \"Dup\"
stages = [{{ metal = 1.0, crystal = 0.0, deuterium = 0.0, duration_secs = 1 }}]
"
        );
        let err = BalanceConfig::parse(&text).unwrap_err();
        assert!(err.to_string().contains("duplicate wonder"), "{err}");
    }

    #[test]
    fn rejects_zero_cost_wonder_stage() {
        let text = format!(
            "{DEFAULT_BALANCE}
[[wonders]]
id = \"free\"
name = \"Free\"
stages = [{{ metal = 0.0, crystal = 0.0, deuterium = 0.0, duration_secs = 1 }}]
"
        );
        let err = BalanceConfig::parse(&text).unwrap_err();
        assert!(err.to_string().contains("positive cost"), "{err}");
    }

    #[test]
    fn rejects_wonder_with_empty_stages() {
        let text = format!(
            "{DEFAULT_BALANCE}
[[wonders]]
id = \"hollow\"
name = \"Hollow\"
stages = []
"
        );
        let err = BalanceConfig::parse(&text).unwrap_err();
        assert!(err.to_string().contains("at least one stage"), "{err}");
    }

    #[test]
    fn override_may_replace_bundled_wonder_ids() {
        let text = DEFAULT_BALANCE.replace("id = \"dyson_ring\"", "id = \"hearth_ring\"");
        let cfg = BalanceConfig::parse(&text).unwrap();
        assert!(cfg.wonders.iter().any(|w| w.id == "hearth_ring"));
        assert!(!cfg.wonders.iter().any(|w| w.id == "dyson_ring"));
        assert!(cfg.achievements.get("dyson").is_none());
        assert!(cfg.achievements.get("wonder_done").is_some());
    }

    #[test]
    fn override_file_changes_metal_rate() {
        let text = DEFAULT_BALANCE.replacen("base_per_hour = 30.0", "base_per_hour = 99.0", 1);
        let cfg = BalanceConfig::parse(&text).unwrap();
        assert_eq!(cfg.metal_mine.base_per_hour, 99.0);
        assert_eq!(cfg.crystal_mine.base_per_hour, 20.0);
    }

    #[test]
    fn load_file_reads_an_override_from_disk() {
        let dir = std::env::temp_dir().join(format!(
            "hoshi-balance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("balance.toml");
        let text = DEFAULT_BALANCE.replacen("base_per_hour = 30.0", "base_per_hour = 77.0", 1);
        std::fs::write(&path, text).unwrap();
        let cfg = BalanceConfig::load_file(&path).unwrap();
        assert_eq!(cfg.metal_mine.base_per_hour, 77.0);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn rejects_unsupported_version() {
        let text = DEFAULT_BALANCE.replacen("version = 1", "version = 2", 1);
        let err = BalanceConfig::parse(&text).unwrap_err();
        assert!(err.to_string().contains("version"), "{err}");
    }

    #[test]
    fn rejects_non_positive_growth() {
        let text = DEFAULT_BALANCE.replacen("growth = 1.5", "growth = 0.0", 1);
        let err = BalanceConfig::parse(&text).unwrap_err();
        assert!(err.to_string().contains("growth"), "{err}");
    }

    #[test]
    fn rejects_zero_first_upgrade_target() {
        let text =
            DEFAULT_BALANCE.replacen("first_upgrade_secs = 300", "first_upgrade_secs = 0", 1);
        let err = BalanceConfig::parse(&text).unwrap_err();
        assert!(err.to_string().contains("first_upgrade_secs"), "{err}");
    }

    #[test]
    fn rejects_empty_build_queue() {
        let text = DEFAULT_BALANCE.replacen("max_build_queue = 5", "max_build_queue = 0", 1);
        let err = BalanceConfig::parse(&text).unwrap_err();
        assert!(err.to_string().contains("max_build_queue"), "{err}");
    }

    #[test]
    fn rejects_unknown_building_prereq() {
        let text = DEFAULT_BALANCE.replacen(
            r#"prerequisites = [{ building = "robotics_factory", level = 1 }]"#,
            r#"prerequisites = [{ building = "nope", level = 1 }]"#,
            1,
        );
        let err = BalanceConfig::parse(&text).unwrap_err();
        assert!(err.to_string().contains("unknown building"), "{err}");
    }
}
