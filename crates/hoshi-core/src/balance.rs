//! Data-driven production, energy, storage and construction. Numbers live here, not in the tick.

use crate::buildings::{BuildingKind, Buildings};
use crate::state::Resources;

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
    pub starting_resources: Resources,
    pub starting_buildings: Buildings,
}

impl Default for BalanceConfig {
    fn default() -> Self {
        Self {
            metal_mine: ProducerSpec {
                base_per_hour: 30.0,
                growth: 1.5,
                base_energy: 10.0,
                energy_growth: 1.1,
            },
            crystal_mine: ProducerSpec {
                base_per_hour: 20.0,
                growth: 1.6,
                base_energy: 10.0,
                energy_growth: 1.1,
            },
            deuterium_synthesizer: ProducerSpec {
                base_per_hour: 10.0,
                growth: 1.5,
                base_energy: 20.0,
                energy_growth: 1.1,
            },
            solar_plant: EnergyProducerSpec {
                base_energy: 22.0,
                growth: 1.1,
            },
            metal_storage: StorageSpec {
                base: 10_000.0,
                growth: 2.0,
            },
            crystal_storage: StorageSpec {
                base: 10_000.0,
                growth: 2.0,
            },
            deuterium_storage: StorageSpec {
                base: 10_000.0,
                growth: 2.0,
            },
            buildings: BuildingCatalog::default(),
            max_build_queue: 5,
            robotics_speed_per_level: 1.0,
            techs: crate::TechTree::default(),
            ships: crate::ShipCatalog::default(),
            max_ship_queue: 5,
            flight: crate::FlightConfig::default(),
            starting_resources: Resources {
                metal: 500.0,
                crystal: 500.0,
                deuterium: 0.0,
            },
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
            },
        }
    }
}
