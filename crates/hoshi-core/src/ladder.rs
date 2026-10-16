//! Data-driven industrial goal ladder. Tiers complete from sustained rates.

use crate::production::ProductionReport;
use crate::state::{GameState, Resources};
use crate::wonders::{WonderError, WonderStage};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_GOALS: &str = include_str!("../data/goals.toml");

/// One completed tier, persisted on the save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderCompletion {
    pub id: String,
    pub at: u64,
}

/// Live progress for the goals panel and CLI.
#[derive(Debug, Clone, PartialEq)]
pub struct LadderProgress {
    pub current: Option<String>,
    pub next: Option<LadderGoalProgress>,
    pub completed: Vec<LadderCompletion>,
}

/// One incomplete (or just-met) goal with per-stat ratios.
#[derive(Debug, Clone, PartialEq)]
pub struct LadderGoalProgress {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub ratio: f64,
    pub next_step: String,
    pub ready: bool,
}

/// Hold timer while the next tier's rates are met.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LadderBoard {
    pub completed: Vec<LadderCompletion>,
    pub hold_id: Option<String>,
    pub hold_since: Option<u64>,
}

/// One industrial step-up.
#[derive(Debug, Clone, PartialEq)]
pub struct LadderTier {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub goods: BTreeMap<String, f64>,
    pub energy: f64,
    pub routes: u32,
    pub reward: Resources,
}

/// Catalog plus optional hold window.
#[derive(Debug, Clone, PartialEq)]
pub struct LadderConfig {
    pub hold_secs: u64,
    pub tiers: Vec<LadderTier>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LadderError {
    Parse(String),
    Invalid(String),
}

impl fmt::Display for LadderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LadderError::Parse(msg) | LadderError::Invalid(msg) => write!(f, "ladder: {msg}"),
        }
    }
}

impl std::error::Error for LadderError {}

impl LadderConfig {
    pub fn parse(toml: &str) -> Result<Self, LadderError> {
        let file: File = toml::from_str(toml).map_err(|e| LadderError::Parse(e.to_string()))?;
        file.into_config()
    }

    pub fn get(&self, id: &str) -> Option<&LadderTier> {
        self.tiers.iter().find(|tier| tier.id == id)
    }

    pub fn next_after(&self, completed: &[LadderCompletion]) -> Option<&LadderTier> {
        self.tiers
            .iter()
            .find(|tier| !completed.iter().any(|row| row.id == tier.id))
    }

    pub fn validate(&self) -> Result<(), LadderError> {
        let mut seen = std::collections::BTreeSet::new();
        if self.tiers.is_empty() {
            return Err(LadderError::Invalid("catalog is empty".into()));
        }
        for tier in &self.tiers {
            if tier.id.is_empty() {
                return Err(LadderError::Invalid("tier id must not be empty".into()));
            }
            if !seen.insert(tier.id.as_str()) {
                return Err(LadderError::Invalid(format!(
                    "duplicate tier id {}",
                    tier.id
                )));
            }
            if tier.name.is_empty() {
                return Err(LadderError::Invalid(format!(
                    "tier {} has no name",
                    tier.id
                )));
            }
            if tier.goods.is_empty() && tier.energy <= 0.0 && tier.routes == 0 {
                return Err(LadderError::Invalid(format!(
                    "tier {} needs a rate, energy, or route requirement",
                    tier.id
                )));
            }
            for (good, rate) in &tier.goods {
                if good.is_empty() {
                    return Err(LadderError::Invalid(format!(
                        "tier {} has an empty good id",
                        tier.id
                    )));
                }
                if !rate.is_finite() || *rate <= 0.0 {
                    return Err(LadderError::Invalid(format!(
                        "tier {} good {good} rate must be > 0",
                        tier.id
                    )));
                }
            }
            if !tier.energy.is_finite() || tier.energy < 0.0 {
                return Err(LadderError::Invalid(format!(
                    "tier {} energy must be >= 0",
                    tier.id
                )));
            }
            for (label, value) in [
                ("reward_metal", tier.reward.iron),
                ("reward_crystal", tier.reward.silicon),
                ("reward_deuterium", tier.reward.deuterium),
            ] {
                if !value.is_finite() || value < 0.0 {
                    return Err(LadderError::Invalid(format!(
                        "tier {} {label} must be >= 0",
                        tier.id
                    )));
                }
            }
        }
        Ok(())
    }
}

