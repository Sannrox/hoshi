//! Production chains: deposits, recipes, and intermediate goods.
//!
//! Construction stockpiles stay `Resources` (frames, electronics, fuel). Raw
//! and refined goods live in a per-planet map. Empty recipe lists keep the
//! legacy mine-to-stockpile path.

use crate::buildings::{BuildingKind, Buildings};
use crate::fleets::Coord;
use crate::planets::PlanetType;
use crate::state::{ResourceKind, Resources};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Raw, refined, or finished construction part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoodKind {
    Raw,
    Refined,
    Component,
}

impl GoodKind {
    fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "raw" => Self::Raw,
            "refined" => Self::Refined,
            "component" => Self::Component,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoodSpec {
    pub id: String,
    pub kind: GoodKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub building: BuildingKind,
    /// Scales output by this deposit when set (extractors).
    pub deposit: Option<String>,
    pub inputs: BTreeMap<String, f64>,
    pub outputs: BTreeMap<String, f64>,
    /// Extra waste heat per hour at building level 1. Scaled by level.
    pub heat: f64,
    /// Extra pollution per hour at building level 1. Scaled by level.
    pub pollution: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChainConfig {
    pub goods: Vec<GoodSpec>,
    pub recipes: Vec<Recipe>,
    pub deposits: BTreeMap<PlanetType, BTreeMap<String, f64>>,
    /// Extra spread around the type deposit, in `[0, 1]`. Zero is exact.
    pub deposit_jitter: f64,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            goods: Vec::new(),
            recipes: Vec::new(),
            deposits: BTreeMap::new(),
            deposit_jitter: 0.0,
        }
    }
}

impl ChainConfig {
    pub fn enabled(&self) -> bool {
        !self.recipes.is_empty()
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut good_ids = BTreeSet::new();
        for good in &self.goods {
            if !good_ids.insert(good.id.as_str()) {
                return Err(format!("duplicate chain good {}", good.id));
            }
        }
        for (kind, table) in &self.deposits {
            for (id, richness) in table {
                if !good_ids.contains(id.as_str()) {
                    return Err(format!(
                        "deposit {} on {} is not a declared good",
                        id,
                        kind.as_key()
                    ));
                }
                if !richness.is_finite() || *richness < 0.0 {
                    return Err(format!("deposit {id} richness must be finite and >= 0"));
                }
            }
        }
        if !(self.deposit_jitter.is_finite() && (0.0..1.0).contains(&self.deposit_jitter)) {
            return Err("chains.deposit_jitter must be in [0, 1)".into());
        }
        let mut produced: BTreeSet<&str> = BTreeSet::new();
        for good in &self.goods {
            if good.kind == GoodKind::Raw {
                produced.insert(good.id.as_str());
            }
        }
        let mut recipe_ids = BTreeSet::new();
        for recipe in &self.recipes {
            if !recipe_ids.insert(recipe.id.as_str()) {
                return Err(format!("duplicate recipe {}", recipe.id));
            }
            if let Some(dep) = &recipe.deposit
                && !good_ids.contains(dep.as_str())
            {
                return Err(format!("recipe {} deposit {dep} is unknown", recipe.id));
            }
            reject_alias_overlap(&recipe.id, "input", recipe.inputs.keys())?;
            reject_alias_overlap(&recipe.id, "output", recipe.outputs.keys())?;
            let mut in_mass = 0.0;
            for (id, qty) in &recipe.inputs {
                if !good_ids.contains(id.as_str()) && component_kind(id).is_none() {
                    return Err(format!("recipe {} input {id} is unknown", recipe.id));
                }
                if !qty.is_finite() || *qty < 0.0 {
                    return Err(format!(
                        "recipe {} input {id} must be finite and >= 0",
                        recipe.id
                    ));
                }
                in_mass += *qty;
            }
            let mut out_mass = 0.0;
            for (id, qty) in &recipe.outputs {
                if !good_ids.contains(id.as_str()) && component_kind(id).is_none() {
                    return Err(format!("recipe {} output {id} is unknown", recipe.id));
                }
                if !qty.is_finite() || *qty <= 0.0 {
                    return Err(format!(
                        "recipe {} output {id} must be finite and > 0",
                        recipe.id
                    ));
                }
                out_mass += *qty;
                produced.insert(id.as_str());
            }
            if recipe.inputs.is_empty() {
                if recipe.deposit.is_none() {
                    return Err(format!(
                        "extractor recipe {} needs a deposit (no mass from nothing)",
                        recipe.id
                    ));
                }
            } else if out_mass > in_mass + 1e-9 {
                return Err(format!(
                    "recipe {} creates mass ({out_mass} out > {in_mass} in)",
                    recipe.id
                ));
            }
        }
        for recipe in &self.recipes {
            for id in recipe.inputs.keys() {
                if !produced.contains(id.as_str()) && component_kind(id).is_none() {
                    return Err(format!(
                        "recipe {} input {id} is not produced by any recipe or deposit",
                        recipe.id
                    ));
                }
            }
        }
        if let Some(cycle) = recipe_cycle(self) {
            return Err(format!("recipe cycle through {cycle}"));
        }
        let mut producers = BTreeSet::new();
        for recipe in &self.recipes {
            for id in recipe.outputs.keys() {
                producers.insert(id.as_str());
            }
        }
        let mut earlier = BTreeSet::new();
        for recipe in &self.recipes {
            for id in recipe.inputs.keys() {
                if producers.contains(id.as_str()) && !earlier.contains(id.as_str()) {
                    return Err(format!(
                        "recipe {} consumes {id} before a producer is listed",
                        recipe.id
                    ));
                }
            }
            for id in recipe.outputs.keys() {
                earlier.insert(id.as_str());
            }
        }
        Ok(())
    }

