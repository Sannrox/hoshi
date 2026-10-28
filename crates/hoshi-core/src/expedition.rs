//! Expedition outcomes. Weights live in balance config; a stored seed makes a pending
//! result survive save/load.

use crate::combat::next_u64;
use crate::fleets::Coord;
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

impl ExpeditionKind {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Resources => "resources",
            Self::Ships => "ships",
            Self::Delay => "delay",
            Self::EarlyReturn => "early",
            Self::Pirates => "pirates",
            Self::Loss => "loss",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "nothing" => Self::Nothing,
            "resources" => Self::Resources,
            "ships" => Self::Ships,
            "delay" => Self::Delay,
            "early" => Self::EarlyReturn,
            "pirates" => Self::Pirates,
            "loss" => Self::Loss,
            _ => return None,
        })
    }
}

/// One placed stockpile. The kind is fixed at galaxy start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    Wreck,
    Cache,
    Lane,
}

impl SiteKind {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Wreck => "wreck",
            Self::Cache => "cache",
            Self::Lane => "lane",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "wreck" => Self::Wreck,
            "cache" => Self::Cache,
            "lane" => Self::Lane,
            _ => return None,
        })
    }
}

/// Iron, silicon, deuterium and hulls a site can still pay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteBudget {
    pub iron: f64,
    pub silicon: f64,
    pub deuterium: f64,
    pub ships: u32,
}

impl Default for SiteBudget {
    fn default() -> Self {
        Self {
            iron: 0.0,
            silicon: 0.0,
            deuterium: 0.0,
            ships: 0,
        }
    }
}

/// Budgets copied onto sites when a galaxy is created.
#[derive(Debug, Clone, PartialEq)]
pub struct SiteBudgets {
    pub wreck: SiteBudget,
    pub cache: SiteBudget,
    pub lane: SiteBudget,
}

impl Default for SiteBudgets {
    fn default() -> Self {
        Self {
            wreck: SiteBudget {
                iron: 4_000.0,
                silicon: 2_000.0,
                deuterium: 500.0,
                ships: 4,
            },
            cache: SiteBudget {
                iron: 9_000.0,
                silicon: 1_000.0,
                deuterium: 3_000.0,
                ships: 0,
            },
            lane: SiteBudget {
                iron: 1_000.0,
                silicon: 1_000.0,
                deuterium: 6_000.0,
                ships: 2,
            },
        }
    }
}

impl SiteBudgets {
    pub fn of(&self, kind: SiteKind) -> SiteBudget {
        match kind {
            SiteKind::Wreck => self.wreck,
            SiteKind::Cache => self.cache,
            SiteKind::Lane => self.lane,
        }
    }
}

/// A finite find in the home system. `outcome` set means the site is resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpeditionSite {
    pub id: u64,
    pub coord: Coord,
    pub kind: SiteKind,
    pub iron: f64,
    pub silicon: f64,
    pub deuterium: f64,
    pub ships: u32,
    pub outcome: Option<ExpeditionKind>,
}

impl ExpeditionSite {
    /// Map glyph. Open sites use the kind. Resolved sites use the outcome.
    pub fn map_mark(&self) -> char {
        match self.outcome {
            Some(ExpeditionKind::Nothing) => 'n',
            Some(ExpeditionKind::Resources) => 'r',
            Some(ExpeditionKind::Ships) => 's',
            Some(ExpeditionKind::Delay) => 'd',
            Some(ExpeditionKind::EarlyReturn) => 'e',
            Some(ExpeditionKind::Pirates) => 'p',
            Some(ExpeditionKind::Loss) => 'x',
            None => match self.kind {
                SiteKind::Wreck => 'w',
                SiteKind::Cache => 'k',
                SiteKind::Lane => 'l',
            },
        }
    }
}

