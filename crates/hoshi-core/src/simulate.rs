//! Scripted greedy pacing run. Uses [`GameState::catch_up`] so the report
//! matches live production, queues and activity.

use crate::activity::ActivityPulse;
use crate::balance::{BalanceConfig, PacingTargets};
use crate::buildings::{BuildError, BuildingKind, Buildings};
use crate::research::{TechBranch, TechNodeState};
use crate::state::{GameState, ResourceKind};
use std::collections::BTreeMap;

/// One simulated prestige run: later runs own more bonuses and produce more.
#[derive(Debug, Clone, PartialEq)]
pub struct PrestigeSimRow {
    pub run: u32,
    pub bonuses: u32,
    pub first_upgrade_secs: Option<u64>,
    pub metal_mine_level: u32,
    pub metal: f64,
    pub metal_per_hour: f64,
}

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
    pub prestige_runs: Vec<PrestigeSimRow>,
    pub heavy: HeavyIndustryReport,
    pub keystone_secs: BTreeMap<String, Option<u64>>,
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

    pub fn keystones_ok(&self) -> bool {
        TechBranch::PLAYABLE.iter().all(|branch| {
            self.keystone_secs
                .get(branch.as_key())
                .and_then(|t| *t)
                .is_some_and(|secs| secs <= self.pacing.keystone_secs)
        })
    }

    pub fn passed(&self) -> bool {
        self.first_upgrade_ok() && self.mine_goal_ok() && self.bridge_ok() && self.keystones_ok()
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
            "- bridge metal ratio at most {:.1}\n",
            self.pacing.max_bridge_metal_ratio
        ));
        out.push_str(&format!(
            "- each branch keystone within {}s\n\n",
            self.pacing.keystone_secs
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
            "- keystones: {} ({})\n\n",
            self.keystone_secs
                .iter()
                .map(|(branch, at)| match at {
                    Some(secs) => format!("{branch} {secs}s"),
                    None => format!("{branch} never"),
                })
                .collect::<Vec<_>>()
                .join(", "),
            mark(self.keystones_ok())
        ));
        if !self.prestige_runs.is_empty() {
            out.push_str("\n## Prestige runs\n\n");
            out.push_str("| run | bonuses | first upgrade | metal mine | metal | metal/h |\n");
            out.push_str("| --- | --- | --- | --- | --- | --- |\n");
            for row in &self.prestige_runs {
                let first = match row.first_upgrade_secs {
                    Some(s) => format!("{s}s"),
                    None => "never".into(),
                };
                out.push_str(&format!(
                    "| {} | {} | {first} | {} | {:.0} | {:.0} |\n",
                    row.run, row.bonuses, row.metal_mine_level, row.metal, row.metal_per_hour
                ));
            }
            out.push('\n');
        }
        out.push_str("## Heavy industry\n\n");
        out.push_str(&format!(
            "- unrest without mitigation: {:.2} ({})\n",
            self.heavy.unrest_without,
            mark(self.heavy.mitigation_needed())
        ));
        out.push_str(&format!(
            "- unrest with radiators and scrubbers: {:.2} ({})\n",
            self.heavy.unrest_with,
            mark(self.heavy.viable_with_mitigation())
        ));
        out.push_str(&format!(
            "- metal/h without {:.0}  with {:.0}\n",
            self.heavy.metal_without, self.heavy.metal_with
        ));
        out.push_str(&format!(
            "- extra pirate ships without {}  with {}\n\n",
            self.heavy.raid_without, self.heavy.raid_with
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
    let quiet = run_once(config.clone(), days, false, 0);
    let busy = run_once(config.clone(), days, true, 0);
    let heavy = heavy_industry_check(&config);
    let keystone_secs = simulate_keystones(config.clone(), pacing.keystone_secs);
    let prestige_runs = vec![
        prestige_row(&quiet, 1, 0),
        prestige_row(&run_once(config.clone(), days, false, 1), 2, 1),
        prestige_row(&run_once(config, days, false, 2), 3, 2),
    ];
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
        prestige_runs,
        heavy,
        keystone_secs,
    }
}

/// Late-game mine snapshot: overflow without mitigation, viable once sinks are built.
#[derive(Debug, Clone, PartialEq)]
pub struct HeavyIndustryReport {
    pub unrest_without: f64,
    pub unrest_with: f64,
    pub metal_without: f64,
    pub metal_with: f64,
    pub raid_without: u32,
    pub raid_with: u32,
}

impl HeavyIndustryReport {
    pub fn mitigation_needed(&self) -> bool {
        self.unrest_without < 1.0 - 1e-9
    }

    pub fn viable_with_mitigation(&self) -> bool {
        self.unrest_with > 0.99 && self.metal_with + 1e-9 >= self.metal_without
    }
}

