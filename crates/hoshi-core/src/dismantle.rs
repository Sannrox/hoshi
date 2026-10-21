//! One-level dismantling shares the governed build queue and powered simulation.
use crate::{
    Actor, BuildError, BuildJob, BuildJobKind, BuildingKind, Buildings, Coord, GameState,
    Permission, ResourceKind, Resources,
};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct DismantleConfig {
    pub time_share: f64,
    pub energy_share: f64,
    pub salvage_share: f64,
    pub condition_floor: f64,
}

impl Default for DismantleConfig {
    fn default() -> Self {
        Self {
            time_share: 0.5,
            energy_share: 0.25,
            salvage_share: 0.35,
            condition_floor: 0.0,
        }
    }
}

impl DismantleConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        for (key, value) in [
            ("time_share", self.time_share),
            ("energy_share", self.energy_share),
            ("salvage_share", self.salvage_share),
            ("condition_floor", self.condition_floor),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(format!("dismantle.{key} must be finite and in 0..=1"));
            }
        }
        if self.time_share <= 0.0 || self.salvage_share >= 1.0 {
            return Err("dismantle needs positive time and salvage below one".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DismantlePreview {
    pub planet: Coord,
    pub building: BuildingKind,
    pub from_level: u32,
    pub duration_secs: u64,
    pub energy_draw: f64,
    pub energy_freed: f64,
    pub heat_freed: f64,
    pub condition: f64,
    pub salvage: Resources,
    pub blockers: Vec<String>,
}

impl GameState {
    fn dismantle_condition(&self, coord: Coord, kind: BuildingKind) -> f64 {
        let key = format!("building:{coord}:{}", kind.as_key());
        self.maintenance
            .assets
            .iter()
            .find(|asset| asset.key == key)
            .map(|asset| asset.condition)
            .unwrap_or(1.0)
    }

    pub fn dismantle_preview(
        &self,
        coord: Coord,
        kind: BuildingKind,
    ) -> Result<DismantlePreview, BuildError> {
        let mut before = *self.buildings_at(coord).ok_or(BuildError::UnknownPlanet)?;
        let queue = self.queue_at(coord).ok_or(BuildError::UnknownPlanet)?;
        let level = crate::state::projected_on(&before, queue, kind, queue.len());
        before.set(kind, level);
        let mut after = before;
        after.set(kind, level.saturating_sub(1));
        let mut only = Buildings::default();
        only.set(kind, level);
        let energy_draw = self.config.energy_draw(&only) * self.config.dismantle.energy_share;
        let condition = self
            .dismantle_condition(coord, kind)
            .max(self.config.dismantle.condition_floor)
            .min(1.0);
        let cost = self
            .config
            .buildings
            .get(kind)
            .cost(level.saturating_sub(1));
        let mut salvage = salvage_resources(cost, self.config.dismantle.salvage_share * condition);
        salvage.deuterium = 0.0;
        salvage.helium_3 = 0.0;
        let construction = self
            .species
            .as_ref()
            .map(|run| run.construction)
            .unwrap_or(1.0);
        let duration = crate::state::duration_with_robotics(
            kind,
            level.saturating_sub(1),
            before.robotics_factory,
            &self.config,
        );
        let planet = self.planet_kind(coord).ok_or(BuildError::UnknownPlanet)?;
        Ok(DismantlePreview {
            planet: coord,
            building: kind,
            from_level: level,
            duration_secs: (duration as f64 * self.config.dismantle.time_share * construction)
                .ceil()
                .max(1.0) as u64,
            energy_draw,
            energy_freed: self.config.energy_draw(&before) - self.config.energy_draw(&after),
            heat_freed: self.waste_for(before, planet).heat_load
                - self.waste_for(after, planet).heat_load,
            condition,
            salvage,
            blockers: self.dismantle_blockers(coord, kind, level, queue.len()),
        })
    }

    pub(crate) fn dismantle_blockers(
        &self,
        coord: Coord,
        kind: BuildingKind,
        level: u32,
        queue_index: usize,
    ) -> Vec<String> {
        let mut reasons = Vec::new();
        if level == 0 {
            reasons.push("building has no remaining level".into());
            return reasons;
        }
        if coord == self.home && self.raid_countdown_secs().is_some() {
            reasons.push("hostile arrival is announced for this planet".into());
        }
        let Some(buildings) = self.buildings_at(coord) else {
            return vec!["unknown planet".into()];
        };
        let queue = self.queue_at(coord).unwrap_or(&[]);
        for dependent in BuildingKind::ALL {
            if dependent == kind
                || crate::state::projected_on(buildings, queue, dependent, queue_index) == 0
            {
                continue;
            }
            for &(requirement, required) in &self.config.buildings.get(dependent).prerequisites {
                if requirement == kind && level - 1 < required {
                    reasons.push(format!("{dependent} requires {kind} {required}"));
                }
            }
        }
        for (index, job) in queue.iter().enumerate().skip(queue_index.saturating_add(1)) {
            if job.kind != BuildJobKind::Build {
                continue;
            }
            for &(requirement, required) in &self.config.buildings.get(job.building).prerequisites {
                let available = crate::state::projected_on(buildings, queue, requirement, index);
                if requirement == kind && available < required {
                    reasons.push(format!(
                        "queued {} requires {kind} {required}",
                        job.building
                    ));
                }
            }
        }
        if coord == self.home
            && let Some(job) = &self.research
            && let Some(spec) = self.config.techs.get(&job.tech)
        {
            for required in &spec.prereq_buildings {
                if required.building == kind {
                    reasons.push(format!(
                        "research {} requires {kind} {}",
                        job.tech, required.level
                    ));
                }
            }
        }
        if kind == BuildingKind::Shipyard && coord == self.home && !self.ship_queue.is_empty() {
            reasons.push("shipyard has queued production".into());
        }
        if kind == BuildingKind::FissionReactor {
            let spent = if coord == self.home {
                self.goods.get("spent_fuel")
            } else {
                self.colonies
                    .iter()
                    .find(|colony| colony.coord == coord)
                    .and_then(|colony| colony.goods.get("spent_fuel"))
            }
            .copied()
            .unwrap_or(0.0);
            let capacity = self.config.fission_reactor.pool_capacity(level - 1);
            if spent > capacity {
                reasons.push(format!(
                    "spent fuel {spent:.3} exceeds the remaining pool capacity {capacity:.3}"
                ));
            }
        }
        let key = format!("building:{coord}:{}", kind.as_key());
        if let Some(asset) = self
            .maintenance
            .assets
            .iter()
            .find(|asset| asset.key == key)
            && self
                .maintenance
                .orders
                .iter()
                .any(|order| order.asset_id == asset.id)
        {
            reasons.push("building has an active maintenance order".into());
        }
        reasons.sort();
        reasons.dedup();
        reasons
    }

    pub fn enqueue_dismantle_as(
        &mut self,
        coord: Coord,
        kind: BuildingKind,
        now: u64,
        actor: Actor,
    ) -> Result<(), BuildError> {
        self.catch_up(now);
        self.authorize(actor, Permission::QueueBuild, Some(coord), 0.0)
            .map_err(|error| BuildError::PermissionDenied(error.to_string()))?;
        let preview = self.dismantle_preview(coord, kind)?;
        if !preview.blockers.is_empty() {
            return Err(BuildError::DismantleBlocked {
                reasons: preview.blockers,
            });
        }
        let cap = self.build_queue_cap();
        if self.queue_at(coord).is_some_and(|queue| queue.len() >= cap) {
            return Err(BuildError::QueueFull { cap });
        }
        let job = BuildJob {
            kind: BuildJobKind::Dismantle,
            work_left: preview.duration_secs as f64,
            building: kind,
            from_level: preview.from_level,
            paid: Resources::ZERO,
            credits: 0.0,
            started_at: None,
            completes_at: None,
        };
        if coord == self.home {
            self.build_queue.push(job);
            self.start_head(now);
        } else {
            let index = self
                .colonies
                .iter()
                .position(|colony| colony.coord == coord)
                .ok_or(BuildError::UnknownPlanet)?;
            self.colonies[index].build_queue.push(job);
            self.start_colony_head(index, now);
        }
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Build,
            actor: actor.as_key(),
            action: "dismantle".into(),
            objects: vec![kind.as_key().into()],
            outcome: format!(
                "queued {} -> {}",
                preview.from_level,
                preview.from_level - 1
            ),
            ..Default::default()
        });
        self.refresh_dismantle_deadlines(now.saturating_add(31_536_000));
        Ok(())
    }

    pub(crate) fn record_dismantle_start(&mut self, coord: Coord, at: u64) {
        let Some(building) = self.dismantling(coord) else {
            return;
        };
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Build,
            actor: "simulation".into(),
            action: "dismantle_start".into(),
            objects: vec![coord.to_string(), building.as_key().into()],
            outcome: "powered work started; building output stopped".into(),
            ..Default::default()
        });
    }

    pub(crate) fn dismantling(&self, coord: Coord) -> Option<BuildingKind> {
        let head = self.queue_at(coord)?.first()?;
        (head.kind == BuildJobKind::Dismantle && head.started_at.is_some()).then_some(head.building)
    }

    pub(crate) fn operational_buildings_at(&self, coord: Coord) -> Option<Buildings> {
        let mut buildings = *self.buildings_at(coord)?;
        if let Some(kind) = self.dismantling(coord) {
            buildings.set(kind, 0);
        }
        Some(buildings)
    }

    pub(crate) fn dismantle_energy_at(&self, coord: Coord) -> f64 {
        let Some(kind) = self.dismantling(coord) else {
            return 0.0;
        };
        let mut building = Buildings::default();
        building.set(
            kind,
            self.buildings_at(coord)
                .map(|all| all.get(kind))
                .unwrap_or(0),
        );
        self.config.energy_draw(&building) * self.config.dismantle.energy_share
    }

    pub(crate) fn energy_consumed_at(&self, coord: Coord) -> f64 {
        self.operational_buildings_at(coord)
            .map(|buildings| self.config.energy_draw(&buildings))
            .unwrap_or(0.0)
            + self.dismantle_energy_at(coord)
            + if coord == self.home {
                self.automation_control_energy()
            } else {
                0.0
            }
    }

    fn dismantle_work_in_secs(&self, coord: Coord, seconds: u64) -> f64 {
        if self.dismantle_energy_at(coord) <= 0.0 {
            return seconds as f64;
        }
        self.reactor_production_factor(coord, seconds as f64 / 3600.0)
            .clamp(0.0, 1.0)
            * seconds as f64
    }

    pub(crate) fn refresh_dismantle_deadlines(&mut self, horizon: u64) {
        let mut forecasts = Vec::new();
        for coord in self.owned_coords() {
            let Some(head) = self.queue_at(coord).and_then(|queue| queue.first()) else {
                continue;
            };
            if head.kind != BuildJobKind::Dismantle || head.started_at.is_none() {
                continue;
            }
            let left = head.work_left;
            let max = horizon
                .max(self.last_tick_unix.saturating_add(31_536_000))
                .saturating_sub(self.last_tick_unix);
            let progress = |secs: u64| self.dismantle_work_in_secs(coord, secs);
            let completion = if progress(max) + 1e-9 < left {
                None
            } else {
                let mut low = 1;
                let mut high = max.max(1);
                while low < high {
                    let mid = low + (high - low) / 2;
                    if progress(mid) + 1e-9 >= left {
                        high = mid;
                    } else {
                        low = mid + 1;
                    }
                }
                Some(self.last_tick_unix.saturating_add(low))
            };
            forecasts.push((coord, completion));
        }
        for (coord, completion) in forecasts {
            if coord == self.home {
                if let Some(head) = self.build_queue.first_mut() {
                    head.completes_at = completion;
                }
            } else if let Some(head) = self
                .colonies
                .iter_mut()
                .find(|colony| colony.coord == coord)
                .and_then(|colony| colony.build_queue.first_mut())
            {
                head.completes_at = completion;
            }
        }
    }

    pub(crate) fn dismantle_progress(&self, seconds: u64) -> Vec<(Coord, f64)> {
        self.owned_coords()
            .into_iter()
            .filter(|coord| self.dismantling(*coord).is_some())
            .map(|coord| (coord, self.dismantle_work_in_secs(coord, seconds)))
            .collect()
    }

    pub(crate) fn apply_dismantle_progress(&mut self, progress: Vec<(Coord, f64)>) {
        for (coord, work) in progress {
            if coord == self.home {
                if let Some(head) = self.build_queue.first_mut() {
                    head.work_left = (head.work_left - work).max(0.0);
                }
            } else if let Some(head) = self
                .colonies
                .iter_mut()
                .find(|colony| colony.coord == coord)
                .and_then(|colony| colony.build_queue.first_mut())
            {
                head.work_left = (head.work_left - work).max(0.0);
            }
        }
    }

    pub(crate) fn complete_dismantle(&mut self, coord: Coord, job: BuildJob, at: u64) {
        let level = job.from_level.saturating_sub(1);
        let mut salvage = salvage_resources(
            self.config.buildings.get(job.building).cost(level),
            self.config.dismantle.salvage_share
                * self
                    .dismantle_condition(coord, job.building)
                    .max(self.config.dismantle.condition_floor)
                    .min(1.0),
        );
        salvage.deuterium = 0.0;
        salvage.helium_3 = 0.0;
        if coord == self.home {
            self.buildings.set(job.building, level);
        } else if let Some(colony) = self
            .colonies
            .iter_mut()
            .find(|colony| colony.coord == coord)
        {
            colony.buildings.set(job.building, level);
        }
        let mut received = Resources::ZERO;
        for kind in ResourceKind::ALL {
            let current = self
                .resources_at(coord)
                .map(|resources| resources.get(kind))
                .unwrap_or(0.0);
            let amount = salvage
                .get(kind)
                .min((self.storage_cap_at(coord, kind) - current).max(0.0));
            received.set(kind, amount);
            if coord == self.home {
                self.resources.set(kind, current + amount);
            } else if let Some(colony) = self
                .colonies
                .iter_mut()
                .find(|colony| colony.coord == coord)
            {
                colony.resources.set(kind, current + amount);
            }
        }
        self.sync_maintenance_assets(at);
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Build,
            actor: "simulation".into(),
            action: "dismantle_complete".into(),
            objects: vec![job.building.as_key().into()],
            outcome: format!(
                "level {level}; salvage {:.3}; overflow lost {:.3}",
                received.total(),
                salvage.total() - received.total()
            ),
            ..Default::default()
        });
    }
}

