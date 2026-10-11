//! Scripted greedy pacing run. Uses [`GameState::catch_up`] so the report
//! matches live production, queues and activity.

use crate::activity::ActivityPulse;
use crate::balance::{BalanceConfig, PacingTargets};
use crate::buildings::{BuildError, BuildingKind, Buildings};
use crate::state::{GameState, ResourceKind};

/// One day's stockpile and mine level at the day boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct DaySnapshot {
    pub day: u32,
    pub metal_mine: u32,
    pub crystal_mine: u32,
    pub solar_plant: u32,
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
}

/// Pacing report for a greedy N-day run.
#[derive(Debug, Clone, PartialEq)]
pub struct SimReport {
    pub days: u32,
    pub first_upgrade_secs: Option<u64>,
    pub metal_mine_level: u32,
    pub metal_without_bridge: f64,
    pub metal_with_bridge: f64,
    pub bridge_metal_ratio: f64,
    pub days_log: Vec<DaySnapshot>,
    pub pacing: PacingTargets,
}

impl SimReport {
    pub fn first_upgrade_ok(&self) -> bool {
        match self.first_upgrade_secs {
            Some(secs) => secs <= self.pacing.first_upgrade_secs,
            None => false,
        }
    }

    pub fn mine_goal_ok(&self) -> bool {
        // Starting metal mine is level 1; require the documented per-day gain.
        let need = self
            .pacing
            .min_metal_mine_per_day
            .saturating_mul(self.days)
            .saturating_add(1);
        self.metal_mine_level >= need
    }

    pub fn bridge_ok(&self) -> bool {
        self.bridge_metal_ratio.is_finite()
            && self.bridge_metal_ratio <= self.pacing.max_bridge_metal_ratio
    }

    pub fn passed(&self) -> bool {
        self.first_upgrade_ok() && self.mine_goal_ok() && self.bridge_ok()
    }

    pub fn to_markdown(&self) -> String {
        let first = match self.first_upgrade_secs {
            Some(s) => format!("{s}s"),
            None => "never".into(),
        };
        let mut out = String::new();
        out.push_str("# Pacing report\n\n");
        out.push_str(&format!("Simulated days: {}\n", self.days));
        out.push_str("Strategy: greedy mines; solar when energy would go short. Raids off.\n\n");
        out.push_str("## Targets\n\n");
        out.push_str(&format!(
            "- first upgrade within {}s\n",
            self.pacing.first_upgrade_secs
        ));
        out.push_str(&format!(
            "- at least {} metal mine level(s) per day\n",
            self.pacing.min_metal_mine_per_day
        ));
        out.push_str(&format!(
            "- bridge metal ratio at most {:.1}\n\n",
            self.pacing.max_bridge_metal_ratio
        ));
        out.push_str("## Results\n\n");
        out.push_str(&format!(
            "- first upgrade: {first} ({})\n",
            mark(self.first_upgrade_ok())
        ));
        out.push_str(&format!(
            "- metal mine level: {} ({})\n",
            self.metal_mine_level,
            mark(self.mine_goal_ok())
        ));
        out.push_str(&format!(
            "- metal without bridge: {:.0}\n",
            self.metal_without_bridge
        ));
        out.push_str(&format!(
            "- metal with max PR-merge: {:.0}\n",
            self.metal_with_bridge
        ));
        out.push_str(&format!(
            "- bridge metal ratio: {:.2} ({})\n\n",
            self.bridge_metal_ratio,
            mark(self.bridge_ok())
        ));
        out.push_str(&format!(
            "Overall: {}\n\n",
            if self.passed() { "pass" } else { "fail" }
        ));
        out.push_str("## Daily (activity off)\n\n");
        out.push_str("| day | metal mine | crystal mine | solar | metal | crystal | deuterium |\n");
        out.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");
        for row in &self.days_log {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {:.0} | {:.0} | {:.0} |\n",
                row.day,
                row.metal_mine,
                row.crystal_mine,
                row.solar_plant,
                row.metal,
                row.crystal,
                row.deuterium
            ));
        }
        out
    }
}

fn mark(ok: bool) -> &'static str {
    if ok { "pass" } else { "fail" }
}

