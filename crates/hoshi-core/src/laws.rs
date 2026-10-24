//! Laws as governed actions: propose, approve, enact, repeal.
//!
//! Catalog lives in `data/laws.toml`. The player is the only approver until a
//! later council exists. Effects reuse production, market and waste maths.

use crate::fleets::Coord;
use crate::research::ModifierKind;
use serde::Deserialize;
use std::fmt;

const DEFAULT_LAWS: &str = include_str!("../data/laws.toml");
pub const PLAYER: &str = "player";
pub const CLOCK: &str = "clock";

#[derive(Debug, Clone, PartialEq)]
pub enum LawError {
    Parse(String),
    DuplicateId(String),
    UnknownTemplate(String),
    UnknownLaw(u64),
    ParamOutOfRange { min: f64, max: f64 },
    JurisdictionMismatch,
    UnownedWorld,
    SlotTaken,
    NotProposed,
    NotEnacted,
    AlreadyApproved,
    Disabled,
    PermissionDenied(String),
}

impl fmt::Display for LawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LawError::Parse(msg) => write!(f, "laws: {msg}"),
            LawError::DuplicateId(id) => write!(f, "laws: duplicate template {id}"),
            LawError::UnknownTemplate(id) => write!(f, "unknown law {id}"),
            LawError::UnknownLaw(id) => write!(f, "unknown law #{id}"),
            LawError::ParamOutOfRange { min, max } => {
                write!(f, "law parameter must be in [{min}, {max}]")
            }
            LawError::JurisdictionMismatch => write!(f, "jurisdiction does not match the template"),
            LawError::UnownedWorld => write!(f, "planet is not owned"),
            LawError::SlotTaken => write!(f, "that law is already in force or pending"),
            LawError::NotProposed => write!(f, "law is not waiting for approval"),
            LawError::NotEnacted => write!(f, "law is not in force"),
            LawError::AlreadyApproved => write!(f, "already approved"),
            LawError::Disabled => write!(f, "laws are off"),
            LawError::PermissionDenied(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for LawError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawEffectKind {
    MetalOutput,
    CrystalOutput,
    DeuteriumOutput,
    ResearchSpeed,
    MarketFee,
    PollutionCap,
    TaxMetalPerHour,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawMetric {
    Output,
    Pollution,
    Revenue,
    Research,
}

impl LawMetric {
    pub fn as_key(self) -> &'static str {
        match self {
            LawMetric::Output => "output",
            LawMetric::Pollution => "pollution",
            LawMetric::Revenue => "revenue",
            LawMetric::Research => "research",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "output" => LawMetric::Output,
            "pollution" => LawMetric::Pollution,
            "revenue" => LawMetric::Revenue,
            "research" => LawMetric::Research,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jurisdiction {
    Empire,
    Planet(Coord),
}

impl Jurisdiction {
    pub fn as_key(self) -> String {
        match self {
            Jurisdiction::Empire => "empire".into(),
            Jurisdiction::Planet(coord) => coord.to_string(),
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        if text == "empire" {
            return Some(Jurisdiction::Empire);
        }
        let mut parts = text.split(':');
        let galaxy = parts.next()?.parse().ok()?;
        let system = parts.next()?.parse().ok()?;
        let position = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Jurisdiction::Planet(Coord {
            galaxy,
            system,
            position,
        }))
    }

    pub fn covers(self, planet: Coord) -> bool {
        match self {
            Jurisdiction::Empire => true,
            Jurisdiction::Planet(coord) => coord == planet,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LawStatus {
    Proposed,
    Enacted,
    RepealProposed,
    Repealed,
}

impl LawStatus {
    pub fn as_key(self) -> &'static str {
        match self {
            LawStatus::Proposed => "proposed",
            LawStatus::Enacted => "enacted",
            LawStatus::RepealProposed => "repeal_proposed",
            LawStatus::Repealed => "repealed",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "proposed" => LawStatus::Proposed,
            "enacted" => LawStatus::Enacted,
            "repeal_proposed" => LawStatus::RepealProposed,
            "repealed" => LawStatus::Repealed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LawActionKind {
    Propose,
    Approve,
    Enact,
    Repeal,
}

impl LawActionKind {
    pub fn as_key(self) -> &'static str {
        match self {
            LawActionKind::Propose => "propose",
            LawActionKind::Approve => "approve",
            LawActionKind::Enact => "enact",
            LawActionKind::Repeal => "repeal",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "propose" => LawActionKind::Propose,
            "approve" => LawActionKind::Approve,
            "enact" => LawActionKind::Enact,
            "repeal" => LawActionKind::Repeal,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LawEffect {
    pub kind: LawEffectKind,
    pub per_unit: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LawForecast {
    pub metric: LawMetric,
    pub per_unit: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LawTemplate {
    pub id: String,
    pub title: String,
    pub jurisdiction: JurisdictionKind,
    pub param_name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub phase_in_secs: u64,
    pub effects: Vec<LawEffect>,
    pub expected: Vec<LawForecast>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JurisdictionKind {
    Empire,
    Planet,
}

impl LawTemplate {
    /// What enacting this law at `param` changes, one phrase per effect:
    /// `+4% iron output`, `emissions held to 80% of the pollution sink`.
    pub fn effect_text(&self, param: f64) -> Vec<String> {
        self.effects
            .iter()
            .map(|effect| {
                let v = param * effect.per_unit;
                match effect.kind {
                    LawEffectKind::MetalOutput => format!("{:+.0}% iron output", v * 100.0),
                    LawEffectKind::CrystalOutput => format!("{:+.0}% silicon output", v * 100.0),
                    LawEffectKind::DeuteriumOutput => {
                        format!("{:+.0}% deuterium output", v * 100.0)
                    }
                    LawEffectKind::ResearchSpeed => format!("{:+.0}% research speed", v * 100.0),
                    LawEffectKind::MarketFee => format!("{:+.0}% market fee", v * 100.0),
                    LawEffectKind::PollutionCap => format!(
                        "emissions held to {:.0}% of the pollution sink; output slows above it",
                        v * 100.0
                    ),
                    LawEffectKind::TaxMetalPerHour => {
                        format!("taxes {v:.0} iron per hour into cash")
                    }
                }
            })
            .collect()
    }

    pub fn has_effect(&self, kind: LawEffectKind) -> bool {
        self.effects.iter().any(|e| e.kind == kind)
    }

    pub fn clamp_param(&self, value: f64) -> Result<f64, LawError> {
        if !value.is_finite() || value < self.min || value > self.max {
            return Err(LawError::ParamOutOfRange {
                min: self.min,
                max: self.max,
            });
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LawConfig {
    pub measure_secs: u64,
    pub history_len: usize,
    templates: Vec<LawTemplate>,
}

impl LawConfig {
    pub fn parse(toml: &str) -> Result<Self, LawError> {
        let file: File = toml::from_str(toml).map_err(|e| LawError::Parse(e.to_string()))?;
        file.into_config()
    }

    pub fn get(&self, id: &str) -> Option<&LawTemplate> {
        self.templates.iter().find(|t| t.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &LawTemplate> {
        self.templates.iter()
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

impl Default for LawConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_LAWS).expect("bundled laws.toml must be valid")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LawApproval {
    pub who: String,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LawAction {
    pub at: u64,
    pub kind: LawActionKind,
    pub law_id: u64,
    pub actor: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LawSample {
    pub at: u64,
    pub metric: LawMetric,
    pub expected: f64,
    pub actual: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Law {
    pub id: u64,
    pub template_id: String,
    pub param: f64,
    pub previous_param: Option<f64>,
    pub owner: String,
    pub jurisdiction: Jurisdiction,
    pub proposed_at: u64,
    pub effective_at: u64,
    pub phase_in_secs: u64,
    pub status: LawStatus,
    pub repeal_effective_at: Option<u64>,
    pub enacted_at: Option<u64>,
    pub repealed_at: Option<u64>,
    pub approvals: Vec<LawApproval>,
    pub baseline_output: f64,
    pub baseline_pollution: f64,
    pub baseline_research: f64,
    pub last_sample_at: u64,
    pub revenue_accrued: f64,
    pub samples: Vec<LawSample>,
}

impl Law {
    pub fn approved(&self) -> bool {
        self.approvals.iter().any(|a| a.who == PLAYER)
    }

    pub fn strength(&self, now: u64) -> f64 {
        if self.status == LawStatus::Repealed {
            return 0.0;
        }
        if self.status != LawStatus::Enacted && self.status != LawStatus::RepealProposed {
            return 0.0;
        }
        let Some(enacted_at) = self.enacted_at else {
            return 0.0;
        };
        if now < enacted_at {
            return 0.0;
        }
        if self.phase_in_secs == 0 {
            return 1.0;
        }
        let elapsed = now.saturating_sub(enacted_at) as f64;
        (elapsed / self.phase_in_secs as f64).clamp(0.0, 1.0)
    }

    pub fn live_param(&self, now: u64) -> f64 {
        self.live_param_with(self.strength(now))
    }

    /// Mean of [`Law::strength`] over `[from, to]`. Linear ramps integrate
    /// exactly so a long catch-up matches second-by-second ticks.
    pub fn average_strength(&self, from: u64, to: u64) -> f64 {
        if to <= from {
            return self.strength(from);
        }
        if self.status == LawStatus::Repealed
            || (self.status != LawStatus::Enacted && self.status != LawStatus::RepealProposed)
        {
            return 0.0;
        }
        let Some(enacted_at) = self.enacted_at else {
            return 0.0;
        };
        let duration = (to - from) as f64;
        let mut area = 0.0;
        if self.phase_in_secs == 0 {
            let start = from.max(enacted_at);
            if start < to {
                area += (to - start) as f64;
            }
            return area / duration;
        }
        let ramp_end = enacted_at.saturating_add(self.phase_in_secs);
        let phase = self.phase_in_secs as f64;
        let ramp_from = from.max(enacted_at);
        let ramp_to = to.min(ramp_end);
        if ramp_from < ramp_to {
            let s0 = ((ramp_from - enacted_at) as f64 / phase).clamp(0.0, 1.0);
            let s1 = ((ramp_to - enacted_at) as f64 / phase).clamp(0.0, 1.0);
            area += (s0 + s1) * 0.5 * (ramp_to - ramp_from) as f64;
        }
        let full_from = from.max(ramp_end);
        if full_from < to {
            area += (to - full_from) as f64;
        }
        area / duration
    }

    pub fn average_live_param(&self, from: u64, to: u64) -> f64 {
        self.live_param_with(self.average_strength(from, to))
    }

    fn live_param_with(&self, strength: f64) -> f64 {
        match self.previous_param {
            Some(prev) => prev + (self.param - prev) * strength,
            None => self.param * strength,
        }
    }
}

/// Production throttle from a pollution-cap ratio. `1.0` is unconstrained.
pub fn emission_throttle(cap: f64, load: f64, sink: f64) -> f64 {
    if cap <= 0.0 || cap >= 1.0 {
        return 1.0;
    }
    if load <= 0.0 {
        return 1.0;
    }
    let allowed = sink * cap;
    if load <= allowed {
        1.0
    } else {
        (allowed / load).clamp(0.0, 1.0)
    }
}

fn emission_linear(cap: f64, load: f64, sink: f64) -> f64 {
    if load <= 0.0 {
        return 1.0;
    }
    (sink * cap / load).clamp(0.0, 1.0)
}

/// Integral of [`emission_throttle`] while the live cap moves linearly from `c0` to `c1`.
fn integrate_linear_throttle(c0: f64, c1: f64, dt: f64, load: f64, sink: f64) -> f64 {
    if dt <= 0.0 {
        return 0.0;
    }
    if (c1 - c0).abs() < 1e-12 {
        return emission_throttle(c0, load, sink) * dt;
    }
    let mut us = vec![0.0, 1.0];
    let delta = c1 - c0;
    for thresh in [
        0.0,
        1.0,
        if sink > 0.0 {
            load / sink
        } else {
            f64::INFINITY
        },
    ] {
        if thresh.is_finite() {
            let u = (thresh - c0) / delta;
            if u > 0.0 && u < 1.0 {
                us.push(u);
            }
        }
    }
    us.sort_by(f64::total_cmp);
    us.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    let mut area = 0.0;
    for pair in us.windows(2) {
        let (u0, u1) = (pair[0], pair[1]);
        let seg = dt * (u1 - u0);
        let ca = c0 + delta * u0;
        let cb = c0 + delta * u1;
        let mid = (ca + cb) * 0.5;
        area += if emission_throttle(mid, load, sink) >= 1.0 - 1e-15 {
            seg
        } else {
            0.5 * (emission_linear(ca, load, sink) + emission_linear(cb, load, sink)) * seg
        };
    }
    area
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LawState {
    pub laws: Vec<Law>,
    pub actions: Vec<LawAction>,
    pub next_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LawMetrics {
    pub output: f64,
    pub pollution: f64,
    pub pollution_load: f64,
    pub pollution_sink: f64,
    pub research: f64,
}

impl LawState {
    pub fn get(&self, id: u64) -> Option<&Law> {
        self.laws.iter().find(|l| l.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Law> {
        self.laws.iter_mut().find(|l| l.id == id)
    }

    fn slot_taken(&self, template_id: &str, jurisdiction: Jurisdiction) -> bool {
        self.laws.iter().any(|law| {
            law.template_id == template_id
                && law.jurisdiction == jurisdiction
                && law.status != LawStatus::Repealed
        })
    }

    fn record(&mut self, at: u64, kind: LawActionKind, law_id: u64, actor: &str) {
        self.actions.push(LawAction {
            at,
            kind,
            law_id,
            actor: actor.to_string(),
        });
    }

    pub fn propose(
        &mut self,
        cfg: &LawConfig,
        template_id: &str,
        param: f64,
        jurisdiction: Jurisdiction,
        effective_at: u64,
        now: u64,
    ) -> Result<u64, LawError> {
        let spec = cfg
            .get(template_id)
            .ok_or_else(|| LawError::UnknownTemplate(template_id.to_string()))?;
        match (spec.jurisdiction, jurisdiction) {
            (JurisdictionKind::Empire, Jurisdiction::Empire) => {}
            (JurisdictionKind::Planet, Jurisdiction::Planet(_)) => {}
            _ => return Err(LawError::JurisdictionMismatch),
        }
        let param = spec.clamp_param(param)?;
        if self.slot_taken(template_id, jurisdiction) {
            return Err(LawError::SlotTaken);
        }
        // A pollution cap is a ratio of the sink. Phase in from unconstrained
        // (1.0) so the first hour does not slam mines to a near-zero allowance.
        let previous_param = spec.has_effect(LawEffectKind::PollutionCap).then_some(1.0);
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.laws.push(Law {
            id,
            template_id: spec.id.clone(),
            param,
            previous_param,
            owner: PLAYER.into(),
            jurisdiction,
            proposed_at: now,
            effective_at: effective_at.max(now),
            phase_in_secs: spec.phase_in_secs,
            status: LawStatus::Proposed,
            repeal_effective_at: None,
            enacted_at: None,
            repealed_at: None,
            approvals: Vec::new(),
            baseline_output: 0.0,
            baseline_pollution: 0.0,
            baseline_research: 0.0,
            last_sample_at: 0,
            revenue_accrued: 0.0,
            samples: Vec::new(),
        });
        self.record(now, LawActionKind::Propose, id, PLAYER);
        Ok(id)
    }

    pub fn approve(&mut self, id: u64, now: u64) -> Result<(), LawError> {
        let law = self.get_mut(id).ok_or(LawError::UnknownLaw(id))?;
        match law.status {
            LawStatus::Proposed | LawStatus::RepealProposed => {}
            _ => return Err(LawError::NotProposed),
        }
        if law.approved() {
            return Err(LawError::AlreadyApproved);
        }
        law.approvals.push(LawApproval {
            who: PLAYER.into(),
            at: now,
        });
        self.record(now, LawActionKind::Approve, id, PLAYER);
        Ok(())
    }

    pub fn propose_repeal(&mut self, id: u64, effective_at: u64, now: u64) -> Result<(), LawError> {
        let law = self.get_mut(id).ok_or(LawError::UnknownLaw(id))?;
        if law.status != LawStatus::Enacted {
            return Err(LawError::NotEnacted);
        }
        law.status = LawStatus::RepealProposed;
        law.repeal_effective_at = Some(effective_at.max(now));
        law.approvals.clear();
        self.record(now, LawActionKind::Propose, id, PLAYER);
        Ok(())
    }

    pub fn next_event_at(&self, cfg: &LawConfig, last_tick: u64) -> Option<u64> {
        let mut soonest = None;
        for law in &self.laws {
            let candidate = match law.status {
                LawStatus::Proposed if law.approved() => Some(law.effective_at),
                LawStatus::Enacted | LawStatus::RepealProposed => {
                    let start = law.last_sample_at.max(law.enacted_at.unwrap_or(0));
                    let sample = start.saturating_add(cfg.measure_secs.max(1));
                    let phase_end = law.enacted_at.and_then(|enacted| {
                        (law.phase_in_secs > 0).then_some(enacted.saturating_add(law.phase_in_secs))
                    });
                    let repeal = if law.status == LawStatus::RepealProposed && law.approved() {
                        law.repeal_effective_at
                    } else {
                        None
                    };
                    [Some(sample), phase_end, repeal]
                        .into_iter()
                        .flatten()
                        .min()
                }
                _ => None,
            };
            if let Some(at) = candidate.filter(|&t| t > last_tick) {
                soonest = Some(soonest.map_or(at, |s: u64| s.min(at)));
            }
        }
        soonest
    }

    /// Enact, repeal and sample anything due at `now`. Returns log lines.
    pub fn apply_clock(
        &mut self,
        cfg: &LawConfig,
        now: u64,
        mut metrics_for: impl FnMut(&Law) -> LawMetrics,
    ) -> Vec<(u64, String)> {
        let mut logs = Vec::new();
        let ids: Vec<u64> = self.laws.iter().map(|l| l.id).collect();
        for id in ids {
            let Some(law) = self.get(id) else {
                continue;
            };
            if law.status == LawStatus::Proposed && law.approved() && now >= law.effective_at {
                let template_id = law.template_id.clone();
                let metrics = metrics_for(law);
                if let Some(law) = self.get_mut(id) {
                    law.status = LawStatus::Enacted;
                    // Phase-in starts when the law actually enters force, not
                    // when it first became eligible. Late approval must not
                    // skip the catalog ramp.
                    law.enacted_at = Some(now);
                    law.baseline_output = metrics.output;
                    law.baseline_pollution = metrics.pollution;
                    law.baseline_research = metrics.research;
                    law.last_sample_at = now;
                }
                self.record(now, LawActionKind::Enact, id, CLOCK);
                logs.push((now, format!("law {template_id} #{id} enacted")));
                // Baseline is pre-enact. Sample after measure_secs so actual is
                // taken while the law is in force (phase-in 0 would otherwise
                // compare a buffed expected to an unbuffed snapshot).
            }
            let Some(law) = self.get(id) else {
                continue;
            };
            if matches!(law.status, LawStatus::Enacted | LawStatus::RepealProposed)
                && law.enacted_at.is_some()
            {
                let due = law
                    .last_sample_at
                    .max(law.enacted_at.unwrap_or(0))
                    .saturating_add(cfg.measure_secs.max(1));
                if now >= due {
                    let metrics = metrics_for(law);
                    self.sample_one(cfg, id, now, metrics);
                }
            }
            let Some(law) = self.get(id) else {
                continue;
            };
            if law.status == LawStatus::RepealProposed
                && law.approved()
                && law.repeal_effective_at.is_some_and(|t| now >= t)
            {
                let template_id = law.template_id.clone();
                let previous = law.previous_param;
                if let Some(law) = self.get_mut(id) {
                    law.status = LawStatus::Repealed;
                    law.repealed_at = law.repeal_effective_at;
                    law.param = previous.unwrap_or(0.0);
                }
                self.record(now, LawActionKind::Repeal, id, CLOCK);
                logs.push((
                    now,
                    format!("law {template_id} #{id} repealed; prior parameters restored"),
                ));
            }
        }
        logs
    }

    fn sample_one(&mut self, cfg: &LawConfig, id: u64, now: u64, metrics: LawMetrics) {
        let Some(spec) = self
            .get(id)
            .and_then(|law| cfg.get(&law.template_id).cloned())
        else {
            return;
        };
        let Some(law) = self.get_mut(id) else {
            return;
        };
        let strength = law.strength(now);
        let hours = if law.last_sample_at >= now {
            0.0
        } else {
            (now - law.last_sample_at) as f64 / 3600.0
        };
        for forecast in &spec.expected {
            let expected = match forecast.metric {
                LawMetric::Pollution => {
                    let cap = law.live_param(now) * forecast.per_unit;
                    if cap <= 0.0 || cap >= 1.0 {
                        metrics.pollution_load
                    } else {
                        let allowed = metrics.pollution_sink * cap;
                        if metrics.pollution_load <= allowed {
                            metrics.pollution_load
                        } else {
                            allowed
                        }
                    }
                }
                LawMetric::Output => {
                    law.baseline_output * (1.0 + law.param * forecast.per_unit * strength)
                }
                LawMetric::Research => {
                    law.baseline_research * (1.0 + law.param * forecast.per_unit * strength)
                }
                LawMetric::Revenue => law.param * forecast.per_unit * strength * hours,
            };
            let actual = match forecast.metric {
                LawMetric::Pollution => metrics.pollution,
                LawMetric::Output => metrics.output,
                LawMetric::Research => metrics.research,
                LawMetric::Revenue => law.revenue_accrued,
            };
            law.samples.push(LawSample {
                at: now,
                metric: forecast.metric,
                expected,
                actual,
            });
        }
        let cap = cfg.history_len.max(1) * spec.expected.len().max(1);
        if law.samples.len() > cap {
            let drop = law.samples.len() - cap;
            law.samples.drain(..drop);
        }
        law.revenue_accrued = 0.0;
        law.last_sample_at = now;
    }

    pub fn effect(&self, kind: LawEffectKind, cfg: &LawConfig, now: u64, planet: Coord) -> f64 {
        self.effect_range(kind, cfg, now, now, planet)
    }

    pub fn effect_range(
        &self,
        kind: LawEffectKind,
        cfg: &LawConfig,
        from: u64,
        to: u64,
        planet: Coord,
    ) -> f64 {
        self.laws
            .iter()
            .filter(|law| {
                matches!(law.status, LawStatus::Enacted | LawStatus::RepealProposed)
                    && law.jurisdiction.covers(planet)
            })
            .filter_map(|law| {
                let spec = cfg.get(&law.template_id)?;
                let per: f64 = spec
                    .effects
                    .iter()
                    .filter(|e| e.kind == kind)
                    .map(|e| e.per_unit)
                    .sum();
                if per == 0.0 {
                    return None;
                }
                Some(law.average_live_param(from, to) * per)
            })
            .sum()
    }

    pub fn modifier(&self, kind: ModifierKind, cfg: &LawConfig, now: u64, planet: Coord) -> f64 {
        self.modifier_range(kind, cfg, now, now, planet)
    }

    pub fn modifier_range(
        &self,
        kind: ModifierKind,
        cfg: &LawConfig,
        from: u64,
        to: u64,
        planet: Coord,
    ) -> f64 {
        let mapped = match kind {
            ModifierKind::MetalOutput => LawEffectKind::MetalOutput,
            ModifierKind::CrystalOutput => LawEffectKind::CrystalOutput,
            ModifierKind::DeuteriumOutput => LawEffectKind::DeuteriumOutput,
            _ => return 0.0,
        };
        self.effect_range(mapped, cfg, from, to, planet)
    }

    /// Time-average of [`emission_throttle`] over `[from, to]` for caps covering `planet`.
    ///
    /// Averaging the live cap and then throttling once misses the kink at
    /// `load / sink`, so a one-shot catch-up would not match second-by-second ticks.
    pub fn emission_factor_range(
        &self,
        cfg: &LawConfig,
        from: u64,
        to: u64,
        planet: Coord,
        load: f64,
        sink: f64,
    ) -> f64 {
        let mut caps: Vec<(&Law, f64)> = Vec::new();
        for law in &self.laws {
            if !matches!(law.status, LawStatus::Enacted | LawStatus::RepealProposed) {
                continue;
            }
            if !law.jurisdiction.covers(planet) {
                continue;
            }
            let Some(spec) = cfg.get(&law.template_id) else {
                continue;
            };
            let per: f64 = spec
                .effects
                .iter()
                .filter(|e| e.kind == LawEffectKind::PollutionCap)
                .map(|e| e.per_unit)
                .sum();
            if per != 0.0 {
                caps.push((law, per));
            }
        }
        if caps.is_empty() {
            return 1.0;
        }
        let cap_at =
            |t: u64| -> f64 { caps.iter().map(|(law, per)| law.live_param(t) * per).sum() };
        if to <= from {
            return emission_throttle(cap_at(from), load, sink);
        }
        let mut points = vec![from, to];
        for (law, _) in &caps {
            if let Some(enacted) = law.enacted_at {
                if enacted > from && enacted < to {
                    points.push(enacted);
                }
                let ramp_end = enacted.saturating_add(law.phase_in_secs);
                if law.phase_in_secs > 0 && ramp_end > from && ramp_end < to {
                    points.push(ramp_end);
                }
            }
        }
        points.sort_unstable();
        points.dedup();
        let duration = (to - from) as f64;
        let mut area = 0.0;
        for window in points.windows(2) {
            let (a, b) = (window[0], window[1]);
            if b <= a {
                continue;
            }
            area += integrate_linear_throttle(cap_at(a), cap_at(b), (b - a) as f64, load, sink);
        }
        area / duration
    }

    pub fn latest_samples(&self, id: u64) -> Vec<&LawSample> {
        let Some(law) = self.get(id) else {
            return Vec::new();
        };
        let mut last: Vec<&LawSample> = Vec::new();
        for sample in law.samples.iter().rev() {
            if last.iter().any(|s| s.metric == sample.metric) {
                continue;
            }
            last.push(sample);
            if last.len() == 4 {
                break;
            }
        }
        last.reverse();
        last
    }
}

#[derive(Deserialize)]
struct File {
    measure_secs: u64,
    history_len: usize,
    templates: Vec<FileTemplate>,
}

#[derive(Deserialize, Clone)]
struct FileTemplate {
    id: String,
    title: String,
    jurisdiction: JurisdictionKind,
    param_name: String,
    min: f64,
    max: f64,
    default: f64,
    #[serde(default)]
    phase_in_secs: u64,
    effects: Vec<FileEffect>,
    expected: Vec<FileForecast>,
}

#[derive(Deserialize, Clone)]
struct FileEffect {
    kind: LawEffectKind,
    per_unit: f64,
}

#[derive(Deserialize, Clone)]
struct FileForecast {
    metric: LawMetric,
    per_unit: f64,
}

impl File {
    fn into_config(self) -> Result<LawConfig, LawError> {
        if self.measure_secs < 1 {
            return Err(LawError::Parse("measure_secs must be >= 1".into()));
        }
        if self.history_len < 1 {
            return Err(LawError::Parse("history_len must be >= 1".into()));
        }
        let mut seen = std::collections::HashSet::new();
        let mut templates = Vec::new();
        for raw in self.templates {
            if !seen.insert(raw.id.clone()) {
                return Err(LawError::DuplicateId(raw.id));
            }
            if raw.id.is_empty() || raw.title.is_empty() {
                return Err(LawError::Parse("template id and title are required".into()));
            }
            if !raw.min.is_finite() || !raw.max.is_finite() || raw.max < raw.min {
                return Err(LawError::Parse(format!(
                    "{} param range is invalid",
                    raw.id
                )));
            }
            if !raw.default.is_finite() || raw.default < raw.min || raw.default > raw.max {
                return Err(LawError::Parse(format!(
                    "{} default is outside [{}, {}]",
                    raw.id, raw.min, raw.max
                )));
            }
            if raw.effects.is_empty() {
                return Err(LawError::Parse(format!("{} has no effects", raw.id)));
            }
            if raw.expected.is_empty() {
                return Err(LawError::Parse(format!(
                    "{} has no expected effects",
                    raw.id
                )));
            }
            for effect in &raw.effects {
                if !effect.per_unit.is_finite() {
                    return Err(LawError::Parse(format!(
                        "{} effect per_unit must be finite",
                        raw.id
                    )));
                }
            }
            templates.push(LawTemplate {
                id: raw.id,
                title: raw.title,
                jurisdiction: raw.jurisdiction,
                param_name: raw.param_name,
                min: raw.min,
                max: raw.max,
                default: raw.default,
                phase_in_secs: raw.phase_in_secs,
                effects: raw
                    .effects
                    .into_iter()
                    .map(|e| LawEffect {
                        kind: e.kind,
                        per_unit: e.per_unit,
                    })
                    .collect(),
                expected: raw
                    .expected
                    .into_iter()
                    .map(|e| LawForecast {
                        metric: e.metric,
                        per_unit: e.per_unit,
                    })
                    .collect(),
            });
        }
        Ok(LawConfig {
            measure_secs: self.measure_secs,
            history_len: self.history_len,
            templates,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> (LawConfig, LawState) {
        (LawConfig::default(), LawState::default())
    }

    fn metrics(output: f64, pollution: f64) -> LawMetrics {
        LawMetrics {
            output,
            pollution,
            pollution_load: pollution,
            pollution_sink: pollution,
            research: 1.0,
        }
    }

    #[test]
    fn bundled_catalog_has_the_five_templates() {
        let cfg = LawConfig::default();
        assert_eq!(cfg.len(), 5);
        for id in [
            "emission_cap",
            "trade_tariff",
            "research_priority",
            "labour_rules",
            "tax_rate",
        ] {
            assert!(cfg.get(id).is_some(), "{id}");
        }
    }

    #[test]
    fn propose_approve_enact_applies_on_effective_date() {
        let (cfg, mut state) = book();
        let id = state
            .propose(&cfg, "labour_rules", 1.0, Jurisdiction::Empire, 100, 0)
            .unwrap();
        assert_eq!(state.get(id).unwrap().status, LawStatus::Proposed);
        state.approve(id, 10).unwrap();
        assert!(state.get(id).unwrap().approved());
        let logs = state.apply_clock(&cfg, 50, |_| metrics(100.0, 10.0));
        assert!(logs.is_empty());
        assert_eq!(state.get(id).unwrap().status, LawStatus::Proposed);
        let logs = state.apply_clock(&cfg, 100, |_| metrics(100.0, 10.0));
        assert!(logs.iter().any(|(_, t)| t.contains("enacted")), "{logs:?}");
        assert_eq!(state.get(id).unwrap().status, LawStatus::Enacted);
        let now = 100 + cfg.get("labour_rules").unwrap().phase_in_secs;
        let bonus = state.effect(LawEffectKind::MetalOutput, &cfg, now, Coord::HOME);
        assert!((bonus - 0.05).abs() < 1e-9, "{bonus}");
        let mid = 100 + cfg.get("labour_rules").unwrap().phase_in_secs / 2;
        let half = state.effect(LawEffectKind::MetalOutput, &cfg, mid, Coord::HOME);
        assert!((half - 0.025).abs() < 1e-9, "{half}");
    }

    #[test]
    fn repeal_restores_prior_parameters() {
        let (cfg, mut state) = book();
        let id = state
            .propose(&cfg, "tax_rate", 40.0, Jurisdiction::Empire, 0, 0)
            .unwrap();
        state.approve(id, 0).unwrap();
        state.apply_clock(&cfg, 0, |_| metrics(10.0, 1.0));
        assert_eq!(state.get(id).unwrap().status, LawStatus::Enacted);
        state.propose_repeal(id, 50, 10).unwrap();
        state.approve(id, 10).unwrap();
        state.apply_clock(&cfg, 50, |_| metrics(10.0, 1.0));
        let law = state.get(id).unwrap();
        assert_eq!(law.status, LawStatus::Repealed);
        assert_eq!(law.param, 0.0);
        assert_eq!(
            state.effect(LawEffectKind::TaxMetalPerHour, &cfg, 50, Coord::HOME),
            0.0
        );
        assert!(
            state
                .actions
                .iter()
                .any(|a| a.kind == LawActionKind::Repeal && a.law_id == id)
        );
    }

    #[test]
    fn samples_record_expected_versus_actual() {
        let (cfg, mut state) = book();
        let id = state
            .propose(
                &cfg,
                "emission_cap",
                0.5,
                Jurisdiction::Planet(Coord::HOME),
                0,
                0,
            )
            .unwrap();
        state.approve(id, 0).unwrap();
        state.apply_clock(&cfg, 0, |_| LawMetrics {
            output: 80.0,
            pollution: 40.0,
            pollution_load: 40.0,
            pollution_sink: 40.0,
            research: 1.0,
        });
        state.apply_clock(&cfg, cfg.measure_secs, |_| LawMetrics {
            output: 80.0,
            pollution: 18.0,
            pollution_load: 40.0,
            pollution_sink: 40.0,
            research: 1.0,
        });
        let samples = state.latest_samples(id);
        let pollution = samples
            .iter()
            .find(|s| s.metric == LawMetric::Pollution)
            .unwrap();
        assert!((pollution.expected - 20.0).abs() < 1e-9, "{pollution:?}");
        assert!((pollution.actual - 18.0).abs() < 1e-9, "{pollution:?}");
    }

    #[test]
    fn late_approval_starts_phase_in_at_enact() {
        let (cfg, mut state) = book();
        let id = state
            .propose(&cfg, "labour_rules", 1.0, Jurisdiction::Empire, 0, 0)
            .unwrap();
        state.approve(id, 2_000).unwrap();
        state.apply_clock(&cfg, 2_000, |_| metrics(100.0, 10.0));
        let law = state.get(id).unwrap();
        assert_eq!(law.status, LawStatus::Enacted);
        assert_eq!(law.enacted_at, Some(2_000));
        assert!((law.strength(2_000) - 0.0).abs() < 1e-9);
        assert!((law.strength(2_000 + law.phase_in_secs / 2) - 0.5).abs() < 1e-9);
        assert!((law.strength(2_000 + law.phase_in_secs) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn emission_cap_average_factor_matches_the_throttle_integral() {
        let (cfg, mut state) = book();
        let id = state
            .propose(
                &cfg,
                "emission_cap",
                0.5,
                Jurisdiction::Planet(Coord::HOME),
                0,
                0,
            )
            .unwrap();
        state.approve(id, 0).unwrap();
        state.apply_clock(&cfg, 0, |_| metrics(80.0, 40.0));
        let law = state.get(id).unwrap();
        let phase = law.phase_in_secs;
        let load = 45.0;
        let sink = 50.0;
        assert!(load < sink && load > sink * 0.5);
        let avg = state.emission_factor_range(&cfg, 0, phase, Coord::HOME, load, sink);
        let mut area = 0.0;
        for t in 0..phase {
            let f0 = emission_throttle(law.live_param(t), load, sink);
            let f1 = emission_throttle(law.live_param(t + 1), load, sink);
            area += 0.5 * (f0 + f1);
        }
        let stepped = area / phase as f64;
        assert!(
            (avg - stepped).abs() < 1e-6,
            "integral {avg} vs stepped {stepped}"
        );
        let naive = emission_throttle(law.average_live_param(0, phase), load, sink);
        assert!(
            (naive - avg).abs() > 1e-3,
            "f(mean cap) {naive} should miss the integral {avg} when the ramp straddles the kink"
        );
    }

    #[test]
    fn phase_in_average_strength_matches_the_ramp_integral() {
        let (cfg, mut state) = book();
        let id = state
            .propose(&cfg, "labour_rules", 1.0, Jurisdiction::Empire, 0, 0)
            .unwrap();
        state.approve(id, 0).unwrap();
        state.apply_clock(&cfg, 0, |_| metrics(100.0, 10.0));
        let law = state.get(id).unwrap();
        let phase = law.phase_in_secs;
        assert_eq!(phase, 1_800);
        assert!((law.average_strength(0, phase) - 0.5).abs() < 1e-9);
        assert!((law.average_strength(0, phase * 2) - 0.75).abs() < 1e-9);
        assert!((law.average_strength(phase, phase * 2) - 1.0).abs() < 1e-9);
        let next = state.next_event_at(&cfg, 0).expect("phase-in wake");
        assert_eq!(next, phase);
    }

    #[test]
    fn planet_cap_does_not_cover_another_world() {
        let (cfg, mut state) = book();
        let other = Coord {
            galaxy: 1,
            system: 1,
            position: 4,
        };
        let id = state
            .propose(
                &cfg,
                "emission_cap",
                0.5,
                Jurisdiction::Planet(Coord::HOME),
                0,
                0,
            )
            .unwrap();
        state.approve(id, 0).unwrap();
        state.apply_clock(&cfg, 0, |_| metrics(1.0, 1.0));
        let later = cfg.get("emission_cap").unwrap().phase_in_secs;
        let home = state.effect(LawEffectKind::PollutionCap, &cfg, later, Coord::HOME);
        let away = state.effect(LawEffectKind::PollutionCap, &cfg, later, other);
        assert!((home - 0.5).abs() < 1e-9, "{home}");
        assert_eq!(away, 0.0);
    }

    #[test]
    fn emission_cap_phase_in_starts_from_full_sink() {
        let (cfg, mut state) = book();
        let id = state
            .propose(
                &cfg,
                "emission_cap",
                0.2,
                Jurisdiction::Planet(Coord::HOME),
                0,
                0,
            )
            .unwrap();
        state.approve(id, 0).unwrap();
        state.apply_clock(&cfg, 0, |_| metrics(80.0, 40.0));
        let law = state.get(id).unwrap();
        assert_eq!(law.previous_param, Some(1.0));
        assert!((law.live_param(0) - 1.0).abs() < 1e-9);
        let mid = law.phase_in_secs / 2;
        let live = law.live_param(mid);
        assert!(
            (live - 0.6).abs() < 1e-9,
            "mid phase-in should be 1.0 + (0.2-1.0)*0.5, got {live}"
        );
        assert!((law.live_param(law.phase_in_secs) - 0.2).abs() < 1e-9);
    }

    #[test]
    fn proposed_and_repealed_emission_cap_does_not_throttle() {
        let mut state = crate::GameState::new(0);
        state.buildings.metal_mine = 18;
        state.buildings.crystal_mine = 18;
        state.buildings.deuterium_synthesizer = 12;
        state.buildings.solar_plant = 24;
        assert!((state.emission_factor(Coord::HOME) - 1.0).abs() < 1e-9);
        let id = state
            .propose_law("emission_cap", 0.2, Jurisdiction::Planet(Coord::HOME), 0, 0)
            .unwrap();
        assert!((state.emission_factor(Coord::HOME) - 1.0).abs() < 1e-9);
        state.approve_law(id, 0).unwrap();
        state.catch_up(3_600);
        assert!(
            state.emission_factor(Coord::HOME) < 1.0,
            "enacted cap should throttle"
        );
        state.repeal_law(id, 0, 3_600).unwrap();
        state.approve_law(id, 3_600).unwrap();
        assert!((state.emission_factor(Coord::HOME) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn delayed_repeal_keeps_sampling_until_effective() {
        let cfg = crate::BalanceConfig {
            story: crate::StoryCatalog::empty(),
            raid: crate::RaidConfig::inactive(),
            market: crate::MarketConfig::inactive(),
            diplomacy: crate::DiplomacyConfig::inactive(),
            chains: crate::ChainConfig::default(),
            ..crate::BalanceConfig::default()
        };
        let mut state = crate::GameState::with_config(0, cfg);
        state.buildings = crate::Buildings::default();
        state.resources.iron = 500.0;
        let id = state
            .propose_law("tax_rate", 100.0, Jurisdiction::Empire, 0, 0)
            .unwrap();
        state.approve_law(id, 0).unwrap();
        state.repeal_law(id, 7_200, 0).unwrap();
        state.approve_law(id, 0).unwrap();
        state.catch_up(3_600);
        assert_eq!(
            state.laws.get(id).unwrap().status,
            LawStatus::RepealProposed
        );
        let mid = state.laws.latest_samples(id);
        assert!(
            mid.iter()
                .any(|s| s.metric == LawMetric::Revenue && s.at == 3_600),
            "{mid:?}"
        );
        state.catch_up(7_200);
        assert_eq!(state.laws.get(id).unwrap().status, LawStatus::Repealed);
        let last = state.laws.latest_samples(id);
        assert!(
            last.iter()
                .any(|s| s.metric == LawMetric::Revenue && s.at == 7_200),
            "{last:?}"
        );
    }

    #[test]
    fn planet_law_refuses_an_unowned_world() {
        let mut state = crate::GameState::new(0);
        let err = state
            .propose_law(
                "emission_cap",
                0.2,
                Jurisdiction::Planet(Coord {
                    galaxy: 1,
                    system: 1,
                    position: 4,
                }),
                0,
                0,
            )
            .unwrap_err();
        assert!(matches!(err, LawError::UnownedWorld), "{err}");
    }

    #[test]
    fn duplicate_slot_is_refused() {
        let (cfg, mut state) = book();
        state
            .propose(&cfg, "tax_rate", 10.0, Jurisdiction::Empire, 0, 0)
            .unwrap();
        let err = state
            .propose(&cfg, "tax_rate", 20.0, Jurisdiction::Empire, 0, 0)
            .unwrap_err();
        assert!(matches!(err, LawError::SlotTaken), "{err}");
    }

    #[test]
    fn emission_cap_throttles_output_and_measured_pollution() {
        let mut state = crate::GameState::new(0);
        state.buildings.metal_mine = 18;
        state.buildings.crystal_mine = 18;
        state.buildings.deuterium_synthesizer = 12;
        state.buildings.solar_plant = 24;
        let before_out = state.production_per_hour(crate::ResourceKind::Iron);
        let before_pollution = state.waste_home().pollution_load;
        let uncapped = state.resources.iron;
        let id = state
            .propose_law("emission_cap", 0.2, Jurisdiction::Planet(Coord::HOME), 0, 0)
            .unwrap();
        state.approve_law(id, 0).unwrap();
        state.catch_up(3_600);
        let after_out = state.production_per_hour(crate::ResourceKind::Iron);
        let emitted = state.waste_home().pollution_load * state.emission_factor(Coord::HOME);
        let gained = state.resources.iron - uncapped;
        assert!(
            after_out < before_out,
            "output {after_out} should drop from {before_out}"
        );
        assert!(
            gained < before_out * 0.9,
            "hour gain {gained} should be throttled vs rate {before_out}"
        );
        assert!(
            emitted < before_pollution,
            "emitted {emitted} should drop from {before_pollution}"
        );
        let samples = state.laws.latest_samples(id);
        let pollution = samples
            .iter()
            .find(|s| s.metric == LawMetric::Pollution)
            .expect("pollution sample");
        let waste = state.waste_home();
        let allowed = waste.pollution_sink * 0.2;
        assert!(
            (pollution.expected - allowed).abs() < 1e-6,
            "expected {pollution:?} allowed {allowed} sink {}",
            waste.pollution_sink
        );
    }

    #[test]
    fn tax_law_drains_metal_and_cli_flow_enacts() {
        let cfg = crate::BalanceConfig {
            story: crate::StoryCatalog::empty(),
            raid: crate::RaidConfig::inactive(),
            market: crate::MarketConfig::inactive(),
            diplomacy: crate::DiplomacyConfig::inactive(),
            ..crate::BalanceConfig::default()
        };
        let mut state = crate::GameState::with_config(0, cfg);
        state.buildings = crate::Buildings::default();
        state.resources.iron = 500.0;
        let id = state
            .propose_law("tax_rate", 100.0, Jurisdiction::Empire, 0, 0)
            .unwrap();
        state.approve_law(id, 0).unwrap();
        let before = state.resources.iron;
        state.catch_up(3_600);
        let drained = before - state.resources.iron;
        assert!(
            (drained - 100.0).abs() < 1.0,
            "drained {drained} iron now {}",
            state.resources.iron
        );
        let revenue = state
            .laws
            .latest_samples(id)
            .into_iter()
            .find(|s| s.metric == LawMetric::Revenue)
            .expect("revenue sample");
        assert!(
            (revenue.expected - 100.0).abs() < 1.0,
            "first hour expected {revenue:?}"
        );
        assert!(
            (revenue.actual - 100.0).abs() < 1.0,
            "first hour actual {revenue:?}"
        );
        state.repeal_law(id, 0, 3_600).unwrap();
        state.approve_law(id, 3_600).unwrap();
        assert_eq!(state.laws.get(id).unwrap().status, LawStatus::Repealed);
        let metal = state.resources.iron;
        state.catch_up(7_200);
        assert!(
            (state.resources.iron - metal).abs() < 1e-6,
            "tax should stop after repeal"
        );
    }

    #[test]
    fn labour_phase_in_catch_up_matches_step_by_step() {
        let cfg = crate::BalanceConfig {
            story: crate::StoryCatalog::empty(),
            raid: crate::RaidConfig::inactive(),
            market: crate::MarketConfig::inactive(),
            diplomacy: crate::DiplomacyConfig::inactive(),
            chains: crate::ChainConfig::default(),
            ..crate::BalanceConfig::default()
        };
        let mut one_shot = crate::GameState::with_config(0, cfg.clone());
        let id = one_shot
            .propose_law("labour_rules", 1.0, Jurisdiction::Empire, 0, 0)
            .unwrap();
        one_shot.approve_law(id, 0).unwrap();
        let mut stepped = one_shot.clone();
        one_shot.catch_up(3_600);
        for t in 1..=3_600 {
            stepped.catch_up(t);
        }
        assert!(
            (one_shot.resources.iron - stepped.resources.iron).abs() < 1e-6,
            "one-shot {} vs stepped {}",
            one_shot.resources.iron,
            stepped.resources.iron
        );
    }

    #[test]
    fn research_priority_raises_research_speed() {
        let mut state = crate::GameState::new(0);
        state.buildings.research_lab = 1;
        state.resources.silicon = 1_000.0;
        state.resources.deuterium = 1_000.0;
        state.start_research("energy", 0).unwrap();
        let before = state.research.as_ref().unwrap().completes_at;
        let id = state
            .propose_law("research_priority", 1.0, Jurisdiction::Empire, 0, 0)
            .unwrap();
        state.approve_law(id, 0).unwrap();
        let after = state.research.as_ref().unwrap().completes_at;
        assert!(
            after < before,
            "completes_at {after} should be sooner than {before}"
        );
        assert!(state.laws.get(id).unwrap().samples.is_empty());
        state.catch_up(3_600);
        let sample = state
            .laws
            .latest_samples(id)
            .into_iter()
            .find(|s| s.metric == LawMetric::Research)
            .expect("research sample");
        assert!(
            (sample.expected - sample.actual).abs() < 1e-6,
            "first sample should match live speed {sample:?}"
        );
    }

    #[test]
    fn trade_tariff_raises_market_fee() {
        let mut state = crate::GameState::new(0);
        let before = state.effective_market_fee();
        let id = state
            .propose_law("trade_tariff", 0.1, Jurisdiction::Empire, 0, 0)
            .unwrap();
        state.approve_law(id, 0).unwrap();
        let after = state.effective_market_fee();
        assert!(
            (after - (before + 0.1)).abs() < 1e-9,
            "fee {after} should be {before} + 0.1"
        );
    }

    #[test]
    fn emission_cap_catch_up_matches_step_by_step() {
        let cfg = crate::BalanceConfig {
            story: crate::StoryCatalog::empty(),
            raid: crate::RaidConfig::inactive(),
            market: crate::MarketConfig::inactive(),
            diplomacy: crate::DiplomacyConfig::inactive(),
            chains: crate::ChainConfig::default(),
            ..crate::BalanceConfig::default()
        };
        let mut one_shot = crate::GameState::with_config(0, cfg);
        one_shot.buildings.metal_mine = 10;
        one_shot.buildings.crystal_mine = 8;
        one_shot.buildings.deuterium_synthesizer = 6;
        one_shot.buildings.solar_plant = 16;
        let waste = one_shot.waste_home();
        assert!(
            waste.pollution_load < waste.pollution_sink
                && waste.pollution_load > waste.pollution_sink * 0.5,
            "need load between final cap and sink to straddle the kink; load {} sink {}",
            waste.pollution_load,
            waste.pollution_sink
        );
        let id = one_shot
            .propose_law("emission_cap", 0.5, Jurisdiction::Planet(Coord::HOME), 0, 0)
            .unwrap();
        one_shot.approve_law(id, 0).unwrap();
        let mut stepped = one_shot.clone();
        one_shot.catch_up(3_600);
        for t in 1..=3_600 {
            stepped.catch_up(t);
        }
        assert!(
            (one_shot.resources.iron - stepped.resources.iron).abs() < 1e-4,
            "one-shot {} vs stepped {}",
            one_shot.resources.iron,
            stepped.resources.iron
        );
    }
}
