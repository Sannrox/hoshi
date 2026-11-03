//! Scanned signals: a short-lived hostile contact in the home system. A scan
//! places one after light-time; a fleet holding there at the deadline fights
//! a seeded hostile fleet, and an unheld signal expires with no fight.

use crate::combat::next_u64;
use crate::fleets::{Coord, FleetError, MissionPhase};
use crate::intel::light_delay_secs;
use crate::ships::{Fleet, ShipKind};
use crate::state::GameState;
use serde::Deserialize;

/// Resolved signals kept for the map and `hoshi state`. The newest resolved
/// signal is always kept, so ids stay monotonic.
const RESOLVED_SIGNALS_KEPT: usize = 8;

/// How a signal left the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalOutcome {
    /// A holding fleet fought the hostile fleet at the deadline.
    Fought,
    /// The deadline passed with no fleet holding.
    Expired,
}

impl SignalOutcome {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Fought => "fought",
            Self::Expired => "expired",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "fought" => Self::Fought,
            "expired" => Self::Expired,
            _ => return None,
        })
    }
}

/// One scanned contact. It is visible from `seen_at` and open until
/// `deadline`. `mission_id` links the hold mission; `outcome` set means resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub id: u64,
    pub coord: Coord,
    pub seen_at: u64,
    pub deadline: u64,
    /// Drawn from the galaxy seed and the id; sizes the hostile fleet and seeds the fight.
    pub seed: u64,
    pub mission_id: Option<u64>,
    pub outcome: Option<SignalOutcome>,
}

impl Signal {
    pub fn is_open(&self) -> bool {
        self.outcome.is_none()
    }
}

/// `[signals]` in balance.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalConfig {
    /// Deuterium one scan burns at the scanning planet.
    pub scan_deuterium: f64,
    /// Seconds a signal stays open after it is seen.
    pub hold_secs: u64,
    pub hostile_kind: ShipKind,
    pub hostile_min: u32,
    pub hostile_max: u32,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            scan_deuterium: 200.0,
            hold_secs: 3_600,
            hostile_kind: ShipKind::LightFighter,
            hostile_min: 2,
            hostile_max: 6,
        }
    }
}

#[derive(Deserialize)]
pub struct FileSignals {
    #[serde(default = "default_scan_deuterium")]
    scan_deuterium: f64,
    #[serde(default = "default_hold_secs")]
    hold_secs: u64,
    #[serde(default = "default_hostile_kind")]
    hostile_kind: String,
    #[serde(default = "default_hostile_min")]
    hostile_min: u32,
    #[serde(default = "default_hostile_max")]
    hostile_max: u32,
}

impl Default for FileSignals {
    fn default() -> Self {
        Self {
            scan_deuterium: default_scan_deuterium(),
            hold_secs: default_hold_secs(),
            hostile_kind: default_hostile_kind(),
            hostile_min: default_hostile_min(),
            hostile_max: default_hostile_max(),
        }
    }
}

fn default_scan_deuterium() -> f64 {
    SignalConfig::default().scan_deuterium
}
fn default_hold_secs() -> u64 {
    SignalConfig::default().hold_secs
}
fn default_hostile_kind() -> String {
    SignalConfig::default().hostile_kind.as_key().into()
}
fn default_hostile_min() -> u32 {
    SignalConfig::default().hostile_min
}
fn default_hostile_max() -> u32 {
    SignalConfig::default().hostile_max
}

impl FileSignals {
    pub fn into_config(self) -> Result<SignalConfig, String> {
        let hostile_kind = ShipKind::from_key(&self.hostile_kind)
            .ok_or_else(|| format!("unknown signals hostile_kind {}", self.hostile_kind))?;
        if self.hostile_min == 0 || self.hostile_max < self.hostile_min {
            return Err("signals needs 1 <= hostile_min <= hostile_max".into());
        }
        if self.hold_secs == 0 {
            return Err("signals hold_secs must be positive".into());
        }
        Ok(SignalConfig {
            scan_deuterium: if self.scan_deuterium.is_finite() {
                self.scan_deuterium.max(0.0)
            } else {
                0.0
            },
            hold_secs: self.hold_secs,
            hostile_kind,
            hostile_min: self.hostile_min,
            hostile_max: self.hostile_max,
        })
    }
}

/// Seed for signal `id` in this galaxy. The same galaxy seed and id always
/// give the same seed, so a reload or a replay cannot re-roll the contact.
pub fn signal_seed(galaxy_seed: u64, id: u64) -> u64 {
    let mut mix = galaxy_seed ^ 0x5167_A150_5167_A150 ^ id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    next_u64(&mut mix)
}

