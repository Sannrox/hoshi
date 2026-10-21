//! Fuel-limited fission power and the heat left after a protective shutdown.

use serde::Deserialize;

/// Last protective shutdown on a planet, retained after repairs finish.
#[derive(Debug, Clone, PartialEq)]
pub struct ReactorShutdown {
    pub at: u64,
    pub thermal_power: f64,
}

/// Electrical power, burnup, cooling-pool capacity, and decay-heat parameters.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReactorConfig {
    pub base_energy: f64,
    pub growth: f64,
    pub reactor_fuel_per_energy_hour: f64,
    pub spent_fuel_per_level: f64,
    pub waste_heat_per_energy: f64,
    pub decay_initial_fraction: f64,
    pub decay_day_fraction: f64,
    pub decay_time_constant_secs: f64,
}

impl Default for ReactorConfig {
    fn default() -> Self {
        Self {
            base_energy: 60.0,
            growth: 1.1,
            reactor_fuel_per_energy_hour: 0.01,
            spent_fuel_per_level: 30.0,
            waste_heat_per_energy: 2.0,
            decay_initial_fraction: 0.07,
            decay_day_fraction: 0.01,
            decay_time_constant_secs: 3_600.0,
        }
    }
}

impl ReactorConfig {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("base_energy", self.base_energy),
            ("growth", self.growth),
            (
                "reactor_fuel_per_energy_hour",
                self.reactor_fuel_per_energy_hour,
            ),
            ("spent_fuel_per_level", self.spent_fuel_per_level),
            ("waste_heat_per_energy", self.waste_heat_per_energy),
            ("decay_time_constant_secs", self.decay_time_constant_secs),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("fission_reactor.{name} must be finite and > 0"));
            }
        }
        if !self.decay_initial_fraction.is_finite()
            || !self.decay_day_fraction.is_finite()
            || self.decay_day_fraction <= 0.0
            || self.decay_day_fraction >= self.decay_initial_fraction
            || self.decay_initial_fraction >= 1.0
        {
            return Err(
                "fission_reactor decay fractions must satisfy 0 < day < initial < 1".into(),
            );
        }
        Ok(())
    }

    pub fn rated_output(&self, level: u32) -> f64 {
        if level == 0 {
            0.0
        } else {
            self.base_energy * self.growth.powf(f64::from(level - 1))
        }
    }

    pub fn pool_capacity(&self, level: u32) -> f64 {
        self.spent_fuel_per_level * f64::from(level)
    }

    /// Cap delivered electricity by cooling, available fuel, and pool space.
    /// `hours` is the simulation interval being integrated, never wall-clock time.
    pub fn output_for_interval(
        &self,
        level: u32,
        free_heat_sink: f64,
        fuel: f64,
        spent: f64,
        hours: f64,
    ) -> f64 {
        if hours <= 0.0 || fuel <= 0.0 {
            return 0.0;
        }
        let burnable = fuel.min((self.pool_capacity(level) - spent).max(0.0));
        self.rated_output(level)
            .min(free_heat_sink.max(0.0) / self.waste_heat_per_energy)
            .min(burnable / (self.reactor_fuel_per_energy_hour * hours))
    }

    fn cooled_output_average(
        &self,
        rated: f64,
        free_sink: f64,
        shutdown: Option<&ReactorShutdown>,
        from: u64,
        hours: f64,
    ) -> f64 {
        let Some(shutdown) = shutdown else {
            return rated.min(free_sink.max(0.0) / self.waste_heat_per_energy);
        };
        let duration = hours * 3600.0;
        if duration <= 0.0 {
            return 0.0;
        }
        let start = from.saturating_sub(shutdown.at) as f64;
        let end = start + duration;
        let exponent = (self.decay_initial_fraction / self.decay_day_fraction).ln()
            / (1.0 + 86_400.0 / self.decay_time_constant_secs).ln();
        let initial = shutdown.thermal_power * self.decay_initial_fraction;
        let tau = self.decay_time_constant_secs;
        let positive_integral = |sink: f64| {
            if sink <= 0.0 {
                return 0.0;
            }
            let crossing = tau * ((initial / sink).powf(1.0 / exponent) - 1.0);
            let lower = start.max(crossing).min(end);
            let decay_integral = if (exponent - 1.0).abs() < 1e-10 {
                initial * tau * ((1.0 + end / tau) / (1.0 + lower / tau)).ln()
            } else {
                initial * tau / (1.0 - exponent)
                    * ((1.0 + end / tau).powf(1.0 - exponent)
                        - (1.0 + lower / tau).powf(1.0 - exponent))
            };
            (sink * (end - lower) - decay_integral).max(0.0)
        };
        ((positive_integral(free_sink)
            - positive_integral(free_sink - rated * self.waste_heat_per_energy))
            / duration
            / self.waste_heat_per_energy)
            .clamp(0.0, rated)
    }

    /// Shutdown thermal power includes both electrical output and rejected heat.
    pub fn rated_thermal_power(&self, level: u32) -> f64 {
        self.rated_output(level) * (1.0 + self.waste_heat_per_energy)
    }

    /// A continuous power-law cooling curve: initial fraction at shutdown,
    /// configured day fraction after 24 hours, then continuing to decline.
    pub fn decay_heat(&self, thermal_power: f64, elapsed_secs: u64) -> f64 {
        self.decay_heat_seconds(thermal_power, elapsed_secs as f64)
    }

    fn decay_heat_seconds(&self, thermal_power: f64, elapsed_secs: f64) -> f64 {
        let exponent = (self.decay_initial_fraction / self.decay_day_fraction).ln()
            / (1.0 + 86_400.0 / self.decay_time_constant_secs).ln();
        thermal_power
            * self.decay_initial_fraction
            * (1.0 + elapsed_secs / self.decay_time_constant_secs).powf(-exponent)
    }
}

