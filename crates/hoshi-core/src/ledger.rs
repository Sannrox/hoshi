//! Where each planet's stockpile went during one catch-up.
//!
//! [`crate::GameState::catch_up_ledger`] fills a [`CatchUpLedger`]. Catch-up
//! phases settle the stock change since the previous settle into one term;
//! flows that share a phase with others (raid plunder, output lost at the
//! storage cap) are recorded where they happen.

use crate::fleets::Coord;
use crate::state::{ResourceKind, Resources};

/// One planet's stock movement over a catch-up. Every term is non-negative.
///
/// `before + produced − capped − spent − plundered + transfers_in −
/// transfers_out + gained == after` per resource.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanetLedger {
    pub coord: Coord,
    pub before: Resources,
    pub after: Resources,
    /// Production before the storage cap, so it includes `capped`.
    pub produced: Resources,
    /// Production lost because storage was full.
    pub capped: Resources,
    /// Builds, research, ships, automation, maintenance and recipe inputs.
    pub spent: Resources,
    pub plundered: Resources,
    pub transfers_in: Resources,
    pub transfers_out: Resources,
    /// Refunds, trades, story rewards and expedition finds.
    pub gained: Resources,
}

impl PlanetLedger {
    fn new(coord: Coord, before: Resources) -> Self {
        Self {
            coord,
            before,
            after: before,
            produced: Resources::ZERO,
            capped: Resources::ZERO,
            spent: Resources::ZERO,
            plundered: Resources::ZERO,
            transfers_in: Resources::ZERO,
            transfers_out: Resources::ZERO,
            gained: Resources::ZERO,
        }
    }

    /// Sum of the terms for one resource: `after − before` up to rounding.
    pub fn net(&self, kind: ResourceKind) -> f64 {
        self.produced.get(kind)
            - self.capped.get(kind)
            - self.spent.get(kind)
            - self.plundered.get(kind)
            + self.transfers_in.get(kind)
            - self.transfers_out.get(kind)
            + self.gained.get(kind)
    }
}

/// What a stock change since the last settle is attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Term {
    /// Gains are production, losses are recipe inputs and upkeep.
    Production,
    /// Gains arrive from another place, losses leave for one.
    Transfer,
    /// Losses are spending, gains are refunds and rewards.
    Other,
}

/// Per-planet ledger plus structured counts for one catch-up.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CatchUpLedger {
    /// Home first, then colonies in the order they were first seen.
    pub planets: Vec<PlanetLedger>,
    pub raids_won: u32,
    pub raids_lost: u32,
    pub fleets_returned: u32,
    pub expeditions: u32,
    /// Stock at the last settle, per planet.
    mark: Vec<(Coord, Resources)>,
}

impl CatchUpLedger {
    pub(crate) fn start(stock: impl IntoIterator<Item = (Coord, Resources)>) -> Self {
        let mark: Vec<_> = stock.into_iter().collect();
        Self {
            planets: mark
                .iter()
                .map(|&(coord, before)| PlanetLedger::new(coord, before))
                .collect(),
            mark,
            ..Self::default()
        }
    }

    pub fn raids(&self) -> u32 {
        self.raids_won.saturating_add(self.raids_lost)
    }

    /// Totals across every planet. Transfers between own planets cancel.
    pub fn total(&self) -> PlanetLedger {
        let mut out = PlanetLedger::new(
            self.planets.first().map_or(Coord::HOME, |p| p.coord),
            Resources::ZERO,
        );
        out.after = Resources::ZERO;
        for p in &self.planets {
            out.before = out.before.saturating_add(p.before);
            out.after = out.after.saturating_add(p.after);
            out.produced = out.produced.saturating_add(p.produced);
            out.capped = out.capped.saturating_add(p.capped);
            out.spent = out.spent.saturating_add(p.spent);
            out.plundered = out.plundered.saturating_add(p.plundered);
            out.gained = out.gained.saturating_add(p.gained);
            out.transfers_in = out.transfers_in.saturating_add(p.transfers_in);
            out.transfers_out = out.transfers_out.saturating_add(p.transfers_out);
        }
        for kind in ResourceKind::ALL {
            let moved = out.transfers_in.get(kind).min(out.transfers_out.get(kind));
            out.transfers_in
                .set(kind, out.transfers_in.get(kind) - moved);
            out.transfers_out
                .set(kind, out.transfers_out.get(kind) - moved);
        }
        out
    }

    fn planet(&mut self, coord: Coord) -> &mut PlanetLedger {
        let index = match self.planets.iter().position(|p| p.coord == coord) {
            Some(index) => index,
            None => {
                self.planets.push(PlanetLedger::new(coord, Resources::ZERO));
                self.planets.len() - 1
            }
        };
        &mut self.planets[index]
    }

