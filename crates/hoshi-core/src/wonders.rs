//! Staged megaprojects. Specs live in balance config; progress ticks in catch-up.

use crate::research::{ModifierKind, TechPrereq};
use crate::state::Resources;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct WonderSpec {
    pub id: String,
    pub name: String,
    pub prereq_techs: Vec<TechPrereq>,
    pub stages: Vec<WonderStage>,
    pub modifiers: Vec<WonderModifier>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WonderStage {
    pub cost: Resources,
    pub duration_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WonderModifier {
    pub kind: ModifierKind,
    pub value: f64,
}

/// One in-progress wonder. `completes_at` is set once the current stage is fully paid.
#[derive(Debug, Clone, PartialEq)]
pub struct WonderJob {
    pub id: String,
    pub stage: usize,
    pub contributed: Resources,
    pub completes_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WonderError {
    Unknown {
        id: String,
    },
    Busy {
        id: String,
    },
    AlreadyComplete {
        id: String,
    },
    MissingTech {
        id: String,
        required: u32,
        actual: u32,
    },
    UnknownPlanet,
    NothingToContribute,
    EmptyStages {
        id: String,
    },
}

impl fmt::Display for WonderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WonderError::Unknown { id } => write!(f, "unknown wonder {id}"),
            WonderError::Busy { id } => write!(f, "wonder {id} is already under construction"),
            WonderError::AlreadyComplete { id } => write!(f, "wonder {id} is already complete"),
            WonderError::MissingTech {
                id,
                required,
                actual,
            } => write!(f, "need {id} {required} (have {actual})"),
            WonderError::UnknownPlanet => write!(f, "unknown planet"),
            WonderError::NothingToContribute => write!(f, "nothing to contribute"),
            WonderError::EmptyStages { id } => write!(f, "wonder {id} has no stages"),
        }
    }
}

impl std::error::Error for WonderError {}

impl WonderSpec {
    pub fn stage(&self, index: usize) -> Option<&WonderStage> {
        self.stages.get(index)
    }
}
