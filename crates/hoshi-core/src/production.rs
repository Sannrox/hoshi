//! Live production flows and the physical bottleneck that limits them.

use crate::chains::{Recipe, add_good, available, component_kind, take_good};
use crate::fleets::{Coord, MissionPhase};
use crate::state::{GameState, ResourceKind, Resources};
use std::collections::BTreeMap;

/// Why a recipe or planet is not running at full physical rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitingFactor {
    None,
    Energy,
    Heat,
    Pollution,
    Input {
        good: String,
    },
    FullBuffer {
        good: String,
    },
    Transport {
        good: String,
        from: Coord,
        eta_secs: Option<u64>,
    },
}

impl LimitingFactor {
    pub fn as_key(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Energy => "energy",
            Self::Heat => "heat",
            Self::Pollution => "pollution",
            Self::Input { .. } => "input",
            Self::FullBuffer { .. } => "buffer",
            Self::Transport { .. } => "transport",
        }
    }

    pub fn suggestion(&self) -> String {
        match self {
            Self::None => "flows are clear".into(),
            Self::Energy => "upgrade solar or cut energy draw".into(),
            Self::Heat => "build radiators to reject heat".into(),
            Self::Pollution => "build scrubbers to dump pollution".into(),
            Self::Input { good } => format!("feed {} into the chain", good.replace('_', " ")),
            Self::FullBuffer { good } => {
                format!("upgrade storage or spend {}", good.replace('_', " "))
            }
            Self::Transport {
                good,
                from,
                eta_secs,
            } => match eta_secs {
                Some(eta) => format!(
                    "await {} from {from} in {}m",
                    good.replace('_', " "),
                    eta.div_ceil(60)
                ),
                None => format!("move {} from {from}", good.replace('_', " ")),
            },
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Self::Energy => 0,
            Self::Heat => 1,
            Self::Pollution => 2,
            Self::Input { .. } => 3,
            Self::FullBuffer { .. } => 4,
            Self::Transport { .. } => 5,
            Self::None => 6,
        }
    }
}

/// One recipe (or mine) on one planet at the current instant.
#[derive(Debug, Clone, PartialEq)]
pub struct RecipeFlow {
    pub id: String,
    pub planet: Coord,
    pub inputs_per_hour: BTreeMap<String, f64>,
    pub outputs_per_hour: BTreeMap<String, f64>,
    pub buffers: BTreeMap<String, f64>,
    pub limited_by: LimitingFactor,
}

/// Empire-wide glance: every active step, energy, heat, and the worst bottleneck.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductionReport {
    pub steps: Vec<RecipeFlow>,
    pub energy_supply: f64,
    pub energy_demand: f64,
    pub heat_load: f64,
    pub heat_sink: f64,
    pub metal_per_hour: f64,
    pub crystal_per_hour: f64,
    pub deuterium_per_hour: f64,
    pub rare_earths_per_hour: f64,
    pub water_ice_per_hour: f64,
    pub helium_3_per_hour: f64,
    pub limited_by: LimitingFactor,
}

impl GameState {
    pub fn production_report(&self) -> ProductionReport {
        let mut steps = Vec::new();
        for coord in self.owned_coords() {
            steps.extend(self.flows_at(coord));
        }
        let mut energy_supply = 0.0;
        let mut energy_demand = 0.0;
        let mut heat_load = 0.0;
        let mut heat_sink = 0.0;
        for coord in self.owned_coords() {
            let Some(buildings) = self.operational_buildings_at(coord) else {
                continue;
            };
            let Some(kind) = self.planet_kind(coord) else {
                continue;
            };
            energy_supply += self.site_energy_produced(coord, buildings, kind);
            energy_demand += self.energy_consumed_at(coord);
            if let Some(waste) = self.waste_at(coord) {
                heat_load += waste.heat_load;
                heat_sink += waste.heat_sink;
            }
        }
        let limited_by = steps
            .iter()
            .map(|s| s.limited_by.clone())
            .min_by_key(LimitingFactor::rank)
            .unwrap_or(LimitingFactor::None);
        ProductionReport {
            steps,
            energy_supply,
            energy_demand,
            heat_load,
            heat_sink,
            metal_per_hour: self.empire_production_per_hour(ResourceKind::Iron),
            crystal_per_hour: self.empire_production_per_hour(ResourceKind::Silicon),
            deuterium_per_hour: self.empire_production_per_hour(ResourceKind::Deuterium),
            rare_earths_per_hour: self.empire_production_per_hour(ResourceKind::RareEarths),
            water_ice_per_hour: self.empire_production_per_hour(ResourceKind::WaterIce),
            helium_3_per_hour: self.empire_production_per_hour(ResourceKind::Helium3),
            limited_by,
        }
    }