fn heavy_snapshot(config: BalanceConfig, radiator: u32, scrubber: u32) -> GameState {
    let mut state = GameState::with_config(0, config);
    state.buildings.metal_mine = 18;
    state.buildings.crystal_mine = 18;
    state.buildings.deuterium_synthesizer = 12;
    state.buildings.solar_plant = 24;
    state.buildings.radiator = radiator;
    state.buildings.scrubber = scrubber;
    state
}

/// Heavy mines on a rocky home: unrest without sinks, full speed once they are built.
pub fn heavy_industry_check(config: &BalanceConfig) -> HeavyIndustryReport {
    let raw = heavy_snapshot(config.clone(), 0, 0);
    let mitigated = heavy_snapshot(config.clone(), 5, 2);
    let waste_raw = raw.waste_home();
    let waste_ok = mitigated.waste_home();
    HeavyIndustryReport {
        unrest_without: waste_raw.unrest_factor(config.waste.unrest_per_overflow),
        unrest_with: waste_ok.unrest_factor(config.waste.unrest_per_overflow),
        metal_without: raw.production_per_hour(ResourceKind::Metal),
        metal_with: mitigated.production_per_hour(ResourceKind::Metal),
        raid_without: waste_raw.raid_ships(config.waste.raid_signature_per_overflow),
        raid_with: waste_ok.raid_ships(config.waste.raid_signature_per_overflow),
    }
}

fn prestige_row(run: &Run, index: u32, bonuses: u32) -> PrestigeSimRow {
    PrestigeSimRow {
        run: index,
        bonuses,
        first_upgrade_secs: run.first_upgrade_secs,
        metal_mine_level: run.metal_mine,
        metal: run.metal,
        metal_per_hour: run.metal_per_hour,
    }
}

struct Run {
    first_upgrade_secs: Option<u64>,
    metal_mine: u32,
    metal: f64,
    metal_per_hour: f64,
    days_log: Vec<DaySnapshot>,
}