/// Place up to one wreck, one cache and one lane on empty home-system coords.
/// The expedition slot and anything in `blocked` are skipped. A full system
/// yields fewer sites. The same seed and block list always yield the same coords.
pub fn place_expedition_sites(
    galaxy_seed: u64,
    home: Coord,
    blocked: &[Coord],
    dest_position: u16,
    budgets: &SiteBudgets,
) -> Vec<ExpeditionSite> {
    let mut slots = Vec::new();
    for position in 1..dest_position {
        let coord = Coord {
            galaxy: home.galaxy,
            system: home.system,
            position,
        };
        if coord == home || blocked.contains(&coord) {
            continue;
        }
        slots.push(coord);
    }
    let mut rng = galaxy_seed ^ 0x51FE_51FE_51FE_51FE;
    for i in (1..slots.len()).rev() {
        let j = (next_u64(&mut rng) as usize) % (i + 1);
        slots.swap(i, j);
    }
    let kinds = [SiteKind::Wreck, SiteKind::Cache, SiteKind::Lane];
    slots
        .into_iter()
        .take(kinds.len())
        .enumerate()
        .map(|(index, coord)| {
            let kind = kinds[index];
            let budget = budgets.of(kind);
            ExpeditionSite {
                id: index as u64 + 1,
                coord,
                kind,
                iron: budget.iron.max(0.0),
                silicon: budget.silicon.max(0.0),
                deuterium: budget.deuterium.max(0.0),
                ships: budget.ships,
                outcome: None,
            }
        })
        .collect()
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
    /// Resource and ship finds times this when the fleet has an explorer.
    /// One explorer is enough; more do not stack.
    pub explorer_mult: f64,
    /// Stockpiles copied onto sites at galaxy start.
    pub sites: SiteBudgets,
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
            explorer_mult: 2.0,
            sites: SiteBudgets::default(),
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
/// Find multiplier for this fleet: `explorer_mult` with any explorer aboard, else 1.
pub fn explorer_factor(ships: &Fleet, cfg: &ExpeditionConfig) -> f64 {
    if ships.get(ShipKind::Explorer) > 0 {
        cfg.explorer_mult.max(0.0)
    } else {
        1.0
    }
}

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
    // Applied to the finished find, so the explorer draws no extra random numbers.
    let factor = explorer_factor(ships, cfg);
    match kind {
        ExpeditionKind::Nothing => ExpeditionResult::Nothing,
        ExpeditionKind::Resources => {
            let scale = 0.5 + (next_u64(&mut rng) % 1001) as f64 / 1000.0;
            let find = |per_point: f64, floor: f64| {
                let base = (points * per_point * scale * cfg.reward_mult.max(0.0))
                    .floor()
                    .max(floor);
                (base * factor).floor()
            };
            ExpeditionResult::Resources(Resources::from_parts(
                find(cfg.metal_per_point, 1.0),
                find(cfg.crystal_per_point, 0.0),
                find(cfg.deuterium_per_point, 0.0),
            ))
        }
        ExpeditionKind::Ships => {
            let n = ((points * cfg.ships_per_point * cfg.reward_mult.max(0.0)).floor() as u32)
                .clamp(1, cfg.ships_max.max(1));
            let mut found = Fleet::default();
            found.add(cfg.ship_kind, (f64::from(n) * factor).floor() as u32);
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
    fn a_full_system_places_fewer_sites() {
        let home = Coord::HOME;
        let mut blocked = Vec::new();
        for position in 1..16 {
            let coord = Coord {
                galaxy: home.galaxy,
                system: home.system,
                position,
            };
            if coord != home {
                blocked.push(coord);
            }
        }
        blocked.pop();
        let sites = place_expedition_sites(7, home, &blocked, 16, &SiteBudgets::default());
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Wreck);
    }

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

    fn explorer_fleet(explorers: u32) -> Fleet {
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 40);
        ships.add(ShipKind::Explorer, explorers);
        ships
    }

    /// The first seed in 0.. whose roll has `kind` for this fleet.
    fn seed_for(kind: ExpeditionKind, ships: &Fleet, cfg: &ExpeditionConfig) -> u64 {
        let catalog = ShipCatalog::default();
        (0..100_000u64)
            .find(|&seed| roll_expedition(seed, ships, 0, 3_600, cfg, &catalog).kind() == kind)
            .expect("a seed for the kind")
    }

    #[test]
    fn an_explorer_doubles_resource_and_ship_finds() {
        let catalog = ShipCatalog::default();
        let ships = explorer_fleet(1);
        let plain = ExpeditionConfig {
            explorer_mult: 1.0,
            ..ExpeditionConfig::default()
        };
        let doubled = ExpeditionConfig::default();
        for kind in [ExpeditionKind::Resources, ExpeditionKind::Ships] {
            let seed = seed_for(kind, &ships, &plain);
            let base = roll_expedition(seed, &ships, 0, 3_600, &plain, &catalog);
            let more = roll_expedition(seed, &ships, 0, 3_600, &doubled, &catalog);
            match (base, more) {
                (ExpeditionResult::Resources(a), ExpeditionResult::Resources(b)) => {
                    assert_eq!(b.iron, a.iron * 2.0);
                    assert_eq!(b.silicon, a.silicon * 2.0);
                    assert_eq!(b.deuterium, a.deuterium * 2.0);
                }
                (ExpeditionResult::Ships(a), ExpeditionResult::Ships(b)) => {
                    assert_eq!(b.total(), a.total() * 2);
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn other_outcomes_ignore_the_explorer_and_explorers_do_not_stack() {
        let catalog = ShipCatalog::default();
        let ships = explorer_fleet(1);
        let plain = ExpeditionConfig {
            explorer_mult: 1.0,
            ..ExpeditionConfig::default()
        };
        let doubled = ExpeditionConfig::default();
        for kind in [
            ExpeditionKind::Pirates,
            ExpeditionKind::Loss,
            ExpeditionKind::Delay,
            ExpeditionKind::EarlyReturn,
            ExpeditionKind::Nothing,
        ] {
            let seed = seed_for(kind, &ships, &plain);
            assert_eq!(
                roll_expedition(seed, &ships, 0, 3_600, &plain, &catalog),
                roll_expedition(seed, &ships, 0, 3_600, &doubled, &catalog),
                "{kind:?}"
            );
        }
        assert_eq!(explorer_factor(&explorer_fleet(2), &doubled), 2.0);
        assert_eq!(explorer_factor(&explorer_fleet(0), &doubled), 1.0);
    }
}
