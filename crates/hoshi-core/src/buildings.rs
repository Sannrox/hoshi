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
    FissionReactor,
    FusionPlant,
    MetalStorage,
    CrystalStorage,
    DeuteriumStorage,
    RoboticsFactory,
    ResearchLab,
    Shipyard,
    Radiator,
    Scrubber,
}

impl BuildingKind {
    pub fn as_key(self) -> &'static str {
        match self {
            BuildingKind::MetalMine => "metal_mine",
            BuildingKind::CrystalMine => "crystal_mine",
            BuildingKind::DeuteriumSynthesizer => "deuterium_synthesizer",
            BuildingKind::SolarPlant => "solar_plant",
            BuildingKind::FissionReactor => "fission_reactor",
            BuildingKind::FusionPlant => "fusion_plant",
            BuildingKind::MetalStorage => "metal_storage",
            BuildingKind::CrystalStorage => "crystal_storage",
            BuildingKind::DeuteriumStorage => "deuterium_storage",
            BuildingKind::RoboticsFactory => "robotics_factory",
            BuildingKind::ResearchLab => "research_lab",
            BuildingKind::Shipyard => "shipyard",
            BuildingKind::Radiator => "radiator",
            BuildingKind::Scrubber => "scrubber",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "metal_mine" => BuildingKind::MetalMine,
            "crystal_mine" => BuildingKind::CrystalMine,
            "deuterium_synthesizer" => BuildingKind::DeuteriumSynthesizer,
            "solar_plant" => BuildingKind::SolarPlant,
            "fission_reactor" => BuildingKind::FissionReactor,
            "fusion_plant" => BuildingKind::FusionPlant,
            "metal_storage" => BuildingKind::MetalStorage,
            "crystal_storage" => BuildingKind::CrystalStorage,
            "deuterium_storage" => BuildingKind::DeuteriumStorage,
            "robotics_factory" => BuildingKind::RoboticsFactory,
            "research_lab" => BuildingKind::ResearchLab,
            "shipyard" => BuildingKind::Shipyard,
            "radiator" => BuildingKind::Radiator,
            "scrubber" => BuildingKind::Scrubber,
            _ => return None,
        })
    }

    pub const ALL: [BuildingKind; 14] = [
        BuildingKind::MetalMine,
        BuildingKind::CrystalMine,
        BuildingKind::DeuteriumSynthesizer,
        BuildingKind::SolarPlant,
        BuildingKind::FissionReactor,
        BuildingKind::FusionPlant,
        BuildingKind::MetalStorage,
        BuildingKind::CrystalStorage,
        BuildingKind::DeuteriumStorage,
        BuildingKind::RoboticsFactory,
        BuildingKind::ResearchLab,
        BuildingKind::Shipyard,
        BuildingKind::Radiator,
        BuildingKind::Scrubber,
    ];
}

impl fmt::Display for BuildingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BuildingKind::MetalMine => "metal mine",
            BuildingKind::CrystalMine => "crystal mine",
            BuildingKind::DeuteriumSynthesizer => "deuterium synthesizer",
            BuildingKind::SolarPlant => "solar plant",
            BuildingKind::FissionReactor => "fission reactor",
            BuildingKind::FusionPlant => "fusion plant",
            BuildingKind::MetalStorage => "metal storage",
            BuildingKind::CrystalStorage => "crystal storage",
            BuildingKind::DeuteriumStorage => "deuterium storage",
            BuildingKind::RoboticsFactory => "robotics factory",
            BuildingKind::ResearchLab => "research lab",
            BuildingKind::Shipyard => "shipyard",
            BuildingKind::Radiator => "radiator",
            BuildingKind::Scrubber => "scrubber",
        })
    }
}

/// Completed building levels on one planet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Buildings {
    pub metal_mine: u32,
    pub crystal_mine: u32,
    pub deuterium_synthesizer: u32,
    pub solar_plant: u32,
    pub fission_reactor: u32,
    pub fusion_plant: u32,
    pub metal_storage: u32,
    pub crystal_storage: u32,
    pub deuterium_storage: u32,
    pub robotics_factory: u32,
    pub research_lab: u32,
    pub shipyard: u32,
    pub radiator: u32,
    pub scrubber: u32,
}

