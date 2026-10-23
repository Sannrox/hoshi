//! Round-based fleet combat. Raids use this resolver; rapid-fire extra shots live here.

use crate::ships::{Fleet, SHIP_KINDS, ShipKind, ShipStats};
use crate::state::{ResourceKind, Resources};

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
    /// RNG value at the start of this fight. Fleet-resolver fights replay with [`Self::replay`].
    pub seed: u64,
    /// Round 0 is the opening fleets; later rows are after each volley.
    pub frames: Vec<CombatFrame>,
    /// Layered planet battles and ground invasions replay from `frames`, not [`Self::replay`].
    pub layered: bool,
}

/// One keypress between rounds. Hold continues; retreat ends the fight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundAction {
    Hold,
    Retreat,
    CommitReserves,
}

impl RoundAction {
    pub fn as_key(self) -> &'static str {
        match self {
            RoundAction::Hold => "hold",
            RoundAction::Retreat => "retreat",
            RoundAction::CommitReserves => "commit",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "hold" => RoundAction::Hold,
            "retreat" => RoundAction::Retreat,
            "commit" | "commit_reserves" => RoundAction::CommitReserves,
            _ => return None,
        })
    }
}

/// Side-wide multipliers. Identity values leave [`resolve_combat`] unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct CombatModifiers {
    pub attack: f64,
    pub shield: f64,
    pub hull: f64,
    pub accuracy: f64,
    pub small_incoming: f64,
    pub rps: [[f64; SHIP_KINDS]; SHIP_KINDS],
    pub small: [bool; SHIP_KINDS],
}

impl CombatModifiers {
    pub fn identity() -> Self {
        Self {
            attack: 1.0,
            shield: 1.0,
            hull: 1.0,
            accuracy: 1.0,
            small_incoming: 1.0,
            rps: [[1.0; SHIP_KINDS]; SHIP_KINDS],
            small: [false; SHIP_KINDS],
        }
    }
}

/// One animation step. Counts are surviving ships after that round.
#[derive(Debug, Clone, PartialEq)]
pub struct CombatFrame {
    pub round: u8,
    pub attacker: Fleet,
    pub defender: Fleet,
    pub shots: u32,
    pub explosions: u32,
}

/// A stored fight, small enough to keep a handful in the save.
#[derive(Debug, Clone, PartialEq)]
pub struct CombatLog {
    pub at: u64,
    pub report: CombatReport,
    /// For a raid on home, what it took (zero when nothing was reachable).
    /// `None` for every other fight, and for raids saved before schema 50.
    pub raid_plunder: Option<crate::Resources>,
}

pub const MAX_COMBAT_LOGS: usize = 8;

impl CombatReport {
    /// Fill seed and, if needed, opening/closing frames for fights that skip the fleet resolver.
    /// Marks the report layered: replay is the stored frames, not [`Self::replay`].
    pub fn with_seed_and_endpoints(mut self, seed: u64) -> Self {
        self.seed = seed;
        self.layered = true;
        if self.frames.is_empty() {
            let explosions = self
                .attacker_before
                .total()
                .saturating_add(self.defender_before.total())
                .saturating_sub(
                    self.attacker_after
                        .total()
                        .saturating_add(self.defender_after.total()),
                );
            self.frames = vec![
                CombatFrame {
                    round: 0,
                    attacker: self.attacker_before,
                    defender: self.defender_before,
                    shots: 0,
                    explosions: 0,
                },
                CombatFrame {
                    round: self.rounds.max(1),
                    attacker: self.attacker_after,
                    defender: self.defender_after,
                    shots: 0,
                    explosions,
                },
            ];
        }
        self
    }

    /// Rebuild a fleet-resolver fight from the stored seed and opening fleets.
    /// Layered planet battles and invasions are not reproduced this way; play `frames`.
    pub fn replay(
        attacker: Fleet,
        defender: Fleet,
        attacker_stats: &[ShipStats; SHIP_KINDS],
        defender_stats: &[ShipStats; SHIP_KINDS],
        max_rounds: u8,
        rapid_fire: &[[u32; SHIP_KINDS]; SHIP_KINDS],
        seed: u64,
    ) -> Self {
        let mut rng = seed;
        resolve_combat(
            attacker,
            defender,
            attacker_stats,
            defender_stats,
            max_rounds,
            rapid_fire,
            &mut rng,
        )
    }
}

