//! Named mission planning: vehicles, payloads, windows, customers and routes.
//!
//! [`GameState::plan_mission`] is the governed action (propose, check, apply).

use crate::buildings::BuildingKind;
use crate::fleets::{Coord, LaunchFuel};
use crate::orbits::{
    fuel_mass_for_dv, heliocentric_radius_m, hohmann_transfer, interstellar_delta_v,
    launch_window_times, mean_motion, orbital_radius_m, rocket_delta_v, to_game_secs,
};
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, ResourceKind, Resources};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Hohmann,
    HighEnergy,
    Assist,
}

impl TransferKind {
    pub fn as_key(self) -> &'static str {
        match self {
            TransferKind::Hohmann => "hohmann",
            TransferKind::HighEnergy => "high_energy",
            TransferKind::Assist => "assist",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "hohmann" => TransferKind::Hohmann,
            "high_energy" => TransferKind::HighEnergy,
            "assist" => TransferKind::Assist,
            _ => return None,
        })
    }

    /// One-line player meaning for the Ops plan form and `hoshi help mission plan`.
    pub fn meaning(self) -> &'static str {
        match self {
            TransferKind::Hohmann => "cheap slow transfer",
            TransferKind::HighEnergy => "more fuel and less coast",
            TransferKind::Assist => "discount when a body sits between the two radii",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Customer {
    Colony,
    Faction(String),
    Market,
}

impl Customer {
    pub fn as_key(&self) -> String {
        match self {
            Customer::Colony => "colony".into(),
            Customer::Faction(id) => format!("faction:{id}"),
            Customer::Market => "market".into(),
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        if text == "colony" {
            return Some(Customer::Colony);
        }
        if text == "market" {
            return Some(Customer::Market);
        }
        let id = text.strip_prefix("faction:")?;
        if id.is_empty() {
            return None;
        }
        Some(Customer::Faction(id.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub origin: Coord,
    pub dest: Coord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchWindow {
    pub opens_at: u64,
    pub closes_at: u64,
}

impl LaunchWindow {
    pub fn contains(self, now: u64) -> bool {
        now >= self.opens_at && now < self.closes_at
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Payload {
    pub cargo: Resources,
}

impl Payload {
    pub fn mass_kg(&self, kg_per_cargo: f64) -> f64 {
        self.cargo.total() * kg_per_cargo
    }

    pub fn volume_m3(&self, m3_per_cargo: f64) -> f64 {
        self.cargo.total() * m3_per_cargo
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vehicle {
    pub id: u64,
    pub name: String,
    pub ships: Fleet,
    pub coord: Coord,
    pub free_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedMission {
    pub id: u64,
    pub vehicle_id: u64,
    pub route: Route,
    pub transfer: TransferKind,
    pub customer: Customer,
    pub window: LaunchWindow,
    pub launched_at: u64,
    pub arrives_at: u64,
    pub delta_v: f64,
    pub fuel_mass: f64,
    pub payload: Payload,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanRequest {
    pub vehicle_id: u64,
    pub dest: Coord,
    pub transfer: TransferKind,
    pub customer: Customer,
    pub payload: Payload,
    pub window_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MissionPlan {
    pub vehicle_id: u64,
    pub route: Route,
    pub transfer: TransferKind,
    pub customer: Customer,
    pub window: LaunchWindow,
    pub payload: Payload,
    pub delta_v: f64,
    pub fuel_mass: f64,
    pub coast_secs: u64,
    pub available_dv: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleAlertKind {
    DoubleBook,
    MissedWindow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleAlert {
    pub kind: ScheduleAlertKind,
    pub vehicle_id: u64,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MissionSchedule {
    pub vehicles: Vec<Vehicle>,
    pub missions: Vec<PlannedMission>,
    pub alerts: Vec<ScheduleAlert>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    EmptyFleet,
    MissingShips {
        kind: ShipKind,
        need: u32,
        have: u32,
    },
    UnknownPlanet,
    UnknownVehicle(u64),
    VehicleBusy {
        id: u64,
        free_at: u64,
    },
    VehicleElsewhere {
        at: Coord,
        want: Coord,
    },
    SameLocation,
    UnknownTransfer(String),
    UnknownCustomer(String),
    MarketContracts,
    DestNotOwned,
    UnknownFaction(String),
    MissedWindow {
        closed_at: u64,
    },
    WindowClosed,
    DoubleBook {
        id: u64,
    },
    NoAssistBody,
    CannotAfford {
        missing: Resources,
    },
    PayloadVolume {
        need: f64,
        have: f64,
    },
    ShortDeltaV {
        need: f64,
        have: f64,
    },
    InvalidCargo,
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::EmptyFleet => write!(f, "vehicle needs ships"),
            PlanError::MissingShips { kind, need, have } => {
                write!(f, "need {need} {kind}, have {have}")
            }
            PlanError::UnknownPlanet => write!(f, "unknown planet"),
            PlanError::UnknownVehicle(id) => write!(f, "unknown vehicle #{id}"),
            PlanError::VehicleBusy { id, free_at } => {
                write!(f, "vehicle #{id} is booked until {free_at}")
            }
            PlanError::VehicleElsewhere { at, want } => {
                write!(f, "vehicle is at {at}, not {want}")
            }
            PlanError::SameLocation => write!(f, "origin and destination are the same"),
            PlanError::UnknownTransfer(id) => write!(f, "unknown transfer {id}"),
            PlanError::UnknownCustomer(id) => write!(f, "unknown customer {id}"),
            PlanError::MarketContracts => {
                write!(
                    f,
                    "market is off; contract customers need an enabled market"
                )
            }
            PlanError::DestNotOwned => write!(f, "colony customer needs an owned destination"),
            PlanError::UnknownFaction(id) => write!(f, "unknown faction {id}"),
            PlanError::MissedWindow { closed_at } => {
                write!(f, "launch window closed at {closed_at}")
            }
            PlanError::WindowClosed => write!(f, "no open launch window"),
            PlanError::DoubleBook { id } => write!(f, "vehicle #{id} is already on a mission"),
            PlanError::NoAssistBody => {
                write!(f, "gravity assist needs a body between the two orbits")
            }
            PlanError::CannotAfford { missing } => write!(
                f,
                "not enough resources (missing iron {:.0}, silicon {:.0}, water ice {:.0}, deuterium {:.0})",
                missing.iron, missing.silicon, missing.water_ice, missing.deuterium
            ),
            PlanError::PayloadVolume { need, have } => {
                write!(f, "payload volume {need:.0} exceeds cargo {have:.0}")
            }
            PlanError::ShortDeltaV { need, have } => {
                write!(f, "need {need:.0} m/s delta-v, vehicle has {have:.0}")
            }
            PlanError::InvalidCargo => write!(f, "cargo amounts must be finite and >= 0"),
        }
    }
}

impl std::error::Error for PlanError {}

#[derive(Debug, Clone, PartialEq)]
pub struct TransferEstimate {
    pub delta_v: f64,
    pub coast_secs: u64,
    pub windows: Vec<LaunchWindow>,
}

struct TransferPhysics {
    delta_v: f64,
    fuel_mass: f64,
    available_dv: f64,
    coast_secs: u64,
}

impl GameState {
    pub fn register_vehicle(
        &mut self,
        planet: Coord,
        ships: Fleet,
        now_unix: u64,
    ) -> Result<u64, PlanError> {
        self.catch_up(now_unix);
        if ships.is_empty() {
            return Err(PlanError::EmptyFleet);
        }
        if !self.owned_coords().contains(&planet) {
            return Err(PlanError::UnknownPlanet);
        }
        let parked = self.parked_fleet(planet).ok_or(PlanError::UnknownPlanet)?;
        for kind in ShipKind::ALL {
            let need = ships.get(kind);
            let have = parked.get(kind);
            if have < need {
                return Err(PlanError::MissingShips { kind, need, have });
            }
        }
        self.take_parked(planet, ships);
        let id = self.next_vehicle_id;
        self.next_vehicle_id = self.next_vehicle_id.saturating_add(1);
        let name = format!("vehicle {id}");
        self.vehicles.push(Vehicle {
            id,
            name: name.clone(),
            ships,
            coord: planet,
            free_at: self.last_tick_unix,
        });
        self.push_event(self.last_tick_unix, format!("registered {name}"));
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: crate::HistoryKind::Fleet,
            actor: "player".into(),
            action: "register".into(),
            objects: vec![format!("vehicle:{id}")],
            outcome: "registered".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(id)
    }

    pub fn dissolve_vehicle(&mut self, id: u64, now_unix: u64) -> Result<(), PlanError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if self.planned.iter().any(|m| m.vehicle_id == id) {
            return Err(PlanError::DoubleBook { id });
        }
        let index = self
            .vehicles
            .iter()
            .position(|v| v.id == id)
            .ok_or(PlanError::UnknownVehicle(id))?;
        if self.vehicles[index].free_at > now {
            return Err(PlanError::VehicleBusy {
                id,
                free_at: self.vehicles[index].free_at,
            });
        }
        let vehicle = self.vehicles.remove(index);
        self.return_parked(vehicle.coord, vehicle.ships);
        self.push_event(now, format!("dissolved vehicle #{id}"));
        Ok(())
    }

    pub fn mission_windows(
        &self,
        origin: Coord,
        dest: Coord,
        transfer: TransferKind,
        now: u64,
    ) -> Result<TransferEstimate, PlanError> {
        self.estimate_transfer(origin, dest, transfer, now)
    }

    /// Propose a plan, check it, then apply. The governed PlanMission action.
    pub fn plan_mission(&mut self, request: PlanRequest, now_unix: u64) -> Result<u64, PlanError> {
        self.catch_up(now_unix);
        let plan = self.propose_mission(&request, self.last_tick_unix)?;
        self.check_mission(&plan, self.last_tick_unix)?;
        self.apply_mission(plan)
    }

    pub fn propose_mission(
        &self,
        request: &PlanRequest,
        now: u64,
    ) -> Result<MissionPlan, PlanError> {
        let vehicle = self
            .vehicles
            .iter()
            .find(|v| v.id == request.vehicle_id)
            .ok_or(PlanError::UnknownVehicle(request.vehicle_id))?;
        if request.dest == vehicle.coord {
            return Err(PlanError::SameLocation);
        }
        let estimate =
            self.estimate_transfer(vehicle.coord, request.dest, request.transfer, now)?;
        let window = match request.window_index {
            Some(i) => *estimate
                .windows
                .get(i)
                .ok_or(PlanError::MissedWindow { closed_at: now })?,
            None => estimate
                .windows
                .iter()
                .copied()
                .find(|w| w.contains(now) || w.opens_at >= now)
                .ok_or(PlanError::WindowClosed)?,
        };
        let (fuel_mass, available_dv) =
            self.propellant_for(vehicle, &request.payload, estimate.delta_v)?;
        Ok(MissionPlan {
            vehicle_id: request.vehicle_id,
            route: Route {
                origin: vehicle.coord,
                dest: request.dest,
            },
            transfer: request.transfer,
            customer: request.customer.clone(),
            window,
            payload: request.payload.clone(),
            delta_v: estimate.delta_v,
            fuel_mass,
            coast_secs: estimate.coast_secs,
            available_dv,
        })
    }

    pub fn check_mission(&self, plan: &MissionPlan, now: u64) -> Result<(), PlanError> {
        if !plan.payload.cargo.is_finite_nonneg() {
            return Err(PlanError::InvalidCargo);
        }
        match &plan.customer {
            Customer::Market => {
                if !self.config.market.enabled {
                    return Err(PlanError::MarketContracts);
                }
            }
            Customer::Colony => {
                if !self.owned_coords().contains(&plan.route.dest) {
                    return Err(PlanError::DestNotOwned);
                }
            }
            Customer::Faction(id) => {
                if self.config.diplomacy.get(id).is_none() {
                    return Err(PlanError::UnknownFaction(id.clone()));
                }
            }
        }
        let vehicle = self
            .vehicles
            .iter()
            .find(|v| v.id == plan.vehicle_id)
            .ok_or(PlanError::UnknownVehicle(plan.vehicle_id))?;
        if vehicle.coord != plan.route.origin {
            return Err(PlanError::VehicleElsewhere {
                at: vehicle.coord,
                want: plan.route.origin,
            });
        }
        if vehicle.free_at > now {
            return Err(PlanError::VehicleBusy {
                id: vehicle.id,
                free_at: vehicle.free_at,
            });
        }
        if self.planned.iter().any(|m| m.vehicle_id == plan.vehicle_id) {
            return Err(PlanError::DoubleBook {
                id: plan.vehicle_id,
            });
        }
        if plan.window.closes_at <= now {
            return Err(PlanError::MissedWindow {
                closed_at: plan.window.closes_at,
            });
        }
        let physics = self.physics_for(plan, now)?;
        let cfg = &self.config.orbits;
        let fuel = self.launch_fuel_from_mass(vehicle.ships, physics.fuel_mass);
        let volume = plan.payload.volume_m3(cfg.m3_per_cargo);
        let cap = self.vehicle_volume(vehicle);
        if volume > cap + 1e-9 {
            return Err(PlanError::PayloadVolume {
                need: volume,
                have: cap,
            });
        }
        if physics.available_dv + 1e-6 < physics.delta_v {
            return Err(PlanError::ShortDeltaV {
                need: physics.delta_v,
                have: physics.available_dv,
            });
        }
        let origin_stock = self
            .resources_at(plan.route.origin)
            .copied()
            .ok_or(PlanError::UnknownPlanet)?;
        let need = plan.payload.cargo.with_launch_fuel(fuel);
        if !origin_stock.can_afford(need) {
            return Err(PlanError::CannotAfford {
                missing: origin_stock.missing(need),
            });
        }
        Ok(())
    }

    pub fn apply_mission(&mut self, plan: MissionPlan) -> Result<u64, PlanError> {
        let now = self.last_tick_unix;
        self.check_mission(&plan, now)?;
        let physics = self.physics_for(&plan, now)?;
        let cfg = self.config.orbits.clone();
        let vehicle = self
            .vehicles
            .iter()
            .find(|v| v.id == plan.vehicle_id)
            .ok_or(PlanError::UnknownVehicle(plan.vehicle_id))?;
        let fuel = self.launch_fuel_from_mass(vehicle.ships, physics.fuel_mass);
        self.debit_at(plan.route.origin, plan.payload.cargo.with_launch_fuel(fuel))?;
        let launched = now.max(plan.window.opens_at);
        let arrives = launched.saturating_add(physics.coast_secs);
        let free_at = arrives.saturating_add(cfg.turnaround_secs);
        if let Some(vehicle) = self.vehicles.iter_mut().find(|v| v.id == plan.vehicle_id) {
            vehicle.free_at = free_at;
        }
        let id = self.next_planned_id;
        self.next_planned_id = self.next_planned_id.saturating_add(1);
        self.planned.push(PlannedMission {
            id,
            vehicle_id: plan.vehicle_id,
            route: plan.route,
            transfer: plan.transfer,
            customer: plan.customer.clone(),
            window: plan.window,
            launched_at: launched,
            arrives_at: arrives,
            delta_v: physics.delta_v,
            fuel_mass: physics.fuel_mass,
            payload: plan.payload.clone(),
        });
        self.push_event(
            now,
            format!(
                "mission {id}: {} {} -> {} dv {:.0}",
                plan.transfer.as_key(),
                plan.route.origin,
                plan.route.dest,
                plan.delta_v
            ),
        );
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Fleet,
            actor: "player".into(),
            action: "plan".into(),
            objects: vec![
                format!("vehicle:{}", plan.vehicle_id),
                format!("mission:{id}"),
                plan.route.dest.to_string(),
            ],
            inputs: format!(
                "transfer={} customer={}",
                plan.transfer.as_key(),
                plan.customer.as_key()
            ),
            outcome: format!("arrives {arrives}"),
            ..crate::HistoryDraft::default()
        });
        Ok(id)
    }

    pub fn mission_schedule(&self, now: u64) -> MissionSchedule {
        let mut alerts = Vec::new();
        for vehicle in &self.vehicles {
            let booked: Vec<_> = self
                .planned
                .iter()
                .filter(|m| m.vehicle_id == vehicle.id)
                .collect();
            if booked.len() > 1 {
                alerts.push(ScheduleAlert {
                    kind: ScheduleAlertKind::DoubleBook,
                    vehicle_id: vehicle.id,
                    detail: format!("{} overlapping missions", booked.len()),
                });
            }
            for mission in &booked {
                if mission.window.closes_at <= mission.launched_at {
                    alerts.push(ScheduleAlert {
                        kind: ScheduleAlertKind::MissedWindow,
                        vehicle_id: vehicle.id,
                        detail: format!("mission {} launched after window", mission.id),
                    });
                }
            }
            if vehicle.free_at > now
                && booked.is_empty()
                && self
                    .planned
                    .iter()
                    .filter(|m| m.vehicle_id == vehicle.id)
                    .count()
                    == 0
            {
                // booked via turnaround with no flight row is not an alert
            }
        }
        MissionSchedule {
            vehicles: self.vehicles.clone(),
            missions: self.planned.clone(),
            alerts,
        }
    }

    pub(crate) fn next_planned_at(&self) -> Option<u64> {
        self.planned.iter().map(|m| m.arrives_at).min()
    }

    pub(crate) fn advance_planned_at(&mut self, done_at: u64) {
        loop {
            let Some(index) = self.planned.iter().position(|m| m.arrives_at == done_at) else {
                return;
            };
            let mission = self.planned.remove(index);
            if let Some(vehicle) = self
                .vehicles
                .iter_mut()
                .find(|v| v.id == mission.vehicle_id)
            {
                vehicle.coord = match &mission.customer {
                    Customer::Colony => mission.route.dest,
                    Customer::Faction(_) | Customer::Market => mission.route.origin,
                };
            }
            match &mission.customer {
                Customer::Colony => {
                    self.credit_at(mission.route.dest, mission.payload.cargo);
                }
                Customer::Faction(id) => {
                    self.apply_faction_delta(id, |f| f.trade_delta, "mission", done_at);
                }
                Customer::Market => {
                    self.credit_market_delivery(mission.route.dest, mission.payload.cargo, done_at);
                }
            }
            self.push_event(
                done_at,
                format!(
                    "mission {}: delivered to {}",
                    mission.id, mission.route.dest
                ),
            );
            self.record_history(crate::HistoryDraft {
                at: done_at,
                kind: crate::HistoryKind::Fleet,
                actor: "simulation".into(),
                action: "deliver".into(),
                objects: vec![format!("mission:{}", mission.id)],
                outcome: "delivered".into(),
                parent_key: Some(format!("mission:{}", mission.id)),
                ..crate::HistoryDraft::default()
            });
        }
    }

    fn physics_for(&self, plan: &MissionPlan, now: u64) -> Result<TransferPhysics, PlanError> {
        let vehicle = self
            .vehicles
            .iter()
            .find(|v| v.id == plan.vehicle_id)
            .ok_or(PlanError::UnknownVehicle(plan.vehicle_id))?;
        let estimate =
            self.estimate_transfer(plan.route.origin, plan.route.dest, plan.transfer, now)?;
        if !estimate.windows.contains(&plan.window) {
            return Err(PlanError::WindowClosed);
        }
        let (fuel_mass, available_dv) =
            self.propellant_for(vehicle, &plan.payload, estimate.delta_v)?;
        if !fuel_mass.is_finite() || fuel_mass < 0.0 {
            return Err(PlanError::InvalidCargo);
        }
        Ok(TransferPhysics {
            delta_v: estimate.delta_v,
            fuel_mass,
            available_dv,
            coast_secs: estimate.coast_secs,
        })
    }

    fn estimate_transfer(
        &self,
        origin: Coord,
        dest: Coord,
        transfer: TransferKind,
        now: u64,
    ) -> Result<TransferEstimate, PlanError> {
        if origin == dest {
            return Err(PlanError::SameLocation);
        }
        let cfg = &self.config.orbits;
        let r1 = heliocentric_radius_m(origin, cfg);
        let r2 = heliocentric_radius_m(dest, cfg);
        let burn = self.base_burn(origin, dest);
        if transfer == TransferKind::Assist && !self.assist_body_between(origin, dest, r1, r2) {
            return Err(PlanError::NoAssistBody);
        }
        let (dv_xfer, coast_real) = match transfer {
            TransferKind::Hohmann => (burn.delta_v, burn.coast_secs),
            TransferKind::HighEnergy => (
                burn.delta_v * cfg.high_energy_dv,
                burn.coast_secs * cfg.high_energy_time,
            ),
            TransferKind::Assist => (burn.delta_v * cfg.assist_dv, burn.coast_secs),
        };
        let wells = self.well_cost(origin) + self.well_cost(dest);
        let delta_v = dv_xfer + wells;
        let coast_secs = to_game_secs(coast_real, cfg.coast_compress);
        let n1 = mean_motion(cfg.mu_star, r1);
        let n2 = mean_motion(cfg.mu_star, r2);
        let windows = launch_window_times(now, n1, n2, coast_real, cfg)
            .into_iter()
            .map(|(opens_at, closes_at)| LaunchWindow {
                opens_at,
                closes_at,
            })
            .collect();
        Ok(TransferEstimate {
            delta_v,
            coast_secs,
            windows,
        })
    }

    /// Hohmann burn between the two orbits, before surface wells. Another
    /// system has its own star: the heliocentric Hohmann keeps the coast
    /// time, but delta-v is escape plus capture, so fuel never falls as the
    /// destination gets farther away.
    fn base_burn(&self, origin: Coord, dest: Coord) -> crate::orbits::Hohmann {
        let cfg = &self.config.orbits;
        let mut burn = hohmann_transfer(
            heliocentric_radius_m(origin, cfg),
            heliocentric_radius_m(dest, cfg),
            cfg.mu_star,
        );
        if (origin.galaxy, origin.system) != (dest.galaxy, dest.system) {
            burn.delta_v = interstellar_delta_v(
                orbital_radius_m(origin, cfg),
                orbital_radius_m(dest, cfg),
                cfg.mu_star,
            );
        }
        burn
    }

    /// Reaction mass a star-map send of `ships` from `origin` to `dest`
    /// burns at launch: the rocket equation on the same delta-v, dry mass
    /// and exhaust velocity a vehicle mission uses. Paid in water ice.
    /// `speed_percent` scales the transfer burn from Hohmann (10%) to the
    /// high-energy burn (100%); the wells cost the same at any speed. A
    /// round trip pays its return leg now as well, at the mass it carries
    /// back.
    #[cfg(test)]
    pub(crate) fn send_fuel(
        &self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        haul: crate::Haul,
        speed_percent: u8,
    ) -> f64 {
        let (out, back) = self.send_fuel_legs(origin, dest, ships, haul, speed_percent);
        out + back
    }

    /// Outbound and return reaction mass for [`Self::send_fuel`], so a
    /// launch can bill each leg to a different owned stockpile.
    pub(crate) fn send_fuel_legs(
        &self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        haul: crate::Haul,
        speed_percent: u8,
    ) -> (f64, f64) {
        let cfg = &self.config.orbits;
        let share = f64::from(speed_percent.clamp(10, 100) - 10) / 90.0;
        let factor = 1.0 + (cfg.high_energy_dv - 1.0) * share;
        let delta_v = self.base_burn(origin, dest).delta_v * factor
            + self.well_cost(origin)
            + self.well_cost(dest);
        let ve = self.fleet_ve(ships);
        let dry = self.fleet_dry_kg(ships);
        let leg = |cargo: crate::Resources| {
            fuel_mass_for_dv(ve, dry + cargo.total() * cfg.kg_per_cargo, delta_v) / cfg.kg_per_cargo
        };
        (leg(haul.out), haul.back.map_or(0.0, leg))
    }

    /// Highest deuterium share among drives in `ships`. A mixed stack still
    /// burns the fusion share of its impulse hulls.
    pub fn fleet_deuterium_share(&self, ships: Fleet) -> f64 {
        let mut share: f64 = 0.0;
        for kind in ShipKind::ALL {
            if ships.get(kind) == 0 {
                continue;
            }
            share = share.max(
                self.config
                    .orbits
                    .deuterium_share_for_drive(self.hull_drive(kind)),
            );
        }
        share
    }

    /// Water ice plus deuterium share for `reaction` mass (cargo units).
    pub fn launch_fuel_from_units(&self, ships: Fleet, water_ice: f64) -> LaunchFuel {
        LaunchFuel {
            water_ice,
            deuterium: water_ice * self.fleet_deuterium_share(ships),
        }
    }

    /// Water ice plus deuterium share for a rocket-equation mass in kg.
    pub fn launch_fuel_from_mass(&self, ships: Fleet, fuel_mass_kg: f64) -> LaunchFuel {
        self.launch_fuel_from_units(ships, fuel_mass_kg / self.config.orbits.kg_per_cargo)
    }

    /// Outbound and return launch bills for `haul`. A one-way send has a
    /// zero return bill.
    pub fn launch_fuel_legs(
        &self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        haul: crate::Haul,
        speed_percent: u8,
    ) -> (LaunchFuel, LaunchFuel) {
        let (out, back) = self.send_fuel_legs(origin, dest, ships, haul, speed_percent);
        (
            self.launch_fuel_from_units(ships, out),
            self.launch_fuel_from_units(ships, back),
        )
    }

    /// Delta-v to climb out of (or descend into) `coord`'s surface well. A
    /// built space elevator lifts ships up its tether instead, so the well
    /// costs nothing there.
    fn well_cost(&self, coord: Coord) -> f64 {
        if self
            .buildings_at(coord)
            .is_some_and(|buildings| buildings.space_elevator > 0)
        {
            return 0.0;
        }
        let kind = self.planet_kind(coord).unwrap_or_else(|| {
            crate::planet_profile(self.galaxy_seed, coord, self.generator_version).0
        });
        self.config.orbits.well_dv(kind)
    }

    fn assist_body_between(&self, origin: Coord, dest: Coord, r1: f64, r2: f64) -> bool {
        let lo = r1.min(r2);
        let hi = r1.max(r2);
        let cfg = &self.config.orbits;
        let mut coords = vec![origin, dest];
        if origin.galaxy == dest.galaxy && origin.system == dest.system {
            for position in 1..=16 {
                coords.push(Coord {
                    galaxy: origin.galaxy,
                    system: origin.system,
                    position,
                });
            }
        }
        coords.into_iter().any(|coord| {
            if coord == origin || coord == dest {
                return false;
            }
            let r = heliocentric_radius_m(coord, cfg);
            r > lo + 1.0 && r < hi - 1.0
        })
    }

    /// Dry mass in kg of a vehicle made of `ships`, as mission fuel uses it.
    pub fn fleet_dry_kg(&self, ships: Fleet) -> f64 {
        let cfg = &self.config.orbits;
        ShipKind::ALL
            .iter()
            .map(|&kind| {
                f64::from(ships.get(kind)) * self.config.ships.get(kind).hull * cfg.kg_per_hull
            })
            .sum()
    }

    /// Lowest exhaust velocity among `ships`' drives: the slowest drive
    /// sets the whole stack's propellant.
    fn fleet_ve(&self, ships: Fleet) -> f64 {
        let mut ve = f64::MAX;
        for kind in ShipKind::ALL {
            if ships.get(kind) == 0 {
                continue;
            }
            let drive = self.hull_drive(kind);
            ve = ve.min(self.drive_ve(drive));
        }
        if ve.is_finite() {
            ve
        } else {
            self.config.orbits.combustion_ve
        }
    }

    /// Drive id of the slowest ship in `ships`, the one that sets propellant.
    fn fleet_slowest_drive(&self, ships: Fleet) -> Option<&str> {
        let mut best: Option<(&str, f64)> = None;
        for kind in ShipKind::ALL {
            if ships.get(kind) == 0 {
                continue;
            }
            let drive = self.hull_drive(kind);
            let ve = self.drive_ve(drive);
            if best.is_none_or(|(_, slow)| ve < slow) {
                best = Some((drive, ve));
            }
        }
        best.map(|(drive, _)| drive)
    }

    /// Why a combustion fleet's send is expensive, with the refit research
    /// that moves its slowest hull onto a faster drive and, between systems,
    /// the cheapest faster hull the player has built or can build that
    /// still carries this cargo. `fuel_ice` is the send's water ice bill.
    /// An in-system send is expensive only when that bill exceeds `cargo`
    /// and a refit exists to point at; faster drives return `None`.
    pub fn expensive_send_hint(
        &self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        cargo: f64,
        fuel_ice: f64,
    ) -> Option<String> {
        let drive = self.fleet_slowest_drive(ships)?;
        if drive != "combustion_drive" {
            return None;
        }
        let interstellar = (origin.galaxy, origin.system) != (dest.galaxy, dest.system);
        let refit = self.refit_off_drive(ships, drive);
        if !interstellar && (fuel_ice <= cargo || refit.is_none()) {
            return None;
        }
        let drive_name = self.display_name(drive);
        let scope = if interstellar {
            "interstellar send"
        } else {
            "send"
        };
        let mut text = format!("{drive_name} makes this {scope} expensive");
        if let Some(tech) = refit {
            text.push_str(&format!("; research {}", tech.name));
            let gaps = self.missing_research_prereqs(&tech.id).unwrap_or_default();
            if !gaps.is_empty() {
                let gaps: Vec<String> = gaps.iter().map(ToString::to_string).collect();
                text.push_str(&format!(" (needs {})", gaps.join(", ")));
            }
        }
        if interstellar
            && let Some(kind) = self.faster_haul_hull(ships, cargo, self.drive_ve(drive))
        {
            let spec = self.config.ships.get(kind);
            let faster = self.display_name(self.hull_drive(kind));
            text.push_str(&format!("; {} flies {faster}", spec.name));
        }
        Some(text)
    }

    /// Another owned world with `ships` parked that would launch the same
    /// mission to `dest` at `speed_percent` for at least a quarter less
    /// fuel, named with its bill: "cheaper from Ice (1:1:9): 797 ice, 64%
    /// less". `None` when `origin` is cheapest or no other world holds the
    /// ships. Used by the dispatch preview and the `fleet send` reply.
    pub fn cheaper_origin_hint(
        &self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        haul: crate::Haul,
        speed_percent: u8,
    ) -> Option<String> {
        // A smaller saving is noise next to the wait for ships to move.
        const MIN_SAVING: f64 = 0.25;
        let bill = |from: Coord| {
            self.flight_legs(from, dest, ships, haul, speed_percent)
                .ok()
                .map(|(_, out, back)| out.plus(back))
        };
        let here = bill(origin)?.total();
        if here <= 0.0 {
            return None;
        }
        let (world, fuel) = self
            .owned_coords()
            .into_iter()
            .filter(|&world| world != origin && world != dest)
            .filter(|&world| {
                self.fleet_at(world).is_some_and(|parked| {
                    ShipKind::ALL
                        .iter()
                        .all(|&kind| parked.get(kind) >= ships.get(kind))
                })
            })
            .filter_map(|world| bill(world).map(|fuel| (world, fuel)))
            .min_by(|a, b| a.1.total().total_cmp(&b.1.total()))?;
        let saving = 1.0 - fuel.total() / here;
        if saving < MIN_SAVING {
            return None;
        }
        let mut text = format!(
            "cheaper from {}: {:.0} ice",
            self.world_label(world),
            fuel.water_ice
        );
        if fuel.deuterium > 1e-9 {
            text.push_str(&format!(" + {:.0} D", fuel.deuterium));
        }
        text.push_str(&format!(", {:.0}% less", saving * 100.0));
        Some(text)
    }

    /// An unresearched tech that refits a hull in `ships` now flying `drive`
    /// onto a drive with a higher base exhaust velocity.
    fn refit_off_drive(&self, ships: Fleet, drive: &str) -> Option<&crate::TechSpec> {
        let orbits = &self.config.orbits;
        let slow_ve = orbits.ve_for_drive(drive);
        self.config.techs.iter().find(|spec| {
            self.tech_level(&spec.id) < 1
                && spec.refits.iter().any(|refit| {
                    ships.get(refit.ship) > 0
                        && self.hull_drive(refit.ship) == drive
                        && orbits.ve_for_drive(&refit.drive) > slow_ve
                })
        })
    }

    fn faster_haul_hull(&self, ships: Fleet, cargo: f64, slow_ve: f64) -> Option<ShipKind> {
        let mut need = cargo;
        for kind in ShipKind::ALL {
            if ships.get(kind) == 0 {
                continue;
            }
            if self.drive_ve(self.hull_drive(kind)) <= slow_ve + 1e-9 {
                need = need.max(self.config.ships.get(kind).cargo);
            }
        }
        let mut best: Option<(ShipKind, f64)> = None;
        for kind in ShipKind::ALL {
            if self.drive_ve(self.hull_drive(kind)) <= slow_ve + 1e-9 {
                continue;
            }
            let spec = self.config.ships.get(kind);
            if spec.cargo + 1e-9 < need {
                continue;
            }
            if !self.owns_hull(kind) && !self.can_build_hull(kind) {
                continue;
            }
            let cost = spec.unit_cost().total();
            if best.is_none_or(|(_, cheapest)| cost < cheapest) {
                best = Some((kind, cost));
            }
        }
        best.map(|(kind, _)| kind)
    }

    fn owns_hull(&self, kind: ShipKind) -> bool {
        self.owned_coords().iter().any(|coord| {
            self.fleet_at(*coord)
                .is_some_and(|fleet| fleet.get(kind) > 0)
        })
    }

    fn can_build_hull(&self, kind: ShipKind) -> bool {
        if !self.missing_ship_prereqs(kind).is_empty() {
            return false;
        }
        let need = self.config.ships.get(kind).shipyard;
        self.owned_coords()
            .iter()
            .any(|coord| self.available_building_level_at(*coord, BuildingKind::Shipyard) >= need)
    }

    pub(crate) fn insufficient_fuel(
        &self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        cargo: f64,
        fuel_ice: f64,
        shortfall: (ResourceKind, f64, f64),
    ) -> crate::FleetError {
        let (kind, missing, cost) = shortfall;
        crate::FleetError::InsufficientFuel {
            missing,
            cost,
            kind,
            hint: self.expensive_send_hint(origin, dest, ships, cargo, fuel_ice),
        }
    }

    fn vehicle_volume(&self, vehicle: &Vehicle) -> f64 {
        let cfg = &self.config.orbits;
        ShipKind::ALL
            .iter()
            .map(|&kind| {
                f64::from(vehicle.ships.get(kind))
                    * self.config.ships.get(kind).cargo
                    * cfg.m3_per_cargo
            })
            .sum()
    }

    fn propellant_for(
        &self,
        vehicle: &Vehicle,
        payload: &Payload,
        need_dv: f64,
    ) -> Result<(f64, f64), PlanError> {
        let cfg = &self.config.orbits;
        let dry = self.fleet_dry_kg(vehicle.ships) + payload.mass_kg(cfg.kg_per_cargo);
        let ve = self.fleet_ve(vehicle.ships);
        let fuel = fuel_mass_for_dv(ve, dry, need_dv);
        let available = rocket_delta_v(ve, dry + fuel, dry);
        Ok((fuel, available))
    }

    /// Ships parked at an owned planet, the pool `register_vehicle` draws from.
    pub fn parked_fleet(&self, planet: Coord) -> Option<Fleet> {
        if planet == self.home {
            return Some(self.fleet);
        }
        self.colonies
            .iter()
            .find(|c| c.coord == planet)
            .map(|c| c.fleet)
    }

    fn take_parked(&mut self, planet: Coord, ships: Fleet) {
        let fleet = if planet == self.home {
            &mut self.fleet
        } else if let Some(colony) = self.colonies.iter_mut().find(|c| c.coord == planet) {
            &mut colony.fleet
        } else {
            return;
        };
        for kind in ShipKind::ALL {
            fleet.saturating_sub(kind, ships.get(kind));
        }
    }

    fn return_parked(&mut self, planet: Coord, ships: Fleet) {
        self.add_fleet_at(planet, ships);
    }

    /// Take `cost` (cargo plus fuel) from `planet`, every stockpile kind, or
    /// refuse with what is missing.
    fn debit_at(&mut self, planet: Coord, cost: Resources) -> Result<(), PlanError> {
        let stock = self
            .resources_at_mut(planet)
            .map_err(|_| PlanError::UnknownPlanet)?;
        if !stock.can_afford(cost) {
            return Err(PlanError::CannotAfford {
                missing: stock.missing(cost),
            });
        }
        for kind in ResourceKind::ALL {
            stock.set(kind, (stock.get(kind) - cost.get(kind)).max(0.0));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Haul;
    use crate::balance::BalanceConfig;
    use crate::orbits::Hohmann;
    use crate::orbits::{heliocentric_radius_m, hohmann_transfer};

    #[test]
    fn transfer_kind_meanings_name_the_three_burns() {
        assert_eq!(TransferKind::Hohmann.meaning(), "cheap slow transfer");
        assert_eq!(
            TransferKind::HighEnergy.meaning(),
            "more fuel and less coast"
        );
        assert_eq!(
            TransferKind::Assist.meaning(),
            "discount when a body sits between the two radii"
        );
    }

    fn ready(now: u64) -> GameState {
        let mut state = GameState::with_seed(now, BalanceConfig::default(), 1);
        state.fleet.add(ShipKind::SmallCargo, 2);
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 50_000.0;
        state.resources.water_ice = 50_000.0;
        state
    }

    fn neighbor() -> Coord {
        Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        }
    }

    fn colonize_neighbor(state: &mut GameState, now: u64) {
        state.colonies.push(crate::Colony {
            coord: neighbor(),
            kind: crate::PlanetType::Rocky,
            size: 8,
            rare: None,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            build_queue: Vec::new(),
            ship_queue: Vec::new(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            goods: std::collections::BTreeMap::new(),
        });
        let _ = now;
    }

    #[test]
    fn a_star_map_send_pays_the_fuel_a_vehicle_plan_shows() {
        let mut state = ready(1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let cargo = Resources::from_parts(5_000.0, 0.0, 0.0);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let kg = state.config.orbits.kg_per_cargo;
        for (speed, transfer) in [(10, TransferKind::Hohmann), (100, TransferKind::HighEnergy)] {
            let request = PlanRequest {
                vehicle_id: id,
                dest: neighbor(),
                transfer,
                customer: Customer::Colony,
                payload: Payload { cargo },
                window_index: None,
            };
            let plan = state.propose_mission(&request, 1_000).unwrap();
            let one_way = state
                .flight_hauling(Coord::HOME, neighbor(), ships, Haul::one_way(cargo), speed)
                .unwrap()
                .1;
            assert!(
                (one_way.water_ice - plan.fuel_mass / kg).abs() < 1e-6,
                "{speed}%"
            );
        }
        // A transport also pays its empty flight home at launch.
        let one_way = |haul| {
            state
                .flight_hauling(Coord::HOME, neighbor(), ships, haul, 10)
                .unwrap()
                .1
                .water_ice
        };
        let back = one_way(Haul::one_way(Resources::ZERO));
        let round = one_way(Haul::round_trip(cargo, Resources::ZERO));
        assert!((round - one_way(Haul::one_way(cargo)) - back).abs() < 1e-6);
    }

    /// The accepted playability targets on the bundled config, at the
    /// cheapest (Hohmann) send speed. See `docs/balance.md`.
    #[test]
    fn bundled_fuel_meets_the_playability_targets() {
        let mut state = GameState::with_seed(1_000, BalanceConfig::default(), 1);
        state.tech_levels.insert("combustion_drive".into(), 2);
        state.tech_levels.insert("impulse_drive".into(), 1);
        let hop = Coord {
            position: Coord::HOME.position + 1,
            ..Coord::HOME
        };
        let next_system = Coord {
            system: Coord::HOME.system + 1,
            ..Coord::HOME
        };
        let one = |kind| {
            let mut fleet = Fleet::default();
            fleet.add(kind, 1);
            fleet
        };
        let fuel = |state: &GameState, kind, dest, cargo| {
            state
                .flight_hauling(Coord::HOME, dest, one(kind), Haul::one_way(cargo), 10)
                .unwrap()
                .1
        };
        let share = |state: &GameState, kind| state.fleet_deuterium_share(one(kind));
        let full = Resources::from_parts(state.ship_stats(ShipKind::SmallCargo).cargo, 0.0, 0.0);
        // T2: a full Small Cargo hop fits base deuterium storage as water ice.
        let t2 = fuel(&state, ShipKind::SmallCargo, hop, full);
        assert!(t2.water_ice <= 10_000.0);
        assert!(t2.deuterium < 1e-9);
        // T3: a Colony Ship reaches the next system on one storage upgrade of ice.
        let t3 = fuel(&state, ShipKind::ColonyShip, next_system, Resources::ZERO);
        assert!(t3.water_ice <= 20_000.0);
        assert!((t3.deuterium - t3.water_ice * share(&state, ShipKind::ColonyShip)).abs() < 1e-6);
        // T4: an Espionage Probe can reach the next system.
        let t4 = fuel(
            &state,
            ShipKind::EspionageProbe,
            next_system,
            Resources::ZERO,
        );
        assert!(t4.water_ice <= 1_000.0);
        assert!(
            (t4.deuterium - t4.water_ice * share(&state, ShipKind::EspionageProbe)).abs() < 1e-6
        );
        // T1: from a world with an elevator, an empty hop costs at most 48 h
        // of the water ice a rocky home earns with a level-8 synthesizer, and
        // no deuterium.
        state.buildings.space_elevator = 1;
        let t1 = fuel(&state, ShipKind::SmallCargo, hop, Resources::ZERO);
        assert!(
            t1.water_ice
                <= 48.0 * level_8_per_hour(crate::PlanetType::Rocky, ResourceKind::WaterIce)
        );
        assert!(t1.deuterium < 1e-9);
    }

    /// Net `res` per hour at a powered home of `kind` with a level-8
    /// deuterium synthesizer, after the synthesizer's recipes take their share.
    fn level_8_per_hour(kind: crate::PlanetType, res: ResourceKind) -> f64 {
        let mut state = GameState::with_seed(1_000, BalanceConfig::default(), 1);
        state.home_kind = kind;
        state.buildings.deuterium_synthesizer = 8;
        state.buildings.solar_plant = 20;
        state.production_per_hour(res)
    }

    /// Launches pay reaction mass in water ice, so no world type may refine
    /// all the ice it extracts into deuterium (#658).
    #[test]
    fn every_world_type_keeps_spare_water_ice_for_launches() {
        for kind in [
            crate::PlanetType::Rocky,
            crate::PlanetType::Ice,
            crate::PlanetType::GasGiant,
        ] {
            assert!(
                level_8_per_hour(kind, ResourceKind::WaterIce) > 1.0,
                "{kind:?} nets no water ice"
            );
        }
    }

    /// A gas giant's own water ice pays a Large Cargo helium-3 route home
    /// (out loaded, back empty, at 10%) in the time the route fills a load
    /// (#667). Production rates share one power factor, so the ratio holds at
    /// any level or orbit.
    #[test]
    fn gas_giant_ice_fuels_its_helium_3_route_home() {
        let state = ready(1_000);
        let giant = Coord {
            galaxy: 1,
            system: 1,
            position: 13,
        };
        assert_eq!(state.planet_kind(giant), None);
        assert_eq!(
            crate::planet_profile(state.galaxy_seed, giant, state.generator_version).0,
            crate::PlanetType::GasGiant
        );
        let mut ships = Fleet::default();
        ships.add(ShipKind::LargeCargo, 1);
        let load = state.ship_stats(ShipKind::LargeCargo).cargo;
        let mut cargo = Resources::ZERO;
        cargo.set(ResourceKind::Helium3, load);
        let (_, fuel) = state
            .flight_hauling(giant, Coord::HOME, ships, Haul::transport(cargo), 10)
            .unwrap();
        let helium = level_8_per_hour(crate::PlanetType::GasGiant, ResourceKind::Helium3);
        let ice = level_8_per_hour(crate::PlanetType::GasGiant, ResourceKind::WaterIce);
        let fill_hours = load / helium;
        assert!(
            ice * fill_hours >= fuel.water_ice,
            "{:.0} ice made while filling, {:.0} burned",
            ice * fill_hours,
            fuel.water_ice
        );
    }

    /// The rocky home's synthesizer is short of helium-3, so a stock of it
    /// multiplies home deuterium. That is what makes the gas giant export
    /// worth hauling (#667).
    #[test]
    fn helium_3_stock_multiplies_home_deuterium() {
        let mut state = GameState::with_seed(1_000, BalanceConfig::default(), 1);
        state.buildings.deuterium_synthesizer = 8;
        state.buildings.solar_plant = 20;
        let without = state.production_per_hour(ResourceKind::Deuterium);
        state.resources.helium_3 = 1e6;
        let with = state.production_per_hour(ResourceKind::Deuterium);
        assert!(with >= 3.0 * without, "{without:.1} -> {with:.1} D/h");
    }

    #[test]
    fn combustion_launch_debits_water_ice_and_no_deuterium() {
        let mut state = ready(1_000);
        state.resources.water_ice = 1e9;
        state.resources.deuterium = 1e9;
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let dest = neighbor();
        let want = state.flight_from(state.home, dest, ships, 10).unwrap().1;
        assert!(want.water_ice > 0.0);
        assert!(want.deuterium < 1e-9);
        let before = state.resources;
        state
            .launch_fleet(state.home, ships, dest, 10, 0, 1_000)
            .unwrap();
        assert!((before.water_ice - state.resources.water_ice - want.water_ice).abs() < 1e-6);
        assert!((before.deuterium - state.resources.deuterium).abs() < 1e-6);
        assert!((state.missions[0].fuel_water_ice - want.water_ice).abs() < 1e-6);
        assert!(state.missions[0].fuel_paid < 1e-9);
    }

    #[test]
    fn impulse_launch_debits_water_ice_plus_deuterium_share() {
        let mut state = ready(1_000);
        state.tech_levels.insert("impulse_drive".into(), 1);
        state.resources.water_ice = 1e9;
        state.resources.deuterium = 1e9;
        state.fleet.add(ShipKind::LargeCargo, 1);
        let mut ships = Fleet::default();
        ships.add(ShipKind::LargeCargo, 1);
        let dest = neighbor();
        let want = state.flight_from(state.home, dest, ships, 10).unwrap().1;
        assert!(want.water_ice > 0.0);
        assert!(want.deuterium > 0.0);
        assert!(
            (want.deuterium - want.water_ice * state.fleet_deuterium_share(ships)).abs() < 1e-6
        );
        let before = state.resources;
        state
            .launch_fleet(state.home, ships, dest, 10, 0, 1_000)
            .unwrap();
        assert!((before.water_ice - state.resources.water_ice - want.water_ice).abs() < 1e-6);
        assert!((before.deuterium - state.resources.deuterium - want.deuterium).abs() < 1e-6);
        assert!((state.missions[0].fuel_water_ice - want.water_ice).abs() < 1e-6);
        assert!((state.missions[0].fuel_paid - want.deuterium).abs() < 1e-6);
    }

    #[test]
    fn launch_refusal_names_the_short_resource() {
        let mut state = ready(1_000);
        state.tech_levels.insert("impulse_drive".into(), 1);
        state.fleet.add(ShipKind::LargeCargo, 1);
        let mut ships = Fleet::default();
        ships.add(ShipKind::LargeCargo, 1);
        let dest = neighbor();
        let want = state.flight_from(state.home, dest, ships, 10).unwrap().1;
        state.resources.water_ice = 0.0;
        state.resources.deuterium = 1e9;
        let err = state
            .launch_fleet(state.home, ships, dest, 10, 0, 1_000)
            .unwrap_err();
        match err {
            crate::FleetError::InsufficientFuel {
                kind: ResourceKind::WaterIce,
                cost,
                ..
            } => assert!((cost - want.water_ice).abs() < 1e-6),
            other => panic!("{other}"),
        }
        state.resources.water_ice = 1e9;
        state.resources.deuterium = 0.0;
        let err = state
            .launch_fleet(state.home, ships, dest, 10, 0, 1_000)
            .unwrap_err();
        let text = err.to_string();
        match err {
            crate::FleetError::InsufficientFuel {
                kind: ResourceKind::Deuterium,
                cost,
                ..
            } => assert!((cost - want.deuterium).abs() < 1e-6),
            other => panic!("{other}"),
        }
        assert!(text.contains("deuterium"), "{text}");
    }

    #[test]
    fn a_space_elevator_lifts_its_world_out_of_the_surface_well() {
        let mut state = ready(1_000);
        colonize_neighbor(&mut state, 1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let request = PlanRequest {
            vehicle_id: id,
            dest: neighbor(),
            transfer: TransferKind::Hohmann,
            customer: Customer::Colony,
            payload: Payload {
                cargo: Resources::ZERO,
            },
            window_index: None,
        };
        let before = state.propose_mission(&request, 1_000).unwrap();
        state.buildings.space_elevator = 1;
        let origin = state.propose_mission(&request, 1_000).unwrap();
        let home_well = state.config.orbits.well_dv(state.home_kind);
        assert!((before.delta_v - origin.delta_v - home_well).abs() < 1e-6);
        assert!(origin.fuel_mass < before.fuel_mass);
        state.colonies[0].buildings.space_elevator = 1;
        let both = state.propose_mission(&request, 1_000).unwrap();
        let dest_well = state.config.orbits.well_dv(crate::PlanetType::Rocky);
        assert!((origin.delta_v - both.delta_v - dest_well).abs() < 1e-6);
    }

    #[test]
    fn a_space_elevator_needs_an_orbital_station_and_a_solid_surface() {
        let mut state = ready(1_000);
        state.buildings.shipyard = 4;
        let drawn = state.energy_consumed();
        assert!(matches!(
            state.enqueue_build(crate::BuildingKind::SpaceElevator, 1_000),
            Err(crate::BuildError::MissingDefence {
                defence: crate::DefenceKind::OrbitalStation,
                required: 1,
                actual: 0,
            })
        ));
        state.defences.add(crate::DefenceKind::OrbitalStation, 1);
        state
            .enqueue_build(crate::BuildingKind::SpaceElevator, 1_000)
            .unwrap();
        assert!(matches!(
            state.enqueue_build(crate::BuildingKind::SpaceElevator, 1_000),
            Err(crate::BuildError::MaxLevel { cap: 1, .. })
        ));
        state.buildings.space_elevator = 1;
        state.build_queue.clear();
        let built = state
            .describe(
                crate::DetailSubject::Building {
                    kind: crate::BuildingKind::SpaceElevator,
                    planet: Coord::HOME,
                },
                None,
            )
            .unwrap();
        assert!(built.complete, "a built elevator has no next level");
        let draw = state.energy_consumed() - drawn;
        assert!((draw - state.config.orbits.elevator_energy).abs() < 1e-9);

        colonize_neighbor(&mut state, 1_000);
        let giant = &mut state.colonies[0];
        giant.kind = crate::PlanetType::GasGiant;
        giant.buildings.shipyard = 4;
        giant.defences.add(crate::DefenceKind::OrbitalStation, 1);
        giant.resources = state.resources;
        let err = state
            .enqueue_build_on(neighbor(), crate::BuildingKind::SpaceElevator, 1_000)
            .unwrap_err();
        assert!(
            matches!(err, crate::BuildError::WrongPlanetType { .. }),
            "{err}"
        );
    }

    #[test]
    fn a_station_jump_gate_needs_an_orbital_station_and_hyperspace() {
        let mut state = ready(1_000);
        state.buildings.shipyard = 4;
        let drawn = state.energy_consumed();
        assert!(matches!(
            state.enqueue_build(crate::BuildingKind::JumpGate, 1_000),
            Err(crate::BuildError::MissingTechnology {
                ref id,
                required: 5,
                actual: 0,
            }) if id == "hyperspace"
        ));
        state.tech_levels.insert("hyperspace".into(), 5);
        assert!(matches!(
            state.enqueue_build(crate::BuildingKind::JumpGate, 1_000),
            Err(crate::BuildError::MissingDefence {
                defence: crate::DefenceKind::OrbitalStation,
                required: 1,
                actual: 0,
            })
        ));
        state.defences.add(crate::DefenceKind::OrbitalStation, 1);
        state
            .enqueue_build(crate::BuildingKind::JumpGate, 1_000)
            .unwrap();
        assert!(matches!(
            state.enqueue_build(crate::BuildingKind::JumpGate, 1_000),
            Err(crate::BuildError::MaxLevel { cap: 1, .. })
        ));
        state.buildings.jump_gate = 1;
        state.build_queue.clear();
        let draw = state.energy_consumed() - drawn;
        assert!((draw - state.config.orbits.gate_energy).abs() < 1e-9);

        colonize_neighbor(&mut state, 1_000);
        let giant = &mut state.colonies[0];
        giant.kind = crate::PlanetType::GasGiant;
        giant.buildings.shipyard = 4;
        giant.defences.add(crate::DefenceKind::OrbitalStation, 1);
        giant.resources = state.resources;
        state
            .enqueue_build_on(neighbor(), crate::BuildingKind::JumpGate, 1_000)
            .unwrap();
    }

    #[test]
    fn hohmann_beats_high_energy_on_fuel_high_energy_is_faster() {
        let state = ready(1_000);
        let origin = Coord::HOME;
        let dest = neighbor();
        let h = state
            .mission_windows(origin, dest, TransferKind::Hohmann, 1_000)
            .unwrap();
        let e = state
            .mission_windows(origin, dest, TransferKind::HighEnergy, 1_000)
            .unwrap();
        assert!(e.delta_v > h.delta_v);
        assert!(e.coast_secs < h.coast_secs);
        assert_ne!(
            e.windows.first().map(|w| (w.opens_at, w.closes_at)),
            h.windows.first().map(|w| (w.opens_at, w.closes_at)),
            "high-energy coast must shift the departure phase"
        );
    }

    #[test]
    fn plan_mission_books_an_upcoming_window() {
        let mut state = ready(1_000);
        colonize_neighbor(&mut state, 1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let dest = neighbor();
        let estimate = state
            .mission_windows(Coord::HOME, dest, TransferKind::Hohmann, 1_000)
            .unwrap();
        let now = state.last_tick_unix;
        let upcoming = estimate
            .windows
            .iter()
            .position(|w| w.opens_at > now)
            .expect("synodic list includes a later window");
        let opens_at = estimate.windows[upcoming].opens_at;
        let mission_id = state
            .plan_mission(
                PlanRequest {
                    vehicle_id: id,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Colony,
                    payload: Payload {
                        cargo: Resources {
                            iron: 10.0,
                            ..Resources::ZERO
                        },
                    },
                    window_index: Some(upcoming),
                },
                now,
            )
            .unwrap();
        let mission = state
            .planned
            .iter()
            .find(|m| m.id == mission_id)
            .unwrap()
            .clone();
        assert_eq!(mission.launched_at, opens_at);
        assert!(mission.arrives_at > mission.launched_at);
        assert_eq!(state.vehicles[0].coord, Coord::HOME);
        state.catch_up(mission.launched_at.saturating_sub(1));
        assert_eq!(state.vehicles[0].coord, Coord::HOME);
        assert_eq!(state.planned.len(), 1);
        state.catch_up(mission.arrives_at);
        assert_eq!(state.vehicles[0].coord, dest);
        assert!(state.planned.is_empty());
    }

    #[test]
    fn assist_needs_a_body_between_orbits() {
        let state = ready(1_000);
        let adjacent = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let err = state
            .mission_windows(Coord::HOME, adjacent, TransferKind::Assist, 1_000)
            .unwrap_err();
        assert!(matches!(err, PlanError::NoAssistBody), "{err:?}");
        let far = state
            .mission_windows(Coord::HOME, neighbor(), TransferKind::Assist, 1_000)
            .unwrap();
        let hohmann = state
            .mission_windows(Coord::HOME, neighbor(), TransferKind::Hohmann, 1_000)
            .unwrap();
        assert!(far.delta_v < hohmann.delta_v);
    }

    #[test]
    fn plan_mission_refuses_double_book_and_missed_window() {
        let mut state = ready(1_000);
        colonize_neighbor(&mut state, 1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let dest = neighbor();
        let windows = state
            .mission_windows(Coord::HOME, dest, TransferKind::Hohmann, 1_000)
            .unwrap();
        let open = windows
            .windows
            .iter()
            .position(|w| w.contains(state.last_tick_unix))
            .unwrap_or(0);
        if !windows.windows[open].contains(state.last_tick_unix) {
            state.last_tick_unix = windows.windows[open].opens_at;
        }
        let payload = Payload {
            cargo: Resources::from_parts(10.0, 0.0, 0.0),
        };
        let req = PlanRequest {
            vehicle_id: id,
            dest,
            transfer: TransferKind::Hohmann,
            customer: Customer::Colony,
            payload: payload.clone(),
            window_index: Some(open),
        };
        state
            .plan_mission(req.clone(), state.last_tick_unix)
            .unwrap();
        let err = state.plan_mission(req, state.last_tick_unix).unwrap_err();
        assert!(
            matches!(
                err,
                PlanError::DoubleBook { .. } | PlanError::VehicleBusy { .. }
            ),
            "{err:?}"
        );
        let mut other = ready(50);
        colonize_neighbor(&mut other, 50);
        other.fleet.add(ShipKind::SmallCargo, 1);
        let vid = other.register_vehicle(Coord::HOME, ships, 50).unwrap();
        let mut plan = other
            .propose_mission(
                &PlanRequest {
                    vehicle_id: vid,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Colony,
                    payload,
                    window_index: Some(0),
                },
                50,
            )
            .unwrap();
        plan.window.closes_at = 1;
        plan.window.opens_at = 0;
        let missed = other.check_mission(&plan, 50).unwrap_err();
        assert!(
            matches!(missed, PlanError::MissedWindow { .. }),
            "{missed:?}"
        );
    }

    #[test]
    fn payload_mass_raises_fuel_and_delivery_credits_the_colony() {
        let mut light = ready(1_000);
        let mut heavy = ready(1_000);
        colonize_neighbor(&mut light, 1_000);
        colonize_neighbor(&mut heavy, 1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 2);
        let dest = neighbor();
        let a = light.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let b = heavy.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let windows = light
            .mission_windows(Coord::HOME, dest, TransferKind::Hohmann, 1_000)
            .unwrap();
        let open = windows
            .windows
            .iter()
            .position(|w| w.contains(light.last_tick_unix) || w.opens_at >= light.last_tick_unix)
            .unwrap();
        if !windows.windows[open].contains(light.last_tick_unix) {
            light.last_tick_unix = windows.windows[open].opens_at;
            heavy.last_tick_unix = windows.windows[open].opens_at;
        }
        let light_plan = light
            .propose_mission(
                &PlanRequest {
                    vehicle_id: a,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Colony,
                    payload: Payload {
                        cargo: Resources {
                            iron: 10.0,
                            ..Resources::ZERO
                        },
                    },
                    window_index: Some(open),
                },
                light.last_tick_unix,
            )
            .unwrap();
        let heavy_plan = heavy
            .propose_mission(
                &PlanRequest {
                    vehicle_id: b,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Colony,
                    payload: Payload {
                        cargo: Resources {
                            iron: 800.0,
                            ..Resources::ZERO
                        },
                    },
                    window_index: Some(open),
                },
                heavy.last_tick_unix,
            )
            .unwrap();
        assert!(
            heavy_plan.fuel_mass > light_plan.fuel_mass,
            "heavy {} light {}",
            heavy_plan.fuel_mass,
            light_plan.fuel_mass
        );
        light.apply_mission(light_plan).unwrap();
        let arrives = light.planned[0].arrives_at;
        light.catch_up(arrives);
        let delivered = light
            .colonies
            .iter()
            .find(|c| c.coord == dest)
            .unwrap()
            .resources
            .iron;
        assert!((delivered - 10.0).abs() < 1e-6, "{delivered}");
        assert_eq!(light.vehicles[0].coord, dest);
        assert!(light.planned.is_empty());
        let _ = b;
        let _ = heavy_plan;
        let cfg = &light.config.orbits;
        let r1 = heliocentric_radius_m(Coord::HOME, cfg);
        let r2 = heliocentric_radius_m(dest, cfg);
        let Hohmann { .. } = hohmann_transfer(r1, r2, cfg.mu_star);
    }

    #[test]
    fn vehicle_fuel_between_systems_never_falls_with_distance() {
        let mut state = ready(1_000);
        state.fleet.add(ShipKind::LargeCargo, 1);
        let mut ships = Fleet::default();
        ships.add(ShipKind::LargeCargo, 1);
        let vid = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let at = |galaxy, system, position| Coord {
            galaxy,
            system,
            position,
        };
        let plan = |state: &GameState, dest: Coord, iron: f64| {
            let mut cargo = Resources::ZERO;
            cargo.iron = iron;
            state
                .propose_mission(
                    &PlanRequest {
                        vehicle_id: vid,
                        dest,
                        transfer: TransferKind::Hohmann,
                        customer: Customer::Colony,
                        payload: Payload { cargo },
                        window_index: Some(0),
                    },
                    1_000,
                )
                .unwrap()
        };
        // Farther each step: home system, then systems 2 and 9, then galaxy 2.
        // Gravity wells depend on each planet's type, so compare the
        // transfer delta-v without them.
        let transfer_dv: Vec<f64> = [
            at(1, 1, 2),
            at(1, 1, 16),
            at(1, 2, 1),
            at(1, 9, 1),
            at(2, 1, 1),
        ]
        .into_iter()
        .map(|dest| {
            plan(&state, dest, 0.0).delta_v - state.well_cost(Coord::HOME) - state.well_cost(dest)
        })
        .collect();
        assert!(
            transfer_dv.windows(2).all(|w| w[1] >= w[0] - 1e-6),
            "{transfer_dv:?}"
        );
        let neighbour_system = at(1, 2, 1);
        let empty = plan(&state, neighbour_system, 0.0).fuel_mass;
        let base_storage = state.config.deuterium_storage.capacity(0);
        assert!(empty <= base_storage, "{empty} > {base_storage}");
        let loaded = plan(&state, neighbour_system, 5_000.0).fuel_mass;
        assert!(loaded > empty, "{loaded} <= {empty}");
    }

    #[test]
    fn drive_research_cuts_mission_fuel_up_to_the_next_drive() {
        let mut state = ready(1_000);
        state.fleet.add(ShipKind::SmallCargo, 1);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let vid = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let drive = state.config.ships.get(ShipKind::SmallCargo).drive.clone();
        let fuel = |state: &GameState| {
            let mut cargo = Resources::ZERO;
            cargo.iron = 5_000.0;
            state
                .propose_mission(
                    &PlanRequest {
                        vehicle_id: vid,
                        dest: neighbor(),
                        transfer: TransferKind::Hohmann,
                        customer: Customer::Colony,
                        payload: Payload { cargo },
                        window_index: Some(0),
                    },
                    1_000,
                )
                .unwrap()
                .fuel_mass
        };
        let base_ve = state.drive_ve(&drive);
        let base_fuel = fuel(&state);
        state.tech_levels.insert(drive.clone(), 4);
        assert!(state.drive_ve(&drive) > base_ve);
        assert!(fuel(&state) < base_fuel, "{} >= {base_fuel}", fuel(&state));
        // Research never lifts a drive past the next drive's base value.
        state.tech_levels.insert(drive.clone(), 10_000);
        let cap = state.config.orbits.ve_cap_for_drive(&drive).unwrap();
        assert_eq!(state.drive_ve(&drive), cap);
    }

    #[test]
    fn a_small_cargo_refit_pays_impulse_fuel_for_the_same_mass() {
        let mut state = GameState::with_seed(1_000, BalanceConfig::default(), 1);
        state.tech_levels.insert("combustion_drive".into(), 2);
        state.tech_levels.insert("impulse_drive".into(), 4);
        let dest = Coord {
            galaxy: 1,
            system: 2,
            position: 1,
        };
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let mut cargo = Resources::ZERO;
        cargo.water_ice = 5_000.0;
        let haul = Haul::one_way(cargo);
        assert_eq!(state.hull_drive(ShipKind::SmallCargo), "combustion_drive");
        let before = state.send_fuel(Coord::HOME, dest, ships, haul, 10);
        let before_speed = state.ship_stats(ShipKind::SmallCargo).speed;
        state.tech_levels.insert("small_cargo_impulse".into(), 1);
        assert_eq!(state.hull_drive(ShipKind::SmallCargo), "impulse_drive");
        let after = state.send_fuel(Coord::HOME, dest, ships, haul, 10);
        let cfg = &state.config.orbits;
        let delta_v = state.base_burn(Coord::HOME, dest).delta_v
            + state.well_cost(Coord::HOME)
            + state.well_cost(dest);
        let dry = state.fleet_dry_kg(ships) + cargo.total() * cfg.kg_per_cargo;
        let expected =
            crate::orbits::fuel_mass_for_dv(state.drive_ve("impulse_drive"), dry, delta_v)
                / cfg.kg_per_cargo;
        assert!(
            (after - expected).abs() < 1e-6,
            "after {after} expected {expected}"
        );
        assert!(after < before / 10.0, "{after} is not far below {before}");
        assert!(state.ship_stats(ShipKind::SmallCargo).speed > before_speed);
        let detail = state
            .describe(
                crate::DetailSubject::Ship {
                    kind: ShipKind::SmallCargo,
                    planet: Coord::HOME,
                },
                None,
            )
            .unwrap();
        let travel = detail.ship.unwrap();
        assert_eq!(travel.exhaust_velocity, state.drive_ve("impulse_drive"));
        assert_eq!(travel.drive, state.display_name("impulse_drive"));
    }

    #[test]
    fn a_mission_in_flight_keeps_the_fuel_it_booked_after_a_refit() {
        let mut state = ready(1_000);
        state.resources.water_ice = 1e9;
        state.resources.deuterium = 1e9;
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let dest = Coord {
            galaxy: 1,
            system: 2,
            position: 1,
        };
        state
            .launch_fleet(state.home, ships, dest, 10, 0, 1_000)
            .unwrap();
        let ice = state.missions[0].fuel_water_ice;
        let deut = state.missions[0].fuel_paid;
        let arrives = state.missions[0].phase_ends_at;
        state.tech_levels.insert("small_cargo_impulse".into(), 1);
        assert_eq!(state.missions[0].fuel_water_ice, ice);
        assert_eq!(state.missions[0].fuel_paid, deut);
        assert_eq!(state.missions[0].phase_ends_at, arrives);
        let cheaper = state.send_fuel(state.home, dest, ships, Haul::empty_round_trip(), 10);
        assert!(cheaper < ice, "{cheaper} >= {ice}");
    }

    #[test]
    fn schedule_flags_overlapping_vehicle_bookings() {
        let mut state = ready(1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let dest = neighbor();
        let window = LaunchWindow {
            opens_at: 1_000,
            closes_at: 2_000,
        };
        state.planned.push(PlannedMission {
            id: 1,
            vehicle_id: id,
            route: Route {
                origin: Coord::HOME,
                dest,
            },
            transfer: TransferKind::Hohmann,
            customer: Customer::Colony,
            window,
            launched_at: 1_000,
            arrives_at: 2_000,
            delta_v: 1.0,
            fuel_mass: 1.0,
            payload: Payload {
                cargo: Resources::ZERO,
            },
        });
        state.planned.push(PlannedMission {
            id: 2,
            vehicle_id: id,
            route: Route {
                origin: Coord::HOME,
                dest,
            },
            transfer: TransferKind::Hohmann,
            customer: Customer::Colony,
            window: LaunchWindow {
                opens_at: 0,
                closes_at: 1,
            },
            launched_at: 50,
            arrives_at: 90,
            delta_v: 1.0,
            fuel_mass: 1.0,
            payload: Payload {
                cargo: Resources::ZERO,
            },
        });
        let schedule = state.mission_schedule(1_000);
        assert!(
            schedule
                .alerts
                .iter()
                .any(|a| a.kind == ScheduleAlertKind::DoubleBook)
        );
        assert!(
            schedule
                .alerts
                .iter()
                .any(|a| a.kind == ScheduleAlertKind::MissedWindow)
        );
    }

    #[test]
    fn market_customer_is_refused_when_the_market_is_off() {
        let mut state = ready(1_000);
        state.config.market.enabled = false;
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let dest = neighbor();
        let estimate = state
            .mission_windows(Coord::HOME, dest, TransferKind::Hohmann, 1_000)
            .unwrap();
        let now = estimate.windows[0].opens_at;
        state.last_tick_unix = now;
        let plan = state
            .propose_mission(
                &PlanRequest {
                    vehicle_id: id,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Market,
                    payload: Payload {
                        cargo: Resources::ZERO,
                    },
                    window_index: Some(0),
                },
                now,
            )
            .unwrap();
        let err = state.check_mission(&plan, now).unwrap_err();
        assert!(matches!(err, PlanError::MarketContracts), "{err:?}");
    }

    #[test]
    fn market_customer_is_accepted_when_the_market_is_on() {
        let mut state = ready(1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let dest = neighbor();
        let estimate = state
            .mission_windows(Coord::HOME, dest, TransferKind::Hohmann, 1_000)
            .unwrap();
        let now = estimate.windows[0].opens_at;
        state.last_tick_unix = now;
        let plan = state
            .propose_mission(
                &PlanRequest {
                    vehicle_id: id,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Market,
                    payload: Payload {
                        cargo: Resources::ZERO,
                    },
                    window_index: Some(0),
                },
                now,
            )
            .unwrap();
        state.check_mission(&plan, now).unwrap();
    }

    #[test]
    fn apply_mission_ignores_tampered_fuel_mass() {
        let mut state = ready(1_000);
        colonize_neighbor(&mut state, 1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let dest = neighbor();
        let estimate = state
            .mission_windows(Coord::HOME, dest, TransferKind::Hohmann, 1_000)
            .unwrap();
        let now = estimate.windows[0].opens_at;
        state.last_tick_unix = now;
        let ice = state.resources.water_ice;
        let mut plan = state
            .propose_mission(
                &PlanRequest {
                    vehicle_id: id,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Colony,
                    payload: Payload {
                        cargo: Resources::ZERO,
                    },
                    window_index: Some(0),
                },
                now,
            )
            .unwrap();
        let expected_fuel = plan.fuel_mass;
        plan.fuel_mass = -1_000.0;
        plan.delta_v = 0.0;
        plan.available_dv = 0.0;
        plan.coast_secs = 1;
        state.apply_mission(plan).unwrap();
        let used = ice - state.resources.water_ice;
        assert!(
            (used - expected_fuel / state.config.orbits.kg_per_cargo).abs() < 1e-6,
            "used {used} expected {}",
            expected_fuel / state.config.orbits.kg_per_cargo
        );
        assert!(state.planned[0].fuel_mass > 0.0);
        assert!(state.planned[0].delta_v > 0.0);
        assert!(state.planned[0].arrives_at > now);
    }

    #[test]
    fn faction_delivery_returns_the_vehicle_home() {
        let mut state = ready(1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let id = state.register_vehicle(Coord::HOME, ships, 1_000).unwrap();
        let dest = neighbor();
        let estimate = state
            .mission_windows(Coord::HOME, dest, TransferKind::Hohmann, 1_000)
            .unwrap();
        let now = estimate.windows[0].opens_at;
        state.last_tick_unix = now;
        state
            .plan_mission(
                PlanRequest {
                    vehicle_id: id,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Faction("traders".into()),
                    payload: Payload {
                        cargo: Resources {
                            iron: 10.0,
                            ..Resources::ZERO
                        },
                    },
                    window_index: Some(0),
                },
                now,
            )
            .unwrap();
        let arrives = state.planned[0].arrives_at;
        let free_at = state.vehicles[0].free_at;
        let metal_after_launch = state.resources.iron;
        state.catch_up(arrives);
        assert_eq!(state.vehicles[0].coord, Coord::HOME);
        assert!(state.planned.is_empty());
        assert!(
            (state.resources.iron - metal_after_launch).abs() < 1e-9,
            "faction cargo is delivered, not refunded"
        );
        assert!(
            (state.diplomacy.standing("traders") - 1.0).abs() < 1e-9,
            "standing {}",
            state.diplomacy.standing("traders")
        );
        state.catch_up(free_at);
        state
            .plan_mission(
                PlanRequest {
                    vehicle_id: id,
                    dest,
                    transfer: TransferKind::Hohmann,
                    customer: Customer::Faction("traders".into()),
                    payload: Payload {
                        cargo: Resources::ZERO,
                    },
                    window_index: None,
                },
                state.last_tick_unix,
            )
            .expect("vehicle can fly again from home");
    }

    /// Home at 1:1:1 and an ice colony at 1:1:9, each with one Small Cargo.
    fn two_bases() -> (GameState, Coord, Fleet) {
        let mut state = GameState::new(0);
        state.fleet.add(ShipKind::SmallCargo, 1);
        let ice = Coord {
            galaxy: 1,
            system: 1,
            position: 9,
        };
        let mut fleet = Fleet::default();
        fleet.add(ShipKind::SmallCargo, 1);
        state.colonies.push(crate::Colony {
            coord: ice,
            kind: crate::PlanetType::Ice,
            size: 8,
            rare: None,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet,
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            ship_queue: Vec::new(),
            goods: Default::default(),
        });
        (state, ice, fleet)
    }

    #[test]
    fn an_outer_send_from_home_names_the_ice_colony_as_cheaper() {
        let (state, ice, ships) = two_bases();
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 16,
        };
        let haul = crate::Haul::empty_round_trip();
        let hint = state
            .cheaper_origin_hint(Coord::HOME, dest, ships, haul, 100)
            .expect("the ice colony pays less");
        assert!(
            hint.starts_with(&format!("cheaper from {}: ", state.world_label(ice))),
            "{hint}"
        );
        assert!(hint.ends_with("% less"), "{hint}");
        // From the colony itself home is no cheaper.
        assert_eq!(state.cheaper_origin_hint(ice, dest, ships, haul, 100), None);
    }

    #[test]
    fn no_cheaper_origin_without_the_ships_parked_there() {
        let (mut state, _, ships) = two_bases();
        state.colonies[0].fleet = Fleet::default();
        let haul = crate::Haul::empty_round_trip();
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 16,
        };
        assert_eq!(
            state.cheaper_origin_hint(Coord::HOME, dest, ships, haul, 100),
            None
        );
    }

    fn haul_ready(dest: Coord) -> (GameState, Fleet, Resources) {
        let mut state = GameState::new(0);
        state.buildings.shipyard = 4;
        state.tech_levels.insert("combustion_drive".into(), 2);
        state.tech_levels.insert("impulse_drive".into(), 4);
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.water_ice = 5_000.0;
        state.resources.deuterium = 0.0;
        state.colonies.push(crate::Colony {
            coord: dest,
            kind: crate::PlanetType::Rocky,
            size: 8,
            rare: None,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            ship_queue: Vec::new(),
            goods: Default::default(),
        });
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let mut cargo = Resources::ZERO;
        cargo.water_ice = 5_000.0;
        (state, ships, cargo)
    }

    #[test]
    fn an_interstellar_combustion_send_names_the_drive_and_a_faster_hull() {
        let dest = Coord {
            galaxy: 1,
            system: 2,
            position: 1,
        };
        let (mut state, ships, cargo) = haul_ready(dest);
        let hint = state
            .expensive_send_hint(state.home, dest, ships, cargo.total(), 0.0)
            .expect("combustion between systems");
        assert!(hint.contains("Combustion Drive"), "{hint}");
        assert!(
            hint.contains("research Small Cargo Impulse Refit"),
            "{hint}"
        );
        assert!(hint.contains("Large Cargo"), "{hint}");
        assert!(hint.contains("Impulse Drive"), "{hint}");
        let err = state
            .launch_transport(state.home, dest, ships, cargo, 100, 0)
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("not enough water ice"), "{text}");
        assert!(text.contains(&hint), "{text}");
    }

    #[test]
    fn an_in_system_send_names_a_refit_only_when_one_moves_its_hull() {
        let near = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        // No refit moves a Light Fighter, so an in-system strike stays quiet.
        let (state, _, _) = haul_ready(near);
        let mut fighters = Fleet::default();
        fighters.add(ShipKind::LightFighter, 1);
        assert_eq!(
            state.expensive_send_hint(state.home, near, fighters, 0.0, 1_000.0),
            None
        );

        // #668: an outer-slot haul burns far more ice than it carries.
        let far = Coord {
            galaxy: 1,
            system: 1,
            position: 9,
        };
        let (mut state, ships, _) = haul_ready(far);
        state.tech_levels.insert("impulse_drive".into(), 2);
        state.buildings.research_lab = 2;
        let mut cargo = Resources::ZERO;
        cargo.water_ice = 300.0;
        let err = state
            .launch_transport(state.home, far, ships, cargo, 10, 0)
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("not enough water ice"), "{text}");
        assert!(
            text.contains(
                "Combustion Drive makes this send expensive; research Small Cargo Impulse \
                 Refit (needs impulse drive level 4 (have 2))"
            ),
            "{text}"
        );
        assert!(!text.contains("Large Cargo"), "{text}");
    }
}