impl Default for LadderConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_GOALS).expect("bundled goals.toml must be valid")
    }
}

impl LadderBoard {
    pub fn current_id(&self) -> Option<&str> {
        self.completed.last().map(|row| row.id.as_str())
    }
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    hold_secs: u64,
    #[serde(default)]
    tier: Vec<FileTier>,
}

#[derive(Deserialize)]
struct FileTier {
    id: String,
    name: String,
    #[serde(default)]
    blurb: String,
    #[serde(default)]
    goods: BTreeMap<String, f64>,
    #[serde(default)]
    energy: f64,
    #[serde(default)]
    routes: u32,
    #[serde(default)]
    reward_metal: f64,
    #[serde(default)]
    reward_crystal: f64,
    #[serde(default)]
    reward_deuterium: f64,
}

impl File {
    fn into_config(self) -> Result<LadderConfig, LadderError> {
        let cfg = LadderConfig {
            hold_secs: self.hold_secs,
            tiers: self
                .tier
                .into_iter()
                .map(|row| LadderTier {
                    id: row.id,
                    name: row.name,
                    blurb: row.blurb,
                    goods: row.goods,
                    energy: row.energy,
                    routes: row.routes,
                    reward: Resources::from_parts(
                        row.reward_metal,
                        row.reward_crystal,
                        row.reward_deuterium,
                    ),
                })
                .collect(),
        };
        cfg.validate()?;
        Ok(cfg)
    }
}

impl GameState {
    pub fn ladder_progress(&self) -> LadderProgress {
        let next = self
            .config
            .ladder
            .next_after(&self.ladder.completed)
            .map(|tier| {
                let (ratio, next_step, ready) = self.tier_glance(tier);
                LadderGoalProgress {
                    id: tier.id.clone(),
                    name: tier.name.clone(),
                    blurb: tier.blurb.clone(),
                    ratio,
                    next_step,
                    ready,
                }
            });
        LadderProgress {
            current: self.ladder.current_id().map(str::to_string),
            next,
            completed: self.ladder.completed.clone(),
        }
    }

    pub fn good_output_per_hour(&self, id: &str) -> f64 {
        self.production_report().good_output_per_hour(id)
    }

    pub(crate) fn next_ladder_at(&self) -> Option<u64> {
        let hold = self.config.ladder.hold_secs;
        if hold == 0 {
            return None;
        }
        let id = self
            .config
            .ladder
            .next_after(&self.ladder.completed)?
            .id
            .as_str();
        let since = self.ladder.hold_since?;
        if self.ladder.hold_id.as_deref() != Some(id) {
            return None;
        }
        let ready = since.saturating_add(hold);
        (ready > self.last_tick_unix).then_some(ready)
    }

    pub(crate) fn evaluate_ladder(&mut self, at: u64) {
        loop {
            let Some(tier_id) = self
                .config
                .ladder
                .next_after(&self.ladder.completed)
                .map(|tier| tier.id.clone())
            else {
                self.ladder.hold_id = None;
                self.ladder.hold_since = None;
                return;
            };
            let Some(tier) = self.config.ladder.get(&tier_id).cloned() else {
                return;
            };
            if !self.tier_rates_met(&tier) {
                self.ladder.hold_id = None;
                self.ladder.hold_since = None;
                return;
            }
            let hold = self.config.ladder.hold_secs;
            if hold == 0 {
                self.complete_tier(&tier, at);
                continue;
            }
            if self.ladder.hold_id.as_deref() != Some(tier.id.as_str()) {
                self.ladder.hold_id = Some(tier.id.clone());
                self.ladder.hold_since = Some(at);
                return;
            }
            let since = self.ladder.hold_since.unwrap_or(at);
            if at.saturating_sub(since) < hold {
                return;
            }
            self.complete_tier(&tier, at);
        }
    }

