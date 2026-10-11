//! Round-based fleet combat. Raids use this resolver; rapid-fire extra shots live here.

use crate::ships::{Fleet, SHIP_KINDS, ShipKind, ShipStats};
use crate::state::Resources;

/// Splitmix64. Mutates `seed` so combat and raids stay reproducible across saves.
pub fn next_u64(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// How a fight ended. Plunder applies only on [`CombatWinner::Attacker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatWinner {
    Attacker,
    Defender,
    Draw,
}

impl CombatWinner {
    pub fn as_key(self) -> &'static str {
        match self {
            CombatWinner::Attacker => "attacker",
            CombatWinner::Defender => "defender",
            CombatWinner::Draw => "draw",
        }
    }
}

/// One resolved fight. `max_rounds` is the cap; fewer rounds if a side dies first.
#[derive(Debug, Clone, PartialEq)]
pub struct CombatReport {
    pub rounds: u8,
    pub attacker_before: Fleet,
    pub defender_before: Fleet,
    pub attacker_after: Fleet,
    pub defender_after: Fleet,
    pub winner: CombatWinner,
    /// Ground assault, when this report is an invasion.
    pub ground: Option<GroundReport>,
}

/// Strength comparison with seeded variance. Same seed replays the same rolls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundReport {
    pub attacker_strength: u32,
    pub defender_strength: u32,
    pub attacker_roll: u32,
    pub defender_roll: u32,
    pub winner: CombatWinner,
}

/// Scale `base` by `1 ± variance` from `rng`. Variance 0 is an exact comparison.
pub fn resolve_ground(
    attacker_strength: u32,
    defender_strength: u32,
    variance: f64,
    rng: &mut u64,
) -> GroundReport {
    let attacker_roll = vary(attacker_strength, variance, rng);
    let defender_roll = vary(defender_strength, variance, rng);
    let winner = if attacker_roll > defender_roll {
        CombatWinner::Attacker
    } else if defender_roll > attacker_roll {
        CombatWinner::Defender
    } else {
        CombatWinner::Draw
    };
    GroundReport {
        attacker_strength,
        defender_strength,
        attacker_roll,
        defender_roll,
        winner,
    }
}

fn vary(base: u32, variance: f64, rng: &mut u64) -> u32 {
    let v = variance.clamp(0.0, 1.0);
    if v <= 0.0 {
        return base;
    }
    let u = (next_u64(rng) as f64) / (u64::MAX as f64);
    let mult = (1.0 - v) + 2.0 * v * u;
    (f64::from(base) * mult).floor() as u32
}

struct Unit {
    kind: ShipKind,
    hull: f64,
    shield_max: f64,
    shield: f64,
    attack: f64,
}

impl Unit {
    fn alive(&self) -> bool {
        self.hull > 0.0
    }

    fn hit(&mut self, attack: f64) {
        let absorbed = self.shield.min(attack);
        self.shield -= absorbed;
        self.hull -= attack - absorbed;
    }
}

fn expand(fleet: Fleet, stats: &[ShipStats; SHIP_KINDS]) -> Vec<Unit> {
    let mut units = Vec::new();
    for kind in ShipKind::ALL {
        let n = fleet.get(kind);
        let s = stats[kind.index()];
        for _ in 0..n {
            units.push(Unit {
                kind,
                hull: s.hull,
                shield_max: s.shield,
                shield: s.shield,
                attack: s.attack,
            });
        }
    }
    units
}

fn compress(units: &[Unit]) -> Fleet {
    let mut fleet = Fleet::default();
    for unit in units {
        if unit.alive() {
            fleet.add(unit.kind, 1);
        }
    }
    fleet
}

fn living(units: &[Unit]) -> Vec<usize> {
    units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.alive())
        .map(|(i, _)| i)
        .collect()
}

fn shoot(
    firers: &[Unit],
    fire: &[usize],
    targets: &mut [Unit],
    rapid_fire: &[[u32; SHIP_KINDS]; SHIP_KINDS],
    rng: &mut u64,
) {
    for &i in fire {
        loop {
            let live = living(targets);
            if live.is_empty() {
                return;
            }
            let pick = live[(next_u64(rng) as usize) % live.len()];
            let target_kind = targets[pick].kind;
            targets[pick].hit(firers[i].attack);
            let value = rapid_fire[firers[i].kind.index()][target_kind.index()];
            if value < 2 {
                break;
            }
            // Keep firing with probability (value-1)/value.
            if next_u64(rng).is_multiple_of(u64::from(value)) {
                break;
            }
        }
    }
}

