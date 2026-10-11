//! Planet stockpiles, building levels and offline catch-up.

use crate::balance::BalanceConfig;
use crate::buildings::{BuildError, BuildJob, BuildingKind, Buildings};
use crate::combat::{CombatWinner, plunder_from, resolve_combat, stats_from_catalog};
use crate::fleets::{Coord, FleetError, Mission, MissionPhase, distance, flight_secs, fuel_cost};
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
    pub missions: Vec<Mission>,
    pub next_mission_id: u64,
    pub next_raid_at: Option<u64>,
    pub events: Vec<GameEvent>,
    pub last_tick_unix: u64,
    /// Seeded RNG state. Combat and raids consume it; the save stores the current value.
    pub rng_seed: u64,
    pub config: BalanceConfig,
}

impl GameState {
    pub fn new(now_unix: u64) -> Self {
        Self::with_config(now_unix, BalanceConfig::default())
    }

    pub fn with_config(now_unix: u64, config: BalanceConfig) -> Self {
        Self {
            resources: config.starting_resources,
            buildings: config.starting_buildings,
            build_queue: Vec::new(),
            tech_levels: BTreeMap::new(),
            research: None,
            fleet: Fleet::default(),
            ship_queue: Vec::new(),
            home: Coord::HOME,
            missions: Vec::new(),
            next_mission_id: 0,
            next_raid_at: if config.raid.enabled {
                Some(now_unix.saturating_add(config.raid.first_delay_secs))
            } else {
                None
            },
            events: Vec::new(),
            last_tick_unix: now_unix,
            rng_seed: 0,
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
            let build_at = self.build_queue.first().and_then(|job| job.completes_at);
            let research_at = self.research.as_ref().map(|job| job.completes_at);
            let ship_at = self.ship_queue.first().and_then(|job| job.completes_at);
            let mission_at = self.missions.iter().map(|m| m.phase_ends_at).min();
            let raid_at = if self.config.raid.enabled {
                self.next_raid_at
            } else {
                None
            };
            let Some(done_at) = min_time(
                min_time(
                    min_time(min_time(build_at, research_at), ship_at),
                    mission_at,
                ),
                raid_at,
            ) else {
                self.produce_until(now_unix);
                return;
            };
            if done_at > now_unix {
                self.produce_until(now_unix);
                return;
            }
            self.produce_until(done_at);
            if build_at == Some(done_at) {
                let job = self.build_queue.remove(0);
                self.buildings.increment(job.building);
                self.start_head(done_at);
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
            if raid_at == Some(done_at) {
                self.resolve_raid_at(done_at);
            }
        }
    }

    /// Queue an upgrade. Resources are paid immediately; only the head runs.
    /// Waiting jobs get their timer when they become head, so a robotics
    /// completion in front of them shortens their duration.
    pub fn enqueue_build(&mut self, kind: BuildingKind, now_unix: u64) -> Result<(), BuildError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
        let cap = self.config.max_build_queue;
        if self.build_queue.len() >= cap {
            return Err(BuildError::QueueFull { cap });
        }
        let prereqs = self.config.buildings.get(kind).prerequisites.clone();
        for &(req, required) in &prereqs {
            let actual = self.projected_level(req);
            if actual < required {
                return Err(BuildError::MissingPrerequisite {
                    building: req,
                    required,
                    actual,
                });
            }
        }
        let from_level = self.projected_level(kind);
        let cost = self.config.buildings.get(kind).cost(from_level);
        if !self.resources.can_afford(cost) {
            return Err(BuildError::InsufficientResources {
                missing: self.resources.missing(cost),
                cost,
            });
        }
        self.resources = self.resources.saturating_sub(cost);
        self.build_queue.push(BuildJob {
            building: kind,
            from_level,
            paid: cost,
            started_at: None,
            completes_at: None,
        });
        if self.build_queue.len() == 1 {
            self.start_head(now);
        }
        Ok(())
    }

    /// Cancel a queue slot and refund its paid cost in full (may exceed the cap).
    pub fn cancel_build(&mut self, index: usize, now_unix: u64) -> Result<Resources, BuildError> {
        self.catch_up(now_unix);
        let now = self.last_tick_unix;
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
                spec.duration_secs(from_level),
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
        if ships.is_empty() {
            return Err(FleetError::EmptyFleet);
        }
        if dest == self.home {
            return Err(FleetError::SameLocation);
        }
        if !(10..=100).contains(&speed_percent) {
            return Err(FleetError::SpeedPercent { got: speed_percent });
        }
        let dist = distance(self.home, dest, &self.config.flight);
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
        });
        Ok(())
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

    /// Sum of `per_level * tech_level` for every tech that contributes `kind`.
    pub fn modifier(&self, kind: ModifierKind) -> f64 {
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
        let raw = self.config.buildings.get(kind).raw_seconds(from_level);
        let divisor =
            1.0 + f64::from(self.buildings.robotics_factory) * self.config.robotics_speed_per_level;
        let secs = if divisor > 0.0 { raw / divisor } else { raw };
        secs.ceil().max(1.0) as u64
    }

    fn start_head(&mut self, now_unix: u64) {
        let Some(job) = self.build_queue.first() else {
            return;
        };
        if job.completes_at.is_some() {
            return;
        }
        let duration = self.duration_secs(job.building, job.from_level);
        let now = now_unix.max(self.last_tick_unix);
        let job = &mut self.build_queue[0];
        job.started_at = Some(now);
        job.completes_at = Some(now.saturating_add(duration));
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
                }
                MissionPhase::Action => {
                    let ret = self.missions[index].outbound_secs;
                    self.missions[index].phase = MissionPhase::Return;
                    self.missions[index].phase_ends_at = done_at.saturating_add(ret);
                }
                MissionPhase::Return => {
                    let ships = self.missions.remove(index).ships;
                    for kind in ShipKind::ALL {
                        self.fleet.add(kind, ships.get(kind));
                    }
                }
            }
        }
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

    fn resolve_raid_at(&mut self, done_at: u64) {
        let pirates = self.pirate_fleet();
        let atk_stats = stats_from_catalog(&self.config.ships);
        let def_stats = self.player_combat_stats();
        let report = resolve_combat(
            pirates,
            self.fleet,
            &atk_stats,
            &def_stats,
            self.config.raid.max_rounds,
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

    fn player_combat_stats(&self) -> [ShipStats; 9] {
        let mut out = [ShipStats {
            attack: 0.0,
            shield: 0.0,
            hull: 0.0,
            cargo: 0.0,
            speed: 0.0,
            fuel: 0.0,
        }; 9];
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
        for kind in ResourceKind::ALL {
            let rate = self.base_production_per_hour(kind) * factor;
            let cap = self.storage_cap(kind);
            let current = self.resources.get(kind);
            self.resources
                .set(kind, add_capped(current, rate, hours, cap));
        }
        self.last_tick_unix = now_unix;
    }

    pub fn energy_produced(&self) -> f64 {
        self.config
            .solar_plant
            .energy_output(self.buildings.solar_plant)
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
        self.base_production_per_hour(kind) * self.energy_factor()
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
        match kind {
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
        }
    }
}

/// Integrate a constant rate for `hours`, stopping exactly at `cap`.
/// Never lowers `current` (over-cap stockpiles and zero/negative rates stay put).
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
    fn raid_during_offline_gap_matches_step_by_step() {
        let mut gap = GameState::new(0);
        gap.catch_up(7200);
        let mut step = GameState::new(0);
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
}
