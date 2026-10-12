//! Fleet coordinates, flight time, fuel and in-flight missions.

use crate::ships::{Fleet, ShipKind};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Coord {
    pub galaxy: u16,
    pub system: u16,
    pub position: u16,
}

impl Coord {
    pub const HOME: Self = Self {
        galaxy: 1,
        system: 1,
        position: 1,
    };
}

impl fmt::Display for Coord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.galaxy, self.system, self.position)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlightConfig {
    /// Distance added per galaxy step.
    pub galaxy_unit: f64,
    /// Distance floor for a different system in the same galaxy.
    pub system_base: f64,
    /// Distance added per system step.
    pub system_unit: f64,
    /// Distance added per position step in the same system.
    pub position_unit: f64,
    pub min_distance: f64,
    /// Fuel is `sum(count * fuel) * distance / fuel_distance * speed_percent/100`.
    pub fuel_distance: f64,
    pub base_fleet_slots: u32,
}

impl Default for FlightConfig {
    fn default() -> Self {
        Self {
            galaxy_unit: 20_000.0,
            system_base: 2_700.0,
            system_unit: 95.0,
            position_unit: 5.0,
            min_distance: 5.0,
            fuel_distance: 35_000.0,
            base_fleet_slots: 1,
        }
    }
}

pub fn distance(from: Coord, to: Coord, cfg: &FlightConfig) -> f64 {
    let dg = u16::abs_diff(from.galaxy, to.galaxy) as f64;
    let ds = u16::abs_diff(from.system, to.system) as f64;
    let dp = u16::abs_diff(from.position, to.position) as f64;
    if dg > 0.0 {
        cfg.galaxy_unit * dg
    } else if ds > 0.0 {
        cfg.system_base + cfg.system_unit * ds
    } else {
        (cfg.position_unit * dp).max(cfg.min_distance)
    }
}

pub fn flight_secs(distance: f64, slowest_speed: f64, speed_percent: u8) -> u64 {
    let speed = slowest_speed.max(1.0) * (f64::from(speed_percent) / 100.0).max(0.01);
    (distance / speed).ceil().max(1.0) as u64
}

