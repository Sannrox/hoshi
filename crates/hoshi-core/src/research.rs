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

/// Playable tree branch. Ancient stays hidden until prestige unlocks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TechBranch {
    #[default]
    Industry,
    Military,
    Society,
    Ancient,
}

impl TechBranch {
    pub const PLAYABLE: [TechBranch; 3] = [
        TechBranch::Industry,
        TechBranch::Military,
        TechBranch::Society,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            TechBranch::Industry => "industry",
            TechBranch::Military => "military",
            TechBranch::Society => "society",
            TechBranch::Ancient => "ancient",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "industry" => TechBranch::Industry,
            "military" => TechBranch::Military,
            "society" => TechBranch::Society,
            "ancient" => TechBranch::Ancient,
            _ => return None,
        })
    }
}

/// Derived TUI/state for one node. Hidden is omitted from the five visible states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TechNodeState {
    Locked,
    Available,
    InProgress,
    Researched,
    Excluded,
    Hidden,
}

impl TechNodeState {
    pub fn as_key(self) -> &'static str {
        match self {
            TechNodeState::Locked => "locked",
            TechNodeState::Available => "available",
            TechNodeState::InProgress => "in_progress",
            TechNodeState::Researched => "researched",
            TechNodeState::Excluded => "excluded",
            TechNodeState::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TechSpec {
    pub id: String,
    pub name: String,
    pub iron: f64,
    pub silicon: f64,
    pub deuterium: f64,
    pub cost_growth: f64,
    pub base_seconds: f64,
    pub time_growth: f64,
    pub prereq_buildings: Vec<BuildingPrereq>,
    pub prereq_techs: Vec<TechPrereq>,
    pub modifiers: Vec<TechModifier>,
    pub branch: TechBranch,
    pub tier: u32,
    pub exclusive_group: Option<String>,
    pub keystone: bool,
    pub hidden: bool,
    pub max_level: Option<u32>,
}

impl TechSpec {
    pub fn cost(&self, from_level: u32) -> Resources {
        let g = self.cost_growth.powi(from_level as i32);
        Resources::from_parts(self.iron * g, self.silicon * g, self.deuterium * g)
    }

    pub fn duration_secs(&self, from_level: u32) -> u64 {
        let raw = self.base_seconds * self.time_growth.powi(from_level as i32);
        raw.ceil().max(1.0) as u64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TechTree {
    techs: BTreeMap<String, TechSpec>,
    pub starting_techs: Vec<String>,
    pub ancient_prestige_runs: u32,
}

impl TechTree {
    pub fn empty() -> Self {
        Self {
            techs: BTreeMap::new(),
            starting_techs: Vec::new(),
            ancient_prestige_runs: 0,
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
                iron: raw.iron,
                silicon: raw.silicon,
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
                branch: raw.branch,
                tier: raw.tier,
                exclusive_group: raw.exclusive_group,
                keystone: raw.keystone,
                hidden: raw.hidden || raw.branch == TechBranch::Ancient,
                max_level: raw.max_level,
            };
            techs.insert(raw.id, spec);
        }
        for id in &file.tree.starting_techs {
            if !techs.contains_key(id) {
                return Err(TechTreeError::UnknownStartingTech(id.clone()));
            }
        }
        let tree = Self {
            techs,
            starting_techs: file.tree.starting_techs,
            ancient_prestige_runs: file.tree.ancient_prestige_runs,
        };
        tree.validate()?;
        Ok(tree)
    }

    pub fn get(&self, id: &str) -> Option<&TechSpec> {
        self.techs.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TechSpec> {
        self.techs.values()
    }

    pub fn branch_techs(&self, branch: TechBranch) -> Vec<&TechSpec> {
        let mut rows: Vec<&TechSpec> = self
            .techs
            .values()
            .filter(|spec| spec.branch == branch)
            .collect();
        rows.sort_by(|a, b| a.tier.cmp(&b.tier).then(a.id.cmp(&b.id)));
        rows
    }

    pub fn keystone(&self, branch: TechBranch) -> Option<&TechSpec> {
        self.techs
            .values()
            .find(|spec| spec.branch == branch && spec.keystone)
    }

    /// Remove nodes, then insert replacements. Re-validates keystones and exclusive groups.
    pub fn apply_overrides(
        &mut self,
        remove: &[String],
        add: Vec<TechSpec>,
    ) -> Result<(), TechTreeError> {
        for id in remove {
            self.techs.remove(id);
        }
        for spec in add {
            if self.techs.contains_key(&spec.id) {
                return Err(TechTreeError::DuplicateId(spec.id));
            }
            self.techs.insert(spec.id.clone(), spec);
        }
        self.validate()
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
        self.validate_exclusive_groups()?;
        self.validate_keystones()?;
        Ok(())
    }

    fn validate_exclusive_groups(&self) -> Result<(), TechTreeError> {
        let mut groups: BTreeMap<&str, Vec<&TechSpec>> = BTreeMap::new();
        for spec in self.techs.values() {
            if let Some(group) = spec.exclusive_group.as_deref() {
                groups.entry(group).or_default().push(spec);
            }
        }
        for (group, members) in &groups {
            if members.len() < 2 {
                return Err(TechTreeError::ExclusiveTooSmall {
                    group: (*group).to_string(),
                });
            }
            let ids: HashSet<&str> = members.iter().map(|s| s.id.as_str()).collect();
            let independent = members.iter().any(|spec| {
                !spec
                    .prereq_techs
                    .iter()
                    .any(|pre| ids.contains(pre.id.as_str()))
            });
            if !independent {
                return Err(TechTreeError::ExclusiveDeadlock {
                    group: (*group).to_string(),
                });
            }
        }
        Ok(())
    }

    fn validate_keystones(&self) -> Result<(), TechTreeError> {
        let reachable = self.reachable_ids();
        for spec in self.techs.values() {
            if spec.keystone && !reachable.contains(&spec.id) {
                return Err(TechTreeError::UnreachableKeystone(spec.id.clone()));
            }
        }
        if self.techs.len() < 8 {
            return Ok(());
        }
        for branch in TechBranch::PLAYABLE {
            let stones: Vec<_> = self
                .techs
                .values()
                .filter(|spec| spec.branch == branch && spec.keystone)
                .collect();
            if stones.len() != 1 {
                return Err(TechTreeError::KeystoneCount {
                    branch: branch.as_key().to_string(),
                    count: stones.len(),
                });
            }
        }
        Ok(())
    }

    fn reachable_ids(&self) -> HashSet<String> {
        let mut reachable = HashSet::new();
        let mut changed = true;
        while changed {
            changed = false;
            for spec in self.techs.values() {
                if reachable.contains(&spec.id) {
                    continue;
                }
                let ok = spec
                    .prereq_techs
                    .iter()
                    .all(|pre| reachable.contains(&pre.id));
                if ok {
                    reachable.insert(spec.id.clone());
                    changed = true;
                }
            }
        }
        reachable
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
    pub credits: f64,
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
    Excluded { id: String, group: String },
    Hidden { id: String },
    Complete { id: String },
    NeedsApproval { id: u64 },
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
            } => write!(
                f,
                "{} level {required} (have {actual})",
                id.replace('_', " ")
            ),
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
                missing.iron,
                missing.silicon,
                missing.deuterium,
                cost.iron,
                cost.silicon,
                cost.deuterium
            ),
            ResearchError::MissingPrerequisites { gaps } => {
                let list: Vec<String> = gaps.iter().map(ToString::to_string).collect();
                write!(f, "missing prerequisites: {}", list.join("; "))
            }
            ResearchError::Idle => write!(f, "no research in progress"),
            ResearchError::Excluded { id, group } => {
                write!(f, "{id} is excluded by exclusive group {group}")
            }
            ResearchError::Hidden { id } => write!(f, "{id} is locked behind prestige"),
            ResearchError::Complete { id } => write!(f, "{id} is already complete"),
            ResearchError::NeedsApproval { id } => {
                write!(f, "spend waits for approval (#{id})")
            }
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
    UnknownStartingTech(String),
    ExclusiveTooSmall { group: String },
    ExclusiveDeadlock { group: String },
    UnreachableKeystone(String),
    KeystoneCount { branch: String, count: usize },
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
            TechTreeError::UnknownStartingTech(id) => {
                write!(f, "starting tech {id} is not in the tree")
            }
            TechTreeError::ExclusiveTooSmall { group } => {
                write!(f, "exclusive group {group} needs at least two techs")
            }
            TechTreeError::ExclusiveDeadlock { group } => {
                write!(f, "exclusive group {group} deadlocks the branch")
            }
            TechTreeError::UnreachableKeystone(id) => {
                write!(f, "keystone {id} is unreachable")
            }
            TechTreeError::KeystoneCount { branch, count } => {
                write!(f, "{branch} has {count} keystones, want 1")
            }
        }
    }
}

impl std::error::Error for TechTreeError {}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    tree: FileTree,
    tech: Vec<FileTech>,
}