    pub fn deposits_for(
        &self,
        kind: PlanetType,
        galaxy_seed: u64,
        coord: Coord,
    ) -> BTreeMap<String, f64> {
        let Some(base) = self.deposits.get(&kind) else {
            return BTreeMap::new();
        };
        let mut out = BTreeMap::new();
        for (id, richness) in base {
            let mut value = *richness;
            if self.deposit_jitter > 0.0 {
                let mixed = mix_u64(
                    galaxy_seed
                        ^ fnv(id)
                        ^ (u64::from(coord.galaxy) << 48)
                        ^ (u64::from(coord.system) << 32)
                        ^ u64::from(coord.position),
                );
                let unit = (mixed >> 11) as f64 / ((1u64 << 53) as f64);
                value *= 1.0 - self.deposit_jitter + 2.0 * self.deposit_jitter * unit;
            }
            out.insert(id.clone(), value);
        }
        out
    }
}

fn reject_alias_overlap<'a>(
    recipe: &str,
    side: &str,
    ids: impl Iterator<Item = &'a String>,
) -> Result<(), String> {
    let mut seen: Vec<(ResourceKind, &str)> = Vec::new();
    for id in ids {
        if let Some(kind) = component_kind(id) {
            if let Some((_, prev)) = seen.iter().find(|(k, _)| *k == kind) {
                return Err(format!(
                    "recipe {recipe} {side}s {prev} and {id} are the same stockpile"
                ));
            }
            seen.push((kind, id.as_str()));
        }
    }
    Ok(())
}

fn recipe_cycle(config: &ChainConfig) -> Option<String> {
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for recipe in &config.recipes {
        for out in recipe.outputs.keys() {
            for inp in recipe.inputs.keys() {
                adj.entry(out.as_str()).or_default().push(inp.as_str());
            }
        }
    }
    fn visit<'a>(
        node: &'a str,
        adj: &BTreeMap<&str, Vec<&'a str>>,
        stack: &mut Vec<&'a str>,
        seen: &mut BTreeSet<&'a str>,
    ) -> Option<String> {
        if stack.contains(&node) {
            return Some(node.to_string());
        }
        if !seen.insert(node) {
            return None;
        }
        stack.push(node);
        if let Some(nexts) = adj.get(node) {
            for n in nexts {
                if let Some(c) = visit(n, adj, stack, seen) {
                    return Some(c);
                }
            }
        }
        stack.pop();
        None
    }
    let mut seen = BTreeSet::new();
    for node in adj.keys().copied() {
        if let Some(c) = visit(node, &adj, &mut Vec::new(), &mut seen) {
            return Some(c);
        }
    }
    None
}