    pub(crate) fn wonder_tier_ok(&self, stage: &WonderStage) -> Result<(), WonderError> {
        let Some(id) = stage.require_tier.as_deref() else {
            return Ok(());
        };
        let Some(tier) = self.config.ladder.get(id) else {
            return Err(WonderError::NeedTier { id: id.into() });
        };
        if self.tier_rates_met(tier) {
            Ok(())
        } else {
            Err(WonderError::NeedTier { id: id.into() })
        }
    }

    fn complete_tier(&mut self, tier: &LadderTier, at: u64) {
        self.resources = self.resources.saturating_add(tier.reward);
        self.ladder.completed.push(LadderCompletion {
            id: tier.id.clone(),
            at,
        });
        self.ladder.hold_id = None;
        self.ladder.hold_since = None;
        self.push_event(at, format!("goal: {} complete", tier.name));
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Other,
            actor: "simulation".into(),
            action: "complete".into(),
            objects: vec![format!("tier:{}", tier.id)],
            outcome: tier.name.clone(),
            ..crate::HistoryDraft::default()
        });
    }

    fn tier_rates_met(&self, tier: &LadderTier) -> bool {
        let report = self.production_report();
        self.tier_ratio(tier, &report) >= 1.0 - 1e-9
    }

    fn tier_glance(&self, tier: &LadderTier) -> (f64, String, bool) {
        let report = self.production_report();
        let ratio = self.tier_ratio(tier, &report);
        let ready = ratio >= 1.0 - 1e-9;
        let next_step = if ready {
            if self.config.ladder.hold_secs == 0 {
                format!("{} ready", tier.name)
            } else {
                format!("hold {} rates", tier.name)
            }
        } else {
            self.tier_gap(tier, &report)
        };
        (ratio.clamp(0.0, 1.0), next_step, ready)
    }

    fn tier_ratio(&self, tier: &LadderTier, report: &ProductionReport) -> f64 {
        let mut ratios = Vec::new();
        for (good, need) in &tier.goods {
            if *need <= 1e-12 {
                continue;
            }
            ratios.push(report.good_output_per_hour(good) / *need);
        }
        if tier.energy > 1e-12 {
            ratios.push(report.energy_supply / tier.energy);
        }
        if tier.routes > 0 {
            ratios.push(self.routes.len() as f64 / f64::from(tier.routes));
        }
        ratios.into_iter().fold(f64::INFINITY, f64::min)
    }

    fn tier_gap(&self, tier: &LadderTier, report: &ProductionReport) -> String {
        let mut worst: Option<(f64, String)> = None;
        let consider = |worst: &mut Option<(f64, String)>, ratio: f64, step: String| {
            if worst.as_ref().is_none_or(|(r, _)| ratio < *r) {
                *worst = Some((ratio, step));
            }
        };
        for (good, need) in &tier.goods {
            if *need <= 1e-12 {
                continue;
            }
            let got = report.good_output_per_hour(good);
            consider(
                &mut worst,
                got / *need,
                format!("raise {good} to {need}/h (have {got:.2})"),
            );
        }
        if tier.energy > 1e-12 {
            consider(
                &mut worst,
                report.energy_supply / tier.energy,
                format!(
                    "raise energy supply to {:.0} (have {:.0})",
                    tier.energy, report.energy_supply
                ),
            );
        }
        if tier.routes > 0 {
            consider(
                &mut worst,
                self.routes.len() as f64 / f64::from(tier.routes),
                format!("open {} repeating route(s)", tier.routes),
            );
        }
        worst
            .map(|(_, step)| step)
            .unwrap_or_else(|| format!("advance {}", tier.name))
    }
}

impl ProductionReport {
    pub fn good_output_per_hour(&self, id: &str) -> f64 {
        self.steps
            .iter()
            .map(|step| step.outputs_per_hour.get(id).copied().unwrap_or(0.0))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::state::GameState;

    fn quiet_config() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg
    }

