//! Data-driven production, energy, storage and construction. Numbers live in
//! `data/balance.toml`, not in the tick.

use crate::activity::{ActivityChannelConfig, ActivityEffectsConfig};
use crate::buildings::{BuildingKind, Buildings};
use crate::expedition::{ExpeditionConfig, ExpeditionWeights};
use crate::fleets::FlightConfig;
use crate::planets::{PlanetTypeBonus, PlanetTypeCatalog};
use crate::ships::ShipKind;
use crate::state::Resources;
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
}

impl Default for PacingTargets {
    fn default() -> Self {
        Self {
            first_upgrade_secs: 300,
            min_metal_mine_per_day: 1,
            max_bridge_metal_ratio: 8.0,
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
}

#[derive(Deserialize)]
struct FilePacing {
    first_upgrade_secs: u64,
    min_metal_mine_per_day: u32,
    max_bridge_metal_ratio: f64,
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
}

#[derive(Deserialize)]
struct FileBuilding {
    metal_base: f64,
    crystal_base: f64,
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
    metal: f64,
    crystal: f64,
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
        let config = BalanceConfig {
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
            },
            pacing: PacingTargets {
                first_upgrade_secs: self.pacing.first_upgrade_secs,
                min_metal_mine_per_day: self.pacing.min_metal_mine_per_day,
                max_bridge_metal_ratio: self.pacing.max_bridge_metal_ratio,
            },
        };
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
