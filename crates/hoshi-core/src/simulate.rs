//! Scripted greedy pacing run. Uses [`GameState::catch_up`] so the report
//! matches live production, queues and activity.

use crate::Colony;
use crate::activity::ActivityPulse;
use crate::balance::{BalanceConfig, PacingTargets};
use crate::buildings::{BuildError, BuildingKind, Buildings};
use crate::planets::PlanetType;
use crate::research::{TechBranch, TechNodeState};
use crate::ships::Fleet;
use crate::state::{GameState, ResourceKind};
use std::collections::BTreeMap;

/// One simulated prestige run: later runs own more bonuses and produce more.
#[derive(Debug, Clone, PartialEq)]
pub struct PrestigeSimRow {
    pub run: u32,
    pub bonuses: u32,
    pub first_upgrade_secs: Option<u64>,
    pub metal_mine_level: u32,
    pub iron: f64,
    pub metal_per_hour: f64,
}

/// One day's stockpile and mine level at the day boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct DaySnapshot {
    pub day: u32,
    pub metal_mine: u32,
    pub crystal_mine: u32,
    pub solar_plant: u32,
    pub iron: f64,
    pub silicon: f64,
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
    pub tactics_ok: bool,
    pub victory_paths: Vec<VictorySimRow>,
    pub victory_min_secs: u64,
    pub victory_max_secs: u64,
    pub ladder_tiers: Vec<LadderSimRow>,
    pub frontier_unlock_secs: Option<u64>,
}

/// One casual-play victory path and when it landed.
#[derive(Debug, Clone, PartialEq)]
pub struct VictorySimRow {
    pub path: String,
    pub elapsed_secs: Option<u64>,
}

/// When a ladder tier first held its sustained rates in the greedy run.
#[derive(Debug, Clone, PartialEq)]
pub struct LadderSimRow {
    pub id: String,
    pub elapsed_secs: Option<u64>,
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
        self.first_upgrade_ok()
            && self.mine_goal_ok()
            && self.bridge_ok()
            && self.keystones_ok()
            && self.tactics_ok
            && self.ladder_ok()
            && self.frontier_ok()
    }

    pub fn frontier_ok(&self) -> bool {
        match self.frontier_unlock_secs {
            Some(secs) => secs <= self.victory_max_secs.max(1),
            None => u64::from(self.days).saturating_mul(86_400) < self.victory_min_secs.max(1),
        }
    }

    pub fn ladder_ok(&self) -> bool {
        if self.ladder_tiers.is_empty() {
            return true;
        }
        let max = self.victory_max_secs.max(1);
        self.ladder_tiers
            .iter()
            .all(|row| row.elapsed_secs.is_some_and(|secs| secs <= max))
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
        out.push_str(&format!(
            "- tactics: no combo dominates ({})\n",
            mark(self.tactics_ok)
        ));
        if self.frontier_unlock_secs.is_some() || self.victory_max_secs > 0 {
            let elapsed = match self.frontier_unlock_secs {
                Some(secs) => format!("{secs}s"),
                None => "never".into(),
            };
            out.push_str(&format!(
                "- frontier unlock: {elapsed} ({})\n",
                mark(self.frontier_ok())
            ));
        }
        if !self.ladder_tiers.is_empty() {
            out.push_str("\n## Goal ladder\n\n");
            out.push_str("| tier | elapsed | band |\n");
            out.push_str("| --- | --- | --- |\n");
            let max = self.victory_max_secs;
            for row in &self.ladder_tiers {
                let elapsed = match row.elapsed_secs {
                    Some(secs) => format!("{secs}s"),
                    None => "never".into(),
                };
                let band = match row.elapsed_secs {
                    Some(secs) if secs <= max => "pass",
                    _ => "fail",
                };
                out.push_str(&format!("| {} | {elapsed} | {band} |\n", row.id));
            }
            out.push_str(&format!(
                "\nLadder within {max}s: {}\n",
                mark(self.ladder_ok())
            ));
        }
        if !self.victory_paths.is_empty() {
            out.push_str("\n## Victory paths\n\n");
            out.push_str("Casual opener of 14 days, then a focused finish per path.\n\n");
            out.push_str("| path | elapsed | band |\n");
            out.push_str("| --- | --- | --- |\n");
            let min = self.victory_min_secs;
            let max = self.victory_max_secs;
            for row in &self.victory_paths {
                let elapsed = match row.elapsed_secs {
                    Some(secs) => format!("{secs}s"),
                    None => "never".into(),
                };
                let in_band = row
                    .elapsed_secs
                    .is_some_and(|secs| secs >= min && secs <= max);
                out.push_str(&format!(
                    "| {} | {elapsed} | {} |\n",
                    row.path,
                    mark(in_band)
                ));
            }
            out.push('\n');
        }
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
                    row.run, row.bonuses, row.metal_mine_level, row.iron, row.metal_per_hour
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
                row.iron,
                row.silicon,
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
    let tactics_ok = tactics_no_combo_dominates(&config);
    let victory_min_secs = config.victory.casual_min_secs;
    let victory_max_secs = config.victory.casual_max_secs;
    let victory_paths = if u64::from(days).saturating_mul(86_400) >= victory_min_secs {
        simulate_victory_paths(config.clone())
    } else {
        Vec::new()
    };
    let ladder_tiers = simulate_ladder_tiers(config.clone(), days);
    let frontier_unlock_secs = if u64::from(days).saturating_mul(86_400) >= victory_min_secs {
        simulate_frontier_unlock(config.clone())
    } else {
        None
    };
    let prestige_runs = vec![
        prestige_row(&quiet, 1, 0),
        prestige_row(&run_once(config.clone(), days, false, 1), 2, 1),
        prestige_row(&run_once(config, days, false, 2), 3, 2),
    ];
    let ratio = if quiet.iron > 1e-9 {
        busy.iron / quiet.iron
    } else {
        f64::INFINITY
    };
    SimReport {
        days,
        first_upgrade_secs: quiet.first_upgrade_secs,
        metal_mine_level: quiet.metal_mine,
        metal_without_bridge: quiet.iron,
        metal_with_bridge: busy.iron,
        bridge_metal_ratio: ratio,
        days_log: quiet.days_log,
        pacing,
        prestige_runs,
        heavy,
        keystone_secs,
        tactics_ok,
        victory_paths,
        victory_min_secs,
        victory_max_secs,
        ladder_tiers,
        frontier_unlock_secs,
    }
}