// Adaptive Simpson integration handles cooling and energy-cap crossings without
// making offline cost proportional to elapsed seconds.
fn integrate_unit_interval(f: &impl Fn(f64) -> f64) -> f64 {
    fn integrate(
        f: &impl Fn(f64) -> f64,
        a: f64,
        b: f64,
        values: [f64; 3],
        whole: f64,
        depth: u8,
    ) -> f64 {
        let [left, mid, right] = values;
        let center = (a + b) / 2.0;
        let lm = f((a + center) / 2.0);
        let rm = f((center + b) / 2.0);
        let first = (center - a) * (left + 4.0 * lm + mid) / 6.0;
        let second = (b - center) * (mid + 4.0 * rm + right) / 6.0;
        let delta = first + second - whole;
        if depth == 0 || delta.abs() < 1e-12 * (b - a) {
            first + second + delta / 15.0
        } else {
            integrate(f, a, center, [left, lm, mid], first, depth - 1)
                + integrate(f, center, b, [mid, rm, right], second, depth - 1)
        }
    }
    let values = [f(0.0), f(0.5), f(1.0)];
    let whole = (values[0] + 4.0 * values[1] + values[2]) / 6.0;
    integrate(f, 0.0, 1.0, values, whole, 16)
}

impl crate::GameState {
    /// Electrical output after fuel, on-site pool, maintenance, and cooling limits.
    pub fn reactor_output_at(&self, coord: crate::Coord) -> f64 {
        let power = self.reactor_output_for_hours(coord, 1.0 / 3600.0);
        if power <= 0.0 {
            return 0.0;
        }
        let Some(waste) = self.base_waste_at(coord) else {
            return 0.0;
        };
        power.min(
            ((waste.heat_sink - waste.heat_load - self.reactor_decay_heat_at(coord))
                / self.config.fission_reactor.waste_heat_per_energy)
                .max(0.0),
        )
    }