pub fn fuel_cost(units_fuel: f64, distance: f64, speed_percent: u8, cfg: &FlightConfig) -> f64 {
    units_fuel * distance / cfg.fuel_distance * (f64::from(speed_percent) / 100.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionPhase {
    Outbound,
    Action,
    Return,
}

impl MissionPhase {
    pub fn as_key(self) -> &'static str {
        match self {
            MissionPhase::Outbound => "outbound",
            MissionPhase::Action => "action",
            MissionPhase::Return => "return",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "outbound" => MissionPhase::Outbound,
            "action" => MissionPhase::Action,
            "return" => MissionPhase::Return,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mission {
    pub id: u64,
    pub ships: Fleet,
    pub origin: Coord,
    pub dest: Coord,
    pub speed_percent: u8,
    pub phase: MissionPhase,
    pub launched_at: u64,
    pub outbound_secs: u64,
    pub hold_secs: u64,
    pub phase_ends_at: u64,
    pub fuel_paid: f64,
    /// When true, Action tries to found a colony instead of holding for return.
    pub colonize: bool,
    /// Seed drawn at launch. `None` means this is not an expedition.
    pub expedition_seed: Option<u64>,
    /// Outcome already applied when Action started, so a delay cannot re-roll.
    pub expedition_applied: bool,
    /// Cargo delivered on Action when `transport` is set.
    pub cargo: crate::state::Resources,
    pub transport: bool,
    /// Action fights the outpost's orbital defences.
    pub attack: bool,
    /// Action lands troops. Refused while defences remain.
    pub invade: bool,
    /// Action writes a spy report and may lose probes.
    pub spy: bool,
}

/// A repeating transport between two owned planets.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeRoute {
    pub id: u64,
    pub origin: Coord,
    pub dest: Coord,
    pub ships: Fleet,
    pub cargo: crate::state::Resources,
    pub speed_percent: u8,
    pub interval_secs: u64,
    pub next_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FleetError {
    EmptyFleet,
    SameLocation,
    SpeedPercent {
        got: u8,
    },
    MissingShips {
        kind: ShipKind,
        need: u32,
        have: u32,
    },
    InsufficientFuel {
        missing: f64,
        cost: f64,
    },
    NoSlots {
        used: u32,
        cap: u32,
    },
    UnknownMission {
        id: u64,
    },
    AlreadyReturning,
    NeedColonyShip,
    HoldOutOfRange {
        got: u64,
        min: u64,
        max: u64,
    },
    NoExpeditionSlots {
        used: u32,
        cap: u32,
    },
    CargoExceedsCapacity {
        need: f64,
        cap: f64,
    },
    InvalidCargo,
    InsufficientCargo {
        missing: crate::state::Resources,
    },
    UnknownPlanet,
    NotOwned,
    UnknownOutpost,
    DefencesRemain,
    NeedTroopTransport,
    NeedEspionageProbe,
    ColonyCap {
        used: u32,
        cap: u32,
    },
}

impl fmt::Display for FleetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FleetError::EmptyFleet => write!(f, "fleet has no ships"),
            FleetError::SameLocation => write!(f, "destination is the origin"),
            FleetError::SpeedPercent { got } => {
                write!(f, "speed percent {got} is outside 10..=100")
            }
            FleetError::MissingShips { kind, need, have } => {
                write!(f, "not enough {kind} (need {need}, have {have})")
            }
            FleetError::InsufficientFuel { missing, cost } => {
                write!(
                    f,
                    "not enough deuterium (missing {missing:.0}, cost {cost:.0})"
                )
            }
            FleetError::NoSlots { used, cap } => {
                write!(f, "no free fleet slots ({used}/{cap})")
            }
            FleetError::UnknownMission { id } => write!(f, "no mission {id}"),
            FleetError::AlreadyReturning => write!(f, "mission is already returning"),
            FleetError::NeedColonyShip => write!(f, "colony mission needs a colony ship"),
            FleetError::HoldOutOfRange { got, min, max } => {
                write!(f, "hold {got}s is outside {min}..={max}")
            }
            FleetError::NoExpeditionSlots { used, cap } => {
                write!(f, "no free expedition slots ({used}/{cap})")
            }
            FleetError::CargoExceedsCapacity { need, cap } => {
                write!(f, "cargo {need:.0} exceeds capacity {cap:.0}")
            }
            FleetError::InvalidCargo => {
                write!(f, "cargo amounts must be finite and non-negative")
            }
            FleetError::InsufficientCargo { missing } => write!(
                f,
                "not enough cargo (missing metal {:.0}, crystal {:.0}, deuterium {:.0})",
                missing.metal, missing.crystal, missing.deuterium
            ),
            FleetError::UnknownPlanet => write!(f, "unknown planet"),
            FleetError::NotOwned => write!(f, "destination is not an owned planet"),
            FleetError::UnknownOutpost => write!(f, "no pirate outpost at destination"),
            FleetError::DefencesRemain => {
                write!(f, "invasion refused while orbital defences remain")
            }
            FleetError::NeedTroopTransport => {
                write!(f, "invasion needs a troop transport")
            }
            FleetError::NeedEspionageProbe => {
                write!(f, "spy mission needs an espionage probe")
            }
            FleetError::ColonyCap { used, cap } => {
                write!(f, "colony cap reached ({used}/{cap})")
            }
        }
    }
}

impl std::error::Error for FleetError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_time_and_fuel_match_fixed_examples() {
        let cfg = FlightConfig::default();
        let home = Coord::HOME;
        let neighbor = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let dist = distance(home, neighbor, &cfg);
        assert!((dist - 5.0).abs() < 1e-9);
        assert_eq!(flight_secs(dist, 12_500.0, 100), 1);
        let fuel = fuel_cost(20.0, dist, 100, &cfg);
        assert!((fuel - 20.0 * 5.0 / 35_000.0).abs() < 1e-12);

        let other_sys = Coord {
            galaxy: 1,
            system: 2,
            position: 1,
        };
        let dsys = distance(home, other_sys, &cfg);
        assert!((dsys - 2_795.0).abs() < 1e-9);
        assert_eq!(flight_secs(dsys, 10_000.0, 50), 1);

        let other_gal = Coord {
            galaxy: 2,
            system: 1,
            position: 1,
        };
        let dgal = distance(home, other_gal, &cfg);
        assert!((dgal - 20_000.0).abs() < 1e-9);
        assert_eq!(flight_secs(dgal, 10_000.0, 100), 2);
        let fuel_gal = fuel_cost(75.0, dgal, 100, &cfg);
        assert!((fuel_gal - 75.0 * 20_000.0 / 35_000.0).abs() < 1e-12);
    }
}