/// The free coord a signal with `seed` lands on: one of `free`, picked by seed.
pub fn signal_coord(seed: u64, free: &[Coord]) -> Option<Coord> {
    if free.is_empty() {
        return None;
    }
    let mut rng = seed;
    let pick = (next_u64(&mut rng) % free.len() as u64) as usize;
    free.get(pick).copied()
}

/// The hostile fleet a signal with `seed` brings: `hostile_min..=hostile_max`
/// of `hostile_kind`.
pub fn hostile_fleet(seed: u64, cfg: &SignalConfig) -> Fleet {
    let mut rng = seed ^ 0x0F1E_E70F_1EE7_0F1E;
    let span = u64::from(cfg.hostile_max.saturating_sub(cfg.hostile_min)) + 1;
    let n = cfg.hostile_min + (next_u64(&mut rng) % span) as u32;
    let mut fleet = Fleet::default();
    fleet.add(cfg.hostile_kind, n);
    fleet
}

impl GameState {
    pub fn signal(&self, id: u64) -> Option<&Signal> {
        self.signals.iter().find(|signal| signal.id == id)
    }

    /// The open signal at `coord`, seen or not.
    pub fn open_signal_at(&self, coord: Coord) -> Option<&Signal> {
        self.signals
            .iter()
            .find(|signal| signal.coord == coord && signal.is_open())
    }

    /// Open signals the player can see at `now`.
    pub fn seen_signals(&self, now: u64) -> impl Iterator<Item = &Signal> {
        self.signals
            .iter()
            .filter(move |signal| signal.is_open() && signal.seen_at <= now)
    }

    /// Home-system coords a scan may place a signal on: below the expedition
    /// slot, and not home, an owned planet, an outpost, an open expedition
    /// site, or another open signal.
    pub fn free_signal_coords(&self) -> Vec<Coord> {
        let owned = self.owned_coords();
        (1..self.config.expedition.dest_position)
            .map(|position| Coord {
                galaxy: self.home.galaxy,
                system: self.home.system,
                position,
            })
            .filter(|coord| {
                !owned.contains(coord)
                    && !self.outposts.iter().any(|outpost| outpost.coord == *coord)
                    && !self
                        .sites
                        .iter()
                        .any(|site| site.coord == *coord && site.outcome.is_none())
                    && self.open_signal_at(*coord).is_none()
            })
            .collect()
    }

    /// Burn `signals.scan_deuterium` at `origin` and place one signal on a
    /// free home-system coord. It becomes visible after the light delay from
    /// `origin` and stays open `signals.hold_secs` after that.
    pub fn scan_signal(&mut self, origin: Coord, now: u64) -> Result<u64, FleetError> {
        self.catch_up(now);
        let now = self.last_tick_unix;
        if !self.owned_coords().contains(&origin) {
            return Err(FleetError::UnknownPlanet);
        }
        let cost = self.config.signals.scan_deuterium;
        let have = self.resources_at_mut(origin)?.deuterium;
        if have + 1e-9 < cost {
            return Err(FleetError::InsufficientFuel {
                missing: (cost - have).max(0.0),
                cost,
            });
        }
        let id = self
            .signals
            .iter()
            .map(|signal| signal.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let seed = signal_seed(self.galaxy_seed, id);
        let coord =
            signal_coord(seed, &self.free_signal_coords()).ok_or(FleetError::NoSignalCoord)?;
        let res = self.resources_at_mut(origin)?;
        res.deuterium = (res.deuterium - cost).max(0.0);
        let seen_at = now.saturating_add(light_delay_secs(origin, coord, &self.config.orbits));
        self.signals.push(Signal {
            id,
            coord,
            seen_at,
            deadline: seen_at.saturating_add(self.config.signals.hold_secs),
            seed,
            mission_id: None,
            outcome: None,
        });
        self.push_event(now, format!("scan from {origin}: signal #{id} pending"));
        Ok(id)
    }

    /// Fly `ships` from home to a seen, open signal and hold until its
    /// deadline. The fleet must arrive before the deadline, and one fleet
    /// holds a signal at a time.
    pub fn hold_signal(
        &mut self,
        signal_id: u64,
        ships: Fleet,
        speed_percent: u8,
        now: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now);
        let now = self.last_tick_unix;
        let Some(signal) = self.signal(signal_id).cloned() else {
            return Err(FleetError::UnknownSignal { id: signal_id });
        };
        if !signal.is_open() {
            return Err(FleetError::SignalResolved { id: signal_id });
        }
        if signal.seen_at > now {
            return Err(FleetError::UnknownSignal { id: signal_id });
        }
        if signal.mission_id.is_some_and(|id| {
            self.missions
                .iter()
                .any(|m| m.id == id && m.phase != MissionPhase::Return)
        }) {
            return Err(FleetError::SignalBusy { id: signal_id });
        }
        let (outbound_secs, _) = self.flight_time_and_fuel(ships, signal.coord, speed_percent)?;
        let arrives = now.saturating_add(outbound_secs);
        if arrives >= signal.deadline {
            return Err(FleetError::SignalTooLate {
                id: signal_id,
                arrives,
                deadline: signal.deadline,
            });
        }
        let hold = signal.deadline - arrives;
        self.launch_fleet(ships, signal.coord, speed_percent, hold, now)?;
        let Some(mission_id) = self.missions.last().map(|m| m.id) else {
            return Err(FleetError::EmptyFleet);
        };
        if let Some(signal) = self.signals.iter_mut().find(|s| s.id == signal_id) {
            signal.mission_id = Some(mission_id);
        }
        Ok(mission_id)
    }