/// True when no formation+stance pair wins every seeded sample.
pub fn tactics_no_combo_dominates(config: &BalanceConfig) -> bool {
    use crate::combat::{rapid_fire_table, stats_from_catalog};
    use crate::ships::ShipKind;
    use crate::{Fleet, Formation, Stance, TacticalPlan, Terrain, resolve_tactical};
    let stats = stats_from_catalog(&config.ships);
    let rf = rapid_fire_table(&config.ships);
    let mut atk = Fleet::default();
    atk.add(ShipKind::LightFighter, 8);
    atk.add(ShipKind::Cruiser, 2);
    atk.add(ShipKind::Battleship, 1);
    let mut def = Fleet::default();
    def.add(ShipKind::LightFighter, 6);
    def.add(ShipKind::HeavyFighter, 4);
    def.add(ShipKind::Battleship, 2);
    let mut wins = Vec::new();
    for formation in Formation::ALL {
        for stance in Stance::ALL {
            let plan = TacticalPlan {
                formation,
                stance,
                terrain: Terrain::Open,
            };
            let mut n = 0u32;
            for seed in 1u64..=8 {
                let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let report = resolve_tactical(
                    atk,
                    def,
                    &stats,
                    &stats,
                    &rf,
                    &mut rng,
                    &config.tactics,
                    plan,
                    TacticalPlan::default(),
                    crate::FleetTactics::default(),
                    None,
                    None,
                    &[],
                    Fleet::default(),
                );
                if report.winner == crate::CombatWinner::Attacker {
                    n += 1;
                }
            }
            wins.push(n);
        }
    }
    let max = wins.iter().copied().max().unwrap_or(0);
    let min = wins.iter().copied().min().unwrap_or(0);
    max < 8 && max.saturating_sub(min) < 7
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
        metal_without: raw.production_per_hour(ResourceKind::Iron),
        metal_with: mitigated.production_per_hour(ResourceKind::Iron),
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
        iron: run.iron,
        metal_per_hour: run.metal_per_hour,
    }
}