#[derive(Deserialize, Default)]
struct FileTree {
    #[serde(default)]
    starting_techs: Vec<String>,
    #[serde(default)]
    ancient_prestige_runs: u32,
}

#[derive(Deserialize)]
struct FileTech {
    id: String,
    name: String,
    #[serde(alias = "metal")]
    iron: f64,
    #[serde(alias = "crystal")]
    silicon: f64,
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
    #[serde(default)]
    branch: TechBranch,
    #[serde(default)]
    tier: u32,
    #[serde(default)]
    exclusive_group: Option<String>,
    #[serde(default)]
    keystone: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    max_level: Option<u32>,
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
        assert_eq!(
            tree.keystone(TechBranch::Industry).unwrap().id,
            "dyson_blueprint"
        );
        assert_eq!(
            tree.keystone(TechBranch::Military).unwrap().id,
            "invasion_doctrine"
        );
        assert_eq!(
            tree.keystone(TechBranch::Society).unwrap().id,
            "galactic_senate"
        );
        assert!(tree.get("deep_core").unwrap().exclusive_group.is_some());
        assert!(tree.get("ancient_drive").unwrap().hidden);
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

    #[test]
    fn exclusive_group_of_one_is_rejected() {
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
exclusive_group = "solo"
"#;
        match TechTree::parse(toml) {
            Err(TechTreeError::ExclusiveTooSmall { group }) => assert_eq!(group, "solo"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn exclusive_deadlock_is_rejected() {
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
exclusive_group = "g"
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
exclusive_group = "g"
prereq_techs = [{ id = "a", level = 1 }]
"#;
        match TechTree::parse(toml) {
            Err(TechTreeError::ExclusiveDeadlock { group }) => assert_eq!(group, "g"),
            Err(TechTreeError::Cycle(_)) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn unreachable_keystone_is_rejected() {
        let toml = r#"
[[tech]]
id = "root"
name = "Root"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0

[[tech]]
id = "stone"
name = "Stone"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
keystone = true
prereq_techs = [{ id = "missing_path", level = 1 }]
"#;
        match TechTree::parse(toml) {
            Err(TechTreeError::UnknownPrereq { missing, .. }) => {
                assert_eq!(missing, "missing_path");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn keystone_behind_a_missing_chain_is_unreachable() {
        let toml = r#"
[[tech]]
id = "root"
name = "Root"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0

[[tech]]
id = "loop_a"
name = "Loop A"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_techs = [{ id = "loop_b", level = 1 }]

[[tech]]
id = "loop_b"
name = "Loop B"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_techs = [{ id = "loop_a", level = 1 }]

[[tech]]
id = "stone"
name = "Stone"
metal = 0.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
keystone = true
prereq_techs = [{ id = "loop_a", level = 1 }]
"#;
        match TechTree::parse(toml) {
            Err(TechTreeError::Cycle(_)) => {}
            Err(TechTreeError::UnreachableKeystone(id)) => assert_eq!(id, "stone"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