fn run_once(mut config: BalanceConfig, days: u32, activity: bool, mine_focus: u32) -> Run {
    config.raid = crate::RaidConfig::inactive();
    config.activity.enabled = activity;
    let start = 0_u64;
    let end = u64::from(days) * 86_400;
    let mut state = GameState::with_config(start, config);
    if mine_focus > 0 {
        state.prestige.owned.insert("mine_focus".into(), mine_focus);
        state.resources = state.config.starting_resources;
        let (metal, crystal, deuterium) = state.prestige.starting_bonus(&state.config.prestige);
        state.resources.metal += metal;
        state.resources.crystal += crystal;
        state.resources.deuterium += deuterium;
    }
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
        metal_per_hour: state.production_per_hour(ResourceKind::Metal),
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

fn simulate_keystones(mut config: BalanceConfig, limit: u64) -> BTreeMap<String, Option<u64>> {
    config.raid = crate::RaidConfig::inactive();
    let mut state = GameState::with_config(0, config);
    state.resources.metal = 50_000.0;
    state.resources.crystal = 50_000.0;
    state.resources.deuterium = 50_000.0;
    let mut reached: BTreeMap<String, Option<u64>> = TechBranch::PLAYABLE
        .iter()
        .map(|b| (b.as_key().to_string(), None))
        .collect();
    let mut steps = 0_u32;
    while state.last_tick_unix < limit && steps < 200_000 {
        steps += 1;
        let _ = state.enqueue_build(BuildingKind::ResearchLab, state.last_tick_unix);
        let _ = state.enqueue_build(BuildingKind::SolarPlant, state.last_tick_unix);
        try_research_toward_keystones(&mut state);
        let mut jump = limit;
        if let Some(job) = state.build_queue.first()
            && let Some(at) = job.completes_at
        {
            jump = jump.min(at);
        }
        if let Some(job) = &state.research {
            jump = jump.min(job.completes_at);
        }
        if jump <= state.last_tick_unix {
            jump = state.last_tick_unix.saturating_add(1).min(limit);
        }
        state.catch_up(jump);
        for branch in TechBranch::PLAYABLE {
            let key = branch.as_key().to_string();
            if reached.get(&key).and_then(|t| *t).is_none() && state.keystone_complete(branch) {
                reached.insert(key, Some(state.last_tick_unix));
            }
        }
        if reached.values().all(|t| t.is_some()) {
            break;
        }
        if state.build_queue.is_empty() && state.research.is_none() {
            if jump >= limit {
                break;
            }
            state.catch_up(limit);
            break;
        }
    }
    reached
}

fn try_research_toward_keystones(state: &mut GameState) {
    if state.research.is_some() {
        return;
    }
    let mut candidates: Vec<(u64, String)> = Vec::new();
    for spec in state.config.techs.iter() {
        if spec.branch == TechBranch::Ancient {
            continue;
        }
        if state.tech_node_state(&spec.id) != TechNodeState::Available {
            continue;
        }
        let cost = spec.cost(state.tech_level(&spec.id));
        let weight = (cost.metal + cost.crystal + cost.deuterium) as u64;
        let prefer = on_keystone_path(state, &spec.id);
        let rank = if prefer {
            weight
        } else {
            weight.saturating_add(1_000_000)
        };
        candidates.push((rank, spec.id.clone()));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    for (_, id) in candidates {
        if state.start_research(&id, state.last_tick_unix).is_ok() {
            return;
        }
    }
}

fn on_keystone_path(state: &GameState, id: &str) -> bool {
    for branch in TechBranch::PLAYABLE {
        let Some(stone) = state.config.techs.keystone(branch) else {
            continue;
        };
        if state.keystone_complete(branch) {
            continue;
        }
        if id == stone.id || requires_tech(state, &stone.id, id) {
            return true;
        }
    }
    false
}

fn requires_tech(state: &GameState, from: &str, want: &str) -> bool {
    let mut stack = vec![from.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(spec) = state.config.techs.get(&id) else {
            continue;
        };
        for pre in &spec.prereq_techs {
            if pre.id == want {
                return true;
            }
            stack.push(pre.id.clone());
        }
    }
    false
}

fn fill_queue(state: &mut GameState) {
    let cap = state.build_queue_cap();
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

fn candidate_order(state: &GameState) -> Vec<BuildingKind> {
    let mut order = Vec::new();
    if !energy_ok_after(state, BuildingKind::MetalMine) {
        order.push(BuildingKind::SolarPlant);
    }
    if !heat_ok_after(state, BuildingKind::MetalMine) {
        order.push(BuildingKind::Radiator);
    }
    if !pollution_ok_after(state, BuildingKind::MetalMine) {
        order.push(BuildingKind::Scrubber);
    }
    order.extend([
        BuildingKind::MetalMine,
        BuildingKind::CrystalMine,
        BuildingKind::DeuteriumSynthesizer,
        BuildingKind::RoboticsFactory,
        BuildingKind::MetalStorage,
    ]);
    if energy_ok_after(state, BuildingKind::MetalMine) {
        order.push(BuildingKind::SolarPlant);
    }
    order
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
    let consumed = state.config.energy_draw(&buildings);
    consumed <= 0.0 || produced + 1e-9 >= consumed
}

fn waste_after(state: &GameState, extra: BuildingKind) -> crate::WasteReport {
    let mut buildings = projected_buildings(state);
    buildings.increment(extra);
    crate::waste::report(
        &state.config.waste,
        &state.config.chains.recipes,
        state.config.industry_energy(&buildings),
        buildings,
        state.home_kind,
        0.0,
        0.0,
    )
}

fn heat_ok_after(state: &GameState, extra: BuildingKind) -> bool {
    waste_after(state, extra).heat_ratio() <= 1.0 + 1e-9
}

fn pollution_ok_after(state: &GameState, extra: BuildingKind) -> bool {
    waste_after(state, extra).pollution_ratio() <= 1.0 + 1e-9
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

    #[test]
    fn prestige_runs_get_faster_without_trivialising_the_start() {
        let report = simulate_greedy(BalanceConfig::default(), 1);
        assert_eq!(report.prestige_runs.len(), 3);
        let first = &report.prestige_runs[0];
        let second = &report.prestige_runs[1];
        let third = &report.prestige_runs[2];
        assert!(
            first.metal_per_hour + 1e-9 < second.metal_per_hour,
            "{first:?} {second:?}"
        );
        assert!(
            second.metal_per_hour + 1e-9 < third.metal_per_hour,
            "{second:?} {third:?}"
        );
        let first_up = first.first_upgrade_secs.expect("run 1 upgrade");
        assert!(first_up > 0);
        assert!(first_up <= report.pacing.first_upgrade_secs);
        let later = third.first_upgrade_secs.expect("run 3 upgrade");
        assert!(later > 0, "later run still spends construction time");
        let md = report.to_markdown();
        assert!(md.contains("## Prestige runs"), "{md}");
    }

    #[test]
    fn heavy_industry_needs_mitigation_and_stays_viable_with_it() {
        let report = simulate_greedy(BalanceConfig::default(), 1);
        assert!(
            report.heavy.mitigation_needed(),
            "unrest {}",
            report.heavy.unrest_without
        );
        assert!(
            report.heavy.viable_with_mitigation(),
            "with unrest {} metal {} vs {}",
            report.heavy.unrest_with,
            report.heavy.metal_with,
            report.heavy.metal_without
        );
        assert!(
            report.heavy.raid_without > report.heavy.raid_with,
            "{:?}",
            report.heavy
        );
        let md = report.to_markdown();
        assert!(md.contains("## Heavy industry"), "{md}");
    }

    #[test]
    fn keystones_meet_galaxy_pacing() {
        let report = simulate_greedy(BalanceConfig::default(), 1);
        assert!(report.keystones_ok(), "{}", report.to_markdown());
        assert!(report.passed(), "{}", report.to_markdown());
        let md = report.to_markdown();
        assert!(md.contains("each branch keystone"), "{md}");
    }
}
