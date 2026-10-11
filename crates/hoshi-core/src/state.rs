//! Planet stockpiles, building levels and offline catch-up.

use crate::balance::BalanceConfig;

/// Stored resources. Energy is a balance, not a stockpile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resources {
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
}

impl Resources {
    pub fn get(self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Metal => self.metal,
            ResourceKind::Crystal => self.crystal,
            ResourceKind::Deuterium => self.deuterium,
        }
    }

    pub fn set(&mut self, kind: ResourceKind, value: f64) {
        match kind {
            ResourceKind::Metal => self.metal = value,
            ResourceKind::Crystal => self.crystal = value,
            ResourceKind::Deuterium => self.deuterium = value,
        }
    }
}

/// Building levels that feed production, energy and storage caps.
///
/// The build queue in a later issue mutates these; catch-up only reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buildings {
    pub metal_mine: u32,
    pub crystal_mine: u32,
    pub deuterium_synthesizer: u32,
    pub solar_plant: u32,
    pub metal_storage: u32,
    pub crystal_storage: u32,
    pub deuterium_storage: u32,
}

/// Stockpiled resource kinds. Energy is computed, not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Metal,
    Crystal,
    Deuterium,
}

impl ResourceKind {
    pub const ALL: [ResourceKind; 3] = [
        ResourceKind::Metal,
        ResourceKind::Crystal,
        ResourceKind::Deuterium,
    ];
}

/// Game state. All time advancement goes through [`GameState::catch_up`].
#[derive(Debug, Clone, PartialEq)]
pub struct GameState {
    pub resources: Resources,
    pub buildings: Buildings,
    pub last_tick_unix: u64,
    pub config: BalanceConfig,
}

impl GameState {
    pub fn new(now_unix: u64) -> Self {
        Self::with_config(now_unix, BalanceConfig::default())
    }

    pub fn with_config(now_unix: u64, config: BalanceConfig) -> Self {
        Self {
            resources: config.starting_resources,
            buildings: config.starting_buildings,
            last_tick_unix: now_unix,
            config,
        }
    }

    /// Advance production to `now_unix`. A one-second tick and a multi-day
    /// offline gap share this path. A clock that moves backwards is a no-op.
    pub fn catch_up(&mut self, now_unix: u64) {
        if now_unix <= self.last_tick_unix {
            return;
        }
        let hours = (now_unix - self.last_tick_unix) as f64 / 3600.0;
        let factor = self.energy_factor();
        for kind in ResourceKind::ALL {
            let rate = self.base_production_per_hour(kind) * factor;
            let cap = self.storage_cap(kind);
            let current = self.resources.get(kind);
            self.resources
                .set(kind, add_capped(current, rate, hours, cap));
        }
        self.last_tick_unix = now_unix;
    }

    pub fn energy_produced(&self) -> f64 {
        self.config
            .solar_plant
            .energy_output(self.buildings.solar_plant)
    }

    pub fn energy_consumed(&self) -> f64 {
        self.config.metal_mine.energy_use(self.buildings.metal_mine)
            + self
                .config
                .crystal_mine
                .energy_use(self.buildings.crystal_mine)
            + self
                .config
                .deuterium_synthesizer
                .energy_use(self.buildings.deuterium_synthesizer)
    }

    /// Produced minus consumed. Negative when mines draw more than the plant supplies.
    pub fn energy_balance(&self) -> f64 {
        self.energy_produced() - self.energy_consumed()
    }

    /// Scales mine output when energy is short. Always in `[0, 1]`.
    pub fn energy_factor(&self) -> f64 {
        let consumed = self.energy_consumed();
        if consumed <= 0.0 {
            1.0
        } else {
            (self.energy_produced() / consumed).clamp(0.0, 1.0)
        }
    }

    pub fn production_per_hour(&self, kind: ResourceKind) -> f64 {
        self.base_production_per_hour(kind) * self.energy_factor()
    }

    pub fn storage_cap(&self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Metal => self
                .config
                .metal_storage
                .capacity(self.buildings.metal_storage),
            ResourceKind::Crystal => self
                .config
                .crystal_storage
                .capacity(self.buildings.crystal_storage),
            ResourceKind::Deuterium => self
                .config
                .deuterium_storage
                .capacity(self.buildings.deuterium_storage),
        }
    }

    fn base_production_per_hour(&self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Metal => self
                .config
                .metal_mine
                .production_per_hour(self.buildings.metal_mine),
            ResourceKind::Crystal => self
                .config
                .crystal_mine
                .production_per_hour(self.buildings.crystal_mine),
            ResourceKind::Deuterium => self
                .config
                .deuterium_synthesizer
                .production_per_hour(self.buildings.deuterium_synthesizer),
        }
    }
}

