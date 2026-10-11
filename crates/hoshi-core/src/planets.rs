//! Planet types, sizes, and colonies. Type and size are a pure function of seed + coord.

use crate::buildings::{BuildJob, Buildings};
use crate::combat::next_u64;
use crate::fleets::Coord;
use crate::ships::Fleet;
use crate::state::Resources;
use std::collections::BTreeMap;

/// Slot class in a system. Inner slots are rocky; outer slots ice or gas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlanetType {
    Rocky,
    Ice,
    GasGiant,
}

impl PlanetType {
    pub fn as_key(self) -> &'static str {
        match self {
            PlanetType::Rocky => "rocky",
            PlanetType::Ice => "ice",
            PlanetType::GasGiant => "gas_giant",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "rocky" => PlanetType::Rocky,
            "ice" => PlanetType::Ice,
            "gas_giant" => PlanetType::GasGiant,
            _ => return None,
        })
    }
}

/// Production multipliers added through [`crate::GameState::modifier_for`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanetTypeBonus {
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
    pub energy: f64,
}

impl PlanetTypeBonus {
    pub const ZERO: Self = Self {
        metal: 0.0,
        crystal: 0.0,
        deuterium: 0.0,
        energy: 0.0,
    };
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanetTypeCatalog {
    pub rocky: PlanetTypeBonus,
    pub ice: PlanetTypeBonus,
    pub gas_giant: PlanetTypeBonus,
}

impl PlanetTypeCatalog {
    pub fn get(&self, kind: PlanetType) -> PlanetTypeBonus {
        match kind {
            PlanetType::Rocky => self.rocky,
            PlanetType::Ice => self.ice,
            PlanetType::GasGiant => self.gas_giant,
        }
    }
}

impl Default for PlanetTypeCatalog {
    fn default() -> Self {
        Self {
            rocky: PlanetTypeBonus {
                metal: 0.10,
                crystal: 0.10,
                deuterium: 0.0,
                energy: 0.0,
            },
            ice: PlanetTypeBonus {
                metal: 0.0,
                crystal: 0.0,
                deuterium: 0.25,
                energy: -0.10,
            },
            gas_giant: PlanetTypeBonus {
                metal: 0.0,
                crystal: 0.0,
                deuterium: 0.50,
                energy: 0.0,
            },
        }
    }
}

/// A settled world besides the home planet.
#[derive(Debug, Clone, PartialEq)]
pub struct Colony {
    pub coord: Coord,
    pub kind: PlanetType,
    pub size: u8,
    pub resources: Resources,
    pub buildings: Buildings,
    pub fleet: Fleet,
    pub build_queue: Vec<BuildJob>,
    /// Rare-planet bonus id from config, when this world was a special outpost.
    pub rare: Option<String>,
    /// Intermediate goods. Construction parts stay in `resources`.
    pub goods: BTreeMap<String, f64>,
}

// Fail the build if GALAXY_GENERATOR_VERSION bumps without a new table arm.
const _: () = assert!(crate::GALAXY_GENERATOR_VERSION == 1);

/// Type from the slot band, size from a seeded roll. Gas giants stay small.
///
/// `version` is the save's generator table. Only table 1 exists; colonize and
/// outpost rolls pass the stored version so a later table keeps old galaxies.
pub fn planet_profile(galaxy_seed: u64, coord: Coord, version: u32) -> (PlanetType, u8) {
    galaxy_profile(version)(galaxy_seed, coord)
}

#[allow(clippy::match_same_arms)] // table 2 lands here when GALAXY_GENERATOR_VERSION bumps
fn galaxy_profile(version: u32) -> fn(u64, Coord) -> (PlanetType, u8) {
    match version {
        0 | 1 => profile_v1,
        _ => profile_v1,
    }
}

fn profile_v1(galaxy_seed: u64, coord: Coord) -> (PlanetType, u8) {
    let mut seed = galaxy_seed
        ^ (u64::from(coord.galaxy) << 32)
        ^ (u64::from(coord.system) << 16)
        ^ u64::from(coord.position);
    let roll = next_u64(&mut seed);
    let kind = match coord.position {
        0..=8 => PlanetType::Rocky,
        9..=12 => PlanetType::Ice,
        _ => PlanetType::GasGiant,
    };
    let size = match kind {
        PlanetType::Rocky => 120 + (roll % 80) as u8,
        PlanetType::Ice => 80 + (roll % 50) as u8,
        PlanetType::GasGiant => 15 + (roll % 20) as u8,
    };
    (kind, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_and_coord_repeat() {
        let a = planet_profile(
            7,
            Coord {
                galaxy: 1,
                system: 2,
                position: 14,
            },
            1,
        );
        let b = planet_profile(
            7,
            Coord {
                galaxy: 1,
                system: 2,
                position: 14,
            },
            1,
        );
        assert_eq!(a, b);
        assert_eq!(a.0, PlanetType::GasGiant);
        let inner = planet_profile(
            7,
            Coord {
                galaxy: 1,
                system: 2,
                position: 3,
            },
            1,
        );
        assert_eq!(inner.0, PlanetType::Rocky);
        let other = planet_profile(
            8,
            Coord {
                galaxy: 1,
                system: 2,
                position: 14,
            },
            1,
        );
        assert_ne!(a.1, other.1);
    }
}