/// Run the greedy strategy for `days` with and without maxed PR-merge pulses.
pub fn simulate_greedy(config: BalanceConfig, days: u32) -> SimReport {
    let days = days.max(1);
    let pacing = config.pacing.clone();
    let quiet = run_once(config.clone(), days, false);
    let busy = run_once(config, days, true);
    let ratio = if quiet.metal > 1e-9 {
        busy.metal / quiet.metal
    } else {
        f64::INFINITY
    };
    SimReport {
        days,
        first_upgrade_secs: quiet.first_upgrade_secs,
        metal_mine_level: quiet.metal_mine,
        metal_without_bridge: quiet.metal,
        metal_with_bridge: busy.metal,
        bridge_metal_ratio: ratio,
        days_log: quiet.days_log,
        pacing,
    }
}

struct Run {
    first_upgrade_secs: Option<u64>,
    metal_mine: u32,
    metal: f64,
    days_log: Vec<DaySnapshot>,
}

fn run_once(mut config: BalanceConfig, days: u32, activity: bool) -> Run {
    config.raid = crate::RaidConfig::inactive();
    config.activity.enabled = activity;
    let start = 0_u64;
    let end = u64::from(days) * 86_400;
    let mut state = GameState::with_config(start, config);
    let start_buildings = state.buildings;
    let mut first_upgrade_secs = None;
    let mut days_log = Vec::new();
    let mut next_day = 86_400_u64;
    let mut next_pulse = 0_u64;
    let cooldown = state.config.activity.pr_merged.cooldown_secs.max(1);

    let mut steps = 0_u32;
    while state.last_tick_unix < end && steps < 100_000 {
        steps += 1;
        if activity {
            while next_pulse <= state.last_tick_unix {
                let _ = state.apply_activity(
                    ActivityPulse::PrMerged { ts: next_pulse },
                    state.last_tick_unix,
                );
                next_pulse = next_pulse.saturating_add(cooldown);
            }
        }
        fill_queue(&mut state);
        if first_upgrade_secs.is_none() && state.buildings != start_buildings {
            first_upgrade_secs = Some(state.last_tick_unix.saturating_sub(start));
        }
        let completion = state
            .build_queue
            .first()
            .and_then(|job| job.completes_at)
            .unwrap_or(u64::MAX);
        let afford = next_afford_at(&state).unwrap_or(u64::MAX);
        let pulse_at = if activity { next_pulse } else { u64::MAX };
        let mut jump = completion.min(afford).min(pulse_at).min(next_day).min(end);
        if jump <= state.last_tick_unix {
            jump = state.last_tick_unix.saturating_add(1).min(end);
        }
        state.catch_up(jump);
        if first_upgrade_secs.is_none() && state.buildings != start_buildings {
            first_upgrade_secs = Some(state.last_tick_unix.saturating_sub(start));
        }
        while state.last_tick_unix >= next_day && days_log.len() < days as usize {
            days_log.push(snapshot(&state, days_log.len() as u32 + 1));
            next_day = next_day.saturating_add(86_400);
        }
    }
    if days_log.len() < days as usize {
        state.catch_up(end);
        while days_log.len() < days as usize {
            days_log.push(snapshot(&state, days_log.len() as u32 + 1));
        }
    }
    Run {
        first_upgrade_secs,
        metal_mine: state.buildings.metal_mine,
        metal: state.resources.metal,
        days_log,
    }
}

fn snapshot(state: &GameState, day: u32) -> DaySnapshot {
    DaySnapshot {
        day,
        metal_mine: state.buildings.metal_mine,
        crystal_mine: state.buildings.crystal_mine,
        solar_plant: state.buildings.solar_plant,
        metal: state.resources.metal,
        crystal: state.resources.crystal,
        deuterium: state.resources.deuterium,
    }
}

fn fill_queue(state: &mut GameState) {
    let cap = state.config.max_build_queue;
    while state.build_queue.len() < cap {
        let order = candidate_order(state);
        let mut enqueued = false;
        for kind in order {
            match state.enqueue_build(kind, state.last_tick_unix) {
                Ok(()) => {
                    enqueued = true;
                    break;
                }
                Err(BuildError::QueueFull { .. }) => return,
                Err(BuildError::InsufficientResources { .. })
                | Err(BuildError::MissingPrerequisite { .. })
                | Err(BuildError::EmptySlot { .. })
                | Err(BuildError::UnknownPlanet)
                | Err(BuildError::NeedLogistics) => {}
            }
        }
        if !enqueued {
            break;
        }
    }
}