    /// Next signal instant after the last tick: one becoming visible, or an
    /// open deadline.
    pub(crate) fn next_signal_at(&self) -> Option<u64> {
        self.signals
            .iter()
            .filter(|signal| signal.is_open())
            .flat_map(|signal| [signal.seen_at, signal.deadline])
            .filter(|&at| at > self.last_tick_unix)
            .min()
    }

    /// Fight at the end of a signal hold. Returns true when the mission was
    /// removed (no survivors). Survivors return through the normal Action end.
    pub(crate) fn try_hold_signal_at(&mut self, index: usize, done_at: u64) -> bool {
        let mission_id = self.missions[index].id;
        let dest = self.missions[index].dest;
        let Some(at) = self.signals.iter().position(|signal| {
            signal.mission_id == Some(mission_id)
                && signal.coord == dest
                && signal.is_open()
                && signal.deadline <= done_at
        }) else {
            return false;
        };
        let signal = self.signals[at].clone();
        let enemy = hostile_fleet(signal.seed, &self.config.signals);
        let mut rng = signal.seed.wrapping_add(0xA5A5_A5A5_A5A5_A5A5);
        let report = self.resolve_player_attack(self.missions[index].ships, enemy, &mut rng);
        self.push_combat_log(done_at, report.clone());
        let debris = self.fight_debris(&report);
        self.note_fight_debris(done_at, dest, &report);
        self.signals[at].outcome = Some(SignalOutcome::Fought);
        self.missions[index].ships = report.attacker_after;
        self.push_event(
            done_at,
            format!(
                "signal #{} at {dest}: fought {} {}, {} rounds, survivors {}, debris {debris:.0}",
                signal.id,
                enemy.total(),
                self.config.signals.hostile_kind,
                report.rounds,
                report.attacker_after.total()
            ),
        );
        self.prune_signals();
        if self.missions[index].ships.is_empty() {
            self.missions.remove(index);
            return true;
        }
        false
    }

    /// Log signals that became visible and expire open signals whose
    /// deadline passed with no fight. Runs after missions at the same
    /// instant, so a hold that ends on the deadline fights first.
    pub(crate) fn advance_signals_at(&mut self, done_at: u64) {
        let mut lines = Vec::new();
        for signal in &mut self.signals {
            if !signal.is_open() {
                continue;
            }
            if signal.deadline <= done_at {
                signal.outcome = Some(SignalOutcome::Expired);
                lines.push(format!(
                    "signal #{} at {} expired unheld",
                    signal.id, signal.coord
                ));
            } else if signal.seen_at == done_at {
                lines.push(format!(
                    "signal #{} seen at {}, open {}s",
                    signal.id,
                    signal.coord,
                    signal.deadline - done_at
                ));
            }
        }
        for line in lines {
            self.push_event(done_at, line);
        }
        self.prune_signals();
    }

    /// Keep every open signal and the newest resolved ones.
    fn prune_signals(&mut self) {
        let resolved = self.signals.iter().filter(|s| !s.is_open()).count();
        let mut drop = resolved.saturating_sub(RESOLVED_SIGNALS_KEPT);
        if drop == 0 {
            return;
        }
        // Ids grow with time, so the lowest resolved ids are the oldest.
        self.signals.sort_by_key(|signal| signal.id);
        self.signals.retain(|signal| {
            if drop > 0 && !signal.is_open() {
                drop -= 1;
                return false;
            }
            true
        });
    }
}