fn salvage_resources(cost: Resources, factor: f64) -> Resources {
    let mut salvage = Resources::ZERO;
    for kind in ResourceKind::ALL {
        salvage.set(kind, cost.get(kind) * factor);
    }
    salvage
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> GameState {
        let config = crate::BalanceConfig {
            chains: crate::ChainConfig::default(),
            raid: crate::RaidConfig::inactive(),
            story: crate::StoryCatalog::empty(),
            ..Default::default()
        };
        let mut state = GameState::with_seed(100, config, 7);
        state.buildings.metal_mine = 3;
        state.buildings.crystal_mine = 0;
        state.buildings.deuterium_synthesizer = 0;
        state.buildings.solar_plant = 8;
        state.resources = Resources::ZERO;
        state.maintenance.next_at = 1_000_000_000_000;
        state
    }

    #[test]
    fn one_level_stops_output_consumes_power_and_salvages_only_at_completion() {
        let mut state = state();
        let preview = state
            .dismantle_preview(state.home, BuildingKind::MetalMine)
            .unwrap();
        let raw_time =
            crate::state::duration_with_robotics(BuildingKind::MetalMine, 2, 0, &state.config);
        assert_eq!(preview.duration_secs, (raw_time as f64 * 0.5).ceil() as u64);
        assert!(preview.salvage.iron < state.config.buildings.metal_mine.cost(2).iron);
        state
            .enqueue_dismantle_as(state.home, BuildingKind::MetalMine, 100, Actor::Player)
            .unwrap();
        assert_eq!(state.buildings.metal_mine, 3);
        assert_eq!(state.production_per_hour(ResourceKind::Iron), 0.0);
        assert!((state.energy_consumed() - preview.energy_draw).abs() < 1e-9);
        state.catch_up(101);
        assert_eq!(
            state.build_queue[0].completes_at,
            Some(100 + preview.duration_secs)
        );
        state.catch_up(100 + preview.duration_secs - 1);
        assert_eq!(state.resources.iron, 0.0);
        assert_eq!(state.buildings.metal_mine, 3);
        state.catch_up(100 + preview.duration_secs);
        assert_eq!(state.buildings.metal_mine, 2);
        assert!((state.resources.iron - preview.salvage.iron).abs() < 1e-9);
        assert_eq!(state.resources.deuterium, 0.0);
        assert!(state.production_per_hour(ResourceKind::Iron) > 0.0);
    }

    #[test]
    fn home_solar_dismantle_removes_output_from_coordinate_queries() {
        let mut state = state();
        assert!(state.energy_produced_at(state.home).unwrap() > 0.0);
        state
            .enqueue_dismantle_as(state.home, BuildingKind::SolarPlant, 100, Actor::Player)
            .unwrap();
        assert_eq!(state.energy_produced(), 0.0);
        assert_eq!(state.energy_produced_at(state.home), Some(0.0));
    }

    #[test]
    fn power_starvation_waits_and_offline_queue_matches_stepped_work() {
        let mut bulk = state();
        bulk.buildings.solar_plant = 0;
        bulk.enqueue_dismantle_as(bulk.home, BuildingKind::MetalMine, 100, Actor::Player)
            .unwrap();
        let work = bulk.build_queue[0].work_left;
        bulk.catch_up(160);
        assert_eq!(bulk.build_queue[0].work_left, work);
        assert_eq!(bulk.buildings.metal_mine, 3);
        bulk.buildings.solar_plant = 8;
        bulk.enqueue_dismantle_as(bulk.home, BuildingKind::MetalMine, 160, Actor::Player)
            .unwrap();
        let mut live = bulk.clone();
        bulk.catch_up(400);
        for now in 161..=400 {
            live.catch_up(now);
        }
        assert_eq!(bulk.buildings, live.buildings);
        assert_eq!(bulk.build_queue, live.build_queue);
        assert!((bulk.resources.iron - live.resources.iron).abs() < 1e-8);
        assert_eq!(bulk.buildings.metal_mine, 1);
    }

    #[test]
    fn refusal_lists_dependencies_and_raid_and_cancel_has_no_salvage() {
        let mut state = state();
        state.buildings.robotics_factory = 1;
        state.buildings.shipyard = 1;
        assert!(
            state
                .dismantle_preview(state.home, BuildingKind::RoboticsFactory)
                .unwrap()
                .blockers
                .iter()
                .any(|reason| reason.contains("shipyard"))
        );
        state.config.raid.enabled = true;
        state.next_raid_at = Some(110);
        assert!(
            state
                .enqueue_dismantle_as(state.home, BuildingKind::MetalMine, 100, Actor::Player)
                .is_err()
        );
        state.config.raid.enabled = false;
        state
            .enqueue_dismantle_as(state.home, BuildingKind::MetalMine, 100, Actor::Player)
            .unwrap();
        assert_eq!(
            state.cancel_build_on(state.home, 0, 100).unwrap(),
            Resources::ZERO
        );
        assert_eq!(state.buildings.metal_mine, 3);
        assert_eq!(state.resources, Resources::ZERO);
        assert!(state.production_per_hour(ResourceKind::Iron) > 0.0);
    }

    #[test]
    fn worn_salvage_respects_storage_and_maintenance_cannot_overlap() {
        let mut state = state();
        state.sync_maintenance_assets(100);
        let asset = state
            .maintenance
            .assets
            .iter_mut()
            .find(|asset| asset.key.ends_with(":metal_mine"))
            .unwrap();
        asset.condition = 0.5;
        let asset_id = asset.id;
        let preview = state
            .dismantle_preview(state.home, BuildingKind::MetalMine)
            .unwrap();
        let cost = state.config.buildings.metal_mine.cost(2);
        assert!((preview.salvage.iron - cost.iron * 0.35 * 0.5).abs() < 1e-9);
        state.resources.iron = state.storage_cap_at(state.home, ResourceKind::Iron) - 1.0;
        let before = state.resources.iron;
        state
            .enqueue_dismantle_as(state.home, BuildingKind::MetalMine, 100, Actor::Player)
            .unwrap();
        assert_eq!(
            state.schedule_maintenance(asset_id, crate::PlanKind::Time, Actor::Player, 100),
            Err(crate::MaintenanceError::Busy)
        );
        state.catch_up(state.build_queue[0].completes_at.unwrap());
        assert!((state.resources.iron - before - 1.0).abs() < 1e-9);
        assert!(
            state
                .history
                .events
                .iter()
                .any(|event| event.action == "dismantle_complete"
                    && event.outcome.contains("overflow lost"))
        );
        assert_eq!(state.resources.deuterium, 0.0);
    }

    #[test]
    fn lab_removal_preserves_knowledge_and_reserves_running_research() {
        let mut state = state();
        state.buildings.research_lab = 2;
        state.resources = Resources::from_parts(10_000.0, 10_000.0, 10_000.0);
        state.tech_levels.insert("energy".into(), 1);
        state.start_research("energy", 100).unwrap();
        let blockers = state
            .dismantle_preview(state.home, BuildingKind::ResearchLab)
            .unwrap()
            .blockers;
        assert!(
            blockers
                .iter()
                .any(|reason| reason.contains("research energy"))
        );
        state.cancel_research(100).unwrap();
        state.buildings.research_lab = 1;
        state
            .enqueue_dismantle_as(state.home, BuildingKind::ResearchLab, 100, Actor::Player)
            .unwrap();
        state.enqueue_build(BuildingKind::ResearchLab, 100).unwrap();
        assert!(matches!(
            state.start_research("energy", 100),
            Err(crate::ResearchError::MissingPrerequisites { .. })
        ));
        state.cancel_build_on(state.home, 1, 100).unwrap();
        state.catch_up(state.build_queue[0].completes_at.unwrap());
        assert_eq!(state.buildings.research_lab, 0);
        assert_eq!(state.tech_level("energy"), 1);
        assert!(matches!(
            state.start_research("energy", state.last_tick_unix),
            Err(crate::ResearchError::MissingPrerequisites { .. })
        ));
    }

    #[test]
    fn cancelling_an_upgrade_reprices_waiting_dismantle_without_charging_it() {
        let mut state = state();
        state.resources = Resources::from_parts(10_000.0, 10_000.0, 10_000.0);
        let before = state.resources;
        state.enqueue_build(BuildingKind::MetalMine, 100).unwrap();
        state
            .enqueue_dismantle_as(state.home, BuildingKind::MetalMine, 100, Actor::Player)
            .unwrap();
        assert_eq!(state.build_queue[1].from_level, 4);
        state.cancel_build_on(state.home, 0, 100).unwrap();
        assert_eq!(state.resources, before);
        assert_eq!(state.build_queue[0].from_level, 3);
        assert_eq!(state.build_queue[0].paid, Resources::ZERO);
        assert_eq!(state.build_queue[0].credits, 0.0);
        state.cancel_build_on(state.home, 0, 100).unwrap();
        assert_eq!(state.resources, before);
        assert_eq!(state.buildings.metal_mine, 3);
    }

    #[test]
    fn dismantle_uses_robotics_available_when_it_reaches_the_head() {
        let mut state = state();
        state.resources = Resources::from_parts(100_000.0, 100_000.0, 100_000.0);
        state
            .enqueue_build(BuildingKind::RoboticsFactory, 100)
            .unwrap();
        state
            .enqueue_dismantle_as(state.home, BuildingKind::MetalMine, 100, Actor::Player)
            .unwrap();
        let upgrade_end = state.build_queue[0].completes_at.unwrap();
        state.catch_up(upgrade_end);
        let expected =
            (crate::state::duration_with_robotics(BuildingKind::MetalMine, 2, 1, &state.config)
                as f64
                * state.config.dismantle.time_share)
                .ceil();
        assert_eq!(state.build_queue[0].work_left, expected);
    }

    #[test]
    fn shipyard_reservation_blocks_new_ships_and_defences() {
        let mut state = state();
        state.buildings.shipyard = 1;
        state.buildings.robotics_factory = 1;
        state.resources = Resources::from_parts(100_000.0, 100_000.0, 100_000.0);
        state
            .enqueue_dismantle_as(state.home, BuildingKind::Shipyard, 100, Actor::Player)
            .unwrap();
        state.enqueue_build(BuildingKind::Shipyard, 100).unwrap();
        assert!(matches!(
            state.enqueue_ships(crate::ShipKind::LightFighter, 1, 100),
            Err(crate::ShipError::NeedShipyard { .. })
        ));
        assert!(matches!(
            state.enqueue_defence(crate::DefenceKind::LightCannon, 1, 100),
            Err(crate::ShipError::NeedShipyard { .. })
        ));
    }

    #[test]
    fn queued_lab_restoration_satisfies_later_reactor_without_deadlock() {
        let mut state = state();
        state.buildings.research_lab = 2;
        state.tech_levels.insert("fission".into(), 1);
        state.resources = Resources::from_parts(1_000_000.0, 1_000_000.0, 1_000_000.0);
        state.config.max_build_queue = 10;
        state.enqueue_build(BuildingKind::MetalMine, 100).unwrap();
        state
            .enqueue_dismantle_as(state.home, BuildingKind::ResearchLab, 100, Actor::Player)
            .unwrap();
        state.enqueue_build(BuildingKind::ResearchLab, 100).unwrap();
        state
            .enqueue_build(BuildingKind::FissionReactor, 100)
            .unwrap();
        state.catch_up(10_000);
        assert!(state.build_queue.is_empty(), "{:?}", state.build_queue);
        assert_eq!(state.buildings.research_lab, 2);
        assert_eq!(state.buildings.fission_reactor, 1);
    }

    #[test]
    fn reactor_is_defuelled_before_work_and_spent_pool_blocks_retirement() {
        let mut state = state();
        state.buildings.fission_reactor = 1;
        state.goods.insert("reactor_fuel".into(), 10.0);
        state.goods.insert("spent_fuel".into(), 1.0);
        assert!(
            state
                .enqueue_dismantle_as(state.home, BuildingKind::FissionReactor, 100, Actor::Player)
                .is_err()
        );
        state.goods.insert("spent_fuel".into(), 0.0);
        state
            .enqueue_dismantle_as(state.home, BuildingKind::FissionReactor, 100, Actor::Player)
            .unwrap();
        assert_eq!(state.reactor_output_at(state.home), 0.0);
        assert_eq!(state.goods["reactor_fuel"], 10.0);
        assert!(state.reactor_decay_heat_at(state.home) > 0.0);
        state
            .enqueue_dismantle_as(state.home, BuildingKind::MetalMine, 100, Actor::Player)
            .unwrap();
        assert!(state.build_queue[1].started_at.is_none());
        let colony_coord = Coord::from_key("1:1:8").unwrap();
        let mut buildings = state.buildings;
        buildings.fission_reactor = 0;
        state.colonies.push(crate::Colony {
            coord: colony_coord,
            kind: crate::PlanetType::Rocky,
            size: 12,
            resources: Resources::ZERO,
            buildings,
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        for _ in 0..2 {
            state
                .enqueue_dismantle_as(colony_coord, BuildingKind::MetalMine, 100, Actor::Player)
                .unwrap();
        }
        let mut conn = crate::open_save(std::path::Path::new(":memory:")).unwrap();
        crate::save_state(&mut conn, &state).unwrap();
        let mut loaded = crate::load_state(&conn, state.config.clone()).unwrap();
        assert_eq!(loaded.build_queue, state.build_queue);
        assert_eq!(
            loaded.colonies[0].build_queue,
            state.colonies[0].build_queue
        );
        assert_eq!(loaded.production_at(colony_coord, ResourceKind::Iron), 0.0);
        let until = loaded.build_queue[0].completes_at.unwrap();
        loaded.catch_up(until);
        assert_eq!(loaded.buildings.fission_reactor, 0);
        assert_eq!(loaded.goods["reactor_fuel"], 10.0);
        assert!(loaded.reactor_decay_heat_at(loaded.home) > 0.0);
    }
}
