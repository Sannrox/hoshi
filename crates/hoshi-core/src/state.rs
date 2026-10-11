//! Planet stockpiles, building levels and offline catch-up.

use crate::balance::BalanceConfig;
use crate::buildings::{BuildError, BuildJob, BuildingKind, Buildings};

/// Stored resources. Energy is a balance, not a stockpile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resources {
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
}

impl Resources {
    pub const ZERO: Self = Self {
        metal: 0.0,
        crystal: 0.0,
        deuterium: 0.0,
    };

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

    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            metal: self.metal + other.metal,
            crystal: self.crystal + other.crystal,
            deuterium: self.deuterium + other.deuterium,
        }
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self {
            metal: (self.metal - other.metal).max(0.0),
            crystal: (self.crystal - other.crystal).max(0.0),
            deuterium: (self.deuterium - other.deuterium).max(0.0),
        }
    }

    pub fn missing(self, cost: Self) -> Self {
        Self {
            metal: (cost.metal - self.metal).max(0.0),
            crystal: (cost.crystal - self.crystal).max(0.0),
            deuterium: (cost.deuterium - self.deuterium).max(0.0),
        }
    }

    pub fn can_afford(self, cost: Self) -> bool {
        self.metal + 1e-9 >= cost.metal
            && self.crystal + 1e-9 >= cost.crystal
            && self.deuterium + 1e-9 >= cost.deuterium
    }
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
    pub build_queue: Vec<BuildJob>,
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
            build_queue: Vec::new(),
            last_tick_unix: now_unix,
            config,
        }
    }

    /// Advance production and the build queue to `now_unix`. Completions that
    /// fall in the gap are applied in timestamp order and production is
    /// recomputed from each completion instant. A backward clock is a no-op.
    pub fn catch_up(&mut self, now_unix: u64) {
        if now_unix <= self.last_tick_unix {
            return;
        }
        loop {
            let Some(done_at) = self.build_queue.first().and_then(|job| job.completes_at) else {
                self.produce_until(now_unix);
                return;
            };
            if done_at > now_unix {
                self.produce_until(now_unix);
                return;
            }
            self.produce_until(done_at);
            let job = self.build_queue.remove(0);
            self.buildings.increment(job.building);
            self.start_head(done_at);
        }
    }

    /// Queue an upgrade. Resources are paid immediately; only the head runs.
    /// Waiting jobs get their timer when they become head, so a robotics
    /// completion in front of them shortens their duration.
    pub fn enqueue_build(&mut self, kind: BuildingKind, now_unix: u64) -> Result<(), BuildError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let cap = self.config.max_build_queue;
        if self.build_queue.len() >= cap {
            return Err(BuildError::QueueFull { cap });
        }
        let prereqs = self.config.buildings.get(kind).prerequisites.clone();
        for &(req, required) in &prereqs {
            let actual = self.projected_level(req);
            if actual < required {
                return Err(BuildError::MissingPrerequisite {
                    building: req,
                    required,
                    actual,
                });
            }
        }
        let from_level = self.projected_level(kind);
        let cost = self.config.buildings.get(kind).cost(from_level);
        if !self.resources.can_afford(cost) {
            return Err(BuildError::InsufficientResources {
                missing: self.resources.missing(cost),
                cost,
            });
        }
        self.resources = self.resources.saturating_sub(cost);
        self.build_queue.push(BuildJob {
            building: kind,
            from_level,
            paid: cost,
            started_at: None,
            completes_at: None,
        });
        if self.build_queue.len() == 1 {
            self.start_head(now);
        }
        Ok(())
    }

    /// Cancel a queue slot and refund its paid cost in full (may exceed the cap).
    pub fn cancel_build(&mut self, index: usize, now_unix: u64) -> Result<Resources, BuildError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let len = self.build_queue.len();
        if index >= len {
            return Err(BuildError::EmptySlot { index, len });
        }
        let job = self.build_queue.remove(index);
        self.resources = self.resources.saturating_add(job.paid);
        self.reprice_queue();
        if index == 0 {
            if let Some(head) = self.build_queue.first_mut() {
                head.started_at = None;
                head.completes_at = None;
            }
            self.start_head(now);
        }
        Ok(job.paid)
    }

    pub fn build_cost(&self, kind: BuildingKind) -> Resources {
        self.config
            .buildings
            .get(kind)
            .cost(self.projected_level(kind))
    }

    pub fn build_time_secs(&self, kind: BuildingKind) -> u64 {
        self.duration_secs(kind, self.projected_level(kind))
    }

    fn projected_level(&self, kind: BuildingKind) -> u32 {
        self.projected_level_before(kind, self.build_queue.len())
    }

    fn projected_level_before(&self, kind: BuildingKind, index: usize) -> u32 {
        self.buildings.get(kind)
            + self.build_queue[..index]
                .iter()
                .filter(|job| job.building == kind)
                .count() as u32
    }

    fn prereqs_ok_at(&self, index: usize) -> bool {
        let kind = self.build_queue[index].building;
        let prereqs = self.config.buildings.get(kind).prerequisites.clone();
        prereqs
            .iter()
            .all(|&(req, required)| self.projected_level_before(req, index) >= required)
    }

    /// After a cancellation, drop jobs whose prereqs no longer hold and
    /// recompute `from_level` / paid cost for the rest.
    fn reprice_queue(&mut self) {
        let mut i = 0;
        while i < self.build_queue.len() {
            if !self.prereqs_ok_at(i) {
                let job = self.build_queue.remove(i);
                self.resources = self.resources.saturating_add(job.paid);
                continue;
            }
            let kind = self.build_queue[i].building;
            let from_level = self.projected_level_before(kind, i);
            let new_cost = self.config.buildings.get(kind).cost(from_level);
            let old_paid = self.build_queue[i].paid;
            self.resources = self.resources.saturating_add(old_paid);
            if !self.resources.can_afford(new_cost) {
                self.build_queue.remove(i);
                continue;
            }
            self.resources = self.resources.saturating_sub(new_cost);
            self.build_queue[i].from_level = from_level;
            self.build_queue[i].paid = new_cost;
            i += 1;
        }
    }

    fn duration_secs(&self, kind: BuildingKind, from_level: u32) -> u64 {
        let raw = self.config.buildings.get(kind).raw_seconds(from_level);
        let divisor =
            1.0 + f64::from(self.buildings.robotics_factory) * self.config.robotics_speed_per_level;
        let secs = if divisor > 0.0 { raw / divisor } else { raw };
        secs.ceil().max(1.0) as u64
    }

    fn start_head(&mut self, now_unix: u64) {
        let Some(job) = self.build_queue.first() else {
            return;
        };
        if job.completes_at.is_some() {
            return;
        }
        let duration = self.duration_secs(job.building, job.from_level);
        let now = now_unix.max(self.last_tick_unix);
        let job = &mut self.build_queue[0];
        job.started_at = Some(now);
        job.completes_at = Some(now.saturating_add(duration));
    }

    fn produce_until(&mut self, now_unix: u64) {
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
    use crate::buildings::{BuildError, BuildingKind};
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
                robotics_factory: 0,
                research_lab: 0,
                shipyard: 0,
            },
            buildings: crate::BuildingCatalog::default(),
            max_build_queue: 5,
            robotics_speed_per_level: 1.0,
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

    fn cheap_spec(seconds: f64) -> crate::BuildingSpec {
        crate::BuildingSpec {
            metal_base: 0.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: seconds,
            time_growth: 1.0,
            prerequisites: vec![],
        }
    }

    fn queue_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.metal_storage.base = 100_000.0;
        cfg.crystal_storage.base = 100_000.0;
        cfg.deuterium_storage.base = 100_000.0;
        cfg.starting_resources = Resources {
            metal: 1_000.0,
            crystal: 1_000.0,
            deuterium: 1_000.0,
        };
        cfg.buildings.metal_mine = cheap_spec(1_800.0);
        cfg.buildings.solar_plant = cheap_spec(3_600.0);
        cfg.buildings.robotics_factory = cheap_spec(100.0);
        cfg.buildings.shipyard = crate::BuildingSpec {
            metal_base: 10.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: 100.0,
            time_growth: 1.0,
            prerequisites: vec![(BuildingKind::RoboticsFactory, 1)],
        };
        cfg.buildings.metal_storage = crate::BuildingSpec {
            metal_base: 50.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: 10.0,
            time_growth: 1.0,
            prerequisites: vec![],
        };
        cfg.max_build_queue = 5;
        cfg.robotics_speed_per_level = 1.0;
        cfg
    }

    #[test]
    fn mine_finishing_mid_gap_changes_production_from_completion_time() {
        let cfg = queue_config();
        let mut one_shot = GameState::with_config(0, cfg.clone());
        one_shot.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        assert_eq!(one_shot.buildings.metal_mine, 1);
        assert_eq!(one_shot.build_queue[0].completes_at, Some(1_800));

        one_shot.catch_up(3_600);
        assert_eq!(one_shot.buildings.metal_mine, 2);
        assert!(one_shot.build_queue.is_empty());
        // 0.5 h at 100/h then 0.5 h at 200/h, paid 0.
        close(one_shot.resources.metal, 1_000.0 + 50.0 + 100.0);

        let mut stepped = GameState::with_config(0, cfg);
        stepped.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        for t in 1..=3_600 {
            stepped.catch_up(t);
        }
        close(one_shot.resources.metal, stepped.resources.metal);
        assert_eq!(stepped.buildings.metal_mine, 2);
    }

    #[test]
    fn queue_caps_at_five_and_only_head_has_a_timer() {
        let mut state = GameState::with_config(0, queue_config());
        for _ in 0..5 {
            state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        }
        let err = state
            .enqueue_build(BuildingKind::MetalStorage, 0)
            .unwrap_err();
        assert_eq!(err, BuildError::QueueFull { cap: 5 });
        assert!(err.to_string().contains("full"));
        assert_eq!(state.build_queue.len(), 5);
        assert!(state.build_queue[0].completes_at.is_some());
        for job in &state.build_queue[1..] {
            assert!(job.completes_at.is_none());
            assert_eq!(job.building, BuildingKind::MetalStorage);
        }
    }

    #[test]
    fn insufficient_resources_report_missing_amounts() {
        let mut cfg = queue_config();
        cfg.starting_resources = Resources::ZERO;
        cfg.buildings.metal_storage.metal_base = 40.0;
        let mut state = GameState::with_config(0, cfg);
        let err = state
            .enqueue_build(BuildingKind::MetalStorage, 0)
            .unwrap_err();
        match &err {
            BuildError::InsufficientResources { missing, cost } => {
                close(missing.metal, 40.0);
                close(cost.metal, 40.0);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(err.to_string().contains("missing metal 40"));
        assert!(state.build_queue.is_empty());
    }

    #[test]
    fn missing_prerequisite_is_rejected_with_building_and_level() {
        let mut state = GameState::with_config(0, queue_config());
        let err = state.enqueue_build(BuildingKind::Shipyard, 0).unwrap_err();
        assert_eq!(
            err,
            BuildError::MissingPrerequisite {
                building: BuildingKind::RoboticsFactory,
                required: 1,
                actual: 0,
            }
        );
        assert!(err.to_string().contains("robotics factory level 1"));
    }

    #[test]
    fn robotics_level_shortens_the_next_job_when_it_becomes_head() {
        let mut cfg = queue_config();
        cfg.buildings.metal_mine = cheap_spec(100.0);
        let mut state = GameState::with_config(0, cfg);
        state
            .enqueue_build(BuildingKind::RoboticsFactory, 0)
            .unwrap();
        state.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        assert_eq!(state.build_queue[0].completes_at, Some(100));
        assert!(state.build_queue[1].completes_at.is_none());

        state.catch_up(100);
        assert_eq!(state.buildings.robotics_factory, 1);
        assert_eq!(state.build_queue.len(), 1);
        // 100 s / (1 + 1) = 50 s, starting at t=100.
        assert_eq!(state.build_queue[0].completes_at, Some(150));
        assert_eq!(state.build_queue[0].building, BuildingKind::MetalMine);
    }

    #[test]
    fn cancel_refunds_paid_resources_in_full() {
        let mut cfg = queue_config();
        cfg.buildings.metal_storage.metal_base = 80.0;
        cfg.starting_resources.metal = 100.0;
        let mut state = GameState::with_config(0, cfg);
        state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        close(state.resources.metal, 20.0);
        let refund = state.cancel_build(0, 0).unwrap();
        close(refund.metal, 80.0);
        close(state.resources.metal, 100.0);
        assert!(state.build_queue.is_empty());
        assert_eq!(state.buildings.metal_storage, 0);
    }

    #[test]
    fn canceling_an_earlier_job_reprojects_later_from_levels() {
        let mut cfg = queue_config();
        cfg.buildings.metal_storage.metal_base = 10.0;
        cfg.buildings.metal_storage.cost_growth = 2.0;
        let mut state = GameState::with_config(0, cfg);
        for _ in 0..3 {
            state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        }
        assert_eq!(
            state
                .build_queue
                .iter()
                .map(|j| j.from_level)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        state.cancel_build(0, 0).unwrap();
        assert_eq!(
            state
                .build_queue
                .iter()
                .map(|j| j.from_level)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        close(state.build_queue[0].paid.metal, 10.0);
        close(state.build_queue[1].paid.metal, 20.0);
        assert_eq!(state.build_queue[0].completes_at, Some(10));
    }

    #[test]
    fn canceling_a_queued_prerequisite_drops_the_dependent_job() {
        let mut cfg = queue_config();
        cfg.buildings.robotics_factory.metal_base = 5.0;
        cfg.buildings.shipyard.metal_base = 7.0;
        cfg.starting_resources.metal = 100.0;
        let mut state = GameState::with_config(0, cfg);
        state
            .enqueue_build(BuildingKind::RoboticsFactory, 0)
            .unwrap();
        state.enqueue_build(BuildingKind::Shipyard, 0).unwrap();
        assert_eq!(state.build_queue.len(), 2);
        close(state.resources.metal, 88.0);
        state.cancel_build(0, 0).unwrap();
        assert!(state.build_queue.is_empty());
        close(state.resources.metal, 100.0);
        assert_eq!(state.buildings.robotics_factory, 0);
        assert_eq!(state.buildings.shipyard, 0);
    }

    #[test]
    fn enqueue_after_advance_ignores_a_stale_timestamp() {
        let mut state = GameState::with_config(0, queue_config());
        state.catch_up(100);
        state.enqueue_build(BuildingKind::MetalMine, 50).unwrap();
        assert_eq!(state.last_tick_unix, 100);
        assert_eq!(state.build_queue[0].started_at, Some(100));
        assert_eq!(state.build_queue[0].completes_at, Some(1_900));
    }
}
