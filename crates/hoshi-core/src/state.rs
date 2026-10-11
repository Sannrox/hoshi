//! Planet stockpiles, building levels and offline catch-up.

use crate::activity::{
    ActivityApply, ActivityChannel, ActivityPulse, ActivityTracker, resolve_channel,
};
use crate::balance::BalanceConfig;
use crate::buildings::{BuildError, BuildJob, BuildingKind, Buildings};
use crate::combat::{
    CombatReport, CombatWinner, next_u64, plunder_from, rapid_fire_table, resolve_combat,
    stats_from_catalog,
};
use crate::expedition::{ExpeditionResult, roll_expedition};
use crate::fleets::{
    Coord, FleetError, Mission, MissionPhase, TradeRoute, distance, flight_secs, fuel_cost,
};
use crate::planets::{Colony, PlanetType, planet_profile};
use crate::research::{ModifierKind, PrereqGap, ResearchError, ResearchJob};
use crate::ships::{Fleet, ShipError, ShipJob, ShipKind, ShipStats};
use std::collections::BTreeMap;

const EVENT_LOG_CAP: usize = 32;

/// A timestamped line for the TUI and save. Combat reports land here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameEvent {
    pub at: u64,
    pub text: String,
}

/// Stored resources. Energy is a balance, not a stockpile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resources {
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
}

impl Resources {
    pub const ZERO: Self = Self {
        metal: 0.0,
        crystal: 0.0,
        deuterium: 0.0,
    };

    pub fn get(self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Metal => self.metal,
            ResourceKind::Crystal => self.crystal,
            ResourceKind::Deuterium => self.deuterium,
        }
    }

    pub fn set(&mut self, kind: ResourceKind, value: f64) {
        match kind {
            ResourceKind::Metal => self.metal = value,
            ResourceKind::Crystal => self.crystal = value,
            ResourceKind::Deuterium => self.deuterium = value,
        }
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            metal: self.metal + other.metal,
            crystal: self.crystal + other.crystal,
            deuterium: self.deuterium + other.deuterium,
        }
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self {
            metal: (self.metal - other.metal).max(0.0),
            crystal: (self.crystal - other.crystal).max(0.0),
            deuterium: (self.deuterium - other.deuterium).max(0.0),
        }
    }

    pub fn missing(self, cost: Self) -> Self {
        Self {
            metal: (cost.metal - self.metal).max(0.0),
            crystal: (cost.crystal - self.crystal).max(0.0),
            deuterium: (cost.deuterium - self.deuterium).max(0.0),
        }
    }

    pub fn can_afford(self, cost: Self) -> bool {
        self.metal + 1e-9 >= cost.metal
            && self.crystal + 1e-9 >= cost.crystal
            && self.deuterium + 1e-9 >= cost.deuterium
    }
}

/// Stockpiled resource kinds. Energy is computed, not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Metal,
    Crystal,
    Deuterium,
}

impl ResourceKind {
    pub const ALL: [ResourceKind; 3] = [
        ResourceKind::Metal,
        ResourceKind::Crystal,
        ResourceKind::Deuterium,
    ];
}

/// Game state. All time advancement goes through [`GameState::catch_up`].
#[derive(Debug, Clone, PartialEq)]
pub struct GameState {
    pub resources: Resources,
    pub buildings: Buildings,
    pub build_queue: Vec<BuildJob>,
    pub tech_levels: BTreeMap<String, u32>,
    pub research: Option<ResearchJob>,
    pub fleet: Fleet,
    pub ship_queue: Vec<ShipJob>,
    pub home: Coord,
    pub home_kind: PlanetType,
    pub home_size: u8,
    pub colonies: Vec<Colony>,
    pub missions: Vec<Mission>,
    pub next_mission_id: u64,
    pub routes: Vec<TradeRoute>,
    pub next_route_id: u64,
    pub next_raid_at: Option<u64>,
    pub events: Vec<GameEvent>,
    pub activity: ActivityTracker,
    pub last_tick_unix: u64,
    /// Seeded RNG state. Combat and raids consume it; the save stores the current value.
    pub rng_seed: u64,
    /// Stable seed for planet type and size. Does not change when combat consumes `rng_seed`.
    pub galaxy_seed: u64,
    pub config: BalanceConfig,
}

impl GameState {
    pub fn new(now_unix: u64) -> Self {
        Self::with_config(now_unix, BalanceConfig::default())
    }

    pub fn with_config(now_unix: u64, config: BalanceConfig) -> Self {
        let galaxy_seed = 0;
        let (home_kind, home_size) = planet_profile(galaxy_seed, Coord::HOME);
        Self {
            resources: config.starting_resources,
            buildings: config.starting_buildings,
            build_queue: Vec::new(),
            tech_levels: BTreeMap::new(),
            research: None,
            fleet: Fleet::default(),
            ship_queue: Vec::new(),
            home: Coord::HOME,
            home_kind,
            home_size,
            colonies: Vec::new(),
            missions: Vec::new(),
            next_mission_id: 0,
            routes: Vec::new(),
            next_route_id: 0,
            next_raid_at: if config.raid.enabled {
                Some(now_unix.saturating_add(config.raid.first_delay_secs))
            } else {
                None
            },
            events: Vec::new(),
            activity: ActivityTracker::default(),
            last_tick_unix: now_unix,
            rng_seed: 0,
            galaxy_seed,
            config,
        }
    }