    fn industrial(now: u64) -> GameState {
        let mut state = GameState::with_config(now, quiet_config());
        state.buildings.metal_mine = 18;
        state.buildings.crystal_mine = 16;
        state.buildings.deuterium_synthesizer = 12;
        state.buildings.solar_plant = 28;
        state.buildings.radiator = 6;
        state.buildings.scrubber = 3;
        state
    }

    #[test]
    fn bundled_catalog_has_five_industrial_tiers() {
        let cfg = LadderConfig::default();
        assert_eq!(cfg.tiers.len(), 5);
        assert_eq!(cfg.tiers[0].id, "water_propellant");
        assert_eq!(cfg.tiers[4].id, "wonder_supply");
        assert!(cfg.tiers[4].energy > 0.0);
    }

    #[test]
    fn late_industry_completes_the_ladder_from_rates_not_stockpiles() {
        let mut state = industrial(1_000);
        state.resources = Resources::ZERO;
        state.evaluate_ladder(1_000);
        let ids: Vec<_> = state
            .ladder
            .completed
            .iter()
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "water_propellant",
                "steel_electronics",
                "logistics",
                "helium3_fusion",
                "wonder_supply"
            ]
        );
        assert!(state.resources.iron > 0.0);
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text.contains("Wonder supply chain complete"))
        );
    }

    #[test]
    fn starter_empire_does_not_complete_a_tier() {
        let mut state = GameState::with_config(0, quiet_config());
        state.evaluate_ladder(0);
        assert!(state.ladder.completed.is_empty());
        let progress = state.ladder_progress();
        assert!(progress.current.is_none());
        let next = progress.next.expect("first tier");
        assert_eq!(next.id, "water_propellant");
        assert!(next.ratio < 1.0);
        assert!(!next.ready);
    }

    #[test]
    fn catch_up_split_matches_one_shot_for_tier_completion() {
        let mut gap = industrial(0);
        let mut step = industrial(0);
        gap.catch_up(86_400);
        let mut t = 0;
        while t < 86_400 {
            t += 3_600;
            step.catch_up(t);
        }
        assert_eq!(gap.ladder.completed, step.ladder.completed);
        assert_eq!(gap.resources.iron, step.resources.iron);
    }

    #[test]
    fn dyson_ring_needs_wonder_supply_rates() {
        let mut state = GameState::with_config(0, quiet_config());
        state.tech_levels.insert("energy".into(), 8);
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 10_000.0;
        let err = state.start_wonder("dyson_ring", 0).unwrap_err();
        assert!(
            matches!(err, WonderError::NeedTier { ref id } if id == "wonder_supply"),
            "{err}"
        );
        state = industrial(0);
        state.tech_levels.insert("energy".into(), 8);
        state.buildings.metal_storage = 10;
        state.buildings.crystal_storage = 10;
        state.buildings.deuterium_storage = 10;
        state.evaluate_ladder(0);
        state.start_wonder("dyson_ring", 0).unwrap();
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 10_000.0;
        let paid = state.contribute_wonder(state.home, 0).unwrap();
        assert!(paid.iron > 0.0);
    }

    #[test]
    fn hold_secs_delays_completion() {
        let mut cfg = quiet_config();
        cfg.ladder.hold_secs = 3_600;
        let mut state = GameState::with_config(0, cfg);
        state.buildings.metal_mine = 18;
        state.buildings.crystal_mine = 16;
        state.buildings.deuterium_synthesizer = 12;
        state.buildings.solar_plant = 28;
        state.buildings.radiator = 6;
        state.buildings.scrubber = 3;
        state.evaluate_ladder(0);
        assert!(state.ladder.completed.is_empty());
        assert_eq!(state.ladder.hold_id.as_deref(), Some("water_propellant"));
        state.evaluate_ladder(3_599);
        assert!(state.ladder.completed.is_empty());
        state.evaluate_ladder(3_600);
        assert_eq!(state.ladder.completed[0].id, "water_propellant");
    }
}
