//! Expedition outcomes. Weights live in balance config; a stored seed makes a pending
//! result survive save/load.

use crate::combat::next_u64;
use crate::ships::{Fleet, ShipCatalog, ShipKind};
use crate::state::Resources;

/// One row of the outcome table. Integer weights so a 10k-run test has an exact target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpeditionWeights {
    pub nothing: u32,
    pub resources: u32,
    pub ships: u32,
    pub delay: u32,
    pub early_return: u32,
    pub pirates: u32,
    pub loss: u32,
}

impl ExpeditionWeights {
    pub fn total(self) -> u32 {
        self.nothing
            .saturating_add(self.resources)
            .saturating_add(self.ships)
            .saturating_add(self.delay)
            .saturating_add(self.early_return)
            .saturating_add(self.pirates)
            .saturating_add(self.loss)
            .max(1)
    }

    pub fn pick(self, seed: &mut u64) -> ExpeditionKind {
        let mut r = next_u64(seed) % u64::from(self.total());
        let steps: [(u32, ExpeditionKind); 7] = [
            (self.nothing, ExpeditionKind::Nothing),
            (self.resources, ExpeditionKind::Resources),
            (self.ships, ExpeditionKind::Ships),
            (self.delay, ExpeditionKind::Delay),
            (self.early_return, ExpeditionKind::EarlyReturn),
            (self.pirates, ExpeditionKind::Pirates),
            (self.loss, ExpeditionKind::Loss),
        ];
        for (weight, kind) in steps {
            if r < u64::from(weight) {
                return kind;
            }
            r -= u64::from(weight);
        }
        ExpeditionKind::Nothing
    }
}