    fn mark_of(&mut self, coord: Coord) -> &mut Resources {
        let index = match self.mark.iter().position(|(c, _)| *c == coord) {
            Some(index) => index,
            None => {
                self.mark.push((coord, Resources::ZERO));
                self.mark.len() - 1
            }
        };
        &mut self.mark[index].1
    }

    /// Attribute every stock change since the last settle to `term`.
    /// A planet that disappeared settles to zero stock.
    pub(crate) fn settle(
        &mut self,
        stock: impl IntoIterator<Item = (Coord, Resources)>,
        term: Term,
    ) {
        let mut now: Vec<(Coord, Resources)> = stock.into_iter().collect();
        for &(coord, _) in &self.mark {
            if !now.iter().any(|(c, _)| *c == coord) {
                now.push((coord, Resources::ZERO));
            }
        }
        for (coord, stock) in now {
            let mark = *self.mark_of(coord);
            *self.mark_of(coord) = stock;
            let planet = self.planet(coord);
            planet.after = stock;
            for kind in ResourceKind::ALL {
                let delta = stock.get(kind) - mark.get(kind);
                if delta == 0.0 {
                    continue;
                }
                let (gain, loss) = match term {
                    Term::Production => (&mut planet.produced, &mut planet.spent),
                    Term::Transfer => (&mut planet.transfers_in, &mut planet.transfers_out),
                    Term::Other => (&mut planet.gained, &mut planet.spent),
                };
                let target = if delta > 0.0 { gain } else { loss };
                target.set(kind, target.get(kind) + delta.abs());
            }
        }
    }

    /// Record plunder already taken from `coord`, so the next settle does not
    /// count it again.
    pub(crate) fn plunder(&mut self, coord: Coord, taken: Resources) {
        let planet = self.planet(coord);
        planet.plundered = planet.plundered.saturating_add(taken);
        self.unmark(coord, taken);
    }

    /// Record stock already consumed at `coord` (recipe inputs taken in the
    /// same pass that produced), so production is not netted against it.
    pub(crate) fn spend(&mut self, coord: Coord, taken: Resources) {
        let planet = self.planet(coord);
        planet.spent = planet.spent.saturating_add(taken);
        self.unmark(coord, taken);
    }

    fn unmark(&mut self, coord: Coord, taken: Resources) {
        let mark = self.mark_of(coord);
        for kind in ResourceKind::ALL {
            mark.set(kind, mark.get(kind) - taken.get(kind));
        }
    }

    /// Record production that the storage cap discarded. Stock is unchanged,
    /// so it counts towards both `produced` and `capped`.
    pub(crate) fn cap(&mut self, coord: Coord, lost: Resources) {
        let planet = self.planet(coord);
        planet.produced = planet.produced.saturating_add(lost);
        planet.capped = planet.capped.saturating_add(lost);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: Coord = Coord::HOME;

    #[test]
    fn settle_splits_gains_and_losses_by_term() {
        let mut ledger = CatchUpLedger::start([(HOME, Resources::from_parts(100.0, 50.0, 10.0))]);
        ledger.settle(
            [(HOME, Resources::from_parts(160.0, 40.0, 10.0))],
            Term::Production,
        );
        ledger.plunder(HOME, Resources::from_parts(80.0, 0.0, 0.0));
        ledger.settle(
            [(HOME, Resources::from_parts(70.0, 40.0, 15.0))],
            Term::Other,
        );
        ledger.cap(HOME, Resources::from_parts(5.0, 0.0, 0.0));
        let home = ledger.planets[0];
        assert_eq!(home.produced.iron, 65.0);
        assert_eq!(home.capped.iron, 5.0);
        assert_eq!(home.spent.silicon, 10.0);
        assert_eq!(home.plundered.iron, 80.0);
        assert_eq!(home.spent.iron, 10.0);
        assert_eq!(home.gained.deuterium, 5.0);
        for kind in ResourceKind::ALL {
            assert_eq!(home.before.get(kind) + home.net(kind), home.after.get(kind));
        }
    }

    #[test]
    fn transfers_between_own_planets_cancel_in_the_total() {
        let colony = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let mut ledger = CatchUpLedger::start([
            (HOME, Resources::from_parts(100.0, 0.0, 0.0)),
            (colony, Resources::ZERO),
        ]);
        ledger.settle(
            [
                (HOME, Resources::from_parts(60.0, 0.0, 0.0)),
                (colony, Resources::from_parts(40.0, 0.0, 0.0)),
            ],
            Term::Transfer,
        );
        assert_eq!(ledger.planets[0].transfers_out.iron, 40.0);
        assert_eq!(ledger.planets[1].transfers_in.iron, 40.0);
        let total = ledger.total();
        assert_eq!(total.transfers_in.iron, 0.0);
        assert_eq!(total.transfers_out.iron, 0.0);
        assert_eq!(total.before.iron, total.after.iron);
    }
}
