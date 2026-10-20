//! Instantaneous waste heat and pollution versus planet and building sinks.
//!
//! Load is computed each tick from industry energy and recipe rates. Nothing is
//! stored: overflow slows work and raises the pirate signature, then drops as
//! soon as load falls back under capacity.

use crate::buildings::Buildings;
use crate::chains::Recipe;
use crate::planets::PlanetType;

/// Tunable heat and pollution numbers. Live values sit in `[waste]`.
#[derive(Debug, Clone, PartialEq)]
pub struct WasteConfig {
    /// Waste heat per unit of mine energy draw.
    pub heat_per_energy: f64,
    /// Process pollution per unit of mine energy draw.
    pub pollution_per_energy: f64,
    pub rocky_heat_sink: f64,
    pub ice_heat_sink: f64,
    pub gas_giant_heat_sink: f64,
    pub rocky_pollution_sink: f64,
    pub ice_pollution_sink: f64,
    pub gas_giant_pollution_sink: f64,
    /// Extra heat sink per radiator level (linear).
    pub radiator_per_level: f64,
    pub radiator_base_energy: f64,
    pub radiator_energy_growth: f64,
    /// Extra pollution sink per scrubber level (linear).
    pub scrubber_per_level: f64,
    pub scrubber_base_energy: f64,
    pub scrubber_energy_growth: f64,
    /// Ratio of load/sink that starts a visible warning. Must be in `(0, 1]`.
    pub warn_ratio: f64,
    /// Production multiplier is `1 / (1 + overflow * this)` once load exceeds sink.
    pub unrest_per_overflow: f64,
    /// Extra pirate ships per unit of combined heat and pollution overflow.
    pub raid_signature_per_overflow: f64,
}

impl Default for WasteConfig {
    fn default() -> Self {
        Self {
            heat_per_energy: 1.0,
            pollution_per_energy: 0.35,
            rocky_heat_sink: 80.0,
            ice_heat_sink: 140.0,
            gas_giant_heat_sink: 200.0,
            rocky_pollution_sink: 50.0,
            ice_pollution_sink: 18.0,
            gas_giant_pollution_sink: 120.0,
            radiator_per_level: 40.0,
            radiator_base_energy: 6.0,
            radiator_energy_growth: 1.1,
            scrubber_per_level: 22.0,
            scrubber_base_energy: 8.0,
            scrubber_energy_growth: 1.1,
            warn_ratio: 0.8,
            unrest_per_overflow: 0.8,
            raid_signature_per_overflow: 0.05,
        }
    }
}

impl WasteConfig {
    pub fn heat_sink_for(&self, planet: PlanetType) -> f64 {
        match planet {
            PlanetType::Rocky => self.rocky_heat_sink,
            PlanetType::Ice => self.ice_heat_sink,
            PlanetType::GasGiant => self.gas_giant_heat_sink,
        }
    }

    pub fn pollution_sink_for(&self, planet: PlanetType) -> f64 {
        match planet {
            PlanetType::Rocky => self.rocky_pollution_sink,
            PlanetType::Ice => self.ice_pollution_sink,
            PlanetType::GasGiant => self.gas_giant_pollution_sink,
        }
    }

    pub fn radiator_sink(&self, level: u32) -> f64 {
        self.radiator_per_level * f64::from(level)
    }

    pub fn scrubber_sink(&self, level: u32) -> f64 {
        self.scrubber_per_level * f64::from(level)
    }

    pub fn radiator_energy(&self, level: u32) -> f64 {
        energy_use(
            self.radiator_base_energy,
            self.radiator_energy_growth,
            level,
        )
    }

    pub fn scrubber_energy(&self, level: u32) -> f64 {
        energy_use(
            self.scrubber_base_energy,
            self.scrubber_energy_growth,
            level,
        )
    }

    /// Energy drawn by radiators and scrubbers. Mine draw stays on the producer specs.
    pub fn mitigation_energy(&self, buildings: &Buildings) -> f64 {
        self.radiator_energy(buildings.radiator) + self.scrubber_energy(buildings.scrubber)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("waste.heat_per_energy", self.heat_per_energy),
            ("waste.pollution_per_energy", self.pollution_per_energy),
            ("waste.rocky_heat_sink", self.rocky_heat_sink),
            ("waste.ice_heat_sink", self.ice_heat_sink),
            ("waste.gas_giant_heat_sink", self.gas_giant_heat_sink),
            ("waste.rocky_pollution_sink", self.rocky_pollution_sink),
            ("waste.ice_pollution_sink", self.ice_pollution_sink),
            (
                "waste.gas_giant_pollution_sink",
                self.gas_giant_pollution_sink,
            ),
            ("waste.radiator_per_level", self.radiator_per_level),
            ("waste.radiator_base_energy", self.radiator_base_energy),
            ("waste.scrubber_per_level", self.scrubber_per_level),
            ("waste.scrubber_base_energy", self.scrubber_base_energy),
            ("waste.unrest_per_overflow", self.unrest_per_overflow),
            (
                "waste.raid_signature_per_overflow",
                self.raid_signature_per_overflow,
            ),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("{name} must be finite and >= 0"));
            }
        }
        for (name, value) in [
            ("waste.radiator_energy_growth", self.radiator_energy_growth),
            ("waste.scrubber_energy_growth", self.scrubber_energy_growth),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{name} must be finite and > 0"));
            }
        }
        if !(self.warn_ratio.is_finite() && self.warn_ratio > 0.0 && self.warn_ratio <= 1.0) {
            return Err("waste.warn_ratio must be in (0, 1]".into());
        }
        Ok(())
    }
}