impl Buildings {
    pub fn get(self, kind: BuildingKind) -> u32 {
        match kind {
            BuildingKind::MetalMine => self.metal_mine,
            BuildingKind::CrystalMine => self.crystal_mine,
            BuildingKind::DeuteriumSynthesizer => self.deuterium_synthesizer,
            BuildingKind::SolarPlant => self.solar_plant,
            BuildingKind::FissionReactor => self.fission_reactor,
            BuildingKind::FusionPlant => self.fusion_plant,
            BuildingKind::MetalStorage => self.metal_storage,
            BuildingKind::CrystalStorage => self.crystal_storage,
            BuildingKind::DeuteriumStorage => self.deuterium_storage,
            BuildingKind::RoboticsFactory => self.robotics_factory,
            BuildingKind::ResearchLab => self.research_lab,
            BuildingKind::Shipyard => self.shipyard,
            BuildingKind::Radiator => self.radiator,
            BuildingKind::Scrubber => self.scrubber,
        }
    }

    pub fn set(&mut self, kind: BuildingKind, level: u32) {
        match kind {
            BuildingKind::MetalMine => self.metal_mine = level,
            BuildingKind::CrystalMine => self.crystal_mine = level,
            BuildingKind::DeuteriumSynthesizer => self.deuterium_synthesizer = level,
            BuildingKind::SolarPlant => self.solar_plant = level,
            BuildingKind::FissionReactor => self.fission_reactor = level,
            BuildingKind::FusionPlant => self.fusion_plant = level,
            BuildingKind::MetalStorage => self.metal_storage = level,
            BuildingKind::CrystalStorage => self.crystal_storage = level,
            BuildingKind::DeuteriumStorage => self.deuterium_storage = level,
            BuildingKind::RoboticsFactory => self.robotics_factory = level,
            BuildingKind::ResearchLab => self.research_lab = level,
            BuildingKind::Shipyard => self.shipyard = level,
            BuildingKind::Radiator => self.radiator = level,
            BuildingKind::Scrubber => self.scrubber = level,
        }
    }

    pub fn increment(&mut self, kind: BuildingKind) {
        self.set(kind, self.get(kind) + 1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildJobKind {
    Build,
    Dismantle,
}

impl BuildJobKind {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Dismantle => "dismantle",
        }
    }
}

/// One queue slot. Waiting entries have no `completes_at` until they become head.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildJob {
    pub kind: BuildJobKind,
    /// Powered seconds still required by a dismantle job. Zero for upgrades.
    pub work_left: f64,
    pub building: BuildingKind,
    /// Level this job upgrades from (already counts earlier queue entries).
    pub from_level: u32,
    pub paid: Resources,
    /// Treasury credits charged when this job was queued. Zero means the job
    /// never hit the ledger (a migrated pre-finance queue). Refunds use this
    /// amount so a later market move cannot mint or destroy cash.
    pub credits: f64,
    pub started_at: Option<u64>,
    pub completes_at: Option<u64>,
}

/// Why a build command was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildError {
    SolarCap {
        cap: u32,
    },
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
    MissingTechnology {
        id: String,
        required: u32,
        actual: u32,
    },
    EmptySlot {
        index: usize,
        len: usize,
    },
    DismantleBlocked {
        reasons: Vec<String>,
    },
    UnknownPlanet,
    NeedLogistics,
    PermissionDenied(String),
    NeedsApproval {
        id: u64,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::SolarCap { cap } => write!(
                f,
                "solar is at this planet's cap of {cap}; add fission or fusion for more power"
            ),
            BuildError::QueueFull { cap } => {
                write!(f, "build queue is full ({cap}/{cap})")
            }
            BuildError::InsufficientResources { missing, cost } => {
                write!(
                    f,
                    "not enough resources (missing metal {:.0}, crystal {:.0}, deuterium {:.0}; cost metal {:.0}, crystal {:.0}, deuterium {:.0})",
                    missing.iron,
                    missing.silicon,
                    missing.deuterium,
                    cost.iron,
                    cost.silicon,
                    cost.deuterium
                )
            }
            BuildError::MissingPrerequisite {
                building,
                required,
                actual,
            } => write!(f, "requires {building} level {required} (have {actual})"),
            BuildError::MissingTechnology {
                id,
                required,
                actual,
            } => write!(f, "requires {id} level {required} (have {actual})"),
            BuildError::EmptySlot { index, len } => {
                write!(f, "no build job at index {index} (queue length {len})")
            }
            BuildError::DismantleBlocked { reasons } => {
                write!(f, "cannot dismantle: {}", reasons.join("; "))
            }
            BuildError::UnknownPlanet => write!(f, "unknown planet"),
            BuildError::NeedLogistics => {
                write!(
                    f,
                    "logistics research is required to queue on several planets"
                )
            }
            BuildError::PermissionDenied(msg) => write!(f, "{msg}"),
            BuildError::NeedsApproval { id } => {
                write!(f, "spend waits for approval (#{id})")
            }
        }
    }
}

impl std::error::Error for BuildError {}