/// Up to `max_rounds` of simultaneous fire. Shields refill each round; hull does not.
/// Rapid-fire rolls extra shots against the kind just hit.
pub fn resolve_combat(
    attacker: Fleet,
    defender: Fleet,
    attacker_stats: &[ShipStats; SHIP_KINDS],
    defender_stats: &[ShipStats; SHIP_KINDS],
    max_rounds: u8,
    rapid_fire: &[[u32; SHIP_KINDS]; SHIP_KINDS],
    rng: &mut u64,
) -> CombatReport {
    let mut attackers = expand(attacker, attacker_stats);
    let mut defenders = expand(defender, defender_stats);
    let mut rounds = 0;
    if !attackers.is_empty() && !defenders.is_empty() {
        while rounds < max_rounds {
            if living(&attackers).is_empty() || living(&defenders).is_empty() {
                break;
            }
            for unit in attackers.iter_mut().chain(defenders.iter_mut()) {
                if unit.alive() {
                    unit.shield = unit.shield_max;
                }
            }
            let atk_fire = living(&attackers);
            let def_fire = living(&defenders);
            shoot(&attackers, &atk_fire, &mut defenders, rapid_fire, rng);
            shoot(&defenders, &def_fire, &mut attackers, rapid_fire, rng);
            rounds += 1;
        }
    }
    let attacker_after = compress(&attackers);
    let defender_after = compress(&defenders);
    let winner = if defender_after.is_empty() && !attacker_after.is_empty() {
        CombatWinner::Attacker
    } else if attacker_after.is_empty() && !defender_after.is_empty() {
        CombatWinner::Defender
    } else {
        CombatWinner::Draw
    };
    CombatReport {
        rounds,
        attacker_before: attacker,
        defender_before: defender,
        attacker_after,
        defender_after,
        winner,
        ground: None,
    }
}

/// `min(stock * share, cap)` per resource, never negative.
pub fn plunder_from(stock: Resources, share: f64, cap: Resources) -> Resources {
    let take = |have: f64, cap: f64| (have * share).max(0.0).min(cap).min(have);
    Resources {
        metal: take(stock.metal, cap.metal),
        crystal: take(stock.crystal, cap.crystal),
        deuterium: take(stock.deuterium, cap.deuterium),
    }
}

pub fn rapid_fire_table(catalog: &crate::ShipCatalog) -> [[u32; SHIP_KINDS]; SHIP_KINDS] {
    let mut table = [[0u32; SHIP_KINDS]; SHIP_KINDS];
    for kind in ShipKind::ALL {
        for &(target, value) in &catalog.get(kind).rapid_fire {
            table[kind.index()][target.index()] = value;
        }
    }
    table
}

