//! Named mission planning: vehicles, payloads, windows, customers and routes.
//!
//! [`GameState::plan_mission`] is the governed action (propose, check, apply).

use crate::fleets::Coord;
use crate::orbits::{
    fuel_mass_for_dv, heliocentric_radius_m, hohmann_transfer, launch_window_times, mean_motion,
    rocket_delta_v, to_game_secs,
};
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, Resources};
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
        (self.cargo.metal + self.cargo.crystal + self.cargo.deuterium) * kg_per_cargo
    }

    pub fn volume_m3(&self, m3_per_cargo: f64) -> f64 {
        (self.cargo.metal + self.cargo.crystal + self.cargo.deuterium) * m3_per_cargo
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
                "not enough resources (missing metal {:.0}, crystal {:.0}, deuterium {:.0})",
                missing.metal, missing.crystal, missing.deuterium
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
        if !plan.payload.cargo.metal.is_finite()
            || !plan.payload.cargo.crystal.is_finite()
            || !plan.payload.cargo.deuterium.is_finite()
            || plan.payload.cargo.metal < 0.0
            || plan.payload.cargo.crystal < 0.0
            || plan.payload.cargo.deuterium < 0.0
        {
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
        let fuel_units = physics.fuel_mass / cfg.kg_per_cargo;
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
        let need = Resources {
            metal: plan.payload.cargo.metal,
            crystal: plan.payload.cargo.crystal,
            deuterium: plan.payload.cargo.deuterium + fuel_units,
        };
        let missing = Resources {
            metal: (need.metal - origin_stock.metal).max(0.0),
            crystal: (need.crystal - origin_stock.crystal).max(0.0),
            deuterium: (need.deuterium - origin_stock.deuterium).max(0.0),
        };
        if missing.metal > 1e-9 || missing.crystal > 1e-9 || missing.deuterium > 1e-9 {
            return Err(PlanError::CannotAfford { missing });
        }
        Ok(())
    }

    pub fn apply_mission(&mut self, plan: MissionPlan) -> Result<u64, PlanError> {
        let now = self.last_tick_unix;
        self.check_mission(&plan, now)?;
        let physics = self.physics_for(&plan, now)?;
        let cfg = self.config.orbits.clone();
        let fuel_units = physics.fuel_mass / cfg.kg_per_cargo;
        self.debit_at(
            plan.route.origin,
            Resources {
                metal: plan.payload.cargo.metal,
                crystal: plan.payload.cargo.crystal,
                deuterium: plan.payload.cargo.deuterium + fuel_units,
            },
        )?;
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
        let burn = hohmann_transfer(r1, r2, cfg.mu_star);
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

    fn well_cost(&self, coord: Coord) -> f64 {
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

    fn vehicle_dry_kg(&self, vehicle: &Vehicle) -> f64 {
        let cfg = &self.config.orbits;
        ShipKind::ALL
            .iter()
            .map(|&kind| {
                f64::from(vehicle.ships.get(kind))
                    * self.config.ships.get(kind).hull
                    * cfg.kg_per_hull
            })
            .sum()
    }

    fn vehicle_ve(&self, vehicle: &Vehicle) -> f64 {
        let mut ve = f64::MAX;
        for kind in ShipKind::ALL {
            if vehicle.ships.get(kind) == 0 {
                continue;
            }
            let drive = self.config.ships.get(kind).drive.as_str();
            ve = ve.min(self.config.orbits.ve_for_drive(drive));
        }
        if ve.is_finite() {
            ve
        } else {
            self.config.orbits.combustion_ve
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
        let dry = self.vehicle_dry_kg(vehicle) + payload.mass_kg(cfg.kg_per_cargo);
        let ve = self.vehicle_ve(vehicle);
        let fuel = fuel_mass_for_dv(ve, dry, need_dv);
        let available = rocket_delta_v(ve, dry + fuel, dry);
        Ok((fuel, available))
    }

    fn parked_fleet(&self, planet: Coord) -> Option<Fleet> {
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

    fn debit_at(&mut self, planet: Coord, cost: Resources) -> Result<(), PlanError> {
        let stock = self
            .resources_at_mut(planet)
            .map_err(|_| PlanError::UnknownPlanet)?;
        if stock.metal + 1e-9 < cost.metal
            || stock.crystal + 1e-9 < cost.crystal
            || stock.deuterium + 1e-9 < cost.deuterium
        {
            return Err(PlanError::CannotAfford {
                missing: Resources {
                    metal: (cost.metal - stock.metal).max(0.0),
                    crystal: (cost.crystal - stock.crystal).max(0.0),
                    deuterium: (cost.deuterium - stock.deuterium).max(0.0),
                },
            });
        }
        stock.metal = (stock.metal - cost.metal).max(0.0);
        stock.crystal = (stock.crystal - cost.crystal).max(0.0);
        stock.deuterium = (stock.deuterium - cost.deuterium).max(0.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::orbits::Hohmann;
    use crate::orbits::{heliocentric_radius_m, hohmann_transfer};

    fn ready(now: u64) -> GameState {
        let mut state = GameState::with_seed(now, BalanceConfig::default(), 1);
        state.fleet.add(ShipKind::SmallCargo, 2);
        state.resources.metal = 50_000.0;
        state.resources.crystal = 50_000.0;
        state.resources.deuterium = 50_000.0;
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
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            goods: std::collections::BTreeMap::new(),
        });
        let _ = now;
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
                            metal: 10.0,
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
            cargo: Resources {
                metal: 10.0,
                crystal: 0.0,
                deuterium: 0.0,
            },
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
                            metal: 10.0,
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
                            metal: 800.0,
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
            .metal;
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
        let deuterium = state.resources.deuterium;
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
        let used = deuterium - state.resources.deuterium;
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
                            metal: 10.0,
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
        let metal_after_launch = state.resources.metal;
        state.catch_up(arrives);
        assert_eq!(state.vehicles[0].coord, Coord::HOME);
        assert!(state.planned.is_empty());
        assert!(
            (state.resources.metal - metal_after_launch).abs() < 1e-9,
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
}