impl Default for ExpeditionWeights {
    fn default() -> Self {
        Self {
            nothing: 30,
            resources: 32,
            ships: 12,
            delay: 8,
            early_return: 8,
            pirates: 9,
            loss: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpeditionKind {
    Nothing,
    Resources,
    Ships,
    Delay,
    EarlyReturn,
    Pirates,
    Loss,
}

/// Numbers for dest, hold window, slots and outcome scaling.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpeditionConfig {
    pub dest_position: u16,
    pub min_hold_secs: u64,
    pub max_hold_secs: u64,
    pub base_slots: u32,
    pub weights: ExpeditionWeights,
    /// Extra `resources` weight per astrophysics level.
    pub astro_find_per_level: u32,
    /// Extra `resources` weight per hour of hold.
    pub hold_find_per_hour: u32,
    /// Extra `pirates` weight per this many fleet points.
    pub danger_points_unit: f64,
    pub pirate_weight_per_unit: u32,
    pub metal_per_point: f64,
    pub crystal_per_point: f64,
    pub deuterium_per_point: f64,
    pub ships_per_point: f64,
    pub ships_max: u32,
    pub ship_kind: ShipKind,
    pub pirate_per_point: f64,
    pub pirate_min: u32,
    pub delay_secs: u64,
    /// Scales resource and ship finds. Seasonal rule sets set this above 1.
    pub reward_mult: f64,
}

impl Default for ExpeditionConfig {
    fn default() -> Self {
        Self {
            dest_position: 16,
            min_hold_secs: 3600,
            max_hold_secs: 8 * 3600,
            base_slots: 1,
            weights: ExpeditionWeights::default(),
            astro_find_per_level: 4,
            hold_find_per_hour: 2,
            danger_points_unit: 1_000.0,
            pirate_weight_per_unit: 2,
            metal_per_point: 0.5,
            crystal_per_point: 0.25,
            deuterium_per_point: 0.1,
            ships_per_point: 0.001,
            ships_max: 20,
            ship_kind: ShipKind::LightFighter,
            pirate_per_point: 0.002,
            pirate_min: 1,
            delay_secs: 3600,
            reward_mult: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpeditionResult {
    Nothing,
    Resources(Resources),
    Ships(Fleet),
    Delay { extra_secs: u64 },
    EarlyReturn,
    Pirates { enemy: Fleet },
    Loss,
}

impl ExpeditionResult {
    pub fn kind(&self) -> ExpeditionKind {
        match self {
            ExpeditionResult::Nothing => ExpeditionKind::Nothing,
            ExpeditionResult::Resources(_) => ExpeditionKind::Resources,
            ExpeditionResult::Ships(_) => ExpeditionKind::Ships,
            ExpeditionResult::Delay { .. } => ExpeditionKind::Delay,
            ExpeditionResult::EarlyReturn => ExpeditionKind::EarlyReturn,
            ExpeditionResult::Pirates { .. } => ExpeditionKind::Pirates,
            ExpeditionResult::Loss => ExpeditionKind::Loss,
        }
    }
}

/// Hull-sum of the fleet. Used to scale finds and danger.
pub fn fleet_points(ships: &Fleet, catalog: &ShipCatalog) -> f64 {
    ShipKind::ALL
        .iter()
        .map(|&kind| f64::from(ships.get(kind)) * catalog.get(kind).hull)
        .sum()
}

/// Base table plus astrophysics, hold time and fleet-size adjustments.
pub fn scaled_weights(
    ships: &Fleet,
    astro: u32,
    hold_secs: u64,
    cfg: &ExpeditionConfig,
    catalog: &ShipCatalog,
) -> ExpeditionWeights {
    let mut w = cfg.weights;
    let hours = hold_secs / 3600;
    let find = astro
        .saturating_mul(cfg.astro_find_per_level)
        .saturating_add(hours.saturating_mul(u64::from(cfg.hold_find_per_hour)) as u32);
    w.resources = w.resources.saturating_add(find);
    w.ships = w.ships.saturating_add(find / 2);
    let points = fleet_points(ships, catalog);
    let units = if cfg.danger_points_unit > 0.0 {
        (points / cfg.danger_points_unit).floor().max(0.0) as u32
    } else {
        0
    };
    w.pirates = w
        .pirates
        .saturating_add(units.saturating_mul(cfg.pirate_weight_per_unit));
    w
}

/// Draw one outcome from `seed`. The same seed always yields the same result.
pub fn roll_expedition(
    seed: u64,
    ships: &Fleet,
    astro: u32,
    hold_secs: u64,
    cfg: &ExpeditionConfig,
    catalog: &ShipCatalog,
) -> ExpeditionResult {
    let mut rng = seed;
    let weights = scaled_weights(ships, astro, hold_secs, cfg, catalog);
    let kind = weights.pick(&mut rng);
    let points = fleet_points(ships, catalog).max(1.0);
    match kind {
        ExpeditionKind::Nothing => ExpeditionResult::Nothing,
        ExpeditionKind::Resources => {
            let scale = 0.5 + (next_u64(&mut rng) % 1001) as f64 / 1000.0;
            ExpeditionResult::Resources(Resources {
                metal: (points * cfg.metal_per_point * scale * cfg.reward_mult.max(0.0))
                    .floor()
                    .max(1.0),
                crystal: (points * cfg.crystal_per_point * scale * cfg.reward_mult.max(0.0))
                    .floor()
                    .max(0.0),
                deuterium: (points * cfg.deuterium_per_point * scale * cfg.reward_mult.max(0.0))
                    .floor()
                    .max(0.0),
            })
        }
        ExpeditionKind::Ships => {
            let n = ((points * cfg.ships_per_point * cfg.reward_mult.max(0.0)).floor() as u32)
                .clamp(1, cfg.ships_max.max(1));
            let mut found = Fleet::default();
            found.add(cfg.ship_kind, n);
            ExpeditionResult::Ships(found)
        }
        ExpeditionKind::Delay => ExpeditionResult::Delay {
            extra_secs: cfg.delay_secs.max(1),
        },
        ExpeditionKind::EarlyReturn => ExpeditionResult::EarlyReturn,
        ExpeditionKind::Pirates => {
            let n = ((points * cfg.pirate_per_point).floor() as u32).max(cfg.pirate_min);
            let mut enemy = Fleet::default();
            enemy.add(ShipKind::LightFighter, n.max(1));
            ExpeditionResult::Pirates { enemy }
        }
        ExpeditionKind::Loss => ExpeditionResult::Loss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_replays_the_same_outcome() {
        let cfg = ExpeditionConfig::default();
        let catalog = ShipCatalog::default();
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 10);
        let a = roll_expedition(42, &ships, 1, 3600, &cfg, &catalog);
        let b = roll_expedition(42, &ships, 1, 3600, &cfg, &catalog);
        assert_eq!(a, b);
    }

    #[test]
    fn distribution_over_10k_seeded_runs_stays_within_tolerance() {
        let cfg = ExpeditionConfig::default();
        let catalog = ShipCatalog::default();
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 10);
        let astro = 1;
        let hold = 3600;
        let weights = scaled_weights(&ships, astro, hold, &cfg, &catalog);
        let total = f64::from(weights.total());
        let n = 10_000u32;
        let mut counts = [0u32; 7];
        for i in 0..n {
            let kind = roll_expedition(u64::from(i), &ships, astro, hold, &cfg, &catalog).kind();
            let idx = match kind {
                ExpeditionKind::Nothing => 0,
                ExpeditionKind::Resources => 1,
                ExpeditionKind::Ships => 2,
                ExpeditionKind::Delay => 3,
                ExpeditionKind::EarlyReturn => 4,
                ExpeditionKind::Pirates => 5,
                ExpeditionKind::Loss => 6,
            };
            counts[idx] += 1;
        }
        let expected = [
            weights.nothing,
            weights.resources,
            weights.ships,
            weights.delay,
            weights.early_return,
            weights.pirates,
            weights.loss,
        ];
        for (i, &weight) in expected.iter().enumerate() {
            let p = f64::from(weight) / total;
            let mean = p * f64::from(n);
            let sigma = (f64::from(n) * p * (1.0 - p)).sqrt();
            let tol = (5.0 * sigma).max(20.0);
            let got = f64::from(counts[i]);
            assert!(
                (got - mean).abs() <= tol,
                "bucket {i}: got {got} expected {mean} ± {tol} (p={p})"
            );
        }
    }
}