    /// Advance production, the build queue and research to `now_unix`.
    /// Completions that fall in the gap are applied in timestamp order and
    /// production is recomputed from each completion instant. A backward clock
    /// is a no-op.
    pub fn catch_up(&mut self, now_unix: u64) {
        if now_unix <= self.last_tick_unix {
            return;
        }
        loop {
            let home_build_at = self.build_queue.first().and_then(|job| job.completes_at);
            let colony_build_at = self
                .colonies
                .iter()
                .filter_map(|c| c.build_queue.first().and_then(|job| job.completes_at))
                .min();
            let build_at = min_time(home_build_at, colony_build_at);
            let research_at = self.research.as_ref().map(|job| job.completes_at);
            let ship_at = self.ship_queue.first().and_then(|job| job.completes_at);
            let mission_at = self.missions.iter().map(|m| m.phase_ends_at).min();
            let route_at = self.routes.iter().map(|r| r.next_at).min();
            let raid_at = if self.config.raid.enabled {
                self.next_raid_at
            } else {
                None
            };
            let prod_boost_at = self
                .activity
                .prod_boost_until
                .filter(|&t| t > self.last_tick_unix);
            let research_boost_at = self
                .activity
                .research_boost_until
                .filter(|&t| t > self.last_tick_unix);
            let Some(done_at) = min_time(
                min_time(
                    min_time(
                        min_time(
                            min_time(min_time(build_at, research_at), ship_at),
                            mission_at,
                        ),
                        raid_at,
                    ),
                    route_at,
                ),
                min_time(prod_boost_at, research_boost_at),
            ) else {
                self.produce_until(now_unix);
                return;
            };
            if done_at > now_unix {
                self.produce_until(now_unix);
                return;
            }
            self.produce_until(done_at);
            if home_build_at == Some(done_at) {
                let job = self.build_queue.remove(0);
                self.buildings.increment(job.building);
                self.start_head(done_at);
            }
            let colony_done: Vec<usize> = self
                .colonies
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.build_queue.first().and_then(|job| job.completes_at) == Some(done_at)
                })
                .map(|(i, _)| i)
                .collect();
            for i in colony_done {
                let job = self.colonies[i].build_queue.remove(0);
                self.colonies[i].buildings.increment(job.building);
                self.start_colony_head(i, done_at);
            }
            if research_at == Some(done_at)
                && let Some(job) = self.research.take()
            {
                let level = self.tech_levels.entry(job.tech).or_insert(0);
                *level = level.saturating_add(1);
            }
            if ship_at == Some(done_at) {
                self.complete_one_ship(done_at);
            }
            if mission_at == Some(done_at) {
                self.advance_missions_at(done_at);
            }
            if route_at == Some(done_at) {
                self.fire_routes_at(done_at);
            }
            if raid_at == Some(done_at) {
                self.resolve_raid_at(done_at);
            }
            if prod_boost_at == Some(done_at) {
                self.activity.prod_boost_until = None;
                self.activity.prod_boost_factor = 0.0;
            }
            if research_boost_at == Some(done_at) {
                let old = (1.0 + self.activity.research_boost_factor).max(1.0);
                self.activity.research_boost_until = None;
                self.activity.research_boost_factor = 0.0;
                self.rescale_research(done_at, old, 1.0);
            }
        }
    }

    /// Apply a bridge pulse at its timestamp. Catch-up runs first so offline
    /// production through that instant is already in the stockpile.
    /// Events dated after `now_unix` are skipped so a bad feed cannot skip time.
    pub fn apply_activity(&mut self, pulse: ActivityPulse, now_unix: u64) -> ActivityApply {
        if !self.config.activity.enabled {
            return ActivityApply::Skipped;
        }
        let Some((channel, ts)) = resolve_channel(pulse, &self.config.activity) else {
            return ActivityApply::Skipped;
        };
        if ts > now_unix || ts < self.last_tick_unix {
            return ActivityApply::Skipped;
        }
        let spec = self.config.activity.get(channel).clone();
        if !spec.enabled {
            return ActivityApply::Skipped;
        }
        if matches!(
            channel,
            ActivityChannel::CiRed | ActivityChannel::AgentStuck
        ) && !self.config.raid.enabled
        {
            return ActivityApply::Skipped;
        }
        if !self
            .activity
            .admit(channel, ts, spec.cooldown_secs, spec.daily_cap)
        {
            return ActivityApply::Skipped;
        }
        self.catch_up(ts);
        let at = self.last_tick_unix.max(ts);
        match channel {
            ActivityChannel::PrMerged => self.grant_payout(at, spec.payout_hours),
            ActivityChannel::CiGreen => self.set_prod_boost(at, spec.boost_factor, spec.boost_secs),
            ActivityChannel::CiRed | ActivityChannel::AgentStuck => {
                let _ = self.trigger_raid(at);
            }
            ActivityChannel::AgentLongRun => {
                self.set_research_boost(at, spec.boost_factor, spec.boost_secs)
            }
        }
        ActivityApply::Applied
    }

    /// Queue an upgrade on the home planet.
    pub fn enqueue_build(&mut self, kind: BuildingKind, now_unix: u64) -> Result<(), BuildError> {
        self.enqueue_build_on(self.home, kind, now_unix)
    }

    /// Queue an upgrade. Resources are paid from that planet; only the head runs.
    pub fn enqueue_build_on(
        &mut self,
        coord: Coord,
        kind: BuildingKind,
        now_unix: u64,
    ) -> Result<(), BuildError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if coord == self.home {
            let buildings = self.buildings;
            let robotics = buildings.robotics_factory;
            return Self::push_build_job_parts(
                kind,
                now,
                buildings,
                &mut self.resources,
                &mut self.build_queue,
                robotics,
                &self.config,
            );
        }
        let idx = self
            .colonies
            .iter()
            .position(|c| c.coord == coord)
            .ok_or(BuildError::UnknownPlanet)?;
        let buildings = self.colonies[idx].buildings;
        let robotics = buildings.robotics_factory;
        let colony = &mut self.colonies[idx];
        Self::push_build_job_parts(
            kind,
            now,
            buildings,
            &mut colony.resources,
            &mut colony.build_queue,
            robotics,
            &self.config,
        )
    }

    fn push_build_job_parts(
        kind: BuildingKind,
        now: u64,
        buildings: Buildings,
        resources: &mut Resources,
        queue: &mut Vec<BuildJob>,
        robotics: u32,
        config: &BalanceConfig,
    ) -> Result<(), BuildError> {
        let cap = config.max_build_queue;
        if queue.len() >= cap {
            return Err(BuildError::QueueFull { cap });
        }
        let prereqs = config.buildings.get(kind).prerequisites.clone();
        for &(req, required) in &prereqs {
            let actual = projected_on(&buildings, queue, req, queue.len());
            if actual < required {
                return Err(BuildError::MissingPrerequisite {
                    building: req,
                    required,
                    actual,
                });
            }
        }
        let from_level = projected_on(&buildings, queue, kind, queue.len());
        let cost = config.buildings.get(kind).cost(from_level);
        if !resources.can_afford(cost) {
            return Err(BuildError::InsufficientResources {
                missing: resources.missing(cost),
                cost,
            });
        }
        *resources = resources.saturating_sub(cost);
        queue.push(BuildJob {
            building: kind,
            from_level,
            paid: cost,
            started_at: None,
            completes_at: None,
        });
        if queue.len() == 1 {
            start_queue_head(queue, robotics, config, now);
        }
        Ok(())
    }

    /// Queue the same upgrade on every selected planet. Logistics tech is required.
    /// Planets that cannot pay are reported; the others still enqueue.
    pub fn enqueue_build_many(
        &mut self,
        kind: BuildingKind,
        coords: &[Coord],
        now_unix: u64,
    ) -> Vec<(Coord, Result<(), BuildError>)> {
        self.catch_up(now_unix);
        if self.modifier(ModifierKind::Logistics) < 1.0 {
            return coords
                .iter()
                .map(|&c| (c, Err(BuildError::NeedLogistics)))
                .collect();
        }
        coords
            .iter()
            .map(|&c| (c, self.enqueue_build_on(c, kind, now_unix)))
            .collect()
    }

    /// Cancel a queue slot on the home planet and refund its paid cost in full.
    pub fn cancel_build(&mut self, index: usize, now_unix: u64) -> Result<Resources, BuildError> {
        self.cancel_build_on(self.home, index, now_unix)
    }

    pub fn cancel_build_on(
        &mut self,
        coord: Coord,
        index: usize,
        now_unix: u64,
    ) -> Result<Resources, BuildError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if coord == self.home {
            let len = self.build_queue.len();
            if index >= len {
                return Err(BuildError::EmptySlot { index, len });
            }
            let job = self.build_queue.remove(index);
            self.resources = self.resources.saturating_add(job.paid);
            self.reprice_queue();
            if index == 0 {
                if let Some(head) = self.build_queue.first_mut() {
                    head.started_at = None;
                    head.completes_at = None;
                }
                self.start_head(now);
            }
            return Ok(job.paid);
        }
        let idx = self
            .colonies
            .iter()
            .position(|c| c.coord == coord)
            .ok_or(BuildError::UnknownPlanet)?;
        let len = self.colonies[idx].build_queue.len();
        if index >= len {
            return Err(BuildError::EmptySlot { index, len });
        }
        let job = self.colonies[idx].build_queue.remove(index);
        self.colonies[idx].resources = self.colonies[idx].resources.saturating_add(job.paid);
        self.reprice_colony_queue(idx);
        if index == 0 {
            if let Some(head) = self.colonies[idx].build_queue.first_mut() {
                head.started_at = None;
                head.completes_at = None;
            }
            self.start_colony_head(idx, now);
        }
        Ok(job.paid)
    }

    /// Start the single research slot. Fails if another tech is already running.
    pub fn start_research(&mut self, tech: &str, now_unix: u64) -> Result<(), ResearchError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if let Some(current) = &self.research {
            return Err(ResearchError::Busy {
                current: current.tech.clone(),
            });
        }
        let from_level = self.tech_level(tech);
        let (cost, duration, gaps) = {
            let spec = self
                .config
                .techs
                .get(tech)
                .ok_or_else(|| ResearchError::UnknownTech {
                    id: tech.to_string(),
                })?;
            (
                spec.cost(from_level),
                ((spec.duration_secs(from_level) as f64) / self.research_speed())
                    .ceil()
                    .max(1.0) as u64,
                self.research_prereq_gaps(spec),
            )
        };
        if !gaps.is_empty() {
            return Err(ResearchError::MissingPrerequisites { gaps });
        }
        if !self.resources.can_afford(cost) {
            return Err(ResearchError::InsufficientResources {
                missing: self.resources.missing(cost),
                cost,
            });
        }
        self.resources = self.resources.saturating_sub(cost);
        self.research = Some(ResearchJob {
            tech: tech.to_string(),
            from_level,
            paid: cost,
            started_at: now,
            completes_at: now.saturating_add(duration),
        });
        Ok(())
    }

    pub fn cancel_research(&mut self, now_unix: u64) -> Result<Resources, ResearchError> {
        self.catch_up(now_unix);
        let job = self.research.take().ok_or(ResearchError::Idle)?;
        self.resources = self.resources.saturating_add(job.paid);
        Ok(job.paid)
    }

    /// Pay for a batch and append it to the shipyard queue. Each remaining ship
    /// in the head batch completes on its own timestamp.
    pub fn enqueue_ships(
        &mut self,
        kind: ShipKind,
        count: u32,
        now_unix: u64,
    ) -> Result<(), ShipError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        if count == 0 {
            return Err(ShipError::ZeroCount);
        }
        let spec = self.config.ships.get(kind);
        let yard = self.buildings.shipyard;
        if yard < spec.shipyard {
            return Err(ShipError::NeedShipyard {
                required: spec.shipyard,
                actual: yard,
            });
        }
        let gaps = self.missing_ship_prereqs(kind);
        if !gaps.is_empty() {
            return Err(ShipError::MissingPrerequisites { gaps });
        }
        let cap = self.config.max_ship_queue;
        if self.ship_queue.len() >= cap {
            return Err(ShipError::QueueFull { cap });
        }
        let unit = spec.unit_cost();
        let n = f64::from(count);
        let cost = Resources {
            metal: unit.metal * n,
            crystal: unit.crystal * n,
            deuterium: unit.deuterium * n,
        };
        if !self.resources.can_afford(cost) {
            return Err(ShipError::InsufficientResources {
                missing: self.resources.missing(cost),
                cost,
            });
        }
        self.resources = self.resources.saturating_sub(cost);
        self.ship_queue.push(ShipJob {
            ship: kind,
            remaining: count,
            unit_paid: unit,
            started_at: None,
            completes_at: None,
        });
        if self.ship_queue.len() == 1 {
            self.start_ship_head(now);
        }
        Ok(())
    }

    pub fn cancel_ships(&mut self, index: usize, now_unix: u64) -> Result<Resources, ShipError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let len = self.ship_queue.len();
        if index >= len {
            return Err(ShipError::EmptySlot { index, len });
        }
        let job = self.ship_queue.remove(index);
        let n = f64::from(job.remaining);
        let refund = Resources {
            metal: job.unit_paid.metal * n,
            crystal: job.unit_paid.crystal * n,
            deuterium: job.unit_paid.deuterium * n,
        };
        self.resources = self.resources.saturating_add(refund);
        if index == 0 {
            self.start_ship_head(now);
        }
        Ok(refund)
    }

    pub fn ship_cost(&self, kind: ShipKind, count: u32) -> Resources {
        let unit = self.config.ships.get(kind).unit_cost();
        let n = f64::from(count);
        Resources {
            metal: unit.metal * n,
            crystal: unit.crystal * n,
            deuterium: unit.deuterium * n,
        }
    }

    pub fn ship_time_secs(&self, kind: ShipKind) -> u64 {
        self.config.ships.get(kind).duration_secs()
    }

    pub fn fleet_slots(&self) -> u32 {
        self.config
            .flight
            .base_fleet_slots
            .saturating_add(self.modifier(ModifierKind::FleetSlots).floor().max(0.0) as u32)
    }

    pub fn flight_time_and_fuel(
        &self,
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
    ) -> Result<(u64, f64), FleetError> {
        self.flight_from(self.home, dest, ships, speed_percent)
    }

    fn flight_from(
        &self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        speed_percent: u8,
    ) -> Result<(u64, f64), FleetError> {
        if ships.is_empty() {
            return Err(FleetError::EmptyFleet);
        }
        if dest == origin {
            return Err(FleetError::SameLocation);
        }
        if !(10..=100).contains(&speed_percent) {
            return Err(FleetError::SpeedPercent { got: speed_percent });
        }
        let dist = distance(origin, dest, &self.config.flight);
        let mut slowest = f64::MAX;
        let mut units_fuel = 0.0;
        for kind in ShipKind::ALL {
            let n = ships.get(kind);
            if n == 0 {
                continue;
            }
            let stats = self.ship_stats(kind);
            slowest = slowest.min(stats.speed);
            units_fuel += f64::from(n) * stats.fuel;
        }
        Ok((
            flight_secs(dist, slowest, speed_percent),
            fuel_cost(units_fuel, dist, speed_percent, &self.config.flight),
        ))
    }

    /// Detach ships from the planet, pay deuterium, and start outbound.
    pub fn launch_fleet(
        &mut self,
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
        hold_secs: u64,
        now_unix: u64,
    ) -> Result<(), FleetError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let (outbound_secs, fuel) = self.flight_time_and_fuel(ships, dest, speed_percent)?;
        for kind in ShipKind::ALL {
            let need = ships.get(kind);
            let have = self.fleet.get(kind);
            if have < need {
                return Err(FleetError::MissingShips { kind, need, have });
            }
        }
        let cap = self.fleet_slots();
        let used = self.missions.len() as u32;
        if used >= cap {
            return Err(FleetError::NoSlots { used, cap });
        }
        if self.resources.deuterium + 1e-9 < fuel {
            return Err(FleetError::InsufficientFuel {
                missing: (fuel - self.resources.deuterium).max(0.0),
                cost: fuel,
            });
        }
        self.resources.deuterium = (self.resources.deuterium - fuel).max(0.0);
        for kind in ShipKind::ALL {
            self.fleet.saturating_sub(kind, ships.get(kind));
        }
        let id = self.next_mission_id;
        self.next_mission_id = self.next_mission_id.saturating_add(1);
        self.missions.push(Mission {
            id,
            ships,
            origin: self.home,
            dest,
            speed_percent,
            phase: MissionPhase::Outbound,
            launched_at: now,
            outbound_secs,
            hold_secs,
            phase_ends_at: now.saturating_add(outbound_secs),
            fuel_paid: fuel,
            colonize: false,
            expedition_seed: None,
            expedition_applied: false,
            cargo: Resources::ZERO,
            transport: false,
        });
        Ok(())
    }

    pub fn expedition_slots(&self) -> u32 {
        self.config.expedition.base_slots.saturating_add(
            self.modifier(ModifierKind::ExpeditionSlots)
                .floor()
                .max(0.0) as u32,
        )
    }

    fn expedition_used(&self) -> u32 {
        self.missions
            .iter()
            .filter(|m| m.expedition_seed.is_some())
            .count() as u32
    }

    pub fn expedition_dest(&self) -> Coord {
        Coord {
            galaxy: self.home.galaxy,
            system: self.home.system,
            position: self.config.expedition.dest_position,
        }
    }

    /// Send a fleet to the expedition slot for `hold_secs`. The outcome seed is stored
    /// on the mission so a reload cannot re-roll it.
    pub fn launch_expedition(
        &mut self,
        ships: Fleet,
        hold_secs: u64,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        let min = self.config.expedition.min_hold_secs;
        let max = self.config.expedition.max_hold_secs;
        if hold_secs < min || hold_secs > max {
            return Err(FleetError::HoldOutOfRange {
                got: hold_secs,
                min,
                max,
            });
        }
        let cap = self.expedition_slots();
        let used = self.expedition_used();
        if used >= cap {
            return Err(FleetError::NoExpeditionSlots { used, cap });
        }
        let dest = self.expedition_dest();
        self.launch_fleet(ships, dest, speed_percent, hold_secs, now_unix)?;
        let Some(mission) = self.missions.last_mut() else {
            return Err(FleetError::EmptyFleet);
        };
        mission.expedition_seed = Some(next_u64(&mut self.rng_seed));
        mission.expedition_applied = false;
        Ok(mission.id)
    }

    pub fn owned_coords(&self) -> Vec<Coord> {
        let mut out = vec![self.home];
        out.extend(self.colonies.iter().map(|c| c.coord));
        out
    }

    pub fn resources_at(&self, coord: Coord) -> Option<&Resources> {
        if coord == self.home {
            Some(&self.resources)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| &c.resources)
        }
    }

    pub fn buildings_at(&self, coord: Coord) -> Option<&Buildings> {
        if coord == self.home {
            Some(&self.buildings)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| &c.buildings)
        }
    }

    pub fn queue_at(&self, coord: Coord) -> Option<&[BuildJob]> {
        if coord == self.home {
            Some(&self.build_queue)
        } else {
            self.colonies
                .iter()
                .find(|c| c.coord == coord)
                .map(|c| c.build_queue.as_slice())
        }
    }

    pub fn build_cost_on(&self, coord: Coord, kind: BuildingKind) -> Resources {
        let buildings = self.buildings_at(coord).unwrap_or(&self.buildings);
        let queue = self.queue_at(coord).unwrap_or(&self.build_queue);
        let from_level = projected_on(buildings, queue, kind, queue.len());
        self.config.buildings.get(kind).cost(from_level)
    }

    pub fn build_time_secs_on(&self, coord: Coord, kind: BuildingKind) -> u64 {
        let buildings = self.buildings_at(coord).unwrap_or(&self.buildings);
        let queue = self.queue_at(coord).unwrap_or(&self.build_queue);
        let from_level = projected_on(buildings, queue, kind, queue.len());
        duration_with_robotics(kind, from_level, buildings.robotics_factory, &self.config)
    }

    pub fn cargo_capacity(&self, ships: Fleet) -> f64 {
        ShipKind::ALL
            .iter()
            .map(|&kind| f64::from(ships.get(kind)) * self.ship_stats(kind).cargo)
            .sum()
    }

    fn cargo_mass(cargo: Resources) -> f64 {
        cargo.metal + cargo.crystal + cargo.deuterium
    }

    fn check_cargo(cargo: Resources) -> Result<(), FleetError> {
        let ok = [cargo.metal, cargo.crystal, cargo.deuterium]
            .into_iter()
            .all(|n| n.is_finite() && n >= 0.0);
        if ok {
            Ok(())
        } else {
            Err(FleetError::InvalidCargo)
        }
    }

    fn add_fleet_at(&mut self, coord: Coord, ships: Fleet) {
        if coord == self.home {
            for kind in ShipKind::ALL {
                self.fleet.add(kind, ships.get(kind));
            }
            return;
        }
        if let Some(colony) = self.colonies.iter_mut().find(|c| c.coord == coord) {
            for kind in ShipKind::ALL {
                colony.fleet.add(kind, ships.get(kind));
            }
        } else {
            for kind in ShipKind::ALL {
                self.fleet.add(kind, ships.get(kind));
            }
        }
    }

    fn credit_at(&mut self, coord: Coord, found: Resources) {
        if coord == self.home {
            self.credit_capped(found);
            return;
        }
        if let Some(colony) = self.colonies.iter_mut().find(|c| c.coord == coord) {
            let config = &self.config;
            colony.resources.metal = (colony.resources.metal + found.metal).min(
                config
                    .metal_storage
                    .capacity(colony.buildings.metal_storage),
            );
            colony.resources.crystal = (colony.resources.crystal + found.crystal).min(
                config
                    .crystal_storage
                    .capacity(colony.buildings.crystal_storage),
            );
            colony.resources.deuterium = (colony.resources.deuterium + found.deuterium).min(
                config
                    .deuterium_storage
                    .capacity(colony.buildings.deuterium_storage),
            );
        } else {
            self.credit_capped(found);
        }
    }

    fn take_ships_at(&mut self, coord: Coord, ships: Fleet) -> Result<(), FleetError> {
        let fleet = if coord == self.home {
            &mut self.fleet
        } else {
            &mut self
                .colonies
                .iter_mut()
                .find(|c| c.coord == coord)
                .ok_or(FleetError::UnknownPlanet)?
                .fleet
        };
        for kind in ShipKind::ALL {
            let need = ships.get(kind);
            let have = fleet.get(kind);
            if have < need {
                return Err(FleetError::MissingShips { kind, need, have });
            }
        }
        for kind in ShipKind::ALL {
            fleet.saturating_sub(kind, ships.get(kind));
        }
        Ok(())
    }

    fn resources_at_mut(&mut self, coord: Coord) -> Result<&mut Resources, FleetError> {
        if coord == self.home {
            Ok(&mut self.resources)
        } else {
            self.colonies
                .iter_mut()
                .find(|c| c.coord == coord)
                .map(|c| &mut c.resources)
                .ok_or(FleetError::UnknownPlanet)
        }
    }

    /// Move cargo between owned planets. Fuel is paid from the origin at launch.
    pub fn launch_transport(
        &mut self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        cargo: Resources,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        if origin == dest {
            return Err(FleetError::SameLocation);
        }
        if !self.occupied(dest) {
            return Err(FleetError::NotOwned);
        }
        if !self.occupied(origin) {
            return Err(FleetError::UnknownPlanet);
        }
        Self::check_cargo(cargo)?;
        let cap = self.cargo_capacity(ships);
        let need = Self::cargo_mass(cargo);
        if need > cap + 1e-9 {
            return Err(FleetError::CargoExceedsCapacity { need, cap });
        }
        let (outbound_secs, fuel) = self.flight_from(origin, dest, ships, speed_percent)?;
        let needed = Resources {
            metal: cargo.metal,
            crystal: cargo.crystal,
            deuterium: cargo.deuterium + fuel,
        };
        let cargo_missing = {
            let res = self.resources_at_mut(origin)?;
            if !res.can_afford(cargo) {
                Some(res.missing(cargo))
            } else {
                None
            }
        };
        if let Some(missing) = cargo_missing {
            return Err(FleetError::InsufficientCargo { missing });
        }
        let fuel_missing = {
            let res = self.resources_at_mut(origin)?;
            if res.deuterium + 1e-9 < needed.deuterium {
                Some((needed.deuterium - res.deuterium).max(0.0))
            } else {
                None
            }
        };
        if let Some(missing) = fuel_missing {
            return Err(FleetError::InsufficientFuel {
                missing,
                cost: fuel,
            });
        }
        let cap_slots = self.fleet_slots();
        let used = self.missions.len() as u32;
        if used >= cap_slots {
            return Err(FleetError::NoSlots {
                used,
                cap: cap_slots,
            });
        }
        self.take_ships_at(origin, ships)?;
        {
            let res = self.resources_at_mut(origin)?;
            *res = res.saturating_sub(needed);
        }
        let id = self.next_mission_id;
        self.next_mission_id = self.next_mission_id.saturating_add(1);
        let now = self.last_tick_unix;
        self.missions.push(Mission {
            id,
            ships,
            origin,
            dest,
            speed_percent,
            phase: MissionPhase::Outbound,
            launched_at: now,
            outbound_secs,
            hold_secs: 0,
            phase_ends_at: now.saturating_add(outbound_secs),
            fuel_paid: fuel,
            colonize: false,
            expedition_seed: None,
            expedition_applied: false,
            cargo,
            transport: true,
        });
        Ok(id)
    }

    pub fn schedule_route(
        &mut self,
        origin: Coord,
        dest: Coord,
        ships: Fleet,
        cargo: Resources,
        interval_secs: u64,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        if interval_secs == 0 {
            return Err(FleetError::HoldOutOfRange {
                got: 0,
                min: 1,
                max: u64::MAX,
            });
        }
        if origin == dest {
            return Err(FleetError::SameLocation);
        }
        if !self.occupied(origin) || !self.occupied(dest) {
            return Err(FleetError::NotOwned);
        }
        if ships.is_empty() {
            return Err(FleetError::EmptyFleet);
        }
        Self::check_cargo(cargo)?;
        let cap = self.cargo_capacity(ships);
        let need = Self::cargo_mass(cargo);
        if need > cap + 1e-9 {
            return Err(FleetError::CargoExceedsCapacity { need, cap });
        }
        let id = self.next_route_id;
        self.next_route_id = self.next_route_id.saturating_add(1);
        self.routes.push(TradeRoute {
            id,
            origin,
            dest,
            ships,
            cargo,
            speed_percent: 100,
            interval_secs,
            next_at: self.last_tick_unix,
        });
        Ok(id)
    }

    pub fn cancel_route(&mut self, id: u64) -> Result<(), FleetError> {
        let idx = self
            .routes
            .iter()
            .position(|r| r.id == id)
            .ok_or(FleetError::UnknownMission { id })?;
        self.routes.remove(idx);
        Ok(())
    }

    fn fire_routes_at(&mut self, done_at: u64) {
        let ids: Vec<u64> = self
            .routes
            .iter()
            .filter(|r| r.next_at == done_at)
            .map(|r| r.id)
            .collect();
        for id in ids {
            let Some(index) = self.routes.iter().position(|r| r.id == id) else {
                continue;
            };
            let route = self.routes[index].clone();
            self.routes[index].next_at = done_at.saturating_add(route.interval_secs.max(1));
            match self.launch_transport(
                route.origin,
                route.dest,
                route.ships,
                route.cargo,
                route.speed_percent,
                done_at,
            ) {
                Ok(_) => {}
                Err(err) => self.push_event(done_at, format!("route {id}: skipped ({err})")),
            }
        }
    }

    /// Send one colony ship. Settlement is attempted on Action; failure returns the ship.
    pub fn launch_colony(
        &mut self,
        dest: Coord,
        speed_percent: u8,
        now_unix: u64,
    ) -> Result<u64, FleetError> {
        self.catch_up(now_unix);
        if self.fleet.get(ShipKind::ColonyShip) == 0 {
            return Err(FleetError::NeedColonyShip);
        }
        let mut ships = Fleet::default();
        ships.add(ShipKind::ColonyShip, 1);
        self.launch_fleet(ships, dest, speed_percent, 0, now_unix)?;
        let Some(mission) = self.missions.last_mut() else {
            return Err(FleetError::EmptyFleet);
        };
        mission.colonize = true;
        Ok(mission.id)
    }

    pub fn colony_cap(&self) -> u32 {
        1 + self.modifier(ModifierKind::ColonySlots).floor().max(0.0) as u32
    }

    fn occupied(&self, coord: Coord) -> bool {
        coord == self.home || self.colonies.iter().any(|c| c.coord == coord)
    }

    /// Turn a fleet around. Outbound recall uses elapsed time as the return.
    /// `id` is the mission's stable id, so a catch-up that finishes another
    /// fleet cannot retarget this one.
    pub fn recall_fleet(&mut self, id: u64, now_unix: u64) -> Result<(), FleetError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let mission = self
            .missions
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(FleetError::UnknownMission { id })?;
        match mission.phase {
            MissionPhase::Return => return Err(FleetError::AlreadyReturning),
            MissionPhase::Outbound => {
                let elapsed = now.saturating_sub(mission.launched_at);
                mission.phase = MissionPhase::Return;
                mission.phase_ends_at = now.saturating_add(elapsed);
            }
            MissionPhase::Action => {
                let ret = mission.outbound_secs;
                mission.phase = MissionPhase::Return;
                mission.phase_ends_at = now.saturating_add(ret);
            }
        }
        Ok(())
    }

    pub fn missing_ship_prereqs(&self, kind: ShipKind) -> Vec<PrereqGap> {
        let spec = self.config.ships.get(kind);
        spec.prereq_techs
            .iter()
            .filter_map(|pre| {
                let actual = self.tech_level(&pre.id);
                (actual < pre.level).then_some(PrereqGap::Tech {
                    id: pre.id.clone(),
                    required: pre.level,
                    actual,
                })
            })
            .collect()
    }

    /// Config stats scaled by weapons, shields, armour, and the ship's drive.
    pub fn ship_stats(&self, kind: ShipKind) -> crate::ShipStats {
        let spec = self.config.ships.get(kind);
        let attack = spec.attack * (1.0 + self.modifier(ModifierKind::WeaponDamage));
        let shield = spec.shield * (1.0 + self.modifier(ModifierKind::ShieldStrength));
        let hull = spec.hull * (1.0 + self.modifier(ModifierKind::Armour));
        let drive_bonus = self
            .config
            .techs
            .get(&spec.drive)
            .map(|tech| {
                let level = f64::from(self.tech_level(&tech.id));
                tech.modifiers
                    .iter()
                    .filter(|m| m.kind == ModifierKind::DriveSpeed)
                    .map(|m| m.per_level * level)
                    .sum::<f64>()
            })
            .unwrap_or(0.0);
        crate::ShipStats {
            attack,
            shield,
            hull,
            cargo: spec.cargo,
            speed: spec.speed * (1.0 + drive_bonus),
            fuel: spec.fuel,
        }
    }

    pub fn tech_level(&self, id: &str) -> u32 {
        self.tech_levels.get(id).copied().unwrap_or(0)
    }

    /// Tech modifiers plus the home planet's type bonus.
    pub fn modifier(&self, kind: ModifierKind) -> f64 {
        self.modifier_for(self.home_kind, kind)
    }

    /// Tech modifiers plus this planet type's production bonus.
    pub fn modifier_for(&self, planet: PlanetType, kind: ModifierKind) -> f64 {
        self.tech_modifier(kind) + self.planet_bonus(planet, kind)
    }

    fn tech_modifier(&self, kind: ModifierKind) -> f64 {
        self.config
            .techs
            .iter()
            .map(|spec| {
                let level = f64::from(self.tech_level(&spec.id));
                spec.modifiers
                    .iter()
                    .filter(|m| m.kind == kind)
                    .map(|m| m.per_level * level)
                    .sum::<f64>()
            })
            .sum()
    }

    fn planet_bonus(&self, planet: PlanetType, kind: ModifierKind) -> f64 {
        let bonus = self.config.planet_types.get(planet);
        match kind {
            ModifierKind::MetalOutput => bonus.metal,
            ModifierKind::CrystalOutput => bonus.crystal,
            ModifierKind::DeuteriumOutput => bonus.deuterium,
            ModifierKind::EnergyOutput => bonus.energy,
            _ => 0.0,
        }
    }

    pub fn missing_research_prereqs(&self, tech: &str) -> Result<Vec<PrereqGap>, ResearchError> {
        let spec = self
            .config
            .techs
            .get(tech)
            .ok_or_else(|| ResearchError::UnknownTech {
                id: tech.to_string(),
            })?;
        Ok(self.research_prereq_gaps(spec))
    }

    fn research_prereq_gaps(&self, spec: &crate::TechSpec) -> Vec<PrereqGap> {
        let mut gaps = Vec::new();
        for pre in &spec.prereq_buildings {
            let actual = self.buildings.get(pre.building);
            if actual < pre.level {
                gaps.push(PrereqGap::Building {
                    building: pre.building,
                    required: pre.level,
                    actual,
                });
            }
        }
        for pre in &spec.prereq_techs {
            let actual = self.tech_level(&pre.id);
            if actual < pre.level {
                gaps.push(PrereqGap::Tech {
                    id: pre.id.clone(),
                    required: pre.level,
                    actual,
                });
            }
        }
        gaps
    }

    pub fn build_cost(&self, kind: BuildingKind) -> Resources {
        self.config
            .buildings
            .get(kind)
            .cost(self.projected_level(kind))
    }

    pub fn build_time_secs(&self, kind: BuildingKind) -> u64 {
        self.duration_secs(kind, self.projected_level(kind))
    }

    fn projected_level(&self, kind: BuildingKind) -> u32 {
        self.projected_level_before(kind, self.build_queue.len())
    }

    fn projected_level_before(&self, kind: BuildingKind, index: usize) -> u32 {
        self.buildings.get(kind)
            + self.build_queue[..index]
                .iter()
                .filter(|job| job.building == kind)
                .count() as u32
    }

    fn prereqs_ok_at(&self, index: usize) -> bool {
        let kind = self.build_queue[index].building;
        let prereqs = self.config.buildings.get(kind).prerequisites.clone();
        prereqs
            .iter()
            .all(|&(req, required)| self.projected_level_before(req, index) >= required)
    }

    /// After a cancellation, drop jobs whose prereqs no longer hold and
    /// recompute `from_level` / paid cost for the rest.
    fn reprice_queue(&mut self) {
        let mut i = 0;
        while i < self.build_queue.len() {
            if !self.prereqs_ok_at(i) {
                let job = self.build_queue.remove(i);
                self.resources = self.resources.saturating_add(job.paid);
                continue;
            }
            let kind = self.build_queue[i].building;
            let from_level = self.projected_level_before(kind, i);
            let new_cost = self.config.buildings.get(kind).cost(from_level);
            let old_paid = self.build_queue[i].paid;
            self.resources = self.resources.saturating_add(old_paid);
            if !self.resources.can_afford(new_cost) {
                self.build_queue.remove(i);
                continue;
            }
            self.resources = self.resources.saturating_sub(new_cost);
            self.build_queue[i].from_level = from_level;
            self.build_queue[i].paid = new_cost;
            i += 1;
        }
    }

    fn duration_secs(&self, kind: BuildingKind, from_level: u32) -> u64 {
        duration_with_robotics(
            kind,
            from_level,
            self.buildings.robotics_factory,
            &self.config,
        )
    }

    fn start_head(&mut self, now_unix: u64) {
        let robotics = self.buildings.robotics_factory;
        start_queue_head(
            &mut self.build_queue,
            robotics,
            &self.config,
            now_unix.max(self.last_tick_unix),
        );
    }

    fn start_colony_head(&mut self, index: usize, now_unix: u64) {
        let robotics = self.colonies[index].buildings.robotics_factory;
        start_queue_head(
            &mut self.colonies[index].build_queue,
            robotics,
            &self.config,
            now_unix.max(self.last_tick_unix),
        );
    }

    fn reprice_colony_queue(&mut self, index: usize) {
        let mut i = 0;
        while i < self.colonies[index].build_queue.len() {
            let kind = self.colonies[index].build_queue[i].building;
            let prereqs = self.config.buildings.get(kind).prerequisites.clone();
            let ok = prereqs.iter().all(|&(req, required)| {
                projected_on(
                    &self.colonies[index].buildings,
                    &self.colonies[index].build_queue,
                    req,
                    i,
                ) >= required
            });
            if !ok {
                let job = self.colonies[index].build_queue.remove(i);
                self.colonies[index].resources =
                    self.colonies[index].resources.saturating_add(job.paid);
                continue;
            }
            let from_level = projected_on(
                &self.colonies[index].buildings,
                &self.colonies[index].build_queue,
                kind,
                i,
            );
            let new_cost = self.config.buildings.get(kind).cost(from_level);
            let old_paid = self.colonies[index].build_queue[i].paid;
            self.colonies[index].resources =
                self.colonies[index].resources.saturating_add(old_paid);
            if !self.colonies[index].resources.can_afford(new_cost) {
                self.colonies[index].build_queue.remove(i);
                continue;
            }
            self.colonies[index].resources =
                self.colonies[index].resources.saturating_sub(new_cost);
            self.colonies[index].build_queue[i].from_level = from_level;
            self.colonies[index].build_queue[i].paid = new_cost;
            i += 1;
        }
    }

    fn start_ship_head(&mut self, now_unix: u64) {
        let Some(job) = self.ship_queue.first() else {
            return;
        };
        if job.completes_at.is_some() {
            return;
        }
        let duration = self.ship_time_secs(job.ship);
        let now = now_unix.max(self.last_tick_unix);
        let job = &mut self.ship_queue[0];
        job.started_at = Some(now);
        job.completes_at = Some(now.saturating_add(duration));
    }

    fn advance_missions_at(&mut self, done_at: u64) {
        loop {
            let Some(index) = self
                .missions
                .iter()
                .position(|m| m.phase_ends_at == done_at)
            else {
                return;
            };
            match self.missions[index].phase {
                MissionPhase::Outbound => {
                    let hold = self.missions[index].hold_secs;
                    self.missions[index].phase = MissionPhase::Action;
                    self.missions[index].phase_ends_at = done_at.saturating_add(hold);
                    if self.apply_expedition_at(index, done_at) {
                        continue;
                    }
                }
                MissionPhase::Action => {
                    if self.try_colonize_at(index, done_at) {
                        continue;
                    }
                    if self.missions[index].transport {
                        let dest = self.missions[index].dest;
                        let cargo = self.missions[index].cargo;
                        self.credit_at(dest, cargo);
                        self.missions[index].cargo = Resources::ZERO;
                    }
                    let ret = self.missions[index].outbound_secs;
                    self.missions[index].phase = MissionPhase::Return;
                    self.missions[index].phase_ends_at = done_at.saturating_add(ret);
                }
                MissionPhase::Return => {
                    let mission = self.missions.remove(index);
                    self.add_fleet_at(mission.origin, mission.ships);
                    if mission.transport {
                        self.credit_at(mission.origin, mission.cargo);
                    }
                }
            }
        }
    }

    /// Apply a pending expedition when Action starts. Returns true if the mission was removed.
    fn apply_expedition_at(&mut self, index: usize, done_at: u64) -> bool {
        let Some(seed) = self.missions[index].expedition_seed else {
            return false;
        };
        if self.missions[index].expedition_applied {
            return false;
        }
        self.missions[index].expedition_applied = true;
        let ships = self.missions[index].ships;
        let hold = self.missions[index].hold_secs;
        let astro = self.tech_level("astrophysics");
        let result = roll_expedition(
            seed,
            &ships,
            astro,
            hold,
            &self.config.expedition,
            &self.config.ships,
        );
        match result {
            ExpeditionResult::Nothing => {
                self.push_event(done_at, "expedition: nothing found".into());
                false
            }
            ExpeditionResult::Resources(found) => {
                self.credit_capped(found);
                self.push_event(
                    done_at,
                    format!(
                        "expedition: found M{:.0} C{:.0} D{:.0}",
                        found.metal, found.crystal, found.deuterium
                    ),
                );
                false
            }
            ExpeditionResult::Ships(found) => {
                for kind in ShipKind::ALL {
                    self.missions[index].ships.add(kind, found.get(kind));
                }
                let n = found.get(self.config.expedition.ship_kind);
                self.push_event(
                    done_at,
                    format!("expedition: found {n} {}", self.config.expedition.ship_kind),
                );
                false
            }
            ExpeditionResult::Delay { extra_secs } => {
                self.missions[index].phase_ends_at = self.missions[index]
                    .phase_ends_at
                    .saturating_add(extra_secs);
                self.push_event(done_at, format!("expedition: delayed {extra_secs}s"));
                false
            }
            ExpeditionResult::EarlyReturn => {
                let ret = self.missions[index].outbound_secs;
                self.missions[index].phase = MissionPhase::Return;
                self.missions[index].phase_ends_at = done_at.saturating_add(ret);
                self.push_event(done_at, "expedition: returning early".into());
                false
            }
            ExpeditionResult::Pirates { enemy } => {
                let mut fight_seed = seed.wrapping_add(0xA5A5_A5A5_A5A5_A5A5);
                let atk_stats = self.player_combat_stats();
                let def_stats = stats_from_catalog(&self.config.ships);
                let rf = rapid_fire_table(&self.config.ships);
                let report = resolve_combat(
                    ships,
                    enemy,
                    &atk_stats,
                    &def_stats,
                    self.config.raid.max_rounds,
                    &rf,
                    &mut fight_seed,
                );
                self.missions[index].ships = report.attacker_after;
                let survivors: u32 = ShipKind::ALL
                    .iter()
                    .map(|&k| self.missions[index].ships.get(k))
                    .sum();
                self.push_event(
                    done_at,
                    format!(
                        "expedition: pirates. {} rounds. survivors {survivors}",
                        report.rounds
                    ),
                );
                if self.missions[index].ships.is_empty() {
                    self.missions.remove(index);
                    return true;
                }
                let ret = self.missions[index].outbound_secs;
                self.missions[index].phase = MissionPhase::Return;
                self.missions[index].phase_ends_at = done_at.saturating_add(ret);
                false
            }
            ExpeditionResult::Loss => {
                self.push_event(done_at, "expedition: fleet lost".into());
                self.missions.remove(index);
                true
            }
        }
    }

    fn credit_capped(&mut self, found: Resources) {
        for kind in ResourceKind::ALL {
            let cap = self.storage_cap(kind);
            let current = self.resources.get(kind);
            self.resources
                .set(kind, (current + found.get(kind)).min(cap));
        }
    }

    /// Found a colony at Action, or leave the mission to return.
    fn try_colonize_at(&mut self, index: usize, _done_at: u64) -> bool {
        if !self.missions[index].colonize {
            return false;
        }
        let dest = self.missions[index].dest;
        let planets = 1 + self.colonies.len() as u32;
        let no_ship = self.missions[index].ships.get(ShipKind::ColonyShip) == 0;
        if self.occupied(dest) || planets >= self.colony_cap() || no_ship {
            return false;
        }
        let mut ships = self.missions.remove(index).ships;
        ships.saturating_sub(ShipKind::ColonyShip, 1);
        let (kind, size) = planet_profile(self.galaxy_seed, dest);
        self.colonies.push(Colony {
            coord: dest,
            kind,
            size,
            resources: Resources::ZERO,
            buildings: Buildings::default(),
            fleet: ships,
            build_queue: Vec::new(),
        });
        true
    }

    /// Seconds until the next raid when it has been announced, else `None`.
    pub fn raid_countdown_secs(&self) -> Option<u64> {
        if !self.config.raid.enabled {
            return None;
        }
        let lands = self.next_raid_at?;
        if self.last_tick_unix + self.config.raid.min_warning_secs < lands {
            return None;
        }
        Some(lands.saturating_sub(self.last_tick_unix))
    }

    /// Run a raid now. The bridge and the scheduler share this path.
    /// Clears a pending schedule first so `catch_up` does not also resolve it.
    pub fn trigger_raid(&mut self, now_unix: u64) -> CombatReport {
        self.next_raid_at = None;
        self.catch_up(now_unix);
        self.resolve_raid_at(self.last_tick_unix)
    }

    fn resolve_raid_at(&mut self, done_at: u64) -> CombatReport {
        let pirates = self.pirate_fleet();
        let atk_stats = stats_from_catalog(&self.config.ships);
        let def_stats = self.player_combat_stats();
        let rf = rapid_fire_table(&self.config.ships);
        let report = resolve_combat(
            pirates,
            self.fleet,
            &atk_stats,
            &def_stats,
            self.config.raid.max_rounds,
            &rf,
            &mut self.rng_seed,
        );
        self.fleet = report.defender_after;
        let plunder = if report.winner == CombatWinner::Attacker {
            let taken = plunder_from(
                self.resources,
                self.config.raid.plunder_share,
                self.config.raid.plunder_cap,
            );
            self.resources = self.resources.saturating_sub(taken);
            taken
        } else {
            Resources::ZERO
        };
        self.push_event(done_at, format_raid_report(&report, plunder));
        self.next_raid_at = Some(done_at.saturating_add(self.config.raid.interval_secs.max(1)));
        report
    }

    fn pirate_fleet(&self) -> Fleet {
        let points = self.fleet_combat_points();
        let scaled = (points * self.config.raid.strength_factor).floor().max(0.0) as u32;
        let n = scaled.max(self.config.raid.min_ships);
        let mut fleet = Fleet::default();
        if n > 0 {
            fleet.add(ShipKind::LightFighter, n);
        }
        fleet
    }

    fn fleet_combat_points(&self) -> f64 {
        ShipKind::ALL
            .iter()
            .map(|&kind| {
                let stats = self.ship_stats(kind);
                f64::from(self.fleet.get(kind)) * (stats.attack + stats.hull)
            })
            .sum()
    }

    fn player_combat_stats(&self) -> [ShipStats; crate::SHIP_KINDS] {
        let mut out = [ShipStats {
            attack: 0.0,
            shield: 0.0,
            hull: 0.0,
            cargo: 0.0,
            speed: 0.0,
            fuel: 0.0,
        }; crate::SHIP_KINDS];
        for kind in ShipKind::ALL {
            out[kind.index()] = self.ship_stats(kind);
        }
        out
    }

    fn push_event(&mut self, at: u64, text: String) {
        self.events.push(GameEvent { at, text });
        if self.events.len() > EVENT_LOG_CAP {
            let drop = self.events.len() - EVENT_LOG_CAP;
            self.events.drain(..drop);
        }
    }

    fn complete_one_ship(&mut self, done_at: u64) {
        let Some(job) = self.ship_queue.first() else {
            return;
        };
        let kind = job.ship;
        let duration = self.ship_time_secs(kind);
        self.fleet.add(kind, 1);
        let job = &mut self.ship_queue[0];
        job.remaining = job.remaining.saturating_sub(1);
        if job.remaining == 0 {
            self.ship_queue.remove(0);
            self.start_ship_head(done_at);
            return;
        }
        job.started_at = Some(done_at);
        job.completes_at = Some(done_at.saturating_add(duration));
    }

    fn produce_until(&mut self, now_unix: u64) {
        if now_unix <= self.last_tick_unix {
            return;
        }
        let hours = (now_unix - self.last_tick_unix) as f64 / 3600.0;
        let factor = self.energy_factor();
        let boost = self.prod_boost_mult();
        for kind in ResourceKind::ALL {
            let rate = self.base_production_per_hour(kind) * factor * boost;
            let cap = self.storage_cap(kind);
            let current = self.resources.get(kind);
            self.resources
                .set(kind, add_capped(current, rate, hours, cap));
        }
        let config = self.config.clone();
        let metal_t = self.tech_modifier(ModifierKind::MetalOutput);
        let crystal_t = self.tech_modifier(ModifierKind::CrystalOutput);
        let deut_t = self.tech_modifier(ModifierKind::DeuteriumOutput);
        let energy_t = self.tech_modifier(ModifierKind::EnergyOutput);
        for colony in &mut self.colonies {
            let bonus = config.planet_types.get(colony.kind);
            let produced = config
                .solar_plant
                .energy_output(colony.buildings.solar_plant)
                * (1.0 + energy_t + bonus.energy).max(0.0);
            let consumed = config.metal_mine.energy_use(colony.buildings.metal_mine)
                + config
                    .crystal_mine
                    .energy_use(colony.buildings.crystal_mine)
                + config
                    .deuterium_synthesizer
                    .energy_use(colony.buildings.deuterium_synthesizer);
            let factor = if consumed <= 0.0 {
                1.0
            } else {
                (produced / consumed).clamp(0.0, 1.0)
            };
            let metal_rate = config
                .metal_mine
                .production_per_hour(colony.buildings.metal_mine)
                * (1.0 + metal_t + bonus.metal)
                * factor
                * boost;
            let crystal_rate = config
                .crystal_mine
                .production_per_hour(colony.buildings.crystal_mine)
                * (1.0 + crystal_t + bonus.crystal)
                * factor
                * boost;
            let deut_rate = config
                .deuterium_synthesizer
                .production_per_hour(colony.buildings.deuterium_synthesizer)
                * (1.0 + deut_t + bonus.deuterium)
                * factor
                * boost;
            let metal_cap = config
                .metal_storage
                .capacity(colony.buildings.metal_storage);
            let crystal_cap = config
                .crystal_storage
                .capacity(colony.buildings.crystal_storage);
            let deut_cap = config
                .deuterium_storage
                .capacity(colony.buildings.deuterium_storage);
            colony.resources.metal =
                add_capped(colony.resources.metal, metal_rate, hours, metal_cap);
            colony.resources.crystal =
                add_capped(colony.resources.crystal, crystal_rate, hours, crystal_cap);
            colony.resources.deuterium =
                add_capped(colony.resources.deuterium, deut_rate, hours, deut_cap);
        }
        self.last_tick_unix = now_unix;
    }

    pub fn energy_produced(&self) -> f64 {
        self.config
            .solar_plant
            .energy_output(self.buildings.solar_plant)
            * (1.0 + self.modifier(ModifierKind::EnergyOutput)).max(0.0)
    }

    pub fn energy_consumed(&self) -> f64 {
        self.config.metal_mine.energy_use(self.buildings.metal_mine)
            + self
                .config
                .crystal_mine
                .energy_use(self.buildings.crystal_mine)
            + self
                .config
                .deuterium_synthesizer
                .energy_use(self.buildings.deuterium_synthesizer)
    }

    /// Produced minus consumed. Negative when mines draw more than the plant supplies.
    pub fn energy_balance(&self) -> f64 {
        self.energy_produced() - self.energy_consumed()
    }

    /// Scales mine output when energy is short. Always in `[0, 1]`.
    pub fn energy_factor(&self) -> f64 {
        let consumed = self.energy_consumed();
        if consumed <= 0.0 {
            1.0
        } else {
            (self.energy_produced() / consumed).clamp(0.0, 1.0)
        }
    }

    pub fn production_per_hour(&self, kind: ResourceKind) -> f64 {
        self.base_production_per_hour(kind) * self.energy_factor() * self.prod_boost_mult()
    }

    fn prod_boost_mult(&self) -> f64 {
        match self.activity.prod_boost_until {
            Some(until) if self.last_tick_unix < until => 1.0 + self.activity.prod_boost_factor,
            _ => 1.0,
        }
    }

    fn research_speed(&self) -> f64 {
        match self.activity.research_boost_until {
            Some(until) if self.last_tick_unix < until => {
                (1.0 + self.activity.research_boost_factor).max(1.0)
            }
            _ => 1.0,
        }
    }

    fn rescale_research(&mut self, at: u64, old_speed: f64, new_speed: f64) {
        if (old_speed - new_speed).abs() < 1e-12 {
            return;
        }
        let Some(job) = self.research.as_mut() else {
            return;
        };
        let remaining = job.completes_at.saturating_sub(at) as f64;
        let base = (remaining * old_speed).max(1.0);
        let new_rem = (base / new_speed.max(1e-9)).ceil().max(1.0) as u64;
        job.completes_at = at.saturating_add(new_rem);
    }

    fn grant_payout(&mut self, at: u64, hours: f64) {
        if hours <= 0.0 {
            return;
        }
        for kind in ResourceKind::ALL {
            let rate = self.production_per_hour(kind);
            let cap = self.storage_cap(kind);
            let current = self.resources.get(kind);
            self.resources
                .set(kind, add_capped(current, rate, hours, cap));
        }
        self.push_event(
            at,
            format!("activity: merged PR paid {hours}h of production"),
        );
    }

    fn set_prod_boost(&mut self, at: u64, factor: f64, secs: u64) {
        if factor <= 0.0 || secs == 0 {
            return;
        }
        self.activity.prod_boost_factor = factor;
        self.activity.prod_boost_until = Some(at.saturating_add(secs));
        self.push_event(
            at,
            format!("activity: production +{:.0}% for {secs}s", factor * 100.0),
        );
    }

    fn set_research_boost(&mut self, at: u64, factor: f64, secs: u64) {
        if factor <= 0.0 || secs == 0 {
            return;
        }
        let old = self.research_speed();
        self.activity.research_boost_factor = factor;
        self.activity.research_boost_until = Some(at.saturating_add(secs));
        self.rescale_research(at, old, 1.0 + factor);
        self.push_event(
            at,
            format!("activity: research +{:.0}% for {secs}s", factor * 100.0),
        );
    }

    pub fn storage_cap(&self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Metal => self
                .config
                .metal_storage
                .capacity(self.buildings.metal_storage),
            ResourceKind::Crystal => self
                .config
                .crystal_storage
                .capacity(self.buildings.crystal_storage),
            ResourceKind::Deuterium => self
                .config
                .deuterium_storage
                .capacity(self.buildings.deuterium_storage),
        }
    }

    fn base_production_per_hour(&self, kind: ResourceKind) -> f64 {
        let raw = match kind {
            ResourceKind::Metal => self
                .config
                .metal_mine
                .production_per_hour(self.buildings.metal_mine),
            ResourceKind::Crystal => self
                .config
                .crystal_mine
                .production_per_hour(self.buildings.crystal_mine),
            ResourceKind::Deuterium => self
                .config
                .deuterium_synthesizer
                .production_per_hour(self.buildings.deuterium_synthesizer),
        };
        let output = match kind {
            ResourceKind::Metal => ModifierKind::MetalOutput,
            ResourceKind::Crystal => ModifierKind::CrystalOutput,
            ResourceKind::Deuterium => ModifierKind::DeuteriumOutput,
        };
        raw * (1.0 + self.modifier_for(self.home_kind, output))
    }
}