    pub(crate) fn flows_at(&self, coord: Coord) -> Vec<RecipeFlow> {
        let Some(buildings) = self.operational_buildings_at(coord) else {
            return Vec::new();
        };
        let Some(kind) = self.planet_kind(coord) else {
            return Vec::new();
        };
        let extra = self.site_extra(coord);
        let now = self.last_tick_unix;
        if self.config.chains.enabled() {
            let mut resources = self.resources_at(coord).copied().unwrap_or(Resources::ZERO);
            let mut goods = self.goods_at(coord);
            let mut steps = Vec::new();
            for recipe in &self.config.chains.recipes {
                if buildings.get(recipe.building) == 0 {
                    continue;
                }
                let scale = self.recipe_building_scale(
                    buildings,
                    kind,
                    extra,
                    recipe.building,
                    (now, now),
                    coord,
                );
                let flow =
                    self.flow_for_recipe(coord, recipe, scale, buildings, &resources, &goods);
                for (id, amount) in &flow.inputs_per_hour {
                    take_good(&mut resources, &mut goods, id, *amount);
                }
                for (id, amount) in &flow.outputs_per_hour {
                    let cap = component_kind(id).map(|kind| self.storage_cap_for(buildings, kind));
                    add_good(&mut resources, &mut goods, id, *amount, cap);
                }
                steps.push(flow);
            }
            return steps;
        }
        let energy = self.site_energy_factor(coord, buildings, kind);
        let resources = self.resources_at(coord).copied().unwrap_or(Resources::ZERO);
        self.legacy_mine_flows(coord, buildings, energy, &resources)
    }

    fn site_unrest(&self, coord: Coord) -> f64 {
        self.waste_at(coord)
            .map(|w| w.unrest_factor(self.config.waste.unrest_per_overflow))
            .unwrap_or(1.0)
    }

    fn waste_limiter(&self, coord: Coord) -> LimitingFactor {
        let Some(waste) = self.waste_at(coord) else {
            return LimitingFactor::None;
        };
        if waste.heat_overflow() > 1e-12 {
            LimitingFactor::Heat
        } else if waste.pollution_overflow() > 1e-12 {
            LimitingFactor::Pollution
        } else {
            LimitingFactor::None
        }
    }