fn scale_stats(stats: &[ShipStats; SHIP_KINDS], m: &CombatModifiers) -> [ShipStats; SHIP_KINDS] {
    let mut out = *stats;
    for slot in &mut out {
        slot.attack *= m.attack;
        slot.shield *= m.shield;
        slot.hull *= m.hull;
    }
    out
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
    firer_mod: &CombatModifiers,
    target_mod: &CombatModifiers,
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
            let mut skip = false;
            if firer_mod.accuracy < 1.0 - 1e-12 {
                let u = (next_u64(rng) as f64) / (u64::MAX as f64);
                skip = u > firer_mod.accuracy.clamp(0.0, 1.0);
            }
            if !skip {
                let mut dmg =
                    firers[i].attack * firer_mod.rps[firers[i].kind.index()][target_kind.index()];
                if target_mod.small[target_kind.index()] {
                    dmg *= target_mod.small_incoming;
                }
                targets[pick].hit(dmg);
            }
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
    resolve_combat_with(
        attacker,
        defender,
        attacker_stats,
        defender_stats,
        max_rounds,
        rapid_fire,
        rng,
        &CombatModifiers::identity(),
        &CombatModifiers::identity(),
        &[],
        Fleet::default(),
    )
}

/// Same as [`resolve_combat`], with per-side modifiers and between-round actions.
#[allow(clippy::too_many_arguments)]
pub fn resolve_combat_with(
    attacker: Fleet,
    defender: Fleet,
    attacker_stats: &[ShipStats; SHIP_KINDS],
    defender_stats: &[ShipStats; SHIP_KINDS],
    max_rounds: u8,
    rapid_fire: &[[u32; SHIP_KINDS]; SHIP_KINDS],
    rng: &mut u64,
    attacker_mod: &CombatModifiers,
    defender_mod: &CombatModifiers,
    actions: &[RoundAction],
    mut reserves: Fleet,
) -> CombatReport {
    let seed = *rng;
    let atk_stats = scale_stats(attacker_stats, attacker_mod);
    let def_stats = scale_stats(defender_stats, defender_mod);
    let mut attackers = expand(attacker, &atk_stats);
    let mut defenders = expand(defender, &def_stats);
    let mut frames = vec![CombatFrame {
        round: 0,
        attacker,
        defender,
        shots: 0,
        explosions: 0,
    }];
    let mut rounds = 0;
    let mut retreated = false;
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
            let shots = (atk_fire.len() + def_fire.len()) as u32;
            let before = compress(&attackers).total() + compress(&defenders).total();
            shoot(
                &attackers,
                &atk_fire,
                &mut defenders,
                rapid_fire,
                attacker_mod,
                defender_mod,
                rng,
            );
            shoot(
                &defenders,
                &def_fire,
                &mut attackers,
                rapid_fire,
                defender_mod,
                attacker_mod,
                rng,
            );
            rounds += 1;
            let attacker_now = compress(&attackers);
            let defender_now = compress(&defenders);
            let after = attacker_now.total() + defender_now.total();
            frames.push(CombatFrame {
                round: rounds,
                attacker: attacker_now,
                defender: defender_now,
                shots,
                explosions: before.saturating_sub(after),
            });
            match actions
                .get((rounds - 1) as usize)
                .copied()
                .unwrap_or(RoundAction::Hold)
            {
                RoundAction::Hold => {}
                RoundAction::Retreat => {
                    retreated = true;
                    break;
                }
                RoundAction::CommitReserves => {
                    if !reserves.is_empty() {
                        defenders.extend(expand(reserves, &def_stats));
                        reserves = Fleet::default();
                    }
                }
            }
        }
    }
    let attacker_after = compress(&attackers);
    let defender_after = compress(&defenders);
    let winner = if retreated {
        CombatWinner::Draw
    } else if defender_after.is_empty() && !attacker_after.is_empty() {
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
        seed,
        frames,
        layered: false,
    }
}