struct Run {
    first_upgrade_secs: Option<u64>,
    metal_mine: u32,
    iron: f64,
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
        state.resources.iron += metal;
        state.resources.silicon += crystal;
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
        iron: state.resources.iron,
        metal_per_hour: state.production_per_hour(ResourceKind::Iron),
        days_log,
    }
}

fn snapshot(state: &GameState, day: u32) -> DaySnapshot {
    DaySnapshot {
        day,
        metal_mine: state.buildings.metal_mine,
        crystal_mine: state.buildings.crystal_mine,
        solar_plant: state.buildings.solar_plant,
        iron: state.resources.iron,
        silicon: state.resources.silicon,
        deuterium: state.resources.deuterium,
    }
}

fn simulate_keystones(mut config: BalanceConfig, limit: u64) -> BTreeMap<String, Option<u64>> {
    config.raid = crate::RaidConfig::inactive();
    let mut state = GameState::with_config(0, config);
    state.resources.iron = 50_000.0;
    state.resources.silicon = 50_000.0;
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
        let weight = (cost.iron + cost.silicon + cost.deuterium) as u64;
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

fn simulate_frontier_unlock(mut config: BalanceConfig) -> Option<u64> {
    if config.frontier.sectors == 0 {
        return None;
    }
    let max = config.victory.casual_max_secs.max(1);
    config.raid = crate::RaidConfig::inactive();
    config.market.enabled = false;
    let mut state = GameState::with_config(0, config);
    let mut steps = 0_u32;
    while state.last_tick_unix < max && steps < 100_000 {
        steps += 1;
        fill_frontier_queue(&mut state);
        try_research_toward_frontier(&mut state);
        let completion = state
            .build_queue
            .first()
            .and_then(|job| job.completes_at)
            .unwrap_or(u64::MAX);
        let research_at = state
            .research
            .as_ref()
            .map(|job| job.completes_at)
            .unwrap_or(u64::MAX);
        let afford = frontier_next_afford_at(&state).unwrap_or(u64::MAX);
        let mut jump = completion.min(research_at).min(afford).min(max);
        if jump <= state.last_tick_unix {
            jump = state.last_tick_unix.saturating_add(1).min(max);
        }
        state.catch_up(jump);
        if state.frontier.revealed > 0 {
            return Some(state.last_tick_unix);
        }
        if state.build_queue.is_empty() && state.research.is_none() && jump >= max {
            break;
        }
    }
    None
}

fn fill_frontier_queue(state: &mut GameState) {
    let cap = state.build_queue_cap();
    while state.build_queue.len() < cap {
        let order = frontier_building_order(state);
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
                | Err(BuildError::NeedLogistics)
                | Err(BuildError::PermissionDenied(_)) => {}
            }
        }
        if !enqueued {
            break;
        }
    }
}

fn frontier_building_order(state: &GameState) -> Vec<BuildingKind> {
    let mut order = Vec::new();
    let need_lab = frontier_needed_lab(state);
    if !energy_ok_after(state, BuildingKind::ResearchLab)
        || !energy_ok_after(state, BuildingKind::DeuteriumSynthesizer)
    {
        order.push(BuildingKind::SolarPlant);
    }
    if !heat_ok_after(state, BuildingKind::MetalMine) {
        order.push(BuildingKind::Radiator);
    }
    if !pollution_ok_after(state, BuildingKind::MetalMine) {
        order.push(BuildingKind::Scrubber);
    }
    if projected_buildings(state).research_lab < need_lab {
        order.push(BuildingKind::ResearchLab);
    }
    if projected_buildings(state).deuterium_synthesizer < 4 {
        order.push(BuildingKind::DeuteriumSynthesizer);
    }
    if frontier_needs_storage(state, ResourceKind::Silicon) {
        order.push(BuildingKind::CrystalStorage);
    }
    if frontier_needs_storage(state, ResourceKind::Deuterium) {
        order.push(BuildingKind::DeuteriumStorage);
    }
    if frontier_needs_storage(state, ResourceKind::Iron) {
        order.push(BuildingKind::MetalStorage);
    }
    order.extend([
        BuildingKind::CrystalMine,
        BuildingKind::DeuteriumSynthesizer,
        BuildingKind::MetalMine,
        BuildingKind::RoboticsFactory,
        BuildingKind::CrystalStorage,
        BuildingKind::DeuteriumStorage,
        BuildingKind::MetalStorage,
        BuildingKind::SolarPlant,
        BuildingKind::ResearchLab,
    ]);
    order
}