    fn storage_cap_for(&self, buildings: crate::Buildings, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => {
                self.config.metal_storage.capacity(buildings.metal_storage)
            }
            ResourceKind::Silicon => self
                .config
                .crystal_storage
                .capacity(buildings.crystal_storage),
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => self
                .config
                .deuterium_storage
                .capacity(buildings.deuterium_storage),
        }
    }

    pub(crate) fn site_extra(&self, coord: Coord) -> (f64, f64, f64) {
        if coord == self.home {
            return (0.0, 0.0, 0.0);
        }
        let rare = self
            .colonies
            .iter()
            .find(|c| c.coord == coord)
            .and_then(|c| c.rare.as_ref())
            .and_then(|id| {
                self.config
                    .outposts
                    .rare
                    .iter()
                    .find(|row| row.id == *id)
                    .map(|row| (row.iron, row.silicon, row.deuterium))
            })
            .unwrap_or((0.0, 0.0, 0.0));
        let bonus = self.frontier_production_bonus(coord);
        (rare.0 + bonus, rare.1 + bonus, rare.2 + bonus)
    }

    pub(crate) fn site_energy_produced(
        &self,
        coord: Coord,
        buildings: crate::Buildings,
        planet: crate::PlanetType,
    ) -> f64 {
        self.site_solar_produced(coord, buildings, planet)
            + self.reactor_output_at(coord)
            + self.fusion_output_at(coord)
    }

    pub(crate) fn site_solar_produced(
        &self,
        coord: Coord,
        buildings: crate::Buildings,
        planet: crate::PlanetType,
    ) -> f64 {
        if self.dismantling(coord) == Some(crate::BuildingKind::SolarPlant) {
            return 0.0;
        }
        if coord == self.home {
            return self.config.solar_plant.energy_output(buildings.solar_plant)
                * self.config.orbits.solar_flux(coord)
                * (1.0 + self.modifier(crate::ModifierKind::EnergyOutput)).max(0.0);
        }
        let bonus = self.config.planet_types.get(planet);
        let rare = self
            .colonies
            .iter()
            .find(|c| c.coord == coord)
            .and_then(|c| c.rare.as_ref())
            .and_then(|id| self.config.outposts.rare.iter().find(|r| r.id == *id))
            .map(|r| r.energy)
            .unwrap_or(0.0);
        self.config.solar_plant.energy_output(buildings.solar_plant)
            * self.config.orbits.solar_flux(coord)
            * (1.0 + self.tech_modifier(crate::ModifierKind::EnergyOutput) + bonus.energy + rare)
                .max(0.0)
    }

    fn site_energy_factor(
        &self,
        coord: Coord,
        buildings: crate::Buildings,
        planet: crate::PlanetType,
    ) -> f64 {
        let produced = self.site_energy_produced(coord, buildings, planet);
        let consumed = self.energy_consumed_at(coord);
        if consumed <= 0.0 {
            1.0
        } else {
            (produced / consumed).clamp(0.0, 1.0)
        }
    }

    fn goods_at(&self, coord: Coord) -> BTreeMap<String, f64> {
        if coord == self.home {
            self.goods.clone()
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| c.goods.clone())
                .unwrap_or_default()
        }
    }

    fn flow_for_recipe(
        &self,
        coord: Coord,
        recipe: &Recipe,
        building_scale: f64,
        buildings: crate::Buildings,
        resources: &Resources,
        goods: &BTreeMap<String, f64>,
    ) -> RecipeFlow {
        let kind = self.planet_kind(coord).unwrap_or(crate::PlanetType::Rocky);
        let energy = self.site_energy_factor(coord, buildings, kind);
        let unrest = self.site_unrest(coord);
        let deposits = self.chain_deposits(kind, self.galaxy_seed, coord);
        let deposit = recipe
            .deposit
            .as_ref()
            .map(|id| deposits.get(id).copied().unwrap_or(0.0))
            .unwrap_or(1.0);
        let throttle = energy.max(0.0)
            * unrest.max(0.0)
            * self.emission_factor(coord).max(0.0)
            * self.prod_boost_mult().max(0.0);
        let scale = building_scale.max(0.0) * throttle * deposit;
        let mut limited = 1.0_f64;
        let mut limiter = if energy < 1.0 - 1e-12 {
            LimitingFactor::Energy
        } else {
            self.waste_limiter(coord)
        };
        for (id, qty) in &recipe.outputs {
            if *qty <= 0.0 {
                continue;
            }
            let Some(kind) = component_kind(id) else {
                continue;
            };
            let cap = self.storage_cap_for(buildings, kind);
            let current = resources.get(kind);
            if current + 1e-9 >= cap {
                if matches!(limiter, LimitingFactor::None) {
                    limiter = LimitingFactor::FullBuffer { good: id.clone() };
                }
                limited = 0.0;
                break;
            }
        }
        if limited > 0.0 {
            for (id, qty) in &recipe.inputs {
                if *qty <= 0.0 || scale <= 0.0 {
                    continue;
                }
                let need = *qty * scale;
                let have = available(resources, goods, id);
                if have + 1e-9 < need {
                    if matches!(limiter, LimitingFactor::None) {
                        limiter = self
                            .transport_for(coord, id)
                            .unwrap_or_else(|| LimitingFactor::Input { good: id.clone() });
                    }
                    limited = if need > 0.0 {
                        (have / need).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    break;
                }
            }
        }
        let used = scale * limited * self.governor_factor(coord);
        let mut inputs_per_hour = BTreeMap::new();
        for (id, qty) in &recipe.inputs {
            if *qty > 0.0 {
                inputs_per_hour.insert(id.clone(), *qty * used);
            }
        }
        let mut outputs_per_hour = BTreeMap::new();
        for (id, qty) in &recipe.outputs {
            if *qty > 0.0 {
                outputs_per_hour.insert(id.clone(), *qty * used);
            }
        }
        let mut buffers = BTreeMap::new();
        for id in recipe.inputs.keys().chain(recipe.outputs.keys()) {
            buffers.insert(id.clone(), available(resources, goods, id));
        }
        RecipeFlow {
            id: recipe.id.clone(),
            planet: coord,
            inputs_per_hour,
            outputs_per_hour,
            buffers,
            limited_by: limiter,
        }
    }

    fn transport_for(&self, dest: Coord, good: &str) -> Option<LimitingFactor> {
        if let Some(kind) = component_kind(good) {
            for mission in &self.missions {
                if mission.transport
                    && mission.dest == dest
                    && mission.phase == MissionPhase::Outbound
                {
                    let amount = mission.cargo.get(kind);
                    if amount > 1e-9 {
                        let eta = mission.phase_ends_at.saturating_sub(self.last_tick_unix);
                        return Some(LimitingFactor::Transport {
                            good: good.to_string(),
                            from: mission.origin,
                            eta_secs: Some(eta),
                        });
                    }
                }
            }
        }
        for coord in self.owned_coords() {
            if coord == dest {
                continue;
            }
            let resources = self.resources_at(coord).copied().unwrap_or(Resources::ZERO);
            let goods = self.goods_at(coord);
            if available(&resources, &goods, good) > 1e-9 {
                return Some(LimitingFactor::Transport {
                    good: good.to_string(),
                    from: coord,
                    eta_secs: None,
                });
            }
        }
        None
    }

    fn legacy_mine_flows(
        &self,
        coord: Coord,
        buildings: crate::Buildings,
        energy: f64,
        resources: &Resources,
    ) -> Vec<RecipeFlow> {
        let limiter = if energy < 1.0 - 1e-12 {
            LimitingFactor::Energy
        } else {
            self.waste_limiter(coord)
        };
        let mut steps = Vec::new();
        for (id, kind, level) in [
            ("metal_mine", ResourceKind::Iron, buildings.metal_mine),
            (
                "crystal_mine",
                ResourceKind::Silicon,
                buildings.crystal_mine,
            ),
            (
                "deuterium_synth",
                ResourceKind::Deuterium,
                buildings.deuterium_synthesizer,
            ),
        ] {
            if level == 0 {
                continue;
            }
            let mut row_limit = limiter.clone();
            let cap = self.storage_cap_for(buildings, kind);
            let current = resources.get(kind);
            if matches!(row_limit, LimitingFactor::None) && current + 1e-9 >= cap {
                row_limit = LimitingFactor::FullBuffer {
                    good: kind.label().into(),
                };
            }
            let rate = if matches!(row_limit, LimitingFactor::FullBuffer { .. }) {
                0.0
            } else {
                self.production_at(coord, kind)
            };
            let mut outputs = BTreeMap::new();
            outputs.insert(kind.label().into(), rate);
            let mut buffers = BTreeMap::new();
            buffers.insert(kind.label().into(), current);
            steps.push(RecipeFlow {
                id: id.into(),
                planet: coord,
                inputs_per_hour: BTreeMap::new(),
                outputs_per_hour: outputs,
                buffers,
                limited_by: row_limit,
            });
        }
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildings::Buildings;
    use crate::chains::{ChainConfig, GoodKind, GoodSpec};
    use crate::fleets::Mission;
    use crate::planets::{Colony, PlanetType};
    use crate::{BalanceConfig, BuildingKind, Fleet, GameState};

    fn close(got: f64, want: f64) {
        assert!((got - want).abs() < 1e-6, "got {got} want {want}");
    }

    fn test_config() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.leaders.empty_governor = 0.0;
        cfg.leaders.empty_admiral = 0.0;
        cfg
    }

    fn chain_cfg() -> ChainConfig {
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
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
                Recipe {
                    id: "smelt".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("iron_ore".into(), 1.0)]),
                    outputs: BTreeMap::from([("steel".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
                Recipe {
                    id: "forge".into(),
                    building: BuildingKind::MetalMine,
                    deposit: None,
                    inputs: BTreeMap::from([("steel".into(), 1.0)]),
                    outputs: BTreeMap::from([("frames".into(), 1.0)]),
                    energy: 0.0,
                    heat: 0.0,
                    pollution: 0.0,
                },
            ],
            deposits,
            deposit_jitter: 0.0,
        }
    }

    fn smelt_only() -> ChainConfig {
        let mut cfg = chain_cfg();
        cfg.recipes.retain(|recipe| recipe.id != "extract");
        cfg
    }

    #[test]
    fn orbital_solar_output_and_colony_advice() {
        let mut cfg = test_config();
        cfg.chains = ChainConfig::default();
        let mut state = GameState::with_config(0, cfg);
        let rated = state.config.solar_plant.energy_output(1);
        state.buildings.solar_plant = 1;
        close(state.energy_produced(), rated);
        for (position, flux) in [(11, 0.25), (16, 0.16)] {
            let coord = Coord {
                galaxy: 1,
                system: 1,
                position,
            };
            for kind in [PlanetType::Rocky, PlanetType::Ice] {
                let buildings = Buildings {
                    solar_plant: 1,
                    ..Buildings::default()
                };
                close(
                    state.site_energy_produced(coord, buildings, kind),
                    rated * flux * (1.0 + state.config.planet_types.get(kind).energy),
                );
            }
        }
        let coord = Coord {
            galaxy: 1,
            system: 1,
            position: 16,
        };
        state.colonies.push(Colony {
            coord,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::ZERO,
            buildings: Buildings {
                metal_mine: 1,
                solar_plant: 1,
                ..Buildings::default()
            },
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
        let report = state.production_report();
        close(report.energy_supply, state.energy_produced() + rated * 0.16);
        assert!(
            crate::advisor::advise(&state, None)
                .iter()
                .any(|a| a.id == "energy" && a.text.contains(&coord.to_string()))
        );
        let mut stepped = state.clone();
        state.catch_up(3_600);
        for now in 1..=3_600 {
            stepped.catch_up(now);
        }
        close(
            state.colonies[0].resources.iron,
            stepped.colonies[0].resources.iron,
        );
    }

    #[test]
    fn input_shortage_is_the_limiter() {
        let mut cfg = test_config();
        cfg.chains = smelt_only();
        cfg.starting_buildings.crystal_mine = 0;
        cfg.solar_plant.base_energy = 1_000.0;
        let mut state = GameState::with_config(0, cfg);
        state.goods.clear();
        let report = state.production_report();
        assert_eq!(report.limited_by.as_key(), "input");
        let smelt = report.steps.iter().find(|s| s.id == "smelt").unwrap();
        assert_eq!(smelt.limited_by.as_key(), "input");
        assert!(report.limited_by.suggestion().contains("iron ore"));
    }

    #[test]
    fn extract_feeds_smelt_in_the_same_hour() {
        let mut cfg = test_config();
        cfg.chains = chain_cfg();
        cfg.starting_buildings.crystal_mine = 0;
        cfg.solar_plant.base_energy = 1_000.0;
        let mut state = GameState::with_config(0, cfg);
        state.goods.clear();
        let report = state.production_report();
        let smelt = report.steps.iter().find(|s| s.id == "smelt").unwrap();
        assert_ne!(smelt.limited_by.as_key(), "input");
        assert!(*smelt.outputs_per_hour.get("steel").unwrap_or(&0.0) > 0.0);
    }

    #[test]
    fn energy_shortage_is_the_limiter() {
        let mut cfg = test_config();
        cfg.starting_buildings.solar_plant = 0;
        let state = GameState::with_config(0, cfg);
        close(state.energy_factor(), 0.0);
        let report = state.production_report();
        assert_eq!(report.limited_by.as_key(), "energy");
        assert!(report.energy_demand > report.energy_supply);
    }

    #[test]
    fn heat_overflow_is_the_limiter() {
        let mut cfg = test_config();
        cfg.chains = chain_cfg();
        cfg.chains.recipes[0].heat = 10_000.0;
        cfg.solar_plant.base_energy = 1_000.0;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_buildings.radiator = 0;
        let mut state = GameState::with_config(0, cfg);
        state.goods.insert("iron_ore".into(), 100.0);
        assert!(state.waste_home().overflowing());
        let report = state.production_report();
        assert_eq!(report.limited_by.as_key(), "heat");
        assert!(report.heat_load > report.heat_sink);
        assert!(report.limited_by.suggestion().contains("radiator"));
    }

    #[test]
    fn pollution_overflow_is_the_limiter() {
        let mut cfg = test_config();
        cfg.chains = chain_cfg();
        cfg.chains.recipes[0].pollution = 10_000.0;
        cfg.solar_plant.base_energy = 1_000.0;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_buildings.scrubber = 0;
        let mut state = GameState::with_config(0, cfg);
        state.goods.insert("iron_ore".into(), 100.0);
        assert!(state.waste_home().pollution_overflow() > 0.0);
        assert!(state.waste_home().heat_overflow() <= 1e-12);
        let report = state.production_report();
        assert_eq!(report.limited_by.as_key(), "pollution");
        assert!(report.limited_by.suggestion().contains("scrubber"));
    }

    #[test]
    fn full_buffer_is_the_limiter() {
        let mut cfg = test_config();
        cfg.chains = chain_cfg();
        cfg.solar_plant.base_energy = 1_000.0;
        cfg.starting_buildings.crystal_mine = 0;
        let mut state = GameState::with_config(0, cfg);
        state.goods.insert("steel".into(), 50.0);
        state.resources.iron = state.storage_cap(ResourceKind::Iron);
        let report = state.production_report();
        let forge = report.steps.iter().find(|s| s.id == "forge").unwrap();
        assert_eq!(forge.limited_by.as_key(), "buffer");
    }

    #[test]
    fn transport_delay_is_the_limiter() {
        let mut cfg = test_config();
        cfg.chains = smelt_only();
        cfg.solar_plant.base_energy = 1_000.0;
        cfg.starting_buildings.crystal_mine = 0;
        let mut state = GameState::with_config(0, cfg);
        state.goods.clear();
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        state.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::ZERO,
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::from([("iron_ore".into(), 40.0)]),
        });
        let report = state.production_report();
        let smelt = report
            .steps
            .iter()
            .find(|s| s.id == "smelt" && s.planet == state.home)
            .unwrap();
        assert_eq!(smelt.limited_by.as_key(), "transport");
        match &smelt.limited_by {
            LimitingFactor::Transport { good, from, .. } => {
                assert_eq!(good, "iron_ore");
                assert_eq!(*from, dest);
            }
            other => panic!("{other:?}"),
        }
        state.missions.push(Mission {
            id: 1,
            ships: Fleet::default(),
            origin: dest,
            dest: state.home,
            speed_percent: 100,
            phase: MissionPhase::Outbound,
            launched_at: 0,
            outbound_secs: 3_600,
            hold_secs: 0,
            phase_ends_at: 3_600,
            fuel_paid: 0.0,
            colonize: false,
            expedition_seed: None,
            expedition_applied: false,
            cargo: Resources::from_parts(10.0, 0.0, 0.0),
            transport: true,
            attack: false,
            invade: false,
            spy: false,
            site_id: None,
        });
        let with_flight = state.production_report();
        let forge = with_flight.steps.iter().find(|s| s.id == "forge").unwrap();
        let _ = forge;
        let smelt = with_flight
            .steps
            .iter()
            .find(|s| s.id == "smelt" && s.planet == state.home)
            .unwrap();
        assert_eq!(smelt.limited_by.as_key(), "transport");
    }

    #[test]
    fn energy_shortage_scales_chain_rate_once() {
        let mut cfg = test_config();
        cfg.chains = chain_cfg();
        cfg.solar_plant.base_energy = 10.0;
        let state = GameState::with_config(0, cfg);
        close(state.energy_factor(), 0.5);
        let now = state.last_tick_unix;
        let building = state.recipe_building_scale(
            state.buildings,
            state.home_kind,
            (0.0, 0.0, 0.0),
            BuildingKind::MetalMine,
            (now, now),
            state.home,
        );
        let report = state.production_report();
        assert_eq!(report.limited_by.as_key(), "energy");
        let extract = report.steps.iter().find(|s| s.id == "extract").unwrap();
        let got = *extract.outputs_per_hour.get("iron_ore").unwrap_or(&0.0);
        close(got, building * 0.5);
    }

    #[test]
    fn energy_shortfall_still_applies_empty_inputs() {
        let mut cfg = test_config();
        cfg.chains = smelt_only();
        cfg.solar_plant.base_energy = 10.0;
        let mut state = GameState::with_config(0, cfg);
        state.goods.clear();
        close(state.energy_factor(), 0.5);
        let report = state.production_report();
        assert_eq!(report.limited_by.as_key(), "energy");
        let smelt = report.steps.iter().find(|s| s.id == "smelt").unwrap();
        close(*smelt.outputs_per_hour.get("steel").unwrap_or(&0.0), 0.0);
        assert_eq!(smelt.limited_by.as_key(), "energy");
    }

    #[test]
    fn colony_legacy_mines_report_live_rates() {
        let mut cfg = test_config();
        cfg.chains = ChainConfig::default();
        let mut state = GameState::with_config(0, cfg);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        state.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::ZERO,
            buildings: Buildings {
                metal_mine: 1,
                solar_plant: 10,
                ..Buildings::default()
            },
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
        let report = state.production_report();
        let mine = report
            .steps
            .iter()
            .find(|s| s.id == "metal_mine" && s.planet == dest)
            .unwrap();
        let rate = *mine.outputs_per_hour.get("iron").unwrap_or(&0.0);
        assert!(rate > 0.0, "colony mine rate was {rate}");
        let before = state.colonies[0].resources.iron;
        state.catch_up(3_600);
        close(state.colonies[0].resources.iron - before, rate);
    }

    #[test]
    fn colony_buffer_uses_local_storage() {
        let mut cfg = test_config();
        cfg.chains = ChainConfig::default();
        cfg.starting_buildings.metal_storage = 5;
        let mut state = GameState::with_config(0, cfg);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        let buildings = Buildings {
            metal_mine: 1,
            solar_plant: 10,
            metal_storage: 0,
            ..Buildings::default()
        };
        let colony_cap = state.config.metal_storage.capacity(0);
        let home_cap = state.storage_cap(ResourceKind::Iron);
        assert!(home_cap > colony_cap + 1.0);
        state.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::from_parts(colony_cap, 0.0, 0.0),
            buildings,
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
        let report = state.production_report();
        let mine = report
            .steps
            .iter()
            .find(|s| s.id == "metal_mine" && s.planet == dest)
            .unwrap();
        assert_eq!(mine.limited_by.as_key(), "buffer");
        close(*mine.outputs_per_hour.get("metal").unwrap_or(&0.0), 0.0);
    }

    #[test]
    fn empire_energy_and_heat_include_colonies() {
        let mut cfg = test_config();
        cfg.chains = ChainConfig::default();
        let mut state = GameState::with_config(0, cfg);
        let home_supply = state.energy_produced();
        let home_demand = state.energy_consumed();
        let home_waste = state.waste_home();
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        state.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::ZERO,
            buildings: Buildings {
                metal_mine: 8,
                solar_plant: 12,
                ..Buildings::default()
            },
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
        let report = state.production_report();
        let colony_waste = state.waste_at(dest).unwrap();
        assert!(report.energy_supply > home_supply);
        assert!(report.energy_demand > home_demand);
        close(
            report.heat_load,
            home_waste.heat_load + colony_waste.heat_load,
        );
        close(
            report.heat_sink,
            home_waste.heat_sink + colony_waste.heat_sink,
        );
    }
}