fn candidate_order(state: &GameState) -> [BuildingKind; 6] {
    if energy_ok_after(state, BuildingKind::MetalMine) {
        [
            BuildingKind::MetalMine,
            BuildingKind::CrystalMine,
            BuildingKind::SolarPlant,
            BuildingKind::DeuteriumSynthesizer,
            BuildingKind::RoboticsFactory,
            BuildingKind::MetalStorage,
        ]
    } else {
        [
            BuildingKind::SolarPlant,
            BuildingKind::MetalMine,
            BuildingKind::CrystalMine,
            BuildingKind::DeuteriumSynthesizer,
            BuildingKind::RoboticsFactory,
            BuildingKind::MetalStorage,
        ]
    }
}

fn projected_buildings(state: &GameState) -> Buildings {
    let mut buildings = state.buildings;
    for job in &state.build_queue {
        buildings.increment(job.building);
    }
    buildings
}

fn energy_ok_after(state: &GameState, extra: BuildingKind) -> bool {
    let mut buildings = projected_buildings(state);
    buildings.increment(extra);
    let produced = state
        .config
        .solar_plant
        .energy_output(buildings.solar_plant);
    let consumed = state.config.metal_mine.energy_use(buildings.metal_mine)
        + state.config.crystal_mine.energy_use(buildings.crystal_mine)
        + state
            .config
            .deuterium_synthesizer
            .energy_use(buildings.deuterium_synthesizer);
    consumed <= 0.0 || produced + 1e-9 >= consumed
}

fn next_afford_at(state: &GameState) -> Option<u64> {
    let now = state.last_tick_unix;
    let mut best: Option<u64> = None;
    for kind in candidate_order(state) {
        if let Some(wait) = secs_until_afford(state, kind) {
            let at = now.saturating_add(wait);
            best = Some(best.map_or(at, |b| b.min(at)));
        }
    }
    best
}

fn secs_until_afford(state: &GameState, kind: BuildingKind) -> Option<u64> {
    let from_level = projected_buildings(state).get(kind);
    let cost = state.config.buildings.get(kind).cost(from_level);
    let have = state.resources;
    if have.can_afford(cost) {
        return Some(0);
    }
    let mut wait = 0.0_f64;
    for resource in ResourceKind::ALL {
        let need = cost.get(resource);
        let got = have.get(resource);
        if got + 1e-9 >= need {
            continue;
        }
        let rate = state.production_per_hour(resource);
        if rate <= 1e-12 {
            return None;
        }
        wait = wait.max((need - got) / rate * 3600.0);
    }
    Some(wait.ceil().max(1.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greedy_first_upgrade_meets_target() {
        let report = simulate_greedy(BalanceConfig::default(), 1);
        assert!(
            report.first_upgrade_ok(),
            "first upgrade {:?} > {}",
            report.first_upgrade_secs,
            report.pacing.first_upgrade_secs
        );
        assert!(
            report.first_upgrade_secs.unwrap() > 0,
            "upgrade should take construction time"
        );
        assert!(
            report.mine_goal_ok(),
            "mine level {}",
            report.metal_mine_level
        );
        assert!(
            report.bridge_ok(),
            "ratio {} > {}",
            report.bridge_metal_ratio,
            report.pacing.max_bridge_metal_ratio
        );
        assert!(report.passed(), "{}", report.to_markdown());
        assert_eq!(report.days_log.len(), 1);
    }

    #[test]
    fn bridge_pulses_add_metal_without_runaway() {
        let report = simulate_greedy(BalanceConfig::default(), 1);
        assert!(
            report.metal_with_bridge + 1e-9 >= report.metal_without_bridge,
            "bridge should not shrink the stockpile"
        );
        assert!(
            report.bridge_metal_ratio >= 1.0,
            "ratio {}",
            report.bridge_metal_ratio
        );
        assert!(
            report.bridge_metal_ratio <= report.pacing.max_bridge_metal_ratio,
            "ratio {}",
            report.bridge_metal_ratio
        );
    }
}