pub fn stats_from_catalog(catalog: &crate::ShipCatalog) -> [ShipStats; SHIP_KINDS] {
    let mut out = [ShipStats {
        attack: 0.0,
        shield: 0.0,
        hull: 0.0,
        cargo: 0.0,
        speed: 0.0,
        fuel: 0.0,
    }; SHIP_KINDS];
    for kind in ShipKind::ALL {
        let spec = catalog.get(kind);
        out[kind.index()] = ShipStats {
            attack: spec.attack,
            shield: spec.shield,
            hull: spec.hull,
            cargo: spec.cargo,
            speed: spec.speed,
            fuel: spec.fuel,
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShipCatalog;

    fn lf(n: u32) -> Fleet {
        let mut f = Fleet::default();
        f.add(ShipKind::LightFighter, n);
        f
    }

    fn stats() -> [ShipStats; SHIP_KINDS] {
        stats_from_catalog(&ShipCatalog::default())
    }

    #[test]
    fn one_light_fighter_each_lasts_all_six_rounds() {
        // LF hull 400, shield 10, attack 50 → 40 hull per shot, 240 over 6 rounds.
        let mut rng = 1;
        let report = resolve_combat(
            lf(1),
            lf(1),
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut rng,
        );
        assert_eq!(report.rounds, 6);
        assert_eq!(report.attacker_after.get(ShipKind::LightFighter), 1);
        assert_eq!(report.defender_after.get(ShipKind::LightFighter), 1);
        assert_eq!(report.winner, CombatWinner::Draw);
    }

    #[test]
    fn empty_defender_is_an_attacker_win_with_no_rounds() {
        let mut rng = 7;
        let report = resolve_combat(
            lf(3),
            Fleet::default(),
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut rng,
        );
        assert_eq!(report.rounds, 0);
        assert_eq!(report.attacker_after.get(ShipKind::LightFighter), 3);
        assert!(report.defender_after.is_empty());
        assert_eq!(report.winner, CombatWinner::Attacker);
    }

    #[test]
    fn same_seed_replays_the_same_fight() {
        let mut a = Fleet::default();
        a.add(ShipKind::LightFighter, 8);
        a.add(ShipKind::Cruiser, 2);
        let mut b = Fleet::default();
        b.add(ShipKind::HeavyFighter, 6);
        b.add(ShipKind::Battleship, 1);
        let mut rng_a = 0xC0FFEE;
        let mut rng_b = 0xC0FFEE;
        let left = resolve_combat(
            a,
            b,
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut rng_a,
        );
        let right = resolve_combat(
            a,
            b,
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut rng_b,
        );
        assert_eq!(left, right);
        assert_eq!(rng_a, rng_b);
    }

    #[test]
    fn plunder_is_share_then_cap() {
        let stock = Resources {
            metal: 1_000.0,
            crystal: 400.0,
            deuterium: 50.0,
        };
        let cap = Resources {
            metal: 200.0,
            crystal: 1_000.0,
            deuterium: 1_000.0,
        };
        let taken = plunder_from(stock, 0.5, cap);
        assert!((taken.metal - 200.0).abs() < 1e-9);
        assert!((taken.crystal - 200.0).abs() < 1e-9);
        assert!((taken.deuterium - 25.0).abs() < 1e-9);
    }

    fn probes(n: u32) -> Fleet {
        let mut f = Fleet::default();
        f.add(ShipKind::EspionageProbe, n);
        f
    }

    fn cruiser(n: u32) -> Fleet {
        let mut f = Fleet::default();
        f.add(ShipKind::Cruiser, n);
        f
    }

    #[test]
    fn rapid_fire_against_probes_destroys_more_than_one_shot_per_round() {
        let none = [[0u32; SHIP_KINDS]; SHIP_KINDS];
        let mut with = [[0u32; SHIP_KINDS]; SHIP_KINDS];
        with[ShipKind::Cruiser.index()][ShipKind::EspionageProbe.index()] = 5;
        let mut rng_none = 1;
        let mut rng_rf = 1;
        let without = resolve_combat(
            cruiser(1),
            probes(10),
            &stats(),
            &stats(),
            6,
            &none,
            &mut rng_none,
        );
        let extra = resolve_combat(
            cruiser(1),
            probes(10),
            &stats(),
            &stats(),
            6,
            &with,
            &mut rng_rf,
        );
        assert_eq!(without.defender_after.get(ShipKind::EspionageProbe), 4);
        assert_eq!(extra.defender_after.get(ShipKind::EspionageProbe), 0);
    }

    #[test]
    fn ground_combat_replays_for_a_fixed_seed() {
        let mut a = 0xC0FFEE;
        let mut b = 0xC0FFEE;
        let left = resolve_ground(10, 4, 0.2, &mut a);
        let right = resolve_ground(10, 4, 0.2, &mut b);
        assert_eq!(left, right);
        assert_eq!(a, b);
        assert_eq!(left.attacker_strength, 10);
        assert_eq!(left.defender_strength, 4);
        let mut exact = 1;
        let report = resolve_ground(8, 3, 0.0, &mut exact);
        assert_eq!(report.attacker_roll, 8);
        assert_eq!(report.defender_roll, 3);
        assert_eq!(report.winner, CombatWinner::Attacker);
    }
}