/// `min(stock * share, cap)` per resource, never negative.
pub fn plunder_from(stock: Resources, share: f64, cap: Resources) -> Resources {
    let take = |have: f64, cap: f64| (have * share).max(0.0).min(cap).min(have);
    Resources {
        iron: take(stock.iron, cap.iron),
        silicon: take(stock.silicon, cap.silicon),
        rare_earths: take(stock.rare_earths, cap.rare_earths),
        water_ice: take(stock.water_ice, cap.water_ice),
        helium_3: take(stock.helium_3, cap.helium_3),
        deuterium: take(stock.deuterium, cap.deuterium),
    }
}

/// Load `want` into a hold of `capacity`: each resource gets an equal share
/// of the room left, and room a resource does not need passes to the others.
pub fn fill_hold(want: Resources, capacity: f64) -> Resources {
    let mut out = Resources::ZERO;
    let mut room = capacity.max(0.0);
    loop {
        let open: Vec<ResourceKind> = ResourceKind::ALL
            .into_iter()
            .filter(|&kind| want.get(kind) - out.get(kind) > 1e-9)
            .collect();
        if open.is_empty() || room <= 1e-9 {
            return out;
        }
        let share = room / open.len() as f64;
        for kind in open {
            let add = (want.get(kind) - out.get(kind)).min(share);
            out.set(kind, out.get(kind) + add);
            room -= add;
        }
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
    fn identity_modifiers_match_the_plain_resolver() {
        let mut a = 11;
        let mut b = 11;
        let left = resolve_combat(
            lf(4),
            lf(3),
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut a,
        );
        let right = resolve_combat_with(
            lf(4),
            lf(3),
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut b,
            &CombatModifiers::identity(),
            &CombatModifiers::identity(),
            &[],
            Fleet::default(),
        );
        assert_eq!(left, right);
    }

    #[test]
    fn scripted_retreat_is_deterministic() {
        let mut a = 3;
        let mut b = 3;
        let actions = [RoundAction::Hold, RoundAction::Retreat, RoundAction::Hold];
        let left = resolve_combat_with(
            lf(5),
            lf(5),
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut a,
            &CombatModifiers::identity(),
            &CombatModifiers::identity(),
            &actions,
            Fleet::default(),
        );
        let right = resolve_combat_with(
            lf(5),
            lf(5),
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut b,
            &CombatModifiers::identity(),
            &CombatModifiers::identity(),
            &actions,
            Fleet::default(),
        );
        assert_eq!(left, right);
        assert_eq!(left.rounds, 2);
        assert_eq!(left.winner, CombatWinner::Draw);
        assert!(left.attacker_after.total() > 0);
        assert!(left.defender_after.total() > 0);
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
        let replayed = CombatReport::replay(
            a,
            b,
            &stats(),
            &stats(),
            6,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            left.seed,
        );
        assert_eq!(replayed.winner, left.winner);
        assert_eq!(replayed.attacker_after, left.attacker_after);
        assert_eq!(replayed.defender_after, left.defender_after);
        assert_eq!(replayed.frames, left.frames);
    }

    #[test]
    fn hold_fills_evenly_and_passes_spare_room_on() {
        let want = Resources::from_parts(1_000.0, 10.0, 1_000.0);
        let taken = fill_hold(want, 150.0);
        // Three resources share 150; silicon only needs 10, so the rest
        // splits between iron and deuterium.
        assert!((taken.silicon - 10.0).abs() < 1e-9);
        assert!((taken.iron - 70.0).abs() < 1e-9, "{taken:?}");
        assert!((taken.deuterium - 70.0).abs() < 1e-9, "{taken:?}");
        assert_eq!(fill_hold(want, 5_000.0), want);
        assert_eq!(fill_hold(want, 0.0), Resources::ZERO);
    }

    #[test]
    fn plunder_is_share_then_cap() {
        let stock = Resources::from_parts(1_000.0, 400.0, 50.0);
        let cap = Resources::from_parts(200.0, 1_000.0, 1_000.0);
        let taken = plunder_from(stock, 0.5, cap);
        assert!((taken.iron - 200.0).abs() < 1e-9);
        assert!((taken.silicon - 200.0).abs() < 1e-9);
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