fn energy_use(base: f64, growth: f64, level: u32) -> f64 {
    if level == 0 {
        0.0
    } else {
        base * growth.powi(level as i32 - 1)
    }
}

/// Load versus sink for one planet at this instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WasteReport {
    pub heat_load: f64,
    pub heat_sink: f64,
    pub pollution_load: f64,
    pub pollution_sink: f64,
}

impl WasteReport {
    pub fn heat_ratio(&self) -> f64 {
        ratio(self.heat_load, self.heat_sink)
    }

    pub fn pollution_ratio(&self) -> f64 {
        ratio(self.pollution_load, self.pollution_sink)
    }

    pub fn peak_ratio(&self) -> f64 {
        self.heat_ratio().max(self.pollution_ratio())
    }

    pub fn warns(&self, warn_ratio: f64) -> bool {
        self.peak_ratio() + f64::EPSILON >= warn_ratio
    }

    pub fn overflowing(&self) -> bool {
        self.peak_ratio() > 1.0
    }

    /// Work-speed multiplier in `[0, 1]`. Stays 1.0 until load exceeds sink.
    pub fn unrest_factor(&self, unrest_per_overflow: f64) -> f64 {
        let overflow = (self.heat_ratio() - 1.0)
            .max(0.0)
            .max((self.pollution_ratio() - 1.0).max(0.0));
        if overflow <= 0.0 {
            1.0
        } else {
            (1.0 / (1.0 + overflow * unrest_per_overflow)).clamp(0.0, 1.0)
        }
    }

    pub fn heat_overflow(&self) -> f64 {
        (self.heat_load - self.heat_sink).max(0.0)
    }

    pub fn pollution_overflow(&self) -> f64 {
        (self.pollution_load - self.pollution_sink).max(0.0)
    }

    /// Extra pirate light-fighters from the industrial signature.
    pub fn raid_ships(&self, signature: f64) -> u32 {
        let raw = (self.heat_overflow() + self.pollution_overflow()) * signature;
        raw.floor().max(0.0) as u32
    }
}

fn ratio(load: f64, sink: f64) -> f64 {
    if load <= 0.0 {
        0.0
    } else if sink <= 0.0 {
        f64::INFINITY
    } else {
        load / sink
    }
}

