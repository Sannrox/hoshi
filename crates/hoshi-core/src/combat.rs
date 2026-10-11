//! Round-based fleet combat. Raids use this resolver; later combat issues extend it.

use crate::ships::{Fleet, ShipKind, ShipStats};
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

/// One resolved fight. `max_rounds` is the cap; fewer rounds if a side dies first.
#[derive(Debug, Clone, PartialEq)]
pub struct CombatReport {
    pub rounds: u8,
    pub attacker_before: Fleet,
    pub defender_before: Fleet,
    pub attacker_after: Fleet,
    pub defender_after: Fleet,
    pub winner: CombatWinner,
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

fn expand(fleet: Fleet, stats: &[ShipStats; 9]) -> Vec<Unit> {
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

fn shoot(firers: &[Unit], fire: &[usize], targets: &mut [Unit], rng: &mut u64) {
    for &i in fire {
        let live = living(targets);
        if live.is_empty() {
            return;
        }
        let pick = live[(next_u64(rng) as usize) % live.len()];
        targets[pick].hit(firers[i].attack);
    }
}

/// Up to `max_rounds` of simultaneous fire. Shields refill each round; hull does not.
/// Rapid-fire is omitted so a later issue can add it on this same loop.
pub fn resolve_combat(
    attacker: Fleet,
    defender: Fleet,
    attacker_stats: &[ShipStats; 9],
    defender_stats: &[ShipStats; 9],
    max_rounds: u8,
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
            shoot(&attackers, &atk_fire, &mut defenders, rng);
            shoot(&defenders, &def_fire, &mut attackers, rng);
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

pub fn stats_from_catalog(catalog: &crate::ShipCatalog) -> [ShipStats; 9] {
    let mut out = [ShipStats {
        attack: 0.0,
        shield: 0.0,
        hull: 0.0,
        cargo: 0.0,
        speed: 0.0,
        fuel: 0.0,
    }; 9];
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

    fn stats() -> [ShipStats; 9] {
        stats_from_catalog(&ShipCatalog::default())
    }

    #[test]
    fn one_light_fighter_each_lasts_all_six_rounds() {
        // LF hull 400, shield 10, attack 50 → 40 hull per shot, 240 over 6 rounds.
        let mut rng = 1;
        let report = resolve_combat(lf(1), lf(1), &stats(), &stats(), 6, &mut rng);
        assert_eq!(report.rounds, 6);
        assert_eq!(report.attacker_after.get(ShipKind::LightFighter), 1);
        assert_eq!(report.defender_after.get(ShipKind::LightFighter), 1);
        assert_eq!(report.winner, CombatWinner::Draw);
    }

    #[test]
    fn empty_defender_is_an_attacker_win_with_no_rounds() {
        let mut rng = 7;
        let report = resolve_combat(lf(3), Fleet::default(), &stats(), &stats(), 6, &mut rng);
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
        let left = resolve_combat(a, b, &stats(), &stats(), 6, &mut rng_a);
        let right = resolve_combat(a, b, &stats(), &stats(), 6, &mut rng_b);
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
}