fn frontier_needs_storage(state: &GameState, kind: ResourceKind) -> bool {
    let cap = state.storage_cap(kind);
    for (id, want) in frontier_tech_need(state) {
        let have = state.tech_level(&id);
        if have >= want {
            continue;
        }
        let Some(spec) = state.config.techs.get(&id) else {
            continue;
        };
        if spec.cost(have).get(kind) > cap + 1e-9 {
            return true;
        }
    }
    false
}

fn frontier_needed_lab(state: &GameState) -> u32 {
    let mut need = 1_u32;
    for (id, want) in frontier_tech_need(state) {
        if state.tech_level(&id) >= want {
            continue;
        }
        let Some(spec) = state.config.techs.get(&id) else {
            continue;
        };
        for pre in &spec.prereq_buildings {
            if pre.building == BuildingKind::ResearchLab {
                need = need.max(pre.level);
            }
        }
    }
    need
}

fn frontier_tech_need(state: &GameState) -> BTreeMap<String, u32> {
    let cfg = &state.config.frontier;
    let mut need: BTreeMap<String, u32> = BTreeMap::new();
    // Unlock is drive + astrophysics only. Flight drive is a later travel check.
    let mut stack = vec![
        (cfg.drive_id.clone(), cfg.drive_level.max(1)),
        (cfg.astro_id.clone(), cfg.astro_level.max(1)),
    ];
    while let Some((id, level)) = stack.pop() {
        let entry = need.entry(id.clone()).or_insert(0);
        if *entry >= level {
            continue;
        }
        *entry = level;
        let Some(spec) = state.config.techs.get(&id) else {
            continue;
        };
        for pre in &spec.prereq_techs {
            stack.push((pre.id.clone(), pre.level.max(1)));
        }
    }
    need
}

fn try_research_toward_frontier(state: &mut GameState) {
    if state.research.is_some() || state.research_unlocks_frontier() {
        return;
    }
    let need = frontier_tech_need(state);
    let mut candidates: Vec<(u64, String)> = Vec::new();
    for spec in state.config.techs.iter() {
        if spec.branch == TechBranch::Ancient {
            continue;
        }
        let want = need.get(&spec.id).copied().unwrap_or(0);
        if want == 0 {
            continue;
        }
        let have = state.tech_level(&spec.id);
        if have >= want {
            continue;
        }
        match state.tech_node_state(&spec.id) {
            TechNodeState::Available | TechNodeState::Researched => {}
            _ => continue,
        }
        let cost = spec.cost(have);
        let weight = (cost.iron + cost.silicon + cost.deuterium) as u64;
        candidates.push((weight, spec.id.clone()));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    for (_, id) in candidates {
        if state.start_research(&id, state.last_tick_unix).is_ok() {
            return;
        }
    }
}

fn frontier_next_afford_at(state: &GameState) -> Option<u64> {
    let now = state.last_tick_unix;
    let mut best: Option<u64> = None;
    for kind in frontier_building_order(state) {
        if let Some(wait) = secs_until_afford(state, kind) {
            let at = now.saturating_add(wait);
            best = Some(best.map_or(at, |b| b.min(at)));
        }
    }
    if let Some(wait) = secs_until_path_research(state) {
        let at = now.saturating_add(wait);
        best = Some(best.map_or(at, |b| b.min(at)));
    }
    best
}

fn secs_until_path_research(state: &GameState) -> Option<u64> {
    if state.research.is_some() {
        return None;
    }
    let need = frontier_tech_need(state);
    let mut best: Option<u64> = None;
    for spec in state.config.techs.iter() {
        let want = need.get(&spec.id).copied().unwrap_or(0);
        if want == 0 {
            continue;
        }
        let have = state.tech_level(&spec.id);
        if have >= want {
            continue;
        }
        match state.tech_node_state(&spec.id) {
            TechNodeState::Available | TechNodeState::Researched => {}
            _ => continue,
        }
        let cost = spec.cost(have);
        if let Some(wait) = secs_until_cost(state, cost) {
            best = Some(best.map_or(wait, |b| b.min(wait)));
        }
    }
    best
}

fn secs_until_cost(state: &GameState, cost: crate::Resources) -> Option<u64> {
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
        if need > state.storage_cap(resource) + 1e-9 {
            return None;
        }
        let rate = state.production_per_hour(resource);
        if rate <= 1e-12 {
            return None;
        }
        wait = wait.max((need - got) / rate * 3600.0);
    }
    Some(wait.ceil().max(1.0) as u64)
}

