//! Research tree loaded from config and validated on parse.

use crate::buildings::BuildingKind;
use crate::state::Resources;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::fmt;

const DEFAULT_TREE: &str = include_str!("../data/research.toml");

/// Named effect applied as `per_level * tech_level` and summed across the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifierKind {
    WeaponDamage,
    ShieldStrength,
    Armour,
    FleetSlots,
    ExpeditionSlots,
    Espionage,
    DriveSpeed,
    ColonySlots,
    Logistics,
    MetalOutput,
    CrystalOutput,
    DeuteriumOutput,
    EnergyOutput,
    HeatSink,
    PollutionSink,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TechModifier {
    pub kind: ModifierKind,
    pub per_level: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildingPrereq {
    pub building: BuildingKind,
    pub level: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TechPrereq {
    pub id: String,
    pub level: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TechSpec {
    pub id: String,
    pub name: String,
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
    pub cost_growth: f64,
    pub base_seconds: f64,
    pub time_growth: f64,
    pub prereq_buildings: Vec<BuildingPrereq>,
    pub prereq_techs: Vec<TechPrereq>,
    pub modifiers: Vec<TechModifier>,
}

impl TechSpec {
    pub fn cost(&self, from_level: u32) -> Resources {
        let g = self.cost_growth.powi(from_level as i32);
        Resources {
            metal: self.metal * g,
            crystal: self.crystal * g,
            deuterium: self.deuterium * g,
        }
    }

    pub fn duration_secs(&self, from_level: u32) -> u64 {
        let raw = self.base_seconds * self.time_growth.powi(from_level as i32);
        raw.ceil().max(1.0) as u64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TechTree {
    techs: BTreeMap<String, TechSpec>,
}

impl TechTree {
    pub fn empty() -> Self {
        Self {
            techs: BTreeMap::new(),
        }
    }

    pub fn parse(toml: &str) -> Result<Self, TechTreeError> {
        let file: File = toml::from_str(toml).map_err(|e| TechTreeError::Parse(e.to_string()))?;
        let mut techs = BTreeMap::new();
        for raw in file.tech {
            if techs.contains_key(&raw.id) {
                return Err(TechTreeError::DuplicateId(raw.id));
            }
            let spec = TechSpec {
                id: raw.id.clone(),
                name: raw.name,
                metal: raw.metal,
                crystal: raw.crystal,
                deuterium: raw.deuterium,
                cost_growth: raw.cost_growth,
                base_seconds: raw.base_seconds,
                time_growth: raw.time_growth,
                prereq_buildings: raw
                    .prereq_buildings
                    .into_iter()
                    .map(|p| {
                        Ok(BuildingPrereq {
                            building: parse_building(&p.building)?,
                            level: p.level,
                        })
                    })
                    .collect::<Result<Vec<_>, TechTreeError>>()?,
                prereq_techs: raw
                    .prereq_techs
                    .into_iter()
                    .map(|p| TechPrereq {
                        id: p.id,
                        level: p.level,
                    })
                    .collect(),
                modifiers: raw.modifiers,
            };
            techs.insert(raw.id, spec);
        }
        let tree = Self { techs };
        tree.validate()?;
        Ok(tree)
    }

    pub fn get(&self, id: &str) -> Option<&TechSpec> {
        self.techs.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TechSpec> {
        self.techs.values()
    }

    fn validate(&self) -> Result<(), TechTreeError> {
        for spec in self.techs.values() {
            for pre in &spec.prereq_techs {
                if !self.techs.contains_key(&pre.id) {
                    return Err(TechTreeError::UnknownPrereq {
                        tech: spec.id.clone(),
                        missing: pre.id.clone(),
                    });
                }
            }
        }
        for id in self.techs.keys() {
            if let Some(cycle) = self.cycle_from(id) {
                return Err(TechTreeError::Cycle(cycle));
            }
        }
        Ok(())
    }

    fn cycle_from(&self, start: &str) -> Option<Vec<String>> {
        fn dfs(
            tree: &TechTree,
            id: &str,
            stack: &mut Vec<String>,
            on_path: &mut HashSet<String>,
        ) -> Option<Vec<String>> {
            if on_path.contains(id) {
                let mut cycle: Vec<String> = stack
                    .iter()
                    .skip_while(|s| s.as_str() != id)
                    .cloned()
                    .collect();
                cycle.push(id.to_string());
                return Some(cycle);
            }
            if stack.contains(&id.to_string()) {
                return None;
            }
            on_path.insert(id.to_string());
            stack.push(id.to_string());
            if let Some(spec) = tree.techs.get(id) {
                for pre in &spec.prereq_techs {
                    if let Some(cycle) = dfs(tree, &pre.id, stack, on_path) {
                        return Some(cycle);
                    }
                }
            }
            stack.pop();
            on_path.remove(id);
            None
        }
        dfs(self, start, &mut Vec::new(), &mut HashSet::new())
    }
}

impl Default for TechTree {
    fn default() -> Self {
        Self::parse(DEFAULT_TREE).expect("bundled research.toml must be valid")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResearchJob {
    pub tech: String,
    pub from_level: u32,
    pub paid: Resources,
    pub started_at: u64,
    pub completes_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResearchError {
    UnknownTech { id: String },
    Busy { current: String },
    InsufficientResources { missing: Resources, cost: Resources },
    MissingPrerequisites { gaps: Vec<PrereqGap> },
    Idle,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrereqGap {
    Building {
        building: BuildingKind,
        required: u32,
        actual: u32,
    },
    Tech {
        id: String,
        required: u32,
        actual: u32,
    },
}

impl fmt::Display for PrereqGap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrereqGap::Building {
                building,
                required,
                actual,
            } => write!(f, "{building} level {required} (have {actual})"),
            PrereqGap::Tech {
                id,
                required,
                actual,
            } => write!(f, "{id} level {required} (have {actual})"),
        }
    }
}

impl fmt::Display for ResearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResearchError::UnknownTech { id } => write!(f, "unknown technology {id}"),
            ResearchError::Busy { current } => {
                write!(f, "already researching {current}")
            }
            ResearchError::InsufficientResources { missing, cost } => write!(
                f,
                "not enough resources (missing metal {:.0}, crystal {:.0}, deuterium {:.0}; cost metal {:.0}, crystal {:.0}, deuterium {:.0})",
                missing.metal,
                missing.crystal,
                missing.deuterium,
                cost.metal,
                cost.crystal,
                cost.deuterium
            ),
            ResearchError::MissingPrerequisites { gaps } => {
                let list: Vec<String> = gaps.iter().map(ToString::to_string).collect();
                write!(f, "missing prerequisites: {}", list.join("; "))
            }
            ResearchError::Idle => write!(f, "no research in progress"),
        }
    }
}

impl std::error::Error for ResearchError {}

#[derive(Debug, Clone, PartialEq)]
pub enum TechTreeError {
    Parse(String),
    DuplicateId(String),
    UnknownPrereq { tech: String, missing: String },
    UnknownBuilding { name: String },
    Cycle(Vec<String>),
}

impl fmt::Display for TechTreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TechTreeError::Parse(e) => write!(f, "research.toml: {e}"),
            TechTreeError::DuplicateId(id) => write!(f, "duplicate tech id {id}"),
            TechTreeError::UnknownPrereq { tech, missing } => {
                write!(f, "{tech} requires unknown tech {missing}")
            }
            TechTreeError::UnknownBuilding { name } => {
                write!(f, "unknown building {name} in research tree")
            }
            TechTreeError::Cycle(ids) => write!(f, "cycle in research tree: {}", ids.join(" -> ")),
        }
    }
}

impl std::error::Error for TechTreeError {}

#[derive(Deserialize)]
struct File {
    tech: Vec<FileTech>,
}

#[derive(Deserialize)]
struct FileTech {
    id: String,
    name: String,
    metal: f64,
    crystal: f64,
    deuterium: f64,
    cost_growth: f64,
    base_seconds: f64,
    time_growth: f64,
    #[serde(default)]
    prereq_buildings: Vec<FileBuildingPrereq>,
    #[serde(default)]
    prereq_techs: Vec<FileTechPrereq>,
    #[serde(default)]
    modifiers: Vec<TechModifier>,
}

#[derive(Deserialize)]
struct FileBuildingPrereq {
    building: String,
    level: u32,
}

#[derive(Deserialize)]
struct FileTechPrereq {
    id: String,
    level: u32,
}

fn parse_building(name: &str) -> Result<BuildingKind, TechTreeError> {
    BuildingKind::from_key(name).ok_or_else(|| TechTreeError::UnknownBuilding {
        name: name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_tree_loads_and_has_no_cycles() {
        let tree = TechTree::default();
        assert!(tree.get("energy").is_some());
        assert!(tree.get("weapons").is_some());
        assert!(tree.get("astrophysics").is_some());
        assert!(tree.get("logistics").is_some());
        assert!(tree.get("ecology").is_some());
        let energy = tree.get("energy").unwrap();
        assert_eq!(energy.modifiers[0].kind, ModifierKind::HeatSink);
        let weapons = tree.get("weapons").unwrap();
        assert_eq!(weapons.modifiers[0].kind, ModifierKind::WeaponDamage);
        assert!((weapons.modifiers[0].per_level - 0.1).abs() < 1e-12);
    }

    #[test]
    fn unknown_prereq_id_is_rejected() {
        let toml = r#"
[[tech]]
id = "a"
name = "A"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_techs = [{ id = "missing", level = 1 }]
"#;
        match TechTree::parse(toml) {
            Err(TechTreeError::UnknownPrereq { tech, missing }) => {
                assert_eq!(tech, "a");
                assert_eq!(missing, "missing");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn cycle_is_rejected() {
        let toml = r#"
[[tech]]
id = "a"
name = "A"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_techs = [{ id = "b", level = 1 }]

[[tech]]
id = "b"
name = "B"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_techs = [{ id = "a", level = 1 }]
"#;
        match TechTree::parse(toml) {
            Err(TechTreeError::Cycle(ids)) => {
                assert!(ids.contains(&"a".to_string()));
                assert!(ids.contains(&"b".to_string()));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