    pub(crate) fn reactor_output_for_hours(&self, coord: crate::Coord, hours: f64) -> f64 {
        let Some(buildings) = self.buildings_at(coord) else {
            return 0.0;
        };
        if buildings.fission_reactor == 0
            || self.building_health(coord, crate::BuildingKind::FissionReactor) == 0.0
        {
            return 0.0;
        }
        let Some(waste) = self.base_waste_at(coord) else {
            return 0.0;
        };
        let goods = if coord == self.home {
            Some(&self.goods)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| &c.goods)
        };
        let Some(goods) = goods else {
            return 0.0;
        };
        let config = &self.config.fission_reactor;
        let rated = config.rated_output(buildings.fission_reactor)
            * self.building_health(coord, crate::BuildingKind::FissionReactor);
        let power = config.cooled_output_average(
            rated,
            waste.heat_sink - waste.heat_load,
            self.reactor_shutdowns.get(&coord),
            self.last_tick_unix,
            hours,
        );
        if hours <= 0.0 {
            return 0.0;
        }
        let fuel = goods.get("reactor_fuel").copied().unwrap_or(0.0);
        let spent = goods.get("spent_fuel").copied().unwrap_or(0.0);
        let power = power.min(
            (config.pool_capacity(buildings.fission_reactor) - spent).max(0.0)
                / (config.reactor_fuel_per_energy_hour * hours),
        );
        if power <= 0.0 {
            return 0.0;
        }
        if !self
            .config
            .chains
            .recipes
            .iter()
            .any(|recipe| recipe.outputs.contains_key("reactor_fuel"))
        {
            return power.min(fuel / (config.reactor_fuel_per_energy_hour * hours));
        }
        if fuel >= power * hours * config.reactor_fuel_per_energy_hour {
            return power;
        }
        let balance = |output| {
            fuel + self.fabricated_fuel_for_power(coord, hours, output)
                - output * hours * config.reactor_fuel_per_energy_hour
        };
        let at_zero = balance(0.0);
        if at_zero <= 0.0 {
            return 0.0;
        }
        let at_max = balance(power);
        if at_max >= 0.0 {
            return power;
        }
        let candidate = power * at_zero / (at_zero - at_max);
        if balance(candidate).abs() < 1e-12 {
            return candidate;
        }
        let mut low = 0.0;
        let mut high = power;
        for _ in 0..40 {
            let mid = (low + high) / 2.0;
            if balance(mid) >= 0.0 {
                low = mid;
            } else {
                high = mid;
            }
        }
        low
    }

    /// Integrate the energy and unrest multipliers together while shutdown heat cools.
    pub(crate) fn reactor_production_factor(&self, coord: crate::Coord, hours: f64) -> f64 {
        self.reactor_factor_at_power(coord, hours, self.reactor_output_for_hours(coord, hours))
    }

    fn reactor_factor_at_power(&self, coord: crate::Coord, hours: f64, delivered: f64) -> f64 {
        let Some(buildings) = self.operational_buildings_at(coord) else {
            return 0.0;
        };
        let Some(planet) = self.planet_kind(coord) else {
            return 0.0;
        };
        let Some(base) = self.base_waste_at(coord) else {
            return 0.0;
        };
        let solar = self.site_solar_produced(coord, buildings, planet);
        let demand = if coord == self.home {
            self.energy_consumed()
        } else {
            self.energy_consumed_at(coord)
        };
        let energy_factor = |power: f64| {
            if coord == self.home {
                self.energy_factor_for(solar + power)
            } else if demand <= 0.0 {
                1.0
            } else {
                ((solar + power) / demand).clamp(0.0, 1.0)
            }
        };
        let Some(shutdown) = self.reactor_shutdowns.get(&coord) else {
            let mut waste = base;
            waste.heat_load += delivered * self.config.fission_reactor.waste_heat_per_energy;
            return energy_factor(delivered)
                * waste.unrest_factor(self.config.waste.unrest_per_overflow);
        };
        let rated = self
            .config
            .fission_reactor
            .rated_output(buildings.fission_reactor)
            * self.building_health(coord, crate::BuildingKind::FissionReactor);
        let unconstrained = self.config.fission_reactor.cooled_output_average(
            rated,
            base.heat_sink - base.heat_load,
            Some(shutdown),
            self.last_tick_unix,
            hours,
        );
        let fuel_factor = if unconstrained > 0.0 {
            (delivered / unconstrained).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let initial_age = self.last_tick_unix.saturating_sub(shutdown.at) as f64;
        let sample = |fraction: f64| {
            let decay = self.config.fission_reactor.decay_heat_seconds(
                shutdown.thermal_power,
                initial_age + fraction * hours * 3600.0,
            );
            let power = rated.min(
                ((base.heat_sink - base.heat_load - decay)
                    / self.config.fission_reactor.waste_heat_per_energy)
                    .max(0.0),
            ) * fuel_factor;
            let mut waste = base;
            waste.heat_load += decay + power * self.config.fission_reactor.waste_heat_per_energy;
            energy_factor(power) * waste.unrest_factor(self.config.waste.unrest_per_overflow)
        };
        integrate_unit_interval(&sample)
    }

    /// Preview the existing production chain at a proposed electrical output.
    /// This closes the fuel/power loop without scheduling one event per fuel drip.
    fn fabricated_fuel_for_power(&self, coord: crate::Coord, hours: f64, power: f64) -> f64 {
        if !self
            .config
            .chains
            .recipes
            .iter()
            .any(|recipe| recipe.outputs.contains_key("reactor_fuel"))
        {
            return 0.0;
        }
        let Some(buildings) = self.operational_buildings_at(coord) else {
            return 0.0;
        };
        let Some(planet) = self.planet_kind(coord) else {
            return 0.0;
        };
        let Some(mut resources) = self.resources_at(coord).copied() else {
            return 0.0;
        };
        let mut goods = if coord == self.home {
            self.goods.clone()
        } else {
            let Some(colony) = self.colonies.iter().find(|c| c.coord == coord) else {
                return 0.0;
            };
            colony.goods.clone()
        };
        let before = goods.get("reactor_fuel").copied().unwrap_or(0.0);
        let factor = self.reactor_factor_at_power(coord, hours, power);
        let Some(waste) = self.base_waste_at(coord) else {
            return 0.0;
        };
        let now = self.last_tick_unix;
        let to = now.saturating_add((hours * 3600.0).round() as u64);
        let emission = self.laws.emission_factor_range(
            &self.config.laws,
            now,
            to,
            coord,
            waste.pollution_load,
            waste.pollution_sink,
        );
        let extra = self.site_extra(coord);
        let deposits = self.chain_deposits(planet, self.galaxy_seed, coord);
        crate::chains::apply_recipes(
            crate::chains::RecipeRun {
                config: &self.config.chains,
                buildings,
                deposits: &deposits,
                hours: hours * self.governor_factor(coord),
                energy_factor: factor * emission,
                boost: self.prod_boost_mult(),
            },
            |kind| self.recipe_building_scale(buildings, planet, extra, kind, (now, to), coord),
            &mut resources,
            &mut goods,
            |kind| self.storage_cap_at(coord, kind),
        );
        goods.get("reactor_fuel").copied().unwrap_or(0.0) - before
    }

    pub(crate) fn next_reactor_event_secs(&self, coord: crate::Coord) -> Option<f64> {
        let buildings = self.buildings_at(coord)?;
        if buildings.fission_reactor == 0 {
            return None;
        }
        let goods = if coord == self.home {
            &self.goods
        } else {
            &self.colonies.iter().find(|c| c.coord == coord)?.goods
        };
        let fuel = goods.get("reactor_fuel").copied().unwrap_or(0.0);
        let spent = goods.get("spent_fuel").copied().unwrap_or(0.0);
        let space = self
            .config
            .fission_reactor
            .pool_capacity(buildings.fission_reactor)
            - spent;
        if space <= 1e-12 || self.building_health(coord, crate::BuildingKind::FissionReactor) <= 0.0
        {
            return None;
        }
        let waste = self.base_waste_at(coord)?;
        let cfg = &self.config.fission_reactor;
        let rated = cfg.rated_output(buildings.fission_reactor)
            * self.building_health(coord, crate::BuildingKind::FissionReactor);
        if !self.reactor_shutdowns.contains_key(&coord) {
            let power = self.reactor_output_at(coord);
            let burn = power * cfg.reactor_fuel_per_energy_hour;
            if burn <= 1e-12 {
                return None;
            }
            let made = self.fabricated_fuel_for_power(coord, 1.0 / 3600.0, power) * 3600.0;
            let pool_wait = space / burn * 3600.0;
            return Some(if fuel > 1e-9 && burn > made + 1e-9 {
                pool_wait.min(fuel / (burn - made) * 3600.0)
            } else {
                pool_wait
            });
        }
        if waste.heat_sink <= waste.heat_load {
            return None;
        }
        let usage = |seconds: f64| {
            let hours = seconds / 3600.0;
            let power = cfg.cooled_output_average(
                rated,
                waste.heat_sink - waste.heat_load,
                self.reactor_shutdowns.get(&coord),
                self.last_tick_unix,
                hours,
            );
            let burned = power * hours * cfg.reactor_fuel_per_energy_hour;
            let made = self.fabricated_fuel_for_power(coord, hours, power);
            (burned, burned - made)
        };
        let mut first = None;
        for (capacity, net) in [(space, false), (fuel, true)] {
            if capacity <= 1e-9 {
                continue;
            }
            let exceeds = |seconds| {
                let (burned, net_burned) = usage(seconds);
                if net {
                    net_burned >= capacity
                } else {
                    burned >= capacity
                }
            };
            let mut high = 3600.0;
            while high < u64::MAX as f64 / 2.0 && !exceeds(high) {
                high *= 2.0;
            }
            if !exceeds(high) {
                continue;
            }
            let mut low = 0.0;
            for _ in 0..48 {
                let mid = (low + high) / 2.0;
                if exceeds(mid) {
                    high = mid;
                } else {
                    low = mid;
                }
            }
            first = Some(first.map_or(high, |previous: f64| previous.min(high)));
        }
        first
    }

    pub fn reactor_decay_heat_at(&self, coord: crate::Coord) -> f64 {
        self.reactor_shutdowns
            .get(&coord)
            .map(|shutdown| {
                self.config.fission_reactor.decay_heat(
                    shutdown.thermal_power,
                    self.last_tick_unix.saturating_sub(shutdown.at),
                )
            })
            .unwrap_or(0.0)
    }

    pub(crate) fn note_reactor_shutdown(&mut self, coord: crate::Coord, at: u64) {
        let level = self
            .buildings_at(coord)
            .map(|b| b.fission_reactor)
            .unwrap_or(0);
        if level > 0 {
            self.reactor_shutdowns.insert(
                coord,
                ReactorShutdown {
                    at,
                    thermal_power: self.config.fission_reactor.rated_thermal_power(level),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fission_technology_gates_construction_and_research() {
        let mut state = crate::GameState::new(0);
        state.buildings.research_lab = 3;
        state.resources = crate::Resources::from_parts(100_000.0, 100_000.0, 100_000.0);
        assert!(matches!(
            state.enqueue_build(crate::BuildingKind::FissionReactor, 0),
            Err(crate::BuildError::MissingTechnology { .. })
        ));
        state.tech_levels.insert("energy".into(), 2);
        assert!(
            !state
                .missing_research_prereqs("fission")
                .unwrap()
                .is_empty()
        );
        state.tech_levels.insert("energy".into(), 3);
        assert!(
            state
                .missing_research_prereqs("fission")
                .unwrap()
                .is_empty()
        );
        state.tech_levels.insert("fission".into(), 1);
        assert!(
            state
                .enqueue_build(crate::BuildingKind::FissionReactor, 0)
                .is_ok()
        );
    }

    fn isolated_reactor() -> crate::GameState {
        let mut config = crate::BalanceConfig {
            chains: crate::ChainConfig::default(),
            ..Default::default()
        };
        config.raid.enabled = false;
        config.market.enabled = false;
        let mut state = crate::GameState::with_seed(0, config, 9);
        state.deterministic = true;
        state.buildings = crate::Buildings {
            fission_reactor: 1,
            radiator: 2,
            ..Default::default()
        };
        state.maintenance.next_at = u64::MAX;
        state.goods.insert("reactor_fuel".into(), 10.0);
        state
    }

    #[test]
    fn burnup_and_pool_limit_match_live_and_offline() {
        for (fuel, spent, cooling_down) in [
            (10.0, 0.0, false),
            (0.3, 0.0, false),
            (10.0, 29.7, false),
            (10.0, 0.0, true),
        ] {
            let mut bulk = isolated_reactor();
            bulk.goods.insert("reactor_fuel".into(), fuel);
            bulk.goods.insert("spent_fuel".into(), spent);
            if cooling_down {
                bulk.buildings.radiator = 0;
                bulk.note_reactor_shutdown(bulk.home, 0);
            }
            let mut live = bulk.clone();
            bulk.catch_up(3600);
            for at in 1..=3600 {
                live.catch_up(at);
            }
            let burned = fuel - bulk.goods["reactor_fuel"];
            if !cooling_down {
                assert!((burned - 0.6_f64.min(fuel).min(30.0 - spent)).abs() < 1e-9);
            }
            assert!((bulk.goods["spent_fuel"] - spent - burned).abs() < 1e-9);
            assert!((bulk.goods["reactor_fuel"] - live.goods["reactor_fuel"]).abs() < 1e-9);
            assert!((bulk.goods["spent_fuel"] - live.goods["spent_fuel"]).abs() < 1e-9);
        }
    }

    #[test]
    fn recovery_across_energy_demand_matches_live_on_home_and_colony() {
        let mut bulk = isolated_reactor();
        bulk.config.metal_mine.base_energy = 35.0;
        bulk.config.solar_plant.base_energy = 17.0;
        bulk.config.orbits.inner_au = 1.0;
        bulk.config.orbits.au_per_slot = 0.0;
        bulk.buildings.metal_mine = 1;
        bulk.buildings.solar_plant = 1;
        bulk.buildings.radiator = 0;
        bulk.resources = crate::Resources::ZERO;
        bulk.note_reactor_shutdown(bulk.home, 0);
        let colony = crate::Coord {
            position: bulk.home.position + 1,
            ..bulk.home
        };
        bulk.colonies.push(crate::Colony {
            coord: colony,
            kind: crate::PlanetType::Rocky,
            size: 12,
            resources: crate::Resources::ZERO,
            buildings: bulk.buildings,
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: bulk.goods.clone(),
        });
        bulk.note_reactor_shutdown(colony, 0);
        assert!(bulk.energy_factor() < 1.0);
        let mut live = bulk.clone();
        bulk.catch_up(3600);
        for at in (60..=3600).step_by(60) {
            live.catch_up(at);
        }
        assert!(bulk.energy_factor() >= 1.0);
        assert!(
            (bulk.resources.iron - live.resources.iron).abs() < 1e-8,
            "home: {} vs {}",
            bulk.resources.iron,
            live.resources.iron
        );
        assert!(
            (bulk.colonies[0].resources.iron - live.colonies[0].resources.iron).abs() < 1e-8,
            "colony: {} vs {}",
            bulk.colonies[0].resources.iron,
            live.colonies[0].resources.iron
        );
    }

    #[test]
    fn fuel_depletion_during_recovery_has_the_same_shutdown_time() {
        let mut bulk = isolated_reactor();
        bulk.buildings.radiator = 0;
        bulk.goods.insert("reactor_fuel".into(), 0.3);
        bulk.note_reactor_shutdown(bulk.home, 0);
        let mut live = bulk.clone();
        bulk.catch_up(3600);
        for at in (60..=3600).step_by(60) {
            live.catch_up(at);
        }
        assert!(bulk.reactor_shutdowns[&bulk.home].at > 0);
        assert_eq!(bulk.reactor_shutdowns, live.reactor_shutdowns);
        assert!((bulk.goods["spent_fuel"] - live.goods["spent_fuel"]).abs() < 1e-9);
    }

    #[test]
    fn recovery_depletion_and_overflow_match_live() {
        for fuel in [0.3, 10.0] {
            let mut bulk = isolated_reactor();
            bulk.buildings.radiator = 0;
            bulk.buildings.metal_mine = 1;
            bulk.config.metal_mine.base_energy = 75.0;
            bulk.config.solar_plant.base_energy = 100.0;
            bulk.buildings.solar_plant = 1;
            bulk.goods.insert("reactor_fuel".into(), fuel);
            bulk.note_reactor_shutdown(bulk.home, 0);
            let mut live = bulk.clone();
            bulk.catch_up(3600);
            for at in (60..=3600).step_by(60) {
                live.catch_up(at);
            }
            assert!((bulk.resources.iron - live.resources.iron).abs() < 1e-7);
            assert!((bulk.goods["reactor_fuel"] - live.goods["reactor_fuel"]).abs() < 1e-8);
            assert_eq!(bulk.reactor_shutdowns, live.reactor_shutdowns);
        }
    }

    #[test]
    fn fuel_fabrication_catches_up_from_an_empty_core() {
        let mut bulk = isolated_reactor();
        bulk.config.chains = crate::BalanceConfig::default().chains;
        bulk.buildings.metal_mine = 1;
        bulk.buildings.solar_plant = 50;
        bulk.buildings.fission_reactor = 2;
        bulk.goods.clear();
        bulk.goods.insert("uranium_ore".into(), 100.0);
        let fabricated = bulk
            .production_report()
            .steps
            .into_iter()
            .find(|flow| flow.id == "fabricate_fuel")
            .unwrap()
            .outputs_per_hour["reactor_fuel"]
            * 120.0
            / 3600.0;
        assert!(bulk.next_reactor_event_secs(bulk.home).unwrap() > 60.0);
        let mut live = bulk.clone();
        bulk.catch_up(120);
        for at in 1..=120 {
            live.catch_up(at);
        }
        assert!(bulk.goods.get("spent_fuel").copied().unwrap_or(0.0) > 0.0);
        for good in ["uranium_ore", "reactor_fuel", "spent_fuel"] {
            let left = bulk.goods.get(good).copied().unwrap_or(0.0);
            let right = live.goods.get(good).copied().unwrap_or(0.0);
            assert!(
                (left - right).abs() < 1e-8,
                "{good}: bulk {left}, live {right}"
            );
        }
        assert!((bulk.resources.iron - live.resources.iron).abs() < 1e-8);
        assert!((bulk.goods["reactor_fuel"] + bulk.goods["spent_fuel"] - fabricated).abs() < 1e-8);
    }

    #[test]
    fn reactor_cooling_is_local_and_independent_of_sunlight() {
        let mut state = isolated_reactor();
        state.buildings.radiator = 0;
        assert_eq!(state.reactor_output_at(state.home), 40.0);
        assert!(!state.waste_home().overflowing());
        state.buildings.radiator = 1;
        assert_eq!(state.reactor_output_at(state.home), 60.0);
        assert!(!state.waste_home().overflowing());
        state.config.orbits.inner_au = 2.5;
        state.config.orbits.au_per_slot = 0.0;
        state.buildings.solar_plant = 1;
        assert_eq!(state.reactor_output_at(state.home), 60.0);
        assert!((state.energy_produced() - 60.0 - 22.0 * 0.16).abs() < 1e-9);
        state.goods.clear();
        assert_eq!(state.reactor_output_at(state.home), 0.0);
    }

    #[test]
    fn shutdown_survives_save_with_building_fuel_and_technology() {
        let mut state = crate::GameState::new(1_000);
        state.buildings.fission_reactor = 1;
        state.goods.insert("reactor_fuel".into(), 5.0);
        state.goods.insert("spent_fuel".into(), 2.0);
        state.tech_levels.insert("fission".into(), 1);
        state.note_reactor_shutdown(state.home, 900);
        let colony_coord = crate::Coord {
            position: state.home.position + 1,
            ..state.home
        };
        state.colonies.push(crate::Colony {
            coord: colony_coord,
            kind: crate::PlanetType::Ice,
            size: 12,
            resources: crate::Resources::ZERO,
            buildings: crate::Buildings {
                fission_reactor: 1,
                ..Default::default()
            },
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: std::collections::BTreeMap::new(),
        });
        state.note_reactor_shutdown(colony_coord, 950);
        let heat = state.reactor_decay_heat_at(state.home);
        assert!(heat > 0.0);
        let mut conn = crate::save::open_save(std::path::Path::new(":memory:")).unwrap();
        crate::save::save_state(&mut conn, &state).unwrap();
        let loaded = crate::save::load_state(&conn, state.config.clone()).unwrap();
        assert_eq!(loaded.buildings.fission_reactor, 1);
        assert_eq!(loaded.goods, state.goods);
        assert_eq!(loaded.tech_level("fission"), 1);
        assert_eq!(loaded.reactor_shutdowns, state.reactor_shutdowns);
        assert_eq!(loaded.reactor_decay_heat_at(loaded.home), heat);
    }

    #[test]
    fn cooling_and_fuel_bound_delivered_power() {
        let cfg = ReactorConfig::default();
        assert_eq!(cfg.output_for_interval(1, 120.0, 10.0, 0.0, 1.0), 60.0);
        assert_eq!(cfg.output_for_interval(1, 80.0, 10.0, 0.0, 1.0), 40.0);
        assert_eq!(cfg.output_for_interval(1, 120.0, 0.0, 0.0, 1.0), 0.0);
        assert_eq!(cfg.output_for_interval(1, 120.0, 10.0, 30.0, 1.0), 0.0);
        assert!((cfg.output_for_interval(1, 120.0, 0.3, 0.0, 1.0) - 30.0).abs() < 1e-9);
        let thermal = cfg.rated_thermal_power(1);
        assert!((cfg.decay_heat(thermal, 0) - thermal * 0.07).abs() < 1e-9);
        assert!((cfg.decay_heat(thermal, 86_400) - thermal * 0.01).abs() < 1e-9);
        assert!(cfg.decay_heat(thermal, 172_800) < cfg.decay_heat(thermal, 86_400));
    }
}