/// Integrate a constant rate for `hours`, stopping exactly at `cap`.
/// Never lowers `current` (over-cap stockpiles and zero/negative rates stay put).
fn add_capped(current: f64, rate_per_hour: f64, hours: f64, cap: f64) -> f64 {
    if hours <= 0.0 || rate_per_hour <= 0.0 || current >= cap {
        current
    } else {
        (current + rate_per_hour * hours).min(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::clock::{Clock, FakeClock};

    const EPS: f64 = 1e-6;

    fn close(a: f64, b: f64) {
        assert!(
            (a - b).abs() < EPS,
            "left={a} right={b} delta={}",
            (a - b).abs()
        );
    }

    /// Config with round numbers so catch-up tests read the spec fields, not defaults.
    fn test_config() -> BalanceConfig {
        BalanceConfig {
            metal_mine: crate::ProducerSpec {
                base_per_hour: 100.0,
                growth: 2.0,
                base_energy: 10.0,
                energy_growth: 1.0,
            },
            crystal_mine: crate::ProducerSpec {
                base_per_hour: 50.0,
                growth: 2.0,
                base_energy: 10.0,
                energy_growth: 1.0,
            },
            deuterium_synthesizer: crate::ProducerSpec {
                base_per_hour: 20.0,
                growth: 2.0,
                base_energy: 20.0,
                energy_growth: 1.0,
            },
            solar_plant: crate::EnergyProducerSpec {
                base_energy: 20.0,
                growth: 1.0,
            },
            metal_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            crystal_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            deuterium_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            starting_resources: Resources {
                metal: 0.0,
                crystal: 0.0,
                deuterium: 0.0,
            },
            starting_buildings: Buildings {
                metal_mine: 1,
                crystal_mine: 1,
                deuterium_synthesizer: 0,
                solar_plant: 1,
                metal_storage: 0,
                crystal_storage: 0,
                deuterium_storage: 0,
            },
        }
    }

    fn catch_up_via_clock(state: &mut GameState, clock: &FakeClock) {
        state.catch_up(clock.now_unix());
    }

    #[test]
    fn production_and_energy_factor_come_from_config() {
        let cfg = test_config();
        let state = GameState::with_config(0, cfg.clone());
        // Solar 20, mines 10+10, factor 1. Metal 100/h, crystal 50/h from spec fields.
        close(state.energy_produced(), cfg.solar_plant.base_energy);
        close(
            state.energy_consumed(),
            cfg.metal_mine.base_energy + cfg.crystal_mine.base_energy,
        );
        close(state.energy_factor(), 1.0);
        close(
            state.production_per_hour(ResourceKind::Metal),
            cfg.metal_mine.base_per_hour,
        );
        close(
            state.production_per_hour(ResourceKind::Crystal),
            cfg.crystal_mine.base_per_hour,
        );
        close(state.production_per_hour(ResourceKind::Deuterium), 0.0);
        close(
            state.storage_cap(ResourceKind::Metal),
            cfg.metal_storage.base,
        );
    }

    #[test]
    fn energy_shortfall_scales_all_mine_output() {
        let mut cfg = test_config();
        cfg.solar_plant.base_energy = 10.0; // need 20 → factor 0.5
        let mut state = GameState::with_config(0, cfg.clone());
        close(state.energy_factor(), 0.5);
        close(state.energy_balance(), -10.0);
        state.catch_up(3600);
        close(state.resources.metal, cfg.metal_mine.base_per_hour * 0.5);
        close(
            state.resources.crystal,
            cfg.crystal_mine.base_per_hour * 0.5,
        );
        close(state.resources.deuterium, 0.0);
    }

    #[test]
    fn zero_energy_stops_mines() {
        let mut cfg = test_config();
        cfg.starting_buildings.solar_plant = 0;
        let mut state = GameState::with_config(0, cfg);
        close(state.energy_factor(), 0.0);
        state.catch_up(3600 * 10);
        close(state.resources.metal, 0.0);
        close(state.resources.crystal, 0.0);
    }

    #[test]
    fn higher_mine_level_uses_growth_from_config() {
        let mut cfg = test_config();
        cfg.starting_buildings.metal_mine = 3; // 100 * 2^2 = 400
        cfg.solar_plant.base_energy = 1_000.0; // no shortfall
        let mut state = GameState::with_config(0, cfg.clone());
        close(
            state.production_per_hour(ResourceKind::Metal),
            cfg.metal_mine.base_per_hour * cfg.metal_mine.growth.powi(2),
        );
        state.catch_up(3600);
        close(
            state.resources.metal,
            cfg.metal_mine.base_per_hour * cfg.metal_mine.growth.powi(2),
        );
    }

    #[test]
    fn catch_up_over_gap_equals_step_by_step() {
        let mut one_shot = GameState::with_config(0, test_config());
        one_shot.catch_up(7200);

        let mut stepped = GameState::with_config(0, test_config());
        for t in 1..=7200 {
            stepped.catch_up(t);
        }

        close(one_shot.resources.metal, stepped.resources.metal);
        close(one_shot.resources.crystal, stepped.resources.crystal);
        close(one_shot.resources.deuterium, stepped.resources.deuterium);
        assert_eq!(one_shot.last_tick_unix, 7200);
        assert_eq!(stepped.last_tick_unix, 7200);
    }

    #[test]
    fn catch_up_uses_injected_clock() {
        let clock = FakeClock::new(1_000);
        let mut state = GameState::with_config(clock.now_unix(), test_config());
        clock.advance(3600);
        catch_up_via_clock(&mut state, &clock);
        close(state.resources.metal, 100.0);
        assert_eq!(state.last_tick_unix, 4_600);
    }

    #[test]
    fn storage_cap_stops_accumulation_and_partial_tick_is_exact() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 150.0;
        cfg.starting_resources.metal = 100.0;
        // 100/h, 50 room → hits cap in 0.5 h = 1800 s.
        let mut one_shot = GameState::with_config(0, cfg.clone());
        one_shot.catch_up(3600);
        close(one_shot.resources.metal, 150.0);

        let mut stepped = GameState::with_config(0, cfg.clone());
        for t in 1..=3600 {
            stepped.catch_up(t);
        }
        close(stepped.resources.metal, 150.0);
        close(one_shot.resources.metal, stepped.resources.metal);

        // Exactly at the crossing instant.
        let mut at_cap_time = GameState::with_config(0, cfg);
        at_cap_time.catch_up(1800);
        close(at_cap_time.resources.metal, 150.0);
    }

    #[test]
    fn over_cap_stockpile_is_not_drained() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 50.0;
        cfg.starting_resources.metal = 80.0;
        let mut state = GameState::with_config(0, cfg);
        state.catch_up(3600);
        close(state.resources.metal, 80.0);
    }

    #[test]
    fn clock_going_backwards_does_not_lose_resources() {
        let mut state = GameState::with_config(100, test_config());
        state.catch_up(3600);
        let metal = state.resources.metal;
        let crystal = state.resources.crystal;
        let last = state.last_tick_unix;
        state.catch_up(50);
        close(state.resources.metal, metal);
        close(state.resources.crystal, crystal);
        assert_eq!(state.last_tick_unix, last);
    }

    #[test]
    fn default_starter_has_full_energy_for_starting_mines() {
        let state = GameState::new(0);
        assert!(state.energy_factor() >= 1.0 - EPS);
        assert!(state.production_per_hour(ResourceKind::Metal) > 0.0);
        assert!(state.production_per_hour(ResourceKind::Crystal) > 0.0);
    }

    #[test]
    fn catch_up_equals_step_by_step_over_random_gaps() {
        let mut seed = 0xC0FFEE_u64;
        for trial in 0..40 {
            let gap = 1 + splitmix(&mut seed) % (3 * 24 * 3600);
            let metal_level = (splitmix(&mut seed) % 6) as u32;
            let solar_level = 1 + (splitmix(&mut seed) % 8) as u32;
            let fill = (splitmix(&mut seed) % 1000) as f64;

            let mut cfg = test_config();
            cfg.starting_buildings.metal_mine = metal_level;
            cfg.starting_buildings.crystal_mine = metal_level.min(4);
            cfg.starting_buildings.solar_plant = solar_level;
            cfg.starting_resources.metal = fill;
            cfg.starting_resources.crystal = fill / 2.0;

            let mut one_shot = GameState::with_config(0, cfg.clone());
            one_shot.catch_up(gap);

            let mut stepped = GameState::with_config(0, cfg);
            let step = if gap > 7200 { 60 } else { 1 };
            let mut t = 0u64;
            while t + step < gap {
                t += step;
                stepped.catch_up(t);
            }
            stepped.catch_up(gap);

            assert_eq!(one_shot.last_tick_unix, gap, "trial {trial}");
            close(one_shot.resources.metal, stepped.resources.metal);
            close(one_shot.resources.crystal, stepped.resources.crystal);
            close(one_shot.resources.deuterium, stepped.resources.deuterium);
            assert!(
                one_shot.resources.metal <= one_shot.storage_cap(ResourceKind::Metal) + EPS
                    || fill > one_shot.storage_cap(ResourceKind::Metal),
                "cap violated trial {trial}"
            );
        }
    }

    fn splitmix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}
