//! KPIs, targets and generated periodic reports.
//!
//! Numbers come from the live simulation: production, energy, waste, the
//! treasury, leak incidents, mission windows and market ledgers. A target is
//! an optional goal on one of those series. Each period the empire archives
//! a report with measurements, variance versus targets, and source events.

use crate::fleets::Coord;
use crate::history::{HistoryKind, HistoryQuery};
use crate::leaders::{Actor, RoleKind};
use crate::state::{GameState, ResourceKind};
use serde::Deserialize;
use std::fmt;

const DEFAULT: &str = include_str!("../data/kpis.toml");

#[derive(Debug, Clone, PartialEq)]
pub enum KpiError {
    Parse(String),
    UnknownKpi,
    UnknownOwner,
    InvalidTarget,
    NotOwner,
    UnsupportedTarget,
}

impl fmt::Display for KpiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KpiError::Parse(msg) => write!(f, "kpi: {msg}"),
            KpiError::UnknownKpi => write!(f, "unknown kpi"),
            KpiError::UnknownOwner => write!(f, "unknown target owner"),
            KpiError::InvalidTarget => write!(f, "target must be finite"),
            KpiError::NotOwner => write!(f, "actor cannot set this kpi target"),
            KpiError::UnsupportedTarget => write!(f, "kpi does not support that owner"),
        }
    }
}

impl std::error::Error for KpiError {}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct KpiConfig {
    pub period_secs: u64,
    pub archive_cap: usize,
}

impl KpiConfig {
    pub fn bundled() -> Self {
        Self::parse(DEFAULT).expect("bundled kpis.toml")
    }