fn boost_industry_for_ladder(state: &mut GameState) {
    state.buildings.metal_mine = state.buildings.metal_mine.max(18);
    state.buildings.crystal_mine = state.buildings.crystal_mine.max(16);
    state.buildings.deuterium_synthesizer = state.buildings.deuterium_synthesizer.max(12);
    state.buildings.solar_plant = state.buildings.solar_plant.max(28);
    state.buildings.radiator = state.buildings.radiator.max(6);
    state.buildings.scrubber = state.buildings.scrubber.max(3);
    state.buildings.metal_storage = state.buildings.metal_storage.max(8);
    state.buildings.crystal_storage = state.buildings.crystal_storage.max(8);
    state.buildings.deuterium_storage = state.buildings.deuterium_storage.max(8);
}

fn simulate_ladder_tiers(mut config: BalanceConfig, days: u32) -> Vec<LadderSimRow> {
    let ids: Vec<String> = config.ladder.tiers.iter().map(|t| t.id.clone()).collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let max = config
        .victory
        .casual_max_secs
        .max(u64::from(days.saturating_mul(86_400)));
    config.raid = crate::RaidConfig::inactive();
    let mut state = GameState::with_config(0, config);
    let mut reached: std::collections::BTreeMap<String, Option<u64>> =
        ids.iter().map(|id| (id.clone(), None)).collect();
    let mut next_day = 86_400_u64;
    let mut steps = 0_u32;
    while state.last_tick_unix < max && steps < 100_000 {
        steps += 1;
        fill_queue(&mut state);
        let completion = state
            .build_queue
            .first()
            .and_then(|job| job.completes_at)
            .unwrap_or(u64::MAX);
        let afford = next_afford_at(&state).unwrap_or(u64::MAX);
        let mut jump = completion.min(afford).min(next_day).min(max);
        if jump <= state.last_tick_unix {
            jump = state.last_tick_unix.saturating_add(1).min(max);
        }
        state.catch_up(jump);
        for id in &ids {
            if reached.get(id).and_then(|t| *t).is_none()
                && state.ladder.completed.iter().any(|row| row.id == *id)
            {
                let at = state
                    .ladder
                    .completed
                    .iter()
                    .find(|row| row.id == *id)
                    .map(|row| row.at)
                    .unwrap_or(state.last_tick_unix);
                reached.insert(id.clone(), Some(at));
            }
        }
        if reached.values().all(|t| t.is_some()) {
            break;
        }
        while state.last_tick_unix >= next_day {
            next_day = next_day.saturating_add(86_400);
        }
        if state.build_queue.is_empty() && jump >= max {
            break;
        }
    }
    ids.into_iter()
        .map(|id| LadderSimRow {
            elapsed_secs: reached.get(&id).copied().flatten(),
            id,
        })
        .collect()
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
                | Err(BuildError::NeedLogistics)
                | Err(BuildError::PermissionDenied(_)) => {}
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

/// Casual 14-day opener, then a focused finish for each galaxy victory path.
pub fn simulate_victory_paths(config: BalanceConfig) -> Vec<VictorySimRow> {
    simulate_victory_paths_for(config, None, false, None)
}

/// Same paths with three NPC rivals ticking on a daily cadence.
pub fn simulate_victory_paths_with_rivals(config: BalanceConfig) -> Vec<VictorySimRow> {
    simulate_victory_paths_for(config, None, true, None)
}

/// Same paths as [`simulate_victory_paths`], once per bundled species.
pub fn simulate_species_victory_paths(config: BalanceConfig) -> Vec<(String, Vec<VictorySimRow>)> {
    let catalog = crate::SpeciesCatalog::default();
    catalog
        .iter()
        .map(|spec| {
            (
                spec.id.clone(),
                simulate_victory_paths_for(config.clone(), Some(spec), false, None),
            )
        })
        .collect()
}

/// Victory strategies for one species id on a galaxy seed. `species_id` None is Humans defaults.
pub fn simulate_victory_paths_for_seed(
    config: BalanceConfig,
    species_id: Option<&str>,
    seed: u64,
) -> Vec<VictorySimRow> {
    let catalog = crate::SpeciesCatalog::default();
    let spec = species_id.and_then(|id| catalog.get(id));
    simulate_victory_paths_for(config, spec, false, Some(seed))
}

/// Greedy mine curve for one species and seed. Used by the self-play report.
pub fn greedy_curve(
    mut config: BalanceConfig,
    species_id: Option<&str>,
    seed: u64,
    days: u32,
) -> Vec<DaySnapshot> {
    let days = days.max(1);
    config.raid = crate::RaidConfig::inactive();
    config.activity.enabled = false;
    let catalog = crate::SpeciesCatalog::default();
    let spec = species_id.and_then(|id| catalog.get(id));
    if let Some(row) = spec {
        config = catalog
            .overlay(&row.id, config)
            .expect("bundled species overlay")
            .0;
    }
    let start = 0_u64;
    let end = u64::from(days) * 86_400;
    let mut state = GameState::with_seed(start, config, seed);
    if let Some(row) = spec {
        for (faction, delta) in &row.standing {
            let next = state.diplomacy.standing(faction) + *delta;
            state.diplomacy.set_standing(faction, next);
        }
        state.species = Some(row.run());
    }
    let mut days_log = Vec::new();
    let mut next_day = 86_400_u64;
    let mut steps = 0_u32;
    while state.last_tick_unix < end && steps < 100_000 {
        steps += 1;
        fill_queue(&mut state);
        let completion = state
            .build_queue
            .first()
            .and_then(|job| job.completes_at)
            .unwrap_or(u64::MAX);
        let afford = next_afford_at(&state).unwrap_or(u64::MAX);
        let mut jump = completion.min(afford).min(next_day).min(end);
        if jump <= state.last_tick_unix {
            jump = state.last_tick_unix.saturating_add(1).min(end);
        }
        state.catch_up(jump);
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
    days_log
}

fn simulate_victory_paths_for(
    config: BalanceConfig,
    species: Option<&crate::SpeciesSpec>,
    rivals: bool,
    seed: Option<u64>,
) -> Vec<VictorySimRow> {
    let min = config.victory.casual_min_secs;
    let max = config.victory.casual_max_secs;
    vec![
        finish_wonder(config.clone(), min, max, species, rivals, seed),
        finish_diplomacy(config.clone(), min, max, species, rivals, seed),
        finish_conquest(config, min, max, species, rivals, seed),
    ]
}

fn developed(
    mut config: BalanceConfig,
    until: u64,
    species: Option<&crate::SpeciesSpec>,
    rivals: bool,
    seed: Option<u64>,
) -> GameState {
    config.raid = crate::RaidConfig::inactive();
    config.market.enabled = false;
    config.activity.enabled = false;
    if let Some(spec) = species {
        config = crate::SpeciesCatalog::default()
            .overlay(&spec.id, config)
            .expect("bundled species overlay")
            .0;
    }
    let mut state = match seed {
        Some(seed) => GameState::with_seed(0, config, seed),
        None => GameState::with_config(0, config),
    };
    if let Some(spec) = species {
        for (faction, delta) in &spec.standing {
            let next = state.diplomacy.standing(faction) + *delta;
            state.diplomacy.set_standing(faction, next);
        }
        state.species = Some(spec.run());
    }
    if rivals {
        let mut settings = crate::RivalSettings::on();
        settings.tick_secs = 86_400;
        state
            .spawn_rivals(&crate::RivalCatalog::default(), settings)
            .expect("bundled rival catalog");
    }
    let mut steps = 0_u32;
    while state.last_tick_unix < until && steps < 200_000 {
        steps += 1;
        fill_queue(&mut state);
        try_research_toward_keystones(&mut state);
        let mut jump = until;
        if let Some(job) = state.build_queue.first()
            && let Some(at) = job.completes_at
        {
            jump = jump.min(at);
        }
        if let Some(job) = &state.research {
            jump = jump.min(job.completes_at);
        }
        if jump <= state.last_tick_unix {
            jump = state.last_tick_unix.saturating_add(1).min(until);
        }
        state.catch_up(jump);
        if state.build_queue.is_empty() && state.research.is_none() && jump >= until {
            break;
        }
    }
    if state.last_tick_unix < until {
        state.catch_up(until);
    }
    state
}

fn finish_wonder(
    config: BalanceConfig,
    min: u64,
    max: u64,
    species: Option<&crate::SpeciesSpec>,
    rivals: bool,
    seed: Option<u64>,
) -> VictorySimRow {
    let id = config.victory.wonder_id.clone();
    let mut state = developed(config, min, species, rivals, seed);
    state.tech_levels.insert("energy".into(), 8);
    boost_industry_for_ladder(&mut state);
    state.evaluate_ladder(state.last_tick_unix);
    state.resources.iron += 100_000.0;
    state.resources.silicon += 100_000.0;
    state.resources.deuterium += 50_000.0;
    if state.start_wonder(&id, state.last_tick_unix).is_err() {
        return VictorySimRow {
            path: "wonder".into(),
            elapsed_secs: None,
        };
    }
    let mut steps = 0_u32;
    while state.victory.is_none() && state.last_tick_unix < max && steps < 50_000 {
        steps += 1;
        let _ = state.contribute_wonder(state.home, state.last_tick_unix);
        let jump = state
            .wonder
            .as_ref()
            .and_then(|job| job.completes_at)
            .unwrap_or(state.last_tick_unix.saturating_add(86_400))
            .min(max);
        if jump <= state.last_tick_unix {
            state.catch_up(state.last_tick_unix.saturating_add(1).min(max));
        } else {
            state.catch_up(jump);
        }
    }
    VictorySimRow {
        path: "wonder".into(),
        elapsed_secs: state.victory.as_ref().map(|record| record.elapsed_secs),
    }
}

fn finish_diplomacy(
    mut config: BalanceConfig,
    min: u64,
    max: u64,
    species: Option<&crate::SpeciesSpec>,
    rivals: bool,
    seed: Option<u64>,
) -> VictorySimRow {
    config.diplomacy.decay = 0.0;
    let standing = config.diplomacy.max_standing;
    let hold = config.victory.hold_secs;
    let mut state = developed(config, min, species, rivals, seed);
    let ids: Vec<String> = state
        .config
        .diplomacy
        .iter()
        .map(|faction| faction.id.clone())
        .collect();
    for id in ids {
        state.diplomacy.set_standing(&id, standing);
    }
    state.maybe_declare_victory(state.last_tick_unix);
    let ready_at = state.last_tick_unix.saturating_add(hold).min(max);
    if state.last_tick_unix < ready_at {
        state.catch_up(ready_at);
    }
    VictorySimRow {
        path: "diplomacy".into(),
        elapsed_secs: state.victory.as_ref().map(|record| record.elapsed_secs),
    }
}

fn finish_conquest(
    config: BalanceConfig,
    min: u64,
    max: u64,
    species: Option<&crate::SpeciesSpec>,
    rivals: bool,
    seed: Option<u64>,
) -> VictorySimRow {
    let mut state = developed(config, min, species, rivals, seed);
    if let Some(home) = state.pirate_homeworld() {
        state.outposts.retain(|outpost| outpost.coord != home);
        state.colonies.push(Colony {
            coord: home,
            kind: PlanetType::Rocky,
            size: 8,
            resources: crate::Resources::ZERO,
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.maybe_declare_victory(state.last_tick_unix.min(max));
    }
    VictorySimRow {
        path: "conquest".into(),
        elapsed_secs: state.victory.as_ref().map(|record| record.elapsed_secs),
    }
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
        assert!(report.tactics_ok, "{}", report.to_markdown());
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
    fn ladder_fits_galaxy_pacing() {
        let report = simulate_greedy(BalanceConfig::default(), 28);
        assert!(
            report.ladder_ok(),
            "{}",
            report
                .ladder_tiers
                .iter()
                .map(|row| format!("{} {:?}", row.id, row.elapsed_secs))
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert_eq!(report.ladder_tiers.len(), 5);
    }

    #[test]
    fn frontier_unlocks_inside_casual_band() {
        let report = simulate_greedy(BalanceConfig::default(), 28);
        assert!(
            report.frontier_ok(),
            "frontier unlock {:?}",
            report.frontier_unlock_secs
        );
        assert!(report.frontier_unlock_secs.is_some());
        let md = report.to_markdown();
        assert!(md.contains("frontier unlock"), "{md}");
    }

    #[test]
    fn keystones_meet_galaxy_pacing() {
        let report = simulate_greedy(BalanceConfig::default(), 1);
        assert!(report.keystones_ok(), "{}", report.to_markdown());
        assert!(report.passed(), "{}", report.to_markdown());
        let md = report.to_markdown();
        assert!(md.contains("each branch keystone"), "{md}");
    }

    #[test]
    fn season_overlays_keep_three_rule_sets() {
        let cat = crate::SeasonCatalog::default();
        assert!(cat.len() >= 3);
        for ruleset in cat.iter() {
            let mut cfg = BalanceConfig::default();
            ruleset.apply(&mut cfg);
        }
        let mut closed = BalanceConfig::default();
        closed.market.enabled = true;
        cat.get("closed_market").unwrap().apply(&mut closed);
        assert!(!closed.market.enabled);
    }

    #[test]
    fn victory_paths_land_in_the_casual_band() {
        let config = BalanceConfig::default();
        let min = config.victory.casual_min_secs;
        let max = config.victory.casual_max_secs;
        let rows = simulate_victory_paths(config);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            let elapsed = row.elapsed_secs.expect(&row.path);
            assert!(
                elapsed >= min && elapsed <= max,
                "{} at {elapsed}s outside {min}..={max}",
                row.path
            );
        }
    }

    #[test]
    fn every_species_lands_in_the_casual_band_without_a_runaway_lead() {
        let config = BalanceConfig::default();
        let min = config.victory.casual_min_secs;
        let max = config.victory.casual_max_secs;
        let sets = simulate_species_victory_paths(config);
        assert_eq!(sets.len(), 5);
        let mut by_path: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
        for (id, rows) in &sets {
            assert_eq!(rows.len(), 3, "{id}");
            for row in rows {
                let elapsed = row
                    .elapsed_secs
                    .unwrap_or_else(|| panic!("{id} {}", row.path));
                assert!(
                    elapsed >= min && elapsed <= max,
                    "{id} {} at {elapsed}s outside {min}..={max}",
                    row.path
                );
                by_path.entry(row.path.as_str()).or_default().push(elapsed);
            }
        }
        for (path, mut times) in by_path {
            times.sort_unstable();
            let median = times[times.len() / 2];
            let floor = median.saturating_sub(median / 4);
            for elapsed in times {
                assert!(
                    elapsed >= floor,
                    "{path} {elapsed}s is more than 25% faster than median {median}"
                );
            }
        }
    }

    #[test]
    fn rivals_leave_each_victory_path_beatable() {
        let config = BalanceConfig::default();
        let min = config.victory.casual_min_secs;
        let max = config.victory.casual_max_secs;
        let rows = simulate_victory_paths_with_rivals(config);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            let elapsed = row.elapsed_secs.expect(&row.path);
            assert!(
                elapsed >= min && elapsed <= max,
                "{} at {elapsed}s outside {min}..={max}",
                row.path
            );
        }
    }
}