/// Heat and pollution for one planet from current buildings and recipes.
///
/// `industry_energy` is mine draw only. Radiators and scrubbers add sink, not load.
/// `heat_sink_mod` / `pollution_sink_mod` are research bonuses (0.1 → +10% sink).
pub fn report(
    waste: &WasteConfig,
    recipes: &[Recipe],
    industry_energy: f64,
    buildings: Buildings,
    planet: PlanetType,
    heat_sink_mod: f64,
    pollution_sink_mod: f64,
) -> WasteReport {
    let mut heat_load = industry_energy.max(0.0) * waste.heat_per_energy.max(0.0);
    let mut pollution_load = industry_energy.max(0.0) * waste.pollution_per_energy.max(0.0);
    for recipe in recipes {
        let scale = f64::from(buildings.get(recipe.building));
        heat_load += recipe.heat.max(0.0) * scale;
        pollution_load += recipe.pollution.max(0.0) * scale;
    }
    let heat_mod = (1.0 + heat_sink_mod).max(0.0);
    let pollution_mod = (1.0 + pollution_sink_mod).max(0.0);
    WasteReport {
        heat_load,
        heat_sink: (waste.heat_sink_for(planet) + waste.radiator_sink(buildings.radiator))
            * heat_mod,
        pollution_load,
        pollution_sink: (waste.pollution_sink_for(planet)
            + waste.scrubber_sink(buildings.scrubber))
            * pollution_mod,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildings::BuildingKind;
    use crate::chains::Recipe;
    use std::collections::BTreeMap;

    fn recipe(building: BuildingKind, heat: f64, pollution: f64) -> Recipe {
        Recipe {
            id: "r".into(),
            building,
            deposit: None,
            inputs: BTreeMap::new(),
            outputs: BTreeMap::new(),
            energy: 0.0,
            heat,
            pollution,
        }
    }

    #[test]
    fn starter_mines_stay_under_rocky_capacity() {
        let waste = WasteConfig::default();
        let buildings = Buildings {
            metal_mine: 1,
            crystal_mine: 1,
            solar_plant: 1,
            ..Buildings::default()
        };
        let industry = 20.0;
        let report = report(
            &waste,
            &[],
            industry,
            buildings,
            PlanetType::Rocky,
            0.0,
            0.0,
        );
        assert!(
            report.heat_ratio() < waste.warn_ratio,
            "{}",
            report.heat_ratio()
        );
        assert!(
            report.pollution_ratio() < waste.warn_ratio,
            "{}",
            report.pollution_ratio()
        );
        assert!((report.unrest_factor(waste.unrest_per_overflow) - 1.0).abs() < 1e-12);
        assert_eq!(report.raid_ships(waste.raid_signature_per_overflow), 0);
    }

    #[test]
    fn overflow_slows_work_and_raises_raid_signature() {
        let waste = WasteConfig::default();
        let buildings = Buildings {
            metal_mine: 18,
            crystal_mine: 18,
            deuterium_synthesizer: 12,
            solar_plant: 22,
            ..Buildings::default()
        };
        let industry = 180.0;
        let raw = report(
            &waste,
            &[],
            industry,
            buildings,
            PlanetType::Rocky,
            0.0,
            0.0,
        );
        assert!(raw.overflowing(), "ratio {}", raw.peak_ratio());
        assert!(raw.warns(waste.warn_ratio));
        let unrest = raw.unrest_factor(waste.unrest_per_overflow);
        assert!(unrest < 1.0, "{unrest}");
        assert!(raw.raid_ships(waste.raid_signature_per_overflow) > 0);

        let mut mitigated = buildings;
        mitigated.radiator = 4;
        mitigated.scrubber = 2;
        let ok = report(
            &waste,
            &[],
            industry,
            mitigated,
            PlanetType::Rocky,
            0.0,
            0.0,
        );
        assert!(
            !ok.overflowing(),
            "heat {} pol {}",
            ok.heat_ratio(),
            ok.pollution_ratio()
        );
        assert!((ok.unrest_factor(waste.unrest_per_overflow) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn warning_starts_before_unrest() {
        let waste = WasteConfig {
            rocky_heat_sink: 100.0,
            warn_ratio: 0.8,
            ..WasteConfig::default()
        };
        let buildings = Buildings::default();
        let warn = report(&waste, &[], 80.0, buildings, PlanetType::Rocky, 0.0, 0.0);
        assert!(warn.warns(waste.warn_ratio));
        assert!(!warn.overflowing());
        assert!((warn.unrest_factor(waste.unrest_per_overflow) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn research_and_atmosphere_raise_sink() {
        let waste = WasteConfig::default();
        let buildings = Buildings {
            radiator: 1,
            scrubber: 1,
            ..Buildings::default()
        };
        let rocky = report(&waste, &[], 100.0, buildings, PlanetType::Rocky, 0.0, 0.0);
        let gas = report(
            &waste,
            &[],
            100.0,
            buildings,
            PlanetType::GasGiant,
            0.0,
            0.0,
        );
        assert!(gas.heat_sink > rocky.heat_sink);
        assert!(gas.pollution_sink > rocky.pollution_sink);
        let boosted = report(&waste, &[], 100.0, buildings, PlanetType::Rocky, 0.5, 0.5);
        assert!((boosted.heat_sink - rocky.heat_sink * 1.5).abs() < 1e-9);
        assert!((boosted.pollution_sink - rocky.pollution_sink * 1.5).abs() < 1e-9);
    }

    #[test]
    fn recipe_heat_adds_to_mine_energy_load() {
        let waste = WasteConfig::default();
        let buildings = Buildings {
            metal_mine: 2,
            ..Buildings::default()
        };
        let recipes = [recipe(BuildingKind::MetalMine, 3.0, 1.0)];
        let report = report(
            &waste,
            &recipes,
            10.0,
            buildings,
            PlanetType::Rocky,
            0.0,
            0.0,
        );
        assert!((report.heat_load - (10.0 + 6.0)).abs() < 1e-9);
        assert!((report.pollution_load - (10.0 * 0.35 + 2.0)).abs() < 1e-9);
    }

    #[test]
    fn ice_worlds_dump_heat_but_hold_pollution() {
        let waste = WasteConfig::default();
        let rocky = report(
            &waste,
            &[],
            40.0,
            Buildings::default(),
            PlanetType::Rocky,
            0.0,
            0.0,
        );
        let ice = report(
            &waste,
            &[],
            40.0,
            Buildings::default(),
            PlanetType::Ice,
            0.0,
            0.0,
        );
        assert!(ice.heat_sink > rocky.heat_sink);
        assert!(ice.pollution_sink < rocky.pollution_sink);
    }
}