fn fnv(text: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn mix_u64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Construction stockpiles. Intermediate names such as `fuel` and `propellant`
/// stay in the goods map; toml cost aliases map those names at parse time.
pub fn component_kind(id: &str) -> Option<ResourceKind> {
    match id {
        "frames" | "metal" => Some(ResourceKind::Metal),
        "electronics" | "crystal" => Some(ResourceKind::Crystal),
        "deuterium" => Some(ResourceKind::Deuterium),
        _ => None,
    }
}

pub(crate) fn available(resources: &Resources, goods: &BTreeMap<String, f64>, id: &str) -> f64 {
    match component_kind(id) {
        Some(kind) => resources.get(kind),
        None => goods.get(id).copied().unwrap_or(0.0),
    }
}

pub(crate) fn add_good(
    resources: &mut Resources,
    goods: &mut BTreeMap<String, f64>,
    id: &str,
    amount: f64,
    cap: Option<f64>,
) {
    if amount <= 0.0 {
        return;
    }
    match component_kind(id) {
        Some(kind) => {
            let current = resources.get(kind);
            let next = match cap {
                Some(c) if current >= c => current,
                Some(c) => (current + amount).min(c),
                None => current + amount,
            };
            resources.set(kind, next);
        }
        None => {
            *goods.entry(id.to_string()).or_insert(0.0) += amount;
        }
    }
}

pub(crate) fn take_good(
    resources: &mut Resources,
    goods: &mut BTreeMap<String, f64>,
    id: &str,
    amount: f64,
) {
    if amount <= 0.0 {
        return;
    }
    match component_kind(id) {
        Some(kind) => {
            resources.set(kind, (resources.get(kind) - amount).max(0.0));
        }
        None => {
            let slot = goods.entry(id.to_string()).or_insert(0.0);
            *slot = (*slot - amount).max(0.0);
        }
    }
}

/// Inputs for one recipe pass over a planet.
pub(crate) struct RecipeRun<'a> {
    pub config: &'a ChainConfig,
    pub buildings: Buildings,
    pub deposits: &'a BTreeMap<String, f64>,
    pub hours: f64,
    pub energy_factor: f64,
    pub boost: f64,
}

