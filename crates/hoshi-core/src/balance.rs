//! Data-driven production, energy and storage. Numbers live here, not in the tick.

use crate::state::{Buildings, Resources};

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
            },
        }
    }
}