fn fleet_label(fleet: Fleet) -> String {
    let parts: Vec<String> = ShipKind::ALL
        .iter()
        .filter_map(|&kind| {
            let n = fleet.get(kind);
            (n > 0).then(|| format!("{n} {kind}"))
        })
        .collect();
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

fn fleet_losses(before: Fleet, after: Fleet) -> Fleet {
    let mut lost = Fleet::default();
    for kind in ShipKind::ALL {
        lost.add(kind, before.get(kind).saturating_sub(after.get(kind)));
    }
    lost
}

fn format_raid_report(report: &crate::CombatReport, plunder: Resources) -> String {
    format!(
        "raid: pirates {} vs {}. {} rounds. lost {}. pirates lost {}. plunder M{:.0} C{:.0} D{:.0}",
        fleet_label(report.attacker_before),
        fleet_label(report.defender_before),
        report.rounds,
        fleet_label(fleet_losses(report.defender_before, report.defender_after)),
        fleet_label(fleet_losses(report.attacker_before, report.attacker_after)),
        plunder.metal,
        plunder.crystal,
        plunder.deuterium
    )
}

fn min_time(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Integrate a constant rate for `hours`, stopping exactly at `cap`.
/// Never lowers `current` (over-cap stockpiles and zero/negative rates stay put).
fn projected_on(
    buildings: &Buildings,
    queue: &[BuildJob],
    kind: BuildingKind,
    index: usize,
) -> u32 {
    buildings.get(kind)
        + queue[..index]
            .iter()
            .filter(|job| job.building == kind)
            .count() as u32
}

fn duration_with_robotics(
    kind: BuildingKind,
    from_level: u32,
    robotics: u32,
    config: &BalanceConfig,
) -> u64 {
    let raw = config.buildings.get(kind).raw_seconds(from_level);
    let divisor = 1.0 + f64::from(robotics) * config.robotics_speed_per_level;
    let secs = if divisor > 0.0 { raw / divisor } else { raw };
    secs.ceil().max(1.0) as u64
}

fn start_queue_head(queue: &mut [BuildJob], robotics: u32, config: &BalanceConfig, now: u64) {
    let Some(job) = queue.first() else {
        return;
    };
    if job.completes_at.is_some() {
        return;
    }
    let duration = duration_with_robotics(job.building, job.from_level, robotics, config);
    let job = &mut queue[0];
    job.started_at = Some(now);
    job.completes_at = Some(now.saturating_add(duration));
}

fn add_capped(current: f64, rate_per_hour: f64, hours: f64, cap: f64) -> f64 {
    if hours <= 0.0 || rate_per_hour <= 0.0 || current >= cap {
        current
    } else {
        (current + rate_per_hour * hours).min(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::TechTree;
    use crate::buildings::{BuildError, BuildingKind};
    use crate::clock::{Clock, FakeClock};
    use crate::fleets::{Coord, FleetError, MissionPhase};
    use crate::research::{ModifierKind, PrereqGap, ResearchError};
    use crate::ships::{ShipError, ShipKind};

    const EPS: f64 = 1e-6;

    fn close(a: f64, b: f64) {
        assert!(
            (a - b).abs() < EPS,
            "left={a} right={b} delta={}",
            (a - b).abs()
        );
    }

    /// Config with round numbers so catch-up tests read the spec fields, not defaults.
    fn test_config() -> BalanceConfig {
        BalanceConfig {
            metal_mine: crate::ProducerSpec {
                base_per_hour: 100.0,
                growth: 2.0,
                base_energy: 10.0,
                energy_growth: 1.0,
            },
            crystal_mine: crate::ProducerSpec {
                base_per_hour: 50.0,
                growth: 2.0,
                base_energy: 10.0,
                energy_growth: 1.0,
            },
            deuterium_synthesizer: crate::ProducerSpec {
                base_per_hour: 20.0,
                growth: 2.0,
                base_energy: 20.0,
                energy_growth: 1.0,
            },
            solar_plant: crate::EnergyProducerSpec {
                base_energy: 20.0,
                growth: 1.0,
            },
            metal_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            crystal_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            deuterium_storage: crate::StorageSpec {
                base: 1_000.0,
                growth: 2.0,
            },
            starting_resources: Resources {
                metal: 0.0,
                crystal: 0.0,
                deuterium: 0.0,
            },
            starting_buildings: Buildings {
                metal_mine: 1,
                crystal_mine: 1,
                deuterium_synthesizer: 0,
                solar_plant: 1,
                metal_storage: 0,
                crystal_storage: 0,
                deuterium_storage: 0,
                robotics_factory: 0,
                research_lab: 0,
                shipyard: 0,
            },
            buildings: crate::BuildingCatalog::default(),
            max_build_queue: 5,
            robotics_speed_per_level: 1.0,
            techs: TechTree::empty(),
            ships: crate::ShipCatalog::default(),
            max_ship_queue: 5,
            flight: crate::FlightConfig::default(),
            raid: crate::RaidConfig::inactive(),
            planet_types: crate::PlanetTypeCatalog {
                rocky: crate::PlanetTypeBonus::ZERO,
                ice: crate::PlanetTypeBonus::ZERO,
                gas_giant: crate::PlanetTypeBonus::ZERO,
            },
            activity: crate::ActivityEffectsConfig::default(),
            expedition: crate::ExpeditionConfig::default(),
        }
    }

    fn catch_up_via_clock(state: &mut GameState, clock: &FakeClock) {
        state.catch_up(clock.now_unix());
    }

    #[test]
    fn production_and_energy_factor_come_from_config() {
        let cfg = test_config();
        let state = GameState::with_config(0, cfg.clone());
        // Solar 20, mines 10+10, factor 1. Metal 100/h, crystal 50/h from spec fields.
        close(state.energy_produced(), cfg.solar_plant.base_energy);
        close(
            state.energy_consumed(),
            cfg.metal_mine.base_energy + cfg.crystal_mine.base_energy,
        );
        close(state.energy_factor(), 1.0);
        close(
            state.production_per_hour(ResourceKind::Metal),
            cfg.metal_mine.base_per_hour,
        );
        close(
            state.production_per_hour(ResourceKind::Crystal),
            cfg.crystal_mine.base_per_hour,
        );
        close(state.production_per_hour(ResourceKind::Deuterium), 0.0);
        close(
            state.storage_cap(ResourceKind::Metal),
            cfg.metal_storage.base,
        );
    }

    #[test]
    fn energy_shortfall_scales_all_mine_output() {
        let mut cfg = test_config();
        cfg.solar_plant.base_energy = 10.0; // need 20 → factor 0.5
        let mut state = GameState::with_config(0, cfg.clone());
        close(state.energy_factor(), 0.5);
        close(state.energy_balance(), -10.0);
        state.catch_up(3600);
        close(state.resources.metal, cfg.metal_mine.base_per_hour * 0.5);
        close(
            state.resources.crystal,
            cfg.crystal_mine.base_per_hour * 0.5,
        );
        close(state.resources.deuterium, 0.0);
    }

    #[test]
    fn zero_energy_stops_mines() {
        let mut cfg = test_config();
        cfg.starting_buildings.solar_plant = 0;
        let mut state = GameState::with_config(0, cfg);
        close(state.energy_factor(), 0.0);
        state.catch_up(3600 * 10);
        close(state.resources.metal, 0.0);
        close(state.resources.crystal, 0.0);
    }

    #[test]
    fn higher_mine_level_uses_growth_from_config() {
        let mut cfg = test_config();
        cfg.starting_buildings.metal_mine = 3; // 100 * 2^2 = 400
        cfg.solar_plant.base_energy = 1_000.0; // no shortfall
        let mut state = GameState::with_config(0, cfg.clone());
        close(
            state.production_per_hour(ResourceKind::Metal),
            cfg.metal_mine.base_per_hour * cfg.metal_mine.growth.powi(2),
        );
        state.catch_up(3600);
        close(
            state.resources.metal,
            cfg.metal_mine.base_per_hour * cfg.metal_mine.growth.powi(2),
        );
    }

    #[test]
    fn catch_up_over_gap_equals_step_by_step() {
        let mut one_shot = GameState::with_config(0, test_config());
        one_shot.catch_up(7200);

        let mut stepped = GameState::with_config(0, test_config());
        for t in 1..=7200 {
            stepped.catch_up(t);
        }

        close(one_shot.resources.metal, stepped.resources.metal);
        close(one_shot.resources.crystal, stepped.resources.crystal);
        close(one_shot.resources.deuterium, stepped.resources.deuterium);
        assert_eq!(one_shot.last_tick_unix, 7200);
        assert_eq!(stepped.last_tick_unix, 7200);
    }

    #[test]
    fn catch_up_uses_injected_clock() {
        let clock = FakeClock::new(1_000);
        let mut state = GameState::with_config(clock.now_unix(), test_config());
        clock.advance(3600);
        catch_up_via_clock(&mut state, &clock);
        close(state.resources.metal, 100.0);
        assert_eq!(state.last_tick_unix, 4_600);
    }

    #[test]
    fn storage_cap_stops_accumulation_and_partial_tick_is_exact() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 150.0;
        cfg.starting_resources.metal = 100.0;
        // 100/h, 50 room → hits cap in 0.5 h = 1800 s.
        let mut one_shot = GameState::with_config(0, cfg.clone());
        one_shot.catch_up(3600);
        close(one_shot.resources.metal, 150.0);

        let mut stepped = GameState::with_config(0, cfg.clone());
        for t in 1..=3600 {
            stepped.catch_up(t);
        }
        close(stepped.resources.metal, 150.0);
        close(one_shot.resources.metal, stepped.resources.metal);

        // Exactly at the crossing instant.
        let mut at_cap_time = GameState::with_config(0, cfg);
        at_cap_time.catch_up(1800);
        close(at_cap_time.resources.metal, 150.0);
    }

    #[test]
    fn over_cap_stockpile_is_not_drained() {
        let mut cfg = test_config();
        cfg.metal_storage.base = 50.0;
        cfg.starting_resources.metal = 80.0;
        let mut state = GameState::with_config(0, cfg);
        state.catch_up(3600);
        close(state.resources.metal, 80.0);
    }

    #[test]
    fn clock_going_backwards_does_not_lose_resources() {
        let mut state = GameState::with_config(100, test_config());
        state.catch_up(3600);
        let metal = state.resources.metal;
        let crystal = state.resources.crystal;
        let last = state.last_tick_unix;
        state.catch_up(50);
        close(state.resources.metal, metal);
        close(state.resources.crystal, crystal);
        assert_eq!(state.last_tick_unix, last);
    }

    #[test]
    fn default_starter_has_full_energy_for_starting_mines() {
        let state = GameState::new(0);
        assert!(state.energy_factor() >= 1.0 - EPS);
        assert!(state.production_per_hour(ResourceKind::Metal) > 0.0);
        assert!(state.production_per_hour(ResourceKind::Crystal) > 0.0);
    }

    #[test]
    fn catch_up_equals_step_by_step_over_random_gaps() {
        let mut seed = 0xC0FFEE_u64;
        for trial in 0..40 {
            let gap = 1 + splitmix(&mut seed) % (3 * 24 * 3600);
            let metal_level = (splitmix(&mut seed) % 6) as u32;
            let solar_level = 1 + (splitmix(&mut seed) % 8) as u32;
            let fill = (splitmix(&mut seed) % 1000) as f64;

            let mut cfg = test_config();
            cfg.starting_buildings.metal_mine = metal_level;
            cfg.starting_buildings.crystal_mine = metal_level.min(4);
            cfg.starting_buildings.solar_plant = solar_level;
            cfg.starting_resources.metal = fill;
            cfg.starting_resources.crystal = fill / 2.0;

            let mut one_shot = GameState::with_config(0, cfg.clone());
            one_shot.catch_up(gap);

            let mut stepped = GameState::with_config(0, cfg);
            let step = if gap > 7200 { 60 } else { 1 };
            let mut t = 0u64;
            while t + step < gap {
                t += step;
                stepped.catch_up(t);
            }
            stepped.catch_up(gap);

            assert_eq!(one_shot.last_tick_unix, gap, "trial {trial}");
            close(one_shot.resources.metal, stepped.resources.metal);
            close(one_shot.resources.crystal, stepped.resources.crystal);
            close(one_shot.resources.deuterium, stepped.resources.deuterium);
            assert!(
                one_shot.resources.metal <= one_shot.storage_cap(ResourceKind::Metal) + EPS
                    || fill > one_shot.storage_cap(ResourceKind::Metal),
                "cap violated trial {trial}"
            );
        }
    }

    fn splitmix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn cheap_spec(seconds: f64) -> crate::BuildingSpec {
        crate::BuildingSpec {
            metal_base: 0.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: seconds,
            time_growth: 1.0,
            prerequisites: vec![],
        }
    }

    fn queue_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.metal_storage.base = 100_000.0;
        cfg.crystal_storage.base = 100_000.0;
        cfg.deuterium_storage.base = 100_000.0;
        cfg.starting_resources = Resources {
            metal: 1_000.0,
            crystal: 1_000.0,
            deuterium: 1_000.0,
        };
        cfg.buildings.metal_mine = cheap_spec(1_800.0);
        cfg.buildings.solar_plant = cheap_spec(3_600.0);
        cfg.buildings.robotics_factory = cheap_spec(100.0);
        cfg.buildings.shipyard = crate::BuildingSpec {
            metal_base: 10.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: 100.0,
            time_growth: 1.0,
            prerequisites: vec![(BuildingKind::RoboticsFactory, 1)],
        };
        cfg.buildings.metal_storage = crate::BuildingSpec {
            metal_base: 50.0,
            crystal_base: 0.0,
            deuterium_base: 0.0,
            cost_growth: 1.0,
            base_seconds: 10.0,
            time_growth: 1.0,
            prerequisites: vec![],
        };
        cfg.max_build_queue = 5;
        cfg.robotics_speed_per_level = 1.0;
        cfg
    }

    #[test]
    fn mine_finishing_mid_gap_changes_production_from_completion_time() {
        let cfg = queue_config();
        let mut one_shot = GameState::with_config(0, cfg.clone());
        one_shot.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        assert_eq!(one_shot.buildings.metal_mine, 1);
        assert_eq!(one_shot.build_queue[0].completes_at, Some(1_800));

        one_shot.catch_up(3_600);
        assert_eq!(one_shot.buildings.metal_mine, 2);
        assert!(one_shot.build_queue.is_empty());
        // 0.5 h at 100/h then 0.5 h at 200/h, paid 0.
        close(one_shot.resources.metal, 1_000.0 + 50.0 + 100.0);

        let mut stepped = GameState::with_config(0, cfg);
        stepped.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        for t in 1..=3_600 {
            stepped.catch_up(t);
        }
        close(one_shot.resources.metal, stepped.resources.metal);
        assert_eq!(stepped.buildings.metal_mine, 2);
    }

    #[test]
    fn queue_caps_at_five_and_only_head_has_a_timer() {
        let mut state = GameState::with_config(0, queue_config());
        for _ in 0..5 {
            state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        }
        let err = state
            .enqueue_build(BuildingKind::MetalStorage, 0)
            .unwrap_err();
        assert_eq!(err, BuildError::QueueFull { cap: 5 });
        assert!(err.to_string().contains("full"));
        assert_eq!(state.build_queue.len(), 5);
        assert!(state.build_queue[0].completes_at.is_some());
        for job in &state.build_queue[1..] {
            assert!(job.completes_at.is_none());
            assert_eq!(job.building, BuildingKind::MetalStorage);
        }
    }

    #[test]
    fn insufficient_resources_report_missing_amounts() {
        let mut cfg = queue_config();
        cfg.starting_resources = Resources::ZERO;
        cfg.buildings.metal_storage.metal_base = 40.0;
        let mut state = GameState::with_config(0, cfg);
        let err = state
            .enqueue_build(BuildingKind::MetalStorage, 0)
            .unwrap_err();
        match &err {
            BuildError::InsufficientResources { missing, cost } => {
                close(missing.metal, 40.0);
                close(cost.metal, 40.0);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(err.to_string().contains("missing metal 40"));
        assert!(state.build_queue.is_empty());
    }

    #[test]
    fn missing_prerequisite_is_rejected_with_building_and_level() {
        let mut state = GameState::with_config(0, queue_config());
        let err = state.enqueue_build(BuildingKind::Shipyard, 0).unwrap_err();
        assert_eq!(
            err,
            BuildError::MissingPrerequisite {
                building: BuildingKind::RoboticsFactory,
                required: 1,
                actual: 0,
            }
        );
        assert!(err.to_string().contains("robotics factory level 1"));
    }

    #[test]
    fn robotics_level_shortens_the_next_job_when_it_becomes_head() {
        let mut cfg = queue_config();
        cfg.buildings.metal_mine = cheap_spec(100.0);
        let mut state = GameState::with_config(0, cfg);
        state
            .enqueue_build(BuildingKind::RoboticsFactory, 0)
            .unwrap();
        state.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        assert_eq!(state.build_queue[0].completes_at, Some(100));
        assert!(state.build_queue[1].completes_at.is_none());

        state.catch_up(100);
        assert_eq!(state.buildings.robotics_factory, 1);
        assert_eq!(state.build_queue.len(), 1);
        // 100 s / (1 + 1) = 50 s, starting at t=100.
        assert_eq!(state.build_queue[0].completes_at, Some(150));
        assert_eq!(state.build_queue[0].building, BuildingKind::MetalMine);
    }

    #[test]
    fn cancel_refunds_paid_resources_in_full() {
        let mut cfg = queue_config();
        cfg.buildings.metal_storage.metal_base = 80.0;
        cfg.starting_resources.metal = 100.0;
        let mut state = GameState::with_config(0, cfg);
        state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        close(state.resources.metal, 20.0);
        let refund = state.cancel_build(0, 0).unwrap();
        close(refund.metal, 80.0);
        close(state.resources.metal, 100.0);
        assert!(state.build_queue.is_empty());
        assert_eq!(state.buildings.metal_storage, 0);
    }

    #[test]
    fn canceling_an_earlier_job_reprojects_later_from_levels() {
        let mut cfg = queue_config();
        cfg.buildings.metal_storage.metal_base = 10.0;
        cfg.buildings.metal_storage.cost_growth = 2.0;
        let mut state = GameState::with_config(0, cfg);
        for _ in 0..3 {
            state.enqueue_build(BuildingKind::MetalStorage, 0).unwrap();
        }
        assert_eq!(
            state
                .build_queue
                .iter()
                .map(|j| j.from_level)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        state.cancel_build(0, 0).unwrap();
        assert_eq!(
            state
                .build_queue
                .iter()
                .map(|j| j.from_level)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        close(state.build_queue[0].paid.metal, 10.0);
        close(state.build_queue[1].paid.metal, 20.0);
        assert_eq!(state.build_queue[0].completes_at, Some(10));
    }

    #[test]
    fn canceling_a_queued_prerequisite_drops_the_dependent_job() {
        let mut cfg = queue_config();
        cfg.buildings.robotics_factory.metal_base = 5.0;
        cfg.buildings.shipyard.metal_base = 7.0;
        cfg.starting_resources.metal = 100.0;
        let mut state = GameState::with_config(0, cfg);
        state
            .enqueue_build(BuildingKind::RoboticsFactory, 0)
            .unwrap();
        state.enqueue_build(BuildingKind::Shipyard, 0).unwrap();
        assert_eq!(state.build_queue.len(), 2);
        close(state.resources.metal, 88.0);
        state.cancel_build(0, 0).unwrap();
        assert!(state.build_queue.is_empty());
        close(state.resources.metal, 100.0);
        assert_eq!(state.buildings.robotics_factory, 0);
        assert_eq!(state.buildings.shipyard, 0);
    }

    #[test]
    fn enqueue_after_advance_ignores_a_stale_timestamp() {
        let mut state = GameState::with_config(0, queue_config());
        state.catch_up(100);
        state.enqueue_build(BuildingKind::MetalMine, 50).unwrap();
        assert_eq!(state.last_tick_unix, 100);
        assert_eq!(state.build_queue[0].started_at, Some(100));
        assert_eq!(state.build_queue[0].completes_at, Some(1_900));
    }

    fn research_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.starting_resources = Resources {
            metal: 10_000.0,
            crystal: 10_000.0,
            deuterium: 10_000.0,
        };
        cfg.starting_buildings.research_lab = 1;
        cfg.techs = TechTree::parse(
            r#"
[[tech]]
id = "energy"
name = "Energy"
metal = 0.0
crystal = 10.0
deuterium = 0.0
cost_growth = 2.0
base_seconds = 1800.0
time_growth = 1.0
prereq_buildings = [{ building = "research_lab", level = 1 }]
modifiers = []

[[tech]]
id = "weapons"
name = "Weapons"
metal = 10.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 60.0
time_growth = 1.0
prereq_buildings = [{ building = "research_lab", level = 1 }]
modifiers = [{ kind = "weapon_damage", per_level = 0.1 }]

[[tech]]
id = "laser"
name = "Laser"
metal = 10.0
crystal = 0.0
deuterium = 0.0
cost_growth = 1.0
base_seconds = 60.0
time_growth = 1.0
prereq_techs = [{ id = "energy", level = 2 }]
modifiers = []
"#,
        )
        .unwrap();
        cfg
    }

    #[test]
    fn research_completes_during_offline_catch_up() {
        let mut one_shot = GameState::with_config(0, research_config());
        one_shot.start_research("energy", 0).unwrap();
        assert_eq!(one_shot.research.as_ref().unwrap().completes_at, 1_800);
        one_shot.catch_up(3_600);
        assert_eq!(one_shot.tech_level("energy"), 1);
        assert!(one_shot.research.is_none());

        let mut stepped = GameState::with_config(0, research_config());
        stepped.start_research("energy", 0).unwrap();
        for t in 1..=3_600 {
            stepped.catch_up(t);
        }
        assert_eq!(stepped.tech_level("energy"), 1);
        close(one_shot.resources.crystal, stepped.resources.crystal);
    }

    #[test]
    fn modifier_lookup_scales_with_tech_level() {
        let mut state = GameState::with_config(0, research_config());
        close(state.modifier(ModifierKind::WeaponDamage), 0.0);
        state.start_research("weapons", 0).unwrap();
        state.catch_up(60);
        close(state.modifier(ModifierKind::WeaponDamage), 0.1);
        state.start_research("weapons", 60).unwrap();
        state.catch_up(120);
        close(state.modifier(ModifierKind::WeaponDamage), 0.2);
    }

    #[test]
    fn unavailable_tech_lists_missing_prerequisites() {
        let state = GameState::with_config(0, research_config());
        let gaps = state.missing_research_prereqs("laser").unwrap();
        assert_eq!(
            gaps,
            vec![PrereqGap::Tech {
                id: "energy".into(),
                required: 2,
                actual: 0,
            }]
        );
        let err = GameState::with_config(0, research_config())
            .start_research("laser", 0)
            .unwrap_err();
        match err {
            ResearchError::MissingPrerequisites { gaps } => {
                assert!(gaps[0].to_string().contains("energy level 2"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn second_research_is_rejected_while_busy() {
        let mut state = GameState::with_config(0, research_config());
        state.start_research("energy", 0).unwrap();
        let err = state.start_research("weapons", 0).unwrap_err();
        assert_eq!(
            err,
            ResearchError::Busy {
                current: "energy".into(),
            }
        );
    }

    fn yard_state(now: u64) -> GameState {
        let mut state = GameState::new(now);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources.metal = 100_000.0;
        state.resources.crystal = 100_000.0;
        state.resources.deuterium = 100_000.0;
        state
    }

    #[test]
    fn ship_stats_scale_with_research_modifiers() {
        let mut state = GameState::new(0);
        let base = state.ship_stats(ShipKind::LightFighter);
        let spec = state.config.ships.get(ShipKind::LightFighter);
        close(base.attack, spec.attack);
        close(base.speed, spec.speed);
        state.tech_levels.insert("weapons".into(), 2);
        state.tech_levels.insert("shields".into(), 1);
        state.tech_levels.insert("armour".into(), 3);
        state.tech_levels.insert("combustion_drive".into(), 4);
        let scaled = state.ship_stats(ShipKind::LightFighter);
        close(scaled.attack, spec.attack * 1.2);
        close(scaled.shield, spec.shield * 1.1);
        close(scaled.hull, spec.hull * 1.3);
        close(scaled.speed, spec.speed * 1.4);
        close(scaled.cargo, spec.cargo);
        close(scaled.fuel, spec.fuel);
    }

    #[test]
    fn locked_ship_lists_missing_techs() {
        let mut state = GameState::new(0);
        state.buildings.shipyard = 5;
        state.resources.metal = 100_000.0;
        state.resources.crystal = 100_000.0;
        state.resources.deuterium = 100_000.0;
        let gaps = state.missing_ship_prereqs(ShipKind::Cruiser);
        assert!(
            gaps.iter().any(|g| matches!(
                g,
                PrereqGap::Tech {
                    id,
                    required: 4,
                    ..
                } if id == "impulse_drive"
            )),
            "{gaps:?}"
        );
        let err = state.enqueue_ships(ShipKind::Cruiser, 1, 0).unwrap_err();
        match err {
            ShipError::MissingPrerequisites { gaps } => {
                assert!(!gaps.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn ship_batch_completes_one_ship_at_a_time_across_an_offline_gap() {
        let mut gap = yard_state(0);
        gap.enqueue_ships(ShipKind::LightFighter, 5, 0).unwrap();
        let unit = gap.ship_time_secs(ShipKind::LightFighter);
        gap.catch_up(unit * 2);
        assert_eq!(gap.fleet.get(ShipKind::LightFighter), 2);
        assert_eq!(gap.ship_queue[0].remaining, 3);

        let mut step = yard_state(0);
        step.enqueue_ships(ShipKind::LightFighter, 5, 0).unwrap();
        for t in 1..=unit * 2 {
            step.catch_up(t);
        }
        assert_eq!(step.fleet, gap.fleet);
        assert_eq!(step.ship_queue, gap.ship_queue);
        assert_eq!(step.resources, gap.resources);
        assert_eq!(step.last_tick_unix, gap.last_tick_unix);
    }

    #[test]
    fn unaffordable_ship_batch_reports_missing_amounts() {
        let mut state = GameState::new(0);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources = Resources::ZERO;
        let err = state
            .enqueue_ships(ShipKind::LightFighter, 2, 0)
            .unwrap_err();
        match err {
            ShipError::InsufficientResources { missing, cost } => {
                assert!(missing.metal > 0.0);
                close(
                    cost.metal,
                    state.config.ships.get(ShipKind::LightFighter).metal * 2.0,
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn shipyard_is_required_to_queue_ships() {
        let mut state = GameState::new(0);
        state.resources.metal = 100_000.0;
        state.resources.crystal = 100_000.0;
        let err = state.enqueue_ships(ShipKind::SmallCargo, 1, 0).unwrap_err();
        assert_eq!(
            err,
            ShipError::NeedShipyard {
                required: 2,
                actual: 0,
            }
        );
    }

    fn scout_fleet() -> Fleet {
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 2);
        ships
    }

    fn far_dest() -> Coord {
        Coord {
            galaxy: 2,
            system: 1,
            position: 1,
        }
    }

    fn flying_state(now: u64) -> GameState {
        let mut state = GameState::new(now);
        state.fleet.add(ShipKind::LightFighter, 4);
        state.resources.deuterium = 10_000.0;
        state
    }

    #[test]
    fn recall_mid_outbound_returns_in_elapsed_time() {
        let mut state = flying_state(0);
        state
            .launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        let id = state.missions[0].id;
        let outbound = state.missions[0].outbound_secs;
        assert!(outbound > 1);
        state.recall_fleet(id, 1).unwrap();
        assert_eq!(state.missions[0].phase, MissionPhase::Return);
        assert_eq!(state.missions[0].phase_ends_at, 2);
        state.catch_up(2);
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::LightFighter), 4);
    }

    #[test]
    fn arrival_and_return_resolve_across_an_offline_gap() {
        let mut gap = flying_state(0);
        gap.launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        let outbound = gap.missions[0].outbound_secs;
        gap.catch_up(outbound * 2);
        assert!(gap.missions.is_empty());
        assert_eq!(gap.fleet.get(ShipKind::LightFighter), 4);

        let mut step = flying_state(0);
        step.launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        for t in 1..=outbound * 2 {
            step.catch_up(t);
        }
        assert_eq!(step.fleet, gap.fleet);
        assert_eq!(step.missions, gap.missions);
        assert_eq!(step.resources.deuterium, gap.resources.deuterium);
        assert_eq!(step.last_tick_unix, gap.last_tick_unix);
    }

    #[test]
    fn fleet_slots_follow_computer_technology() {
        let mut state = flying_state(0);
        assert_eq!(state.fleet_slots(), 1);
        state
            .launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        let err = state
            .launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap_err();
        assert_eq!(err, FleetError::NoSlots { used: 1, cap: 1 });
        state.tech_levels.insert("computer".into(), 1);
        assert_eq!(state.fleet_slots(), 2);
        state
            .launch_fleet(scout_fleet(), far_dest(), 100, 0, 0)
            .unwrap();
        assert_eq!(state.missions.len(), 2);
    }

    #[test]
    fn disabled_raids_do_not_resolve_a_stale_schedule() {
        let mut state = GameState::with_config(0, test_config());
        state.next_raid_at = Some(10);
        state.resources.metal = 500.0;
        state.catch_up(100);
        assert!(state.events.is_empty());
        close(state.resources.metal, 500.0 + 100.0 * 100.0 / 3600.0);
        assert_eq!(state.next_raid_at, Some(10));
        assert_eq!(state.last_tick_unix, 100);
    }

    #[test]
    fn starter_survives_the_first_raid() {
        let mut state = GameState::new(0);
        assert_eq!(state.next_raid_at, Some(3600));
        assert_eq!(state.raid_countdown_secs(), Some(3600));
        state.catch_up(3599);
        assert!(state.events.is_empty());
        assert_eq!(state.fleet.total(), 0);
        state.catch_up(3600);
        assert_eq!(state.events.len(), 1);
        assert!(state.events[0].text.contains("raid:"));
        assert!(state.events[0].text.contains("plunder"));
        assert!(state.resources.metal > 0.0);
        assert!(state.resources.crystal > 0.0);
        assert_eq!(state.next_raid_at, Some(3600 + 8 * 3600));
    }

    #[test]
    fn trigger_raid_uses_the_same_resolver_as_the_scheduler() {
        let mut bridged = GameState::new(0);
        let report = bridged.trigger_raid(0);
        assert_eq!(report.winner, CombatWinner::Attacker);
        assert_eq!(bridged.events.len(), 1);
        assert!(bridged.events[0].text.contains("lost"));
        assert!(bridged.events[0].text.contains("pirates lost"));
        assert_eq!(bridged.next_raid_at, Some(8 * 3600));
        let mut again = GameState::new(0);
        again.trigger_raid(0);
        assert_eq!(again.fleet, bridged.fleet);
        assert_eq!(again.events, bridged.events);
        assert_eq!(again.rng_seed, bridged.rng_seed);
    }

    #[test]
    fn trigger_raid_at_the_scheduled_instant_resolves_once() {
        let mut state = GameState::new(0);
        state.trigger_raid(3600);
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.events[0].at, 3600);
        assert_eq!(state.next_raid_at, Some(3600 + 8 * 3600));
    }

    #[test]
    fn raid_during_offline_gap_matches_step_by_step() {
        let cfg = BalanceConfig {
            planet_types: crate::PlanetTypeCatalog {
                rocky: crate::PlanetTypeBonus::ZERO,
                ice: crate::PlanetTypeBonus::ZERO,
                gas_giant: crate::PlanetTypeBonus::ZERO,
            },
            ..BalanceConfig::default()
        };
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.catch_up(7200);
        let mut step = GameState::with_config(0, cfg);
        for t in 1..=7200 {
            step.catch_up(t);
        }
        close(gap.resources.metal, step.resources.metal);
        close(gap.resources.crystal, step.resources.crystal);
        assert_eq!(gap.fleet, step.fleet);
        assert_eq!(gap.events, step.events);
        assert_eq!(gap.rng_seed, step.rng_seed);
        assert_eq!(gap.next_raid_at, step.next_raid_at);
        assert_eq!(gap.last_tick_unix, 7200);
    }

    #[test]
    fn no_raid_lands_before_configured_first_delay() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.first_delay_secs = 7200;
        cfg.raid.min_warning_secs = 1800;
        let mut state = GameState::with_config(100, cfg);
        assert_eq!(state.next_raid_at, Some(7300));
        assert_eq!(state.raid_countdown_secs(), None);
        state.catch_up(5500);
        assert!(state.events.is_empty());
        assert_eq!(state.raid_countdown_secs(), Some(1800));
        state.catch_up(7300);
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.events[0].at, 7300);
    }

    #[test]
    fn plunder_cap_is_applied_on_a_lost_raid() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.first_delay_secs = 10;
        cfg.raid.min_warning_secs = 10;
        cfg.raid.plunder_share = 1.0;
        cfg.raid.plunder_cap = Resources {
            metal: 40.0,
            crystal: 30.0,
            deuterium: 5.0,
        };
        cfg.starting_resources = Resources {
            metal: 500.0,
            crystal: 500.0,
            deuterium: 100.0,
        };
        cfg.starting_buildings.metal_mine = 0;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_buildings.solar_plant = 0;
        let mut state = GameState::with_config(0, cfg);
        state.catch_up(10);
        close(state.resources.metal, 460.0);
        close(state.resources.crystal, 470.0);
        close(state.resources.deuterium, 95.0);
        assert!(state.events[0].text.contains("plunder M40 C30 D5"));
    }

    fn colony_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.techs = TechTree::parse(
            r#"
[[tech]]
id = "colonization"
name = "Colonization"
metal = 1000.0
crystal = 4000.0
deuterium = 2000.0
cost_growth = 2.0
base_seconds = 600.0
time_growth = 2.0
prereq_buildings = []
prereq_techs = []
modifiers = [{ kind = "colony_slots", per_level = 1.0 }]
"#,
        )
        .expect("test colonization tech");
        cfg
    }

    fn colony_ready(now: u64) -> GameState {
        let mut state = GameState::with_config(now, colony_config());
        state.resources.deuterium = 10_000.0;
        state.fleet.add(ShipKind::ColonyShip, 1);
        state
    }

    fn empty_slot() -> Coord {
        Coord {
            galaxy: 1,
            system: 1,
            position: 14,
        }
    }

    #[test]
    fn colony_ship_and_colonization_tech_are_in_config() {
        let mut state = GameState::new(0);
        let spec = state.config.ships.get(ShipKind::ColonyShip);
        assert_eq!(spec.kind, ShipKind::ColonyShip);
        assert!(spec.shipyard >= 1);
        let tech = state.config.techs.get("colonization").unwrap();
        assert!(
            tech.modifiers
                .iter()
                .any(|m| m.kind == ModifierKind::ColonySlots && m.per_level == 1.0)
        );
        assert_eq!(state.colony_cap(), 1);
        state.tech_levels.insert("colonization".into(), 2);
        assert_eq!(state.colony_cap(), 3);
        let mut empty = GameState::with_config(0, colony_config());
        empty.resources.deuterium = 10_000.0;
        assert_eq!(
            empty.launch_colony(empty_slot(), 100, 0),
            Err(FleetError::NeedColonyShip)
        );
    }

    #[test]
    fn over_cap_or_occupied_returns_the_colony_ship() {
        let mut state = colony_ready(0);
        assert_eq!(state.colony_cap(), 1);
        state.launch_colony(empty_slot(), 100, 0).unwrap();
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(outbound * 2);
        assert!(state.colonies.is_empty());
        assert_eq!(state.fleet.get(ShipKind::ColonyShip), 1);
        assert!(state.missions.is_empty());

        state.tech_levels.insert("colonization".into(), 1);
        let now = state.last_tick_unix;
        state.launch_colony(empty_slot(), 100, now).unwrap();
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(now + outbound * 2);
        assert_eq!(state.colonies.len(), 1);
        assert_eq!(state.fleet.get(ShipKind::ColonyShip), 0);

        state.fleet.add(ShipKind::ColonyShip, 1);
        state.launch_colony(empty_slot(), 100, 0).unwrap();
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(state.last_tick_unix + outbound * 2);
        assert_eq!(state.colonies.len(), 1);
        assert_eq!(state.fleet.get(ShipKind::ColonyShip), 1);
    }

    #[test]
    fn planet_type_and_size_follow_the_galaxy_seed() {
        let mut a = colony_ready(0);
        a.galaxy_seed = 11;
        a.tech_levels.insert("colonization".into(), 1);
        a.launch_colony(empty_slot(), 100, 0).unwrap();
        let outbound = a.missions[0].outbound_secs;
        a.catch_up(outbound * 2);
        let expected = planet_profile(11, empty_slot());
        assert_eq!(a.colonies[0].kind, expected.0);
        assert_eq!(a.colonies[0].size, expected.1);
        let mut b = colony_ready(0);
        b.galaxy_seed = 11;
        b.tech_levels.insert("colonization".into(), 1);
        b.launch_colony(empty_slot(), 100, 0).unwrap();
        b.catch_up(outbound * 2);
        assert_eq!(a.colonies[0].kind, b.colonies[0].kind);
        assert_eq!(a.colonies[0].size, b.colonies[0].size);
    }

    #[test]
    fn type_bonuses_go_through_modifier_lookup() {
        let mut cfg = test_config();
        cfg.planet_types.gas_giant.deuterium = 0.5;
        cfg.planet_types.gas_giant.energy = 0.0;
        let state = GameState::with_config(0, cfg);
        close(
            state.modifier_for(PlanetType::GasGiant, ModifierKind::DeuteriumOutput),
            0.5,
        );
        close(
            state.modifier_for(PlanetType::Rocky, ModifierKind::DeuteriumOutput),
            0.0,
        );
    }

    #[test]
    fn offline_arrival_founds_the_colony_at_the_landing_time() {
        let mut gap = colony_ready(0);
        gap.tech_levels.insert("colonization".into(), 1);
        gap.launch_colony(empty_slot(), 100, 0).unwrap();
        let outbound = gap.missions[0].outbound_secs;
        gap.catch_up(outbound * 10);
        assert_eq!(gap.colonies.len(), 1);
        assert_eq!(gap.colonies[0].coord, empty_slot());

        let mut step = colony_ready(0);
        step.tech_levels.insert("colonization".into(), 1);
        step.launch_colony(empty_slot(), 100, 0).unwrap();
        for t in 1..=outbound * 10 {
            step.catch_up(t);
        }
        assert_eq!(step.colonies, gap.colonies);
        assert_eq!(step.fleet, gap.fleet);
        assert_eq!(step.last_tick_unix, gap.last_tick_unix);
    }

    fn activity_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.activity.enabled = true;
        cfg.activity.pr_merged.cooldown_secs = 60;
        cfg.activity.pr_merged.daily_cap = 2;
        cfg.activity.pr_merged.payout_hours = 1.0;
        cfg.activity.ci_green.cooldown_secs = 0;
        cfg.activity.ci_green.daily_cap = 10;
        cfg.activity.ci_green.boost_factor = 1.0;
        cfg.activity.ci_green.boost_secs = 1800;
        cfg.activity.ci_red.cooldown_secs = 0;
        cfg.activity.ci_red.daily_cap = 10;
        cfg.activity.agent_long.min_duration_secs = 100;
        cfg.activity.agent_long.boost_factor = 1.0;
        cfg.activity.agent_long.boost_secs = 3_600;
        cfg.activity.agent_long.cooldown_secs = 0;
        cfg.activity.agent_stuck.min_duration_secs = 50;
        cfg.raid = crate::RaidConfig::inactive();
        cfg
    }

    #[test]
    fn merged_pr_pays_an_hour_of_production_at_the_event_timestamp() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg.clone());
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 1800 }, 10_000),
            ActivityApply::Applied
        );
        close(state.resources.metal, 50.0 + 100.0);
        close(state.resources.crystal, 25.0 + 50.0);
        assert_eq!(state.last_tick_unix, 1800);
        assert!(state.events.iter().any(|e| e.text.contains("merged PR")));
    }

    #[test]
    fn historical_events_use_their_own_timestamps_for_cooldowns() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 100 }, 3600),
            ActivityApply::Applied
        );
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 160 }, 3600),
            ActivityApply::Applied
        );
        assert_eq!(state.last_tick_unix, 160);
    }

    #[test]
    fn events_before_last_tick_are_skipped() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        state.catch_up(3600);
        let metal = state.resources.metal;
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 100 }, 4000),
            ActivityApply::Skipped
        );
        close(state.resources.metal, metal);
        assert_eq!(state.last_tick_unix, 3600);
    }

    #[test]
    fn activity_caps_and_cooldowns_drop_a_burst() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        let mut applied = 0;
        for i in 0..10 {
            if state.apply_activity(ActivityPulse::PrMerged { ts: i }, 10_000)
                == ActivityApply::Applied
            {
                applied += 1;
            }
        }
        assert_eq!(applied, 1);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 60 }, 10_000),
            ActivityApply::Applied
        );
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 120 }, 10_000),
            ActivityApply::Skipped
        );
    }

    #[test]
    fn disabled_mapping_is_a_no_op() {
        let mut cfg = activity_config();
        cfg.activity.pr_merged.enabled = false;
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 10 }, 10_000),
            ActivityApply::Skipped
        );
        close(state.resources.metal, 0.0);
    }

    #[test]
    fn production_boost_splits_catch_up_at_expiry() {
        let cfg = activity_config();
        let mut gap = GameState::with_config(0, cfg.clone());
        gap.apply_activity(
            ActivityPulse::WorkflowFinished {
                ts: 0,
                success: true,
            },
            10_000,
        );
        gap.catch_up(3600);
        let mut step = GameState::with_config(0, cfg);
        step.apply_activity(
            ActivityPulse::WorkflowFinished {
                ts: 0,
                success: true,
            },
            10_000,
        );
        for t in 1..=3600 {
            step.catch_up(t);
        }
        close(gap.resources.metal, step.resources.metal);
        close(gap.resources.crystal, step.resources.crystal);
        // 1800s at 2x then 1800s at 1x: metal 100/h -> 50+50 = 100?
        // 0.5h * 200 + 0.5h * 100 = 100+50 = 150
        close(gap.resources.metal, 150.0);
    }

    #[test]
    fn long_agent_run_shortens_research() {
        let mut cfg = research_config();
        cfg.activity = activity_config().activity;
        let mut state = GameState::with_config(0, cfg);
        state.start_research("energy", 0).unwrap();
        let before = state.research.as_ref().unwrap().completes_at;
        assert_eq!(
            state.apply_activity(
                ActivityPulse::AgentFinished {
                    ts: 0,
                    duration_secs: 100,
                },
                10_000,
            ),
            ActivityApply::Applied
        );
        let after = state.research.as_ref().unwrap().completes_at;
        assert!(after < before);
        assert_eq!(
            state.apply_activity(
                ActivityPulse::AgentFinished {
                    ts: 1,
                    duration_secs: 10,
                },
                10_000,
            ),
            ActivityApply::Skipped
        );
    }

    #[test]
    fn red_ci_does_not_consume_limits_when_raids_are_off() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(
                ActivityPulse::WorkflowFinished {
                    ts: 10,
                    success: false,
                },
                10_000,
            ),
            ActivityApply::Skipped
        );
        assert!(state.activity.last_applied[ActivityChannel::CiRed.index()].is_none());
    }

    #[test]
    fn red_ci_uses_the_raid_api() {
        let mut cfg = activity_config();
        cfg.raid = crate::RaidConfig::default_active();
        cfg.raid.first_delay_secs = 10_000;
        let mut state = GameState::with_config(0, cfg);
        let events_before = state.events.len();
        assert_eq!(
            state.apply_activity(
                ActivityPulse::WorkflowFinished {
                    ts: 10,
                    success: false,
                },
                10_000,
            ),
            ActivityApply::Applied
        );
        assert!(state.events.len() > events_before);
        assert!(state.events.iter().any(|e| e.text.starts_with("raid:")));
    }

    #[test]
    fn older_day_events_do_not_reset_today_caps() {
        let mut cfg = activity_config();
        cfg.activity.pr_merged.cooldown_secs = 0;
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 100 }, 86_400 * 2),
            ActivityApply::Applied
        );
        let today = 86_400;
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: today }, today + 10),
            ActivityApply::Applied
        );
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: today + 60 }, today + 70),
            ActivityApply::Applied
        );
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: today + 120 }, today + 130),
            ActivityApply::Skipped
        );
    }

    #[test]
    fn future_activity_timestamps_are_skipped() {
        let cfg = activity_config();
        let mut state = GameState::with_config(0, cfg);
        assert_eq!(
            state.apply_activity(ActivityPulse::PrMerged { ts: 50 }, 10),
            ActivityApply::Skipped
        );
        assert_eq!(state.last_tick_unix, 0);
        close(state.resources.metal, 0.0);
    }

    fn expedition_state(now: u64) -> GameState {
        let mut cfg = test_config();
        cfg.expedition.min_hold_secs = 10;
        cfg.expedition.max_hold_secs = 10_000;
        cfg.expedition.delay_secs = 50;
        cfg.raid = crate::RaidConfig::inactive();
        cfg.metal_storage.base = 1_000_000.0;
        cfg.crystal_storage.base = 1_000_000.0;
        cfg.deuterium_storage.base = 1_000_000.0;
        let mut state = GameState::with_config(now, cfg);
        state.fleet.add(ShipKind::LightFighter, 10);
        state.resources.deuterium = 10_000.0;
        state
    }

    #[test]
    fn expedition_result_lands_in_the_event_log() {
        let mut state = expedition_state(0);
        state.launch_expedition(scout_fleet(), 10, 100, 0).unwrap();
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(outbound);
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text.starts_with("expedition:")),
            "{:?}",
            state.events
        );
    }

    #[test]
    fn reloading_the_save_does_not_change_a_pending_expedition() {
        let mut original = expedition_state(0);
        original
            .launch_expedition(scout_fleet(), 10, 100, 0)
            .unwrap();
        let mut conn = crate::open_save(std::path::Path::new(":memory:")).unwrap();
        crate::save_state(&mut conn, &original).unwrap();
        let mut loaded = crate::load_state(&conn, original.config.clone()).unwrap();
        assert_eq!(
            loaded.missions[0].expedition_seed,
            original.missions[0].expedition_seed
        );
        let outbound = original.missions[0].outbound_secs;
        let hold = original.missions[0].hold_secs;
        original.rng_seed = 0xDEAD_BEEF_DEAD_BEEF;
        original.catch_up(outbound + hold + outbound);
        loaded.catch_up(outbound + hold + outbound);
        assert_eq!(original.events, loaded.events);
        assert_eq!(original.fleet, loaded.fleet);
        close(original.resources.metal, loaded.resources.metal);
        close(original.resources.crystal, loaded.resources.crystal);
        close(original.resources.deuterium, loaded.resources.deuterium);
    }

    #[test]
    fn expedition_slots_follow_astrophysics() {
        let mut state = expedition_state(0);
        assert_eq!(state.expedition_slots(), 1);
        state.launch_expedition(scout_fleet(), 10, 100, 0).unwrap();
        let err = state
            .launch_expedition(scout_fleet(), 10, 100, 0)
            .unwrap_err();
        assert_eq!(err, FleetError::NoExpeditionSlots { used: 1, cap: 1 });
        state.tech_levels.insert("astrophysics".into(), 1);
        state.config.flight.base_fleet_slots = 2;
        state.config.techs = crate::TechTree::parse(
            r#"
[[tech]]
id = "astrophysics"
name = "Astrophysics"
metal = 1.0
crystal = 1.0
deuterium = 1.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_buildings = []
prereq_techs = []
modifiers = [{ kind = "expedition_slots", per_level = 1.0 }]
"#,
        )
        .unwrap();
        assert_eq!(state.expedition_slots(), 2);
        state.launch_expedition(scout_fleet(), 10, 100, 0).unwrap();
        assert_eq!(
            state
                .missions
                .iter()
                .filter(|m| m.expedition_seed.is_some())
                .count(),
            2
        );
    }

    fn neighbor() -> Coord {
        Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        }
    }

    fn plant_colony(state: &mut GameState, coord: Coord) {
        state.colonies.push(Colony {
            coord,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources {
                metal: 100_000.0,
                crystal: 100_000.0,
                deuterium: 100_000.0,
            },
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            build_queue: Vec::new(),
        });
    }

    fn cargo_fleet() -> Fleet {
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        ships
    }

    fn logistics_config() -> BalanceConfig {
        let mut cfg = test_config();
        cfg.techs = TechTree::parse(
            r#"
[[tech]]
id = "logistics"
name = "Logistics"
metal = 1.0
crystal = 1.0
deuterium = 1.0
cost_growth = 1.0
base_seconds = 1.0
time_growth = 1.0
prereq_buildings = []
prereq_techs = []
modifiers = [{ kind = "logistics", per_level = 1.0 }]
"#,
        )
        .expect("test logistics tech");
        cfg.flight.base_fleet_slots = 4;
        cfg.metal_storage.base = 1_000_000.0;
        cfg.crystal_storage.base = 1_000_000.0;
        cfg.deuterium_storage.base = 1_000_000.0;
        cfg
    }

    #[test]
    fn colony_and_home_queues_advance_independently() {
        let mut state = GameState::with_config(0, test_config());
        state.resources = Resources {
            metal: 100_000.0,
            crystal: 100_000.0,
            deuterium: 100_000.0,
        };
        plant_colony(&mut state, neighbor());
        state.colonies[0].buildings.robotics_factory = 1;
        state
            .enqueue_build_on(state.home, BuildingKind::MetalMine, 0)
            .unwrap();
        state
            .enqueue_build_on(neighbor(), BuildingKind::MetalMine, 0)
            .unwrap();
        let home_done = state.build_queue[0].completes_at.unwrap();
        let colony_done = state.colonies[0].build_queue[0].completes_at.unwrap();
        assert!(colony_done < home_done);
        state.catch_up(colony_done);
        assert_eq!(state.colonies[0].buildings.metal_mine, 1);
        assert!(state.colonies[0].build_queue.is_empty());
        assert_eq!(state.buildings.metal_mine, 1);
        assert_eq!(state.build_queue.len(), 1);
        state.catch_up(home_done);
        assert_eq!(state.buildings.metal_mine, 2);
        assert!(state.build_queue.is_empty());
    }

    #[test]
    fn transport_rejects_cargo_over_capacity_and_pays_fuel() {
        let mut state = GameState::with_config(0, logistics_config());
        state.resources.metal = 10_000.0;
        state.resources.crystal = 0.0;
        state.resources.deuterium = 50.0;
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 1);
        let over = Resources {
            metal: 5_001.0,
            crystal: 0.0,
            deuterium: 0.0,
        };
        let err = state
            .launch_transport(state.home, neighbor(), cargo_fleet(), over, 100, 0)
            .unwrap_err();
        match err {
            FleetError::CargoExceedsCapacity { need, cap } => {
                close(need, 5_001.0);
                close(cap, state.cargo_capacity(cargo_fleet()));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
        close(state.resources.deuterium, 50.0);

        state.resources.metal = 50.0;
        let short_metal = Resources {
            metal: 200.0,
            crystal: 0.0,
            deuterium: 0.0,
        };
        match state.launch_transport(state.home, neighbor(), cargo_fleet(), short_metal, 100, 0) {
            Err(FleetError::InsufficientCargo { missing }) => close(missing.metal, 150.0),
            other => panic!("{other:?}"),
        }
        state.resources.metal = 10_000.0;

        let cargo = Resources {
            metal: 100.0,
            crystal: 0.0,
            deuterium: 0.0,
        };
        let id = state
            .launch_transport(state.home, neighbor(), cargo_fleet(), cargo, 100, 0)
            .unwrap();
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 0);
        close(state.resources.metal, 9_900.0);
        let fuel = state.missions[0].fuel_paid;
        assert!(fuel > 0.0);
        close(state.resources.deuterium, 50.0 - fuel);
        let outbound = state.missions[0].outbound_secs;
        state.catch_up(outbound);
        close(state.colonies[0].resources.metal, 100_100.0);
        assert!(state.missions[0].transport);
        assert_eq!(state.missions[0].id, id);
        state.catch_up(outbound * 2);
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
    }

    #[test]
    fn recalling_an_outbound_transport_returns_its_cargo() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.metal = 10_000.0;
        state.resources.deuterium = 50.0;
        let cargo = Resources {
            metal: 100.0,
            crystal: 0.0,
            deuterium: 0.0,
        };
        let id = state
            .launch_transport(state.home, neighbor(), cargo_fleet(), cargo, 100, 0)
            .unwrap();
        let fuel = state.missions[0].fuel_paid;
        let metal_rate = state.production_per_hour(ResourceKind::Metal);
        state.recall_fleet(id, 0).unwrap();
        state.catch_up(1);
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
        close(state.resources.metal, 10_000.0 + metal_rate / 3600.0);
        close(state.resources.deuterium, 50.0 - fuel);
        close(state.colonies[0].resources.metal, 100_000.0);
    }

    #[test]
    fn negative_or_non_finite_cargo_is_rejected() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 2);
        state.resources.metal = 10_000.0;
        state.resources.deuterium = 50.0;
        let bad = Resources {
            metal: -10.0,
            crystal: 0.0,
            deuterium: 0.0,
        };
        assert_eq!(
            state.launch_transport(state.home, neighbor(), cargo_fleet(), bad, 100, 0),
            Err(FleetError::InvalidCargo)
        );
        close(state.resources.metal, 10_000.0);
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 2);
        assert_eq!(
            state.schedule_route(state.home, neighbor(), cargo_fleet(), bad, 10, 0),
            Err(FleetError::InvalidCargo)
        );
        let over = Resources {
            metal: 5_001.0,
            crystal: 0.0,
            deuterium: 0.0,
        };
        match state.schedule_route(state.home, neighbor(), cargo_fleet(), over, 10, 0) {
            Err(FleetError::CargoExceedsCapacity { .. }) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(
            state.schedule_route(
                state.home,
                neighbor(),
                Fleet::default(),
                Resources::ZERO,
                10,
                0
            ),
            Err(FleetError::EmptyFleet)
        );
        assert!(state.routes.is_empty());
    }

    #[test]
    fn repeating_route_launches_when_cargo_and_ships_are_ready() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.metal = 10_000.0;
        state.resources.deuterium = 50.0;
        let cargo = Resources {
            metal: 100.0,
            crystal: 0.0,
            deuterium: 0.0,
        };
        let id = state
            .schedule_route(state.home, neighbor(), cargo_fleet(), cargo, 10, 0)
            .unwrap();
        let metal_rate = state.production_per_hour(ResourceKind::Metal);
        state.catch_up(1);
        assert_eq!(state.missions.len(), 1);
        assert!(state.missions[0].transport);
        close(
            state.resources.metal,
            10_000.0 - 100.0 + metal_rate / 3600.0,
        );
        close(state.colonies[0].resources.metal, 100_100.0);
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 0);
        assert_eq!(state.routes[0].id, id);
        assert_eq!(state.routes[0].next_at, 10);
    }

    #[test]
    fn repeating_route_skips_when_cargo_is_short_and_can_cancel() {
        let mut state = GameState::with_config(0, logistics_config());
        plant_colony(&mut state, neighbor());
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.metal = 10.0;
        state.resources.deuterium = 50.0;
        let cargo = Resources {
            metal: 100.0,
            crystal: 0.0,
            deuterium: 0.0,
        };
        let id = state
            .schedule_route(state.home, neighbor(), cargo_fleet(), cargo, 10, 0)
            .unwrap();
        state.catch_up(1);
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text.contains(&format!("route {id}: skipped"))),
            "{:?}",
            state.events
        );
        assert!(state.missions.is_empty());
        assert_eq!(state.fleet.get(ShipKind::SmallCargo), 1);
        state.cancel_route(id).unwrap();
        let events = state.events.len();
        state.catch_up(100);
        assert_eq!(state.events.len(), events);
        assert!(state.routes.is_empty());
    }

    #[test]
    fn enqueue_build_many_needs_logistics_and_reports_unaffordable() {
        let mut state = GameState::with_config(0, logistics_config());
        state.resources = Resources {
            metal: 100_000.0,
            crystal: 100_000.0,
            deuterium: 100_000.0,
        };
        plant_colony(&mut state, neighbor());
        state.colonies[0].resources = Resources::ZERO;
        let coords = [state.home, neighbor()];
        let blocked = state.enqueue_build_many(BuildingKind::MetalMine, &coords, 0);
        assert!(
            blocked
                .iter()
                .all(|(_, r)| matches!(r, Err(BuildError::NeedLogistics)))
        );
        assert!(state.build_queue.is_empty());
        assert!(state.colonies[0].build_queue.is_empty());

        state.tech_levels.insert("logistics".into(), 1);
        let results = state.enqueue_build_many(BuildingKind::MetalMine, &coords, 0);
        assert!(results[0].1.is_ok(), "{:?}", results[0].1);
        assert!(
            matches!(results[1].1, Err(BuildError::InsufficientResources { .. })),
            "{:?}",
            results[1].1
        );
        assert_eq!(state.build_queue.len(), 1);
        assert!(state.colonies[0].build_queue.is_empty());
    }
}