/// Run every recipe once for `hours`. Extractors first (config order must be
/// topological). Stops a recipe when any input runs out or a component output
/// is already at cap.
pub(crate) fn apply_recipes(
    run: RecipeRun<'_>,
    scale_of: impl Fn(BuildingKind) -> f64,
    resources: &mut Resources,
    goods: &mut BTreeMap<String, f64>,
    cap_of: impl Fn(ResourceKind) -> f64,
) {
    if run.hours <= 0.0 {
        return;
    }
    let throttle = run.energy_factor.max(0.0) * run.boost.max(0.0);
    for recipe in &run.config.recipes {
        if run.buildings.get(recipe.building) == 0 {
            continue;
        }
        let level_scale = scale_of(recipe.building);
        if level_scale <= 0.0 {
            continue;
        }
        let deposit = recipe
            .deposit
            .as_ref()
            .map(|id| run.deposits.get(id).copied().unwrap_or(0.0))
            .unwrap_or(1.0);
        if recipe.deposit.is_some() && deposit <= 0.0 {
            continue;
        }
        let scale = level_scale * throttle * deposit * run.hours;
        if scale <= 0.0 {
            continue;
        }
        let mut limited: f64 = 1.0;
        for (id, qty) in &recipe.inputs {
            if *qty <= 0.0 {
                continue;
            }
            let need = *qty * scale;
            let have = available(resources, goods, id);
            limited = limited.min(have / need);
        }
        for (id, qty) in &recipe.outputs {
            if *qty <= 0.0 {
                continue;
            }
            let Some(kind) = component_kind(id) else {
                continue;
            };
            let cap = cap_of(kind);
            let current = resources.get(kind);
            if current >= cap {
                limited = 0.0;
                break;
            }
            limited = limited.min((cap - current) / (*qty * scale));
        }
        if limited <= 0.0 {
            continue;
        }
        let used = scale * limited;
        for (id, qty) in &recipe.inputs {
            take_good(resources, goods, id, *qty * used);
        }
        for (id, qty) in &recipe.outputs {
            let cap = component_kind(id).map(&cap_of);
            add_good(resources, goods, id, *qty * used, cap);
        }
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct FileChains {
    #[serde(default)]
    goods: Vec<FileGood>,
    #[serde(default)]
    recipes: Vec<FileRecipe>,
    #[serde(default)]
    deposits: FileDeposits,
    #[serde(default)]
    deposit_jitter: Option<f64>,
}

#[derive(Deserialize)]
struct FileGood {
    id: String,
    kind: String,
}

#[derive(Deserialize)]
struct FileRecipe {
    id: String,
    building: String,
    #[serde(default)]
    deposit: Option<String>,
    #[serde(default)]
    inputs: BTreeMap<String, f64>,
    #[serde(default)]
    outputs: BTreeMap<String, f64>,
    #[serde(default)]
    heat: f64,
    #[serde(default)]
    pollution: f64,
}

#[derive(Deserialize, Default)]
struct FileDeposits {
    #[serde(default)]
    rocky: BTreeMap<String, f64>,
    #[serde(default)]
    ice: BTreeMap<String, f64>,
    #[serde(default)]
    gas_giant: BTreeMap<String, f64>,
}

impl FileChains {
    pub(crate) fn into_config(self) -> Result<ChainConfig, String> {
        let mut goods = Vec::new();
        for row in self.goods {
            let kind = GoodKind::parse(&row.kind)
                .ok_or_else(|| format!("unknown good kind {} for {}", row.kind, row.id))?;
            goods.push(GoodSpec { id: row.id, kind });
        }
        let mut recipes = Vec::new();
        for row in self.recipes {
            let building = BuildingKind::from_key(&row.building)
                .ok_or_else(|| format!("unknown recipe building {}", row.building))?;
            recipes.push(Recipe {
                id: row.id,
                building,
                deposit: row.deposit,
                inputs: row.inputs,
                outputs: row.outputs,
                heat: row.heat,
                pollution: row.pollution,
            });
        }
        let mut deposits = BTreeMap::new();
        deposits.insert(PlanetType::Rocky, self.deposits.rocky);
        deposits.insert(PlanetType::Ice, self.deposits.ice);
        deposits.insert(PlanetType::GasGiant, self.deposits.gas_giant);
        let config = ChainConfig {
            goods,
            recipes,
            deposits,
            deposit_jitter: self.deposit_jitter.unwrap_or(0.0),
        };
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ChainConfig {
        let mut deposits = BTreeMap::new();
        deposits.insert(
            PlanetType::Rocky,
            BTreeMap::from([("iron_ore".into(), 1.0)]),
        );
        ChainConfig {
            goods: vec![
                GoodSpec {
                    id: "iron_ore".into(),
                    kind: GoodKind::Raw,
                },
                GoodSpec {
                    id: "steel".into(),
                    kind: GoodKind::Refined,
                },
                GoodSpec {
                    id: "frames".into(),
                    kind: GoodKind::Component,
                },
            ],
            recipes: vec![
                Recipe {
                    id: "extract".into(),
                    building: BuildingKind::MetalMine,
                    deposit: Some("iron_ore".into()),
                    inputs: BTreeMap::new(),
                    outputs: BTreeMap::from([("iron_ore".into(), 1.0)]),
                    heat: 0.0,
                    pollution: 0.0,
                },
                Recipe {
                    id: "smelt".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("iron_ore".into(), 1.0)]),
                    outputs: BTreeMap::from([("steel".into(), 1.0)]),
                    heat: 0.0,
                    pollution: 0.0,
                },
                Recipe {
                    id: "forge".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("steel".into(), 1.0)]),
                    outputs: BTreeMap::from([("frames".into(), 1.0)]),
                    heat: 0.0,
                    pollution: 0.0,
                },
            ],
            deposits,
            deposit_jitter: 0.0,
        }
    }

    #[test]
    fn validate_rejects_mass_creation_and_cycles() {
        let mut cfg = base();
        cfg.recipes[1].outputs.insert("steel".into(), 2.0);
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("mass"), "{err}");
        cfg = base();
        cfg.recipes[1].inputs.insert("frames".into(), 1.0);
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("cycle") || err.contains("mass"), "{err}");
        cfg = base();
        cfg.recipes[2].inputs.insert("metal".into(), 1.0);
        cfg.recipes[2].inputs.insert("frames".into(), 1.0);
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("stockpile"), "{err}");
        cfg = base();
        cfg.recipes.swap(1, 2);
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("before a producer"), "{err}");
    }

    fn tick(
        cfg: &ChainConfig,
        buildings: Buildings,
        deposits: &BTreeMap<String, f64>,
        resources: &mut Resources,
        goods: &mut BTreeMap<String, f64>,
    ) {
        apply_recipes(
            RecipeRun {
                config: cfg,
                buildings,
                deposits,
                hours: 1.0,
                energy_factor: 1.0,
                boost: 1.0,
            },
            |_| 10.0,
            resources,
            goods,
            |_| 10_000.0,
        );
    }

    #[test]
    fn production_stops_when_the_input_runs_out() {
        let cfg = base();
        let buildings = Buildings {
            metal_mine: 1,
            ..Buildings::default()
        };
        let deposits = cfg.deposits_for(PlanetType::Rocky, 0, Coord::HOME);
        let mut resources = Resources::ZERO;
        let mut goods = BTreeMap::new();
        goods.insert("iron_ore".into(), 5.0);
        tick(&cfg, buildings, &deposits, &mut resources, &mut goods);
        // extract +10 ore, smelt uses 15 (5 stock + 10 new) limited by smelt rate 10 → 10 steel, 5 ore left, forge 10 frames
        assert!((resources.metal - 10.0).abs() < 1e-9, "{}", resources.metal);
        assert!((goods.get("iron_ore").copied().unwrap_or(0.0) - 5.0).abs() < 1e-9);
        assert!(goods.get("steel").copied().unwrap_or(0.0).abs() < 1e-9);
        goods.clear();
        resources = Resources::ZERO;
        tick(&cfg, buildings, &deposits, &mut resources, &mut goods);
        assert!((resources.metal - 10.0).abs() < 1e-9);
        let mut empty_dep = deposits.clone();
        empty_dep.insert("iron_ore".into(), 0.0);
        resources = Resources::ZERO;
        goods.clear();
        tick(&cfg, buildings, &empty_dep, &mut resources, &mut goods);
        assert!(resources.metal.abs() < 1e-9);
    }

    #[test]
    fn full_component_cap_does_not_consume_inputs() {
        let cfg = base();
        let buildings = Buildings {
            metal_mine: 1,
            ..Buildings::default()
        };
        let deposits = BTreeMap::from([("iron_ore".into(), 0.0)]);
        let mut resources = Resources {
            metal: 10.0,
            ..Resources::ZERO
        };
        let mut goods = BTreeMap::from([("steel".into(), 50.0)]);
        apply_recipes(
            RecipeRun {
                config: &cfg,
                buildings,
                deposits: &deposits,
                hours: 1.0,
                energy_factor: 1.0,
                boost: 1.0,
            },
            |_| 10.0,
            &mut resources,
            &mut goods,
            |_| 10.0,
        );
        assert!((resources.metal - 10.0).abs() < 1e-9, "{}", resources.metal);
        assert!((goods.get("steel").copied().unwrap_or(0.0) - 50.0).abs() < 1e-9);
        resources.metal = 6.0;
        apply_recipes(
            RecipeRun {
                config: &cfg,
                buildings,
                deposits: &deposits,
                hours: 1.0,
                energy_factor: 1.0,
                boost: 1.0,
            },
            |_| 10.0,
            &mut resources,
            &mut goods,
            |_| 10.0,
        );
        assert!((resources.metal - 10.0).abs() < 1e-9, "{}", resources.metal);
        assert!((goods.get("steel").copied().unwrap_or(0.0) - 46.0).abs() < 1e-9);
    }

    #[test]
    fn deposits_follow_type_and_seed() {
        let mut cfg = base();
        cfg.deposits
            .insert(PlanetType::Ice, BTreeMap::from([("iron_ore".into(), 0.2)]));
        cfg.deposit_jitter = 0.0;
        let rocky = cfg.deposits_for(PlanetType::Rocky, 1, Coord::HOME);
        let ice = cfg.deposits_for(PlanetType::Ice, 1, Coord::HOME);
        assert_eq!(rocky.get("iron_ore").copied(), Some(1.0));
        assert_eq!(ice.get("iron_ore").copied(), Some(0.2));
        cfg.deposit_jitter = 0.1;
        let a = cfg.deposits_for(PlanetType::Rocky, 7, Coord::HOME);
        let b = cfg.deposits_for(PlanetType::Rocky, 7, Coord::HOME);
        assert_eq!(a, b);
        let c = cfg.deposits_for(PlanetType::Rocky, 8, Coord::HOME);
        assert_ne!(a.get("iron_ore"), c.get("iron_ore"));
    }
}