    pub fn parse(raw: &str) -> Result<Self, KpiError> {
        let cfg: Self = toml::from_str(raw).map_err(|e| KpiError::Parse(e.to_string()))?;
        if cfg.period_secs == 0 {
            return Err(KpiError::Parse("period_secs must be > 0".into()));
        }
        if cfg.archive_cap == 0 {
            return Err(KpiError::Parse("archive_cap must be > 0".into()));
        }
        Ok(cfg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KpiId {
    Output,
    EnergyBalance,
    HeatMargin,
    BudgetVariance,
    Incidents,
    MissionOnTime,
    MarketProfit,
}

impl KpiId {
    pub const ALL: [KpiId; 7] = [
        KpiId::Output,
        KpiId::EnergyBalance,
        KpiId::HeatMargin,
        KpiId::BudgetVariance,
        KpiId::Incidents,
        KpiId::MissionOnTime,
        KpiId::MarketProfit,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            KpiId::Output => "output",
            KpiId::EnergyBalance => "energy_balance",
            KpiId::HeatMargin => "heat_margin",
            KpiId::BudgetVariance => "budget_variance",
            KpiId::Incidents => "incidents",
            KpiId::MissionOnTime => "mission_on_time",
            KpiId::MarketProfit => "market_profit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            KpiId::Output => "empire iron output /h",
            KpiId::EnergyBalance => "home energy produced - draw",
            KpiId::HeatMargin => "home heat sink - load",
            KpiId::BudgetVariance => "sum of budget forecast - actual",
            KpiId::Incidents => "open operational incidents",
            KpiId::MissionOnTime => "planned missions inside window",
            KpiId::MarketProfit => "period trade + contract credits",
        }
    }

    fn supports_owner(self, owner: &TargetOwner) -> bool {
        match owner {
            TargetOwner::Empire => true,
            TargetOwner::Colony(_) => matches!(
                self,
                KpiId::Output | KpiId::EnergyBalance | KpiId::HeatMargin | KpiId::BudgetVariance
            ),
            TargetOwner::Role(_) => matches!(self, KpiId::BudgetVariance),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "output" => KpiId::Output,
            "energy_balance" => KpiId::EnergyBalance,
            "heat_margin" => KpiId::HeatMargin,
            "budget_variance" => KpiId::BudgetVariance,
            "incidents" => KpiId::Incidents,
            "mission_on_time" => KpiId::MissionOnTime,
            "market_profit" => KpiId::MarketProfit,
            _ => return None,
        })
    }

    fn history_kind(self) -> HistoryKind {
        match self {
            KpiId::Output => HistoryKind::Build,
            KpiId::EnergyBalance | KpiId::HeatMargin | KpiId::BudgetVariance => HistoryKind::Other,
            KpiId::Incidents => HistoryKind::Combat,
            KpiId::MissionOnTime => HistoryKind::Fleet,
            KpiId::MarketProfit => HistoryKind::Trade,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetOwner {
    Empire,
    Colony(Coord),
    Role(RoleKind),
}

impl TargetOwner {
    pub fn as_key(&self) -> String {
        match self {
            TargetOwner::Empire => "empire".into(),
            TargetOwner::Colony(coord) => format!("colony:{coord}"),
            TargetOwner::Role(role) => format!("role:{}", role.as_key()),
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        if raw == "empire" {
            return Some(TargetOwner::Empire);
        }
        if let Some(rest) = raw.strip_prefix("role:") {
            return RoleKind::from_key(rest).map(TargetOwner::Role);
        }
        if let Some(rest) = raw.strip_prefix("colony:") {
            return Coord::from_key(rest).map(TargetOwner::Colony);
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KpiTarget {
    pub kpi: KpiId,
    pub owner: TargetOwner,
    pub value: f64,
    pub set_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub kpi: KpiId,
    pub owner: TargetOwner,
    pub value: f64,
    pub target: Option<f64>,
    pub event_ids: Vec<u64>,
}

impl Measurement {
    pub fn variance(&self) -> Option<f64> {
        self.target.map(|t| self.value - t)
    }

    pub fn progress(&self) -> Option<f64> {
        let target = self.target?;
        if target.abs() < 1e-12 {
            return Some(1.0);
        }
        Some((self.value / target).clamp(0.0, 4.0))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicReport {
    pub id: u64,
    pub at: u64,
    pub measurements: Vec<Measurement>,
    pub highlights: Vec<String>,
    pub pending_decisions: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KpiState {
    pub next_report_at: u64,
    pub next_id: u64,
    pub targets: Vec<KpiTarget>,
    pub reports: Vec<PeriodicReport>,
    /// Trade and contract credits accrued since the last archived report.
    pub period_trade: f64,
    pub period_contract: f64,
}

impl KpiState {
    pub fn new(now: u64, cfg: &KpiConfig) -> Self {
        Self {
            next_report_at: now.saturating_add(cfg.period_secs),
            next_id: 1,
            targets: Vec::new(),
            reports: Vec::new(),
            period_trade: 0.0,
            period_contract: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KpiSnapshot {
    pub measurements: Vec<Measurement>,
    pub next_report_at: u64,
    pub reports: Vec<PeriodicReport>,
    pub targets: Vec<KpiTarget>,
}

impl GameState {
    pub fn kpi_config(&self) -> KpiConfig {
        KpiConfig::bundled()
    }

    pub fn kpi_snapshot(&self) -> KpiSnapshot {
        let from = self.kpis.reports.last().map(|r| r.at).unwrap_or(
            self.last_tick_unix
                .saturating_sub(self.kpi_config().period_secs),
        );
        KpiSnapshot {
            measurements: self.measure_kpis(from, self.last_tick_unix),
            next_report_at: self.kpis.next_report_at,
            reports: self.kpis.reports.clone(),
            targets: self.kpis.targets.clone(),
        }
    }

    pub fn set_kpi_target(
        &mut self,
        kpi: KpiId,
        owner: TargetOwner,
        value: f64,
        actor: Actor,
        now_unix: u64,
    ) -> Result<(), KpiError> {
        self.catch_up(now_unix);
        if !value.is_finite() {
            return Err(KpiError::InvalidTarget);
        }
        if !kpi.supports_owner(&owner) {
            return Err(KpiError::UnsupportedTarget);
        }
        if let TargetOwner::Colony(coord) = owner
            && !self.owned_coords().contains(&coord)
        {
            return Err(KpiError::UnknownOwner);
        }
        if !self.can_set_kpi_target(actor, &owner) {
            return Err(KpiError::NotOwner);
        }
        if let Some(row) = self
            .kpis
            .targets
            .iter_mut()
            .find(|t| t.kpi == kpi && t.owner == owner)
        {
            row.value = value;
            row.set_at = self.last_tick_unix;
        } else {
            self.kpis.targets.push(KpiTarget {
                kpi,
                owner: owner.clone(),
                value,
                set_at: self.last_tick_unix,
            });
        }
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: crate::HistoryKind::Other,
            actor: actor.as_key(),
            action: "set_target".into(),
            objects: vec![kpi.as_key().into(), owner.as_key()],
            inputs: format!("{value:.4}"),
            outcome: "applied".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub(crate) fn next_kpi_event(&self) -> Option<u64> {
        Some(self.kpis.next_report_at).filter(|&at| at > self.last_tick_unix)
    }

    pub fn apply_kpis_at(&mut self, at: u64) {
        if at < self.kpis.next_report_at {
            return;
        }
        let cfg = self.kpi_config();
        let mut next = self.kpis.next_report_at;
        let mut guard = 0u32;
        while at >= next {
            self.close_kpi_period(next);
            let stepped = next.saturating_add(cfg.period_secs);
            if stepped <= next {
                break;
            }
            next = stepped;
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        self.kpis.next_report_at = next;
    }

    fn close_kpi_period(&mut self, at: u64) {
        let cfg = self.kpi_config();
        let from = at.saturating_sub(cfg.period_secs);
        let measurements = self.measure_kpis(from, at);
        let pending = self.finance.pending.len();
        let mut highlights = Vec::new();
        for row in &measurements {
            if let Some(var) = row.variance()
                && var + 1e-9 < 0.0
            {
                highlights.push(format!(
                    "{} {:.1} vs target {:.1}",
                    row.kpi.as_key(),
                    row.value,
                    row.target.unwrap_or(0.0)
                ));
            }
        }
        if pending > 0 {
            highlights.push(format!("{pending} spends waiting approval"));
        }
        if highlights.is_empty() {
            highlights.push("on track".into());
        }
        let id = self.kpis.next_id;
        self.kpis.next_id = self.kpis.next_id.saturating_add(1);
        self.kpis.reports.push(PeriodicReport {
            id,
            at,
            measurements,
            highlights,
            pending_decisions: pending,
        });
        let cap = cfg.archive_cap.max(1);
        while self.kpis.reports.len() > cap {
            self.kpis.reports.remove(0);
        }
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Other,
            actor: "system".into(),
            action: "generate_report".into(),
            objects: vec![format!("report:{id}")],
            outcome: format!("{} kpis", KpiId::ALL.len()),
            ..crate::HistoryDraft::default()
        });
        self.kpis.period_trade = 0.0;
        self.kpis.period_contract = 0.0;
    }

    fn measure_kpis(&self, from: u64, to: u64) -> Vec<Measurement> {
        let mut rows = Vec::new();
        for &kpi in &KpiId::ALL {
            rows.push(self.measure_one(kpi, TargetOwner::Empire, from, to));
        }
        for target in &self.kpis.targets {
            if target.owner == TargetOwner::Empire {
                continue;
            }
            rows.push(self.measure_one(target.kpi, target.owner.clone(), from, to));
        }
        rows
    }

    fn measure_one(&self, kpi: KpiId, owner: TargetOwner, from: u64, to: u64) -> Measurement {
        let value = self.kpi_value_for(kpi, &owner, to);
        let target = self
            .kpis
            .targets
            .iter()
            .find(|t| t.kpi == kpi && t.owner == owner)
            .map(|t| t.value);
        let event_ids = self
            .history
            .search(&HistoryQuery {
                kind: Some(kpi.history_kind()),
                actor: None,
                object: None,
                from: Some(from),
                to: Some(to),
            })
            .into_iter()
            .map(|event| event.id)
            .collect();
        Measurement {
            kpi,
            owner,
            value,
            target,
            event_ids,
        }
    }

    fn kpi_value_for(&self, kpi: KpiId, owner: &TargetOwner, now: u64) -> f64 {
        match owner {
            TargetOwner::Empire => self.kpi_empire(kpi, now),
            TargetOwner::Colony(coord) => match kpi {
                KpiId::Output => self.production_at(*coord, ResourceKind::Iron),
                KpiId::EnergyBalance => self.energy_balance_at(*coord).unwrap_or(0.0),
                KpiId::HeatMargin => self
                    .waste_at(*coord)
                    .map(|w| w.heat_sink - w.heat_load)
                    .unwrap_or(0.0),
                KpiId::BudgetVariance => self
                    .finance
                    .budgets
                    .iter()
                    .filter(|b| b.center == crate::CostCenter::Colony(*coord))
                    .map(|b| b.forecast - b.actual)
                    .sum(),
                _ => 0.0,
            },
            TargetOwner::Role(role) => match kpi {
                KpiId::BudgetVariance => {
                    let centers: Vec<crate::CostCenter> = match role {
                        RoleKind::Governor => self
                            .owned_coords()
                            .into_iter()
                            .map(crate::CostCenter::Colony)
                            .collect(),
                        RoleKind::Admiral => vec![
                            crate::CostCenter::Fleet,
                            crate::CostCenter::Department(crate::Department::Military),
                        ],
                        RoleKind::Minister => vec![
                            crate::CostCenter::Department(crate::Department::Research),
                            crate::CostCenter::Department(crate::Department::Industry),
                        ],
                    };
                    self.finance
                        .budgets
                        .iter()
                        .filter(|b| centers.contains(&b.center))
                        .map(|b| b.forecast - b.actual)
                        .sum()
                }
                _ => 0.0,
            },
        }
    }

    fn can_set_kpi_target(&self, actor: Actor, owner: &TargetOwner) -> bool {
        match actor {
            Actor::Player => true,
            Actor::Leader(id) => {
                let Some(row) = self.assignment_of(id) else {
                    return false;
                };
                if !row.ready(self.last_tick_unix) {
                    return false;
                }
                match (row.role, owner, row.target) {
                    (
                        RoleKind::Minister,
                        TargetOwner::Empire | TargetOwner::Role(RoleKind::Minister),
                        crate::AssignmentTarget::Empire,
                    ) => true,
                    (
                        RoleKind::Governor,
                        TargetOwner::Colony(coord),
                        crate::AssignmentTarget::Planet(planet),
                    ) => planet == *coord,
                    (
                        RoleKind::Governor,
                        TargetOwner::Role(RoleKind::Governor),
                        crate::AssignmentTarget::Planet(_),
                    ) => true,
                    (
                        RoleKind::Admiral,
                        TargetOwner::Role(RoleKind::Admiral),
                        crate::AssignmentTarget::HomeFleet,
                    ) => true,
                    _ => false,
                }
            }
        }
    }

    fn energy_balance_at(&self, coord: Coord) -> Option<f64> {
        let buildings = self.buildings_at(coord)?;
        let produced = self.config.solar_plant.energy_output(buildings.solar_plant)
            * (1.0 + self.modifier(crate::ModifierKind::EnergyOutput)).max(0.0);
        let consumed = self.config.energy_draw(buildings)
            + if coord == self.home {
                self.automation_control_energy()
            } else {
                0.0
            };
        Some(produced - consumed)
    }

    fn kpi_empire(&self, kpi: KpiId, now: u64) -> f64 {
        match kpi {
            KpiId::Output => self.empire_production_per_hour(ResourceKind::Iron),
            KpiId::EnergyBalance => self.energy_produced() - self.energy_consumed(),
            KpiId::HeatMargin => {
                let waste = self.waste_home();
                waste.heat_sink - waste.heat_load
            }
            KpiId::BudgetVariance => self
                .finance
                .budgets
                .iter()
                .map(|b| b.forecast - b.actual)
                .sum(),
            KpiId::Incidents => self.open_count() as f64,
            KpiId::MissionOnTime => {
                let schedule = self.mission_schedule(now);
                let total = schedule.missions.len();
                if total == 0 {
                    1.0
                } else {
                    let missed = schedule
                        .alerts
                        .iter()
                        .filter(|a| a.kind == crate::ScheduleAlertKind::MissedWindow)
                        .count();
                    ((total - missed.min(total)) as f64 / total as f64).clamp(0.0, 1.0)
                }
            }
            KpiId::MarketProfit => self.kpis.period_trade + self.kpis.period_contract,
        }
    }

    pub fn generate_kpi_report(
        &mut self,
        actor: Actor,
        now_unix: u64,
    ) -> Result<PeriodicReport, KpiError> {
        if actor != Actor::Player {
            return Err(KpiError::NotOwner);
        }
        self.catch_up(now_unix);
        let at = self.last_tick_unix;
        if self.kpis.reports.last().map(|r| r.at) != Some(at) {
            self.close_kpi_period(at);
            let cfg = self.kpi_config();
            self.kpis.next_report_at = at.saturating_add(cfg.period_secs);
        }
        self.kpis
            .reports
            .last()
            .cloned()
            .ok_or(KpiError::InvalidTarget)
    }

    pub fn latest_kpi_report(&self) -> Option<&PeriodicReport> {
        self.kpis.reports.last()
    }

    pub fn kpi_report_markdown(&self) -> String {
        let snap = self.kpi_snapshot();
        let mut out = String::from("# Empire report\n\n");
        out.push_str(&format!(
            "Next report at {}.\n\n## KPIs\n\n",
            snap.next_report_at
        ));
        for row in &snap.measurements {
            let target = row
                .target
                .map(|t| format!("{t:.1}"))
                .unwrap_or_else(|| "-".into());
            let var = row
                .variance()
                .map(|v| format!("{v:+.1}"))
                .unwrap_or_else(|| "-".into());
            out.push_str(&format!(
                "- {} ({}) [{}] {:.1}  target {target}  var {var}  events {:?}\n",
                row.kpi.label(),
                row.kpi.as_key(),
                row.owner.as_key(),
                row.value,
                row.event_ids
            ));
        }
        if let Some(report) = snap.reports.last() {
            out.push_str(&format!(
                "\n## Latest period #{}\n\nAt {}. Pending spends {}.\n\n",
                report.id, report.at, report.pending_decisions
            ));
            for line in &report.highlights {
                out.push_str(&format!("- {line}\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::state::GameState;

    #[test]
    fn bundled_kpis_parse() {
        let cfg = KpiConfig::bundled();
        assert!(cfg.period_secs > 0);
        assert!(cfg.archive_cap > 0);
    }

    #[test]
    fn formulas_match_live_state() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let state = GameState::with_config(1_000, cfg);
        let snap = state.kpi_snapshot();
        let output = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::Output)
            .unwrap();
        assert!((output.value - state.empire_production_per_hour(ResourceKind::Iron)).abs() < 1e-9);
        let energy = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::EnergyBalance)
            .unwrap();
        assert!((energy.value - (state.energy_produced() - state.energy_consumed())).abs() < 1e-9);
        let heat = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::HeatMargin)
            .unwrap();
        let waste = state.waste_home();
        assert!((heat.value - (waste.heat_sink - waste.heat_load)).abs() < 1e-9);
        let on_time = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::MissionOnTime)
            .unwrap();
        assert!((on_time.value - 1.0).abs() < 1e-9);
    }

    #[test]
    fn target_progress_is_tracked() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state
            .set_kpi_target(
                KpiId::Output,
                TargetOwner::Empire,
                1_000.0,
                Actor::Player,
                1_000,
            )
            .unwrap();
        let snap = state.kpi_snapshot();
        let output = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::Output)
            .unwrap();
        assert_eq!(output.target, Some(1_000.0));
        assert!(output.progress().unwrap() < 1.0);
        assert!(output.variance().unwrap() < 0.0);
    }

    #[test]
    fn catch_up_archives_a_period_report() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        assert!(state.kpis.reports.is_empty());
        let due = state.kpis.next_report_at;
        state.catch_up(due);
        assert_eq!(state.kpis.reports.len(), 1);
        assert_eq!(state.kpis.reports[0].at, due);
        assert_eq!(state.kpis.reports[0].measurements.len(), KpiId::ALL.len());
        assert!(!state.kpis.reports[0].highlights.is_empty());
    }

    #[test]
    fn generate_at_deadline_does_not_duplicate_the_period() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        let due = state.kpis.next_report_at;
        state.generate_kpi_report(Actor::Player, due).unwrap();
        assert_eq!(state.kpis.reports.len(), 1);
        assert_eq!(state.kpis.reports[0].at, due);
        state.generate_kpi_report(Actor::Player, due).unwrap();
        assert_eq!(state.kpis.reports.len(), 1);
        let denied = state.generate_kpi_report(Actor::Leader(1), due);
        assert!(matches!(denied, Err(KpiError::NotOwner)));
    }

    #[test]
    fn market_profit_survives_a_finance_period_roll() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.credit_income(12.5, crate::finance::IncomeKind::Trade);
        state.credit_income(7.5, crate::finance::IncomeKind::Contract);
        let finance_due = state.finance.next_period_at;
        state.catch_up(finance_due);
        assert!(state.kpis.reports.is_empty());
        let snap = state.kpi_snapshot();
        let profit = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::MarketProfit)
            .unwrap();
        assert!((profit.value - 20.0).abs() < 1e-9, "{}", profit.value);
        assert_eq!(state.finance.income.trade, 0.0);
    }

    #[test]
    fn colony_and_role_targets_track_progress() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state
            .set_kpi_target(
                KpiId::Output,
                TargetOwner::Colony(state.home),
                1_000.0,
                Actor::Player,
                1_000,
            )
            .unwrap();
        state
            .set_kpi_target(
                KpiId::BudgetVariance,
                TargetOwner::Role(RoleKind::Minister),
                0.0,
                Actor::Player,
                1_000,
            )
            .unwrap();
        let unknown = state.set_kpi_target(
            KpiId::Output,
            TargetOwner::Colony(Coord {
                galaxy: 9,
                system: 9,
                position: 9,
            }),
            1.0,
            Actor::Player,
            1_000,
        );
        assert!(matches!(unknown, Err(KpiError::UnknownOwner)));
        let snap = state.kpi_snapshot();
        let colony = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::Output && m.owner == TargetOwner::Colony(state.home))
            .unwrap();
        assert_eq!(colony.target, Some(1_000.0));
        assert!(colony.progress().unwrap() < 1.0);
        let role = snap
            .measurements
            .iter()
            .find(|m| {
                m.kpi == KpiId::BudgetVariance && m.owner == TargetOwner::Role(RoleKind::Minister)
            })
            .unwrap();
        assert_eq!(role.target, Some(0.0));
        assert_eq!(role.progress(), Some(1.0));
        let unsupported = state.set_kpi_target(
            KpiId::Output,
            TargetOwner::Role(RoleKind::Minister),
            1.0,
            Actor::Player,
            1_000,
        );
        assert!(matches!(unsupported, Err(KpiError::UnsupportedTarget)));
        let leader = state
            .recruit_leader("Nara", "reformer", 1_000)
            .expect("recruit");
        let denied = state.set_kpi_target(
            KpiId::Output,
            TargetOwner::Empire,
            10.0,
            Actor::Leader(leader),
            1_000,
        );
        assert!(matches!(denied, Err(KpiError::NotOwner)));
        state
            .assign_leader(
                leader,
                RoleKind::Minister,
                crate::AssignmentTarget::Empire,
                1_000,
            )
            .unwrap();
        state
            .set_kpi_target(
                KpiId::Output,
                TargetOwner::Empire,
                10.0,
                Actor::Leader(leader),
                1_000,
            )
            .unwrap();
    }

    #[test]
    fn budget_variance_is_forecast_minus_actual() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        for budget in &mut state.finance.budgets {
            budget.actual = 0.0;
            budget.forecast = 0.0;
        }
        state.finance.budgets.push(crate::Budget {
            center: crate::CostCenter::Department(crate::Department::Industry),
            limit: None,
            actual: 8.0,
            forecast: 5.0,
            last_actual: 0.0,
            last_forecast: 0.0,
        });
        let snap = state.kpi_snapshot();
        let empire = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::BudgetVariance && m.owner == TargetOwner::Empire)
            .unwrap();
        assert!(
            (empire.value + 3.0).abs() < 1e-9,
            "empire variance {}",
            empire.value
        );
    }

    #[test]
    fn market_profit_includes_losing_trades() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = true;
        cfg.market.tick_secs = 10_000;
        cfg.market.cargo_free = 1_000.0;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 500.0;
        state
            .trade_market(ResourceKind::Iron, ResourceKind::Silicon, 100.0, 1_000)
            .unwrap();
        assert!(
            state.finance.income.trade < 0.0,
            "expected a fee drag, trade income {}",
            state.finance.income.trade
        );
        let snap = state.kpi_snapshot();
        let profit = snap
            .measurements
            .iter()
            .find(|m| m.kpi == KpiId::MarketProfit)
            .unwrap();
        assert!(
            (profit.value - state.finance.income.trade).abs() < 1e-9,
            "kpi {} ledger {}",
            profit.value,
            state.finance.income.trade
        );
    }
}
