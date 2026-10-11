//! Building kinds, levels and the per-planet build queue.

use crate::state::Resources;
use std::fmt;

/// Buildings a planet can construct. Research modifiers land in a later issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuildingKind {
    MetalMine,
    CrystalMine,
    DeuteriumSynthesizer,
    SolarPlant,
    MetalStorage,
    CrystalStorage,
    DeuteriumStorage,
    RoboticsFactory,
    ResearchLab,
    Shipyard,
}

impl BuildingKind {
    pub const ALL: [BuildingKind; 10] = [
        BuildingKind::MetalMine,
        BuildingKind::CrystalMine,
        BuildingKind::DeuteriumSynthesizer,
        BuildingKind::SolarPlant,
        BuildingKind::MetalStorage,
        BuildingKind::CrystalStorage,
        BuildingKind::DeuteriumStorage,
        BuildingKind::RoboticsFactory,
        BuildingKind::ResearchLab,
        BuildingKind::Shipyard,
    ];
}

impl fmt::Display for BuildingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BuildingKind::MetalMine => "metal mine",
            BuildingKind::CrystalMine => "crystal mine",
            BuildingKind::DeuteriumSynthesizer => "deuterium synthesizer",
            BuildingKind::SolarPlant => "solar plant",
            BuildingKind::MetalStorage => "metal storage",
            BuildingKind::CrystalStorage => "crystal storage",
            BuildingKind::DeuteriumStorage => "deuterium storage",
            BuildingKind::RoboticsFactory => "robotics factory",
            BuildingKind::ResearchLab => "research lab",
            BuildingKind::Shipyard => "shipyard",
        })
    }
}

/// Completed building levels on one planet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buildings {
    pub metal_mine: u32,
    pub crystal_mine: u32,
    pub deuterium_synthesizer: u32,
    pub solar_plant: u32,
    pub metal_storage: u32,
    pub crystal_storage: u32,
    pub deuterium_storage: u32,
    pub robotics_factory: u32,
    pub research_lab: u32,
    pub shipyard: u32,
}

impl Buildings {
    pub fn get(self, kind: BuildingKind) -> u32 {
        match kind {
            BuildingKind::MetalMine => self.metal_mine,
            BuildingKind::CrystalMine => self.crystal_mine,
            BuildingKind::DeuteriumSynthesizer => self.deuterium_synthesizer,
            BuildingKind::SolarPlant => self.solar_plant,
            BuildingKind::MetalStorage => self.metal_storage,
            BuildingKind::CrystalStorage => self.crystal_storage,
            BuildingKind::DeuteriumStorage => self.deuterium_storage,
            BuildingKind::RoboticsFactory => self.robotics_factory,
            BuildingKind::ResearchLab => self.research_lab,
            BuildingKind::Shipyard => self.shipyard,
        }
    }

    pub fn set(&mut self, kind: BuildingKind, level: u32) {
        match kind {
            BuildingKind::MetalMine => self.metal_mine = level,
            BuildingKind::CrystalMine => self.crystal_mine = level,
            BuildingKind::DeuteriumSynthesizer => self.deuterium_synthesizer = level,
            BuildingKind::SolarPlant => self.solar_plant = level,
            BuildingKind::MetalStorage => self.metal_storage = level,
            BuildingKind::CrystalStorage => self.crystal_storage = level,
            BuildingKind::DeuteriumStorage => self.deuterium_storage = level,
            BuildingKind::RoboticsFactory => self.robotics_factory = level,
            BuildingKind::ResearchLab => self.research_lab = level,
            BuildingKind::Shipyard => self.shipyard = level,
        }
    }

    pub fn increment(&mut self, kind: BuildingKind) {
        self.set(kind, self.get(kind) + 1);
    }
}

/// One queue slot. Waiting entries have no `completes_at` until they become head.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildJob {
    pub building: BuildingKind,
    /// Level this job upgrades from (already counts earlier queue entries).
    pub from_level: u32,
    pub paid: Resources,
    pub started_at: Option<u64>,
    pub completes_at: Option<u64>,
}

/// Why a build command was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildError {
    QueueFull {
        cap: usize,
    },
    InsufficientResources {
        missing: Resources,
        cost: Resources,
    },
    MissingPrerequisite {
        building: BuildingKind,
        required: u32,
        actual: u32,
    },
    EmptySlot {
        index: usize,
        len: usize,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::QueueFull { cap } => {
                write!(f, "build queue is full ({cap}/{cap})")
            }
            BuildError::InsufficientResources { missing, cost } => {
                write!(
                    f,
                    "not enough resources (missing metal {:.0}, crystal {:.0}, deuterium {:.0}; cost metal {:.0}, crystal {:.0}, deuterium {:.0})",
                    missing.metal,
                    missing.crystal,
                    missing.deuterium,
                    cost.metal,
                    cost.crystal,
                    cost.deuterium
                )
            }
            BuildError::MissingPrerequisite {
                building,
                required,
                actual,
            } => write!(f, "requires {building} level {required} (have {actual})"),
            BuildError::EmptySlot { index, len } => {
                write!(f, "no build job at index {index} (queue length {len})")
            }
        }
    }
}

impl std::error::Error for BuildError {}
