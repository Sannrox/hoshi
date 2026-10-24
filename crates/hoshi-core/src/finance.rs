//! Treasury, budgets, forecasts and spending approvals.
//!
//! Credits are a unit of account. Construction still spends materials; the
//! ledger values those tonnes (and energy draw) so departments can budget.
//! A missing limit is unlimited, so existing play is not gated until the
//! player sets a budget.

use crate::buildings::BuildingKind;
use crate::fleets::Coord;
use crate::leaders::{Actor, RoleKind};
use crate::ships::ShipKind;
use crate::state::{GameState, ResourceKind, Resources};
use serde::Deserialize;
use std::fmt;

const DEFAULT: &str = include_str!("../data/finance.toml");

#[derive(Debug, Clone, PartialEq)]
pub enum FinanceError {
    Parse(String),
    UnknownCenter,
    InvalidLimit,
    UnknownSpend(u64),
    NotApprover,
    CannotApply(String),
    RequestViaAction,
}

impl fmt::Display for FinanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FinanceError::Parse(msg) => write!(f, "finance: {msg}"),
            FinanceError::UnknownCenter => write!(f, "unknown cost center"),
            FinanceError::InvalidLimit => write!(f, "budget limit must be finite and >= 0"),
            FinanceError::UnknownSpend(id) => write!(f, "unknown spend #{id}"),
            FinanceError::NotApprover => write!(f, "actor cannot approve this spend"),
            FinanceError::CannotApply(msg) => write!(f, "{msg}"),
            FinanceError::RequestViaAction => write!(
                f,
                "request_spend is created when construction, research or ships exceed a budget"
            ),
        }
    }
}

impl std::error::Error for FinanceError {}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FinanceConfig {
    pub period_secs: u64,
    pub energy_credit: f64,
    pub interest_rate: f64,
    pub reserve_target: f64,
    pub starting_cash: f64,
}

impl FinanceConfig {
    pub fn bundled() -> Self {
        Self::parse(DEFAULT).expect("bundled finance.toml")
    }

    pub fn parse(raw: &str) -> Result<Self, FinanceError> {
        let cfg: Self = toml::from_str(raw).map_err(|e| FinanceError::Parse(e.to_string()))?;
        if cfg.period_secs == 0 {
            return Err(FinanceError::Parse("period_secs must be > 0".into()));
        }
        for (name, value) in [
            ("energy_credit", cfg.energy_credit),
            ("interest_rate", cfg.interest_rate),
            ("reserve_target", cfg.reserve_target),
            ("starting_cash", cfg.starting_cash),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(FinanceError::Parse(format!(
                    "{name} must be finite and >= 0"
                )));
            }
        }
        Ok(cfg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Department {
    Industry,
    Research,
    Military,
    Logistics,
}

impl Department {
    pub const ALL: [Department; 4] = [
        Department::Industry,
        Department::Research,
        Department::Military,
        Department::Logistics,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            Department::Industry => "industry",
            Department::Research => "research",
            Department::Military => "military",
            Department::Logistics => "logistics",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "industry" => Department::Industry,
            "research" => Department::Research,
            "military" => Department::Military,
            "logistics" => Department::Logistics,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CostCenter {
    Department(Department),
    Fleet,
    Colony(Coord),
}

impl CostCenter {
    pub fn as_key(self) -> String {
        match self {
            CostCenter::Department(dept) => dept.as_key().into(),
            CostCenter::Fleet => "fleet:home".into(),
            CostCenter::Colony(coord) => format!("colony:{coord}"),
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        if let Some(dept) = Department::parse(raw) {
            return Some(CostCenter::Department(dept));
        }
        if raw == "fleet:home" || raw == "fleet" {
            return Some(CostCenter::Fleet);
        }
        let rest = raw.strip_prefix("colony:").unwrap_or(raw);
        parse_coord(rest).map(CostCenter::Colony)
    }
}

fn parse_coord(text: &str) -> Option<Coord> {
    let mut parts = text.split(':');
    let galaxy = parts.next()?.parse().ok()?;
    let system = parts.next()?.parse().ok()?;
    let position = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Coord {
        galaxy,
        system,
        position,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Budget {
    pub center: CostCenter,
    pub limit: Option<f64>,
    pub actual: f64,
    pub forecast: f64,
    pub last_actual: f64,
    pub last_forecast: f64,
}

impl Budget {
    pub fn variance(&self) -> f64 {
        self.actual - self.forecast
    }

    pub fn run_rate(&self, elapsed_secs: u64, period_secs: u64) -> f64 {
        if elapsed_secs == 0 || period_secs == 0 {
            return 0.0;
        }
        self.actual * (period_secs as f64) / (elapsed_secs as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Ledger {
    pub production: f64,
    pub tax: f64,
    pub trade: f64,
    pub contract: f64,
    pub industry: f64,
    pub research: f64,
    pub military: f64,
    pub logistics: f64,
    pub interest: f64,
}

impl Ledger {
    pub fn total(self) -> f64 {
        self.production
            + self.tax
            + self.trade
            + self.contract
            + self.industry
            + self.research
            + self.military
            + self.logistics
            + self.interest
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PendingKind {
    Build {
        coord: Coord,
        building: BuildingKind,
    },
    Research {
        tech: String,
    },
    Ships {
        kind: ShipKind,
        count: u32,
    },
    Defence {
        kind: crate::DefenceKind,
        count: u32,
    },
}

impl PendingKind {
    pub fn as_key(&self) -> &'static str {
        match self {
            PendingKind::Build { .. } => "build",
            PendingKind::Research { .. } => "research",
            PendingKind::Ships { .. } => "ships",
            PendingKind::Defence { .. } => "defence",
        }
    }

    pub fn summary(&self) -> String {
        match self {
            PendingKind::Build { coord, building } => {
                format!("build {} on {coord}", building.as_key())
            }
            PendingKind::Research { tech } => format!("research {tech}"),
            PendingKind::Ships { kind, count } => format!("queue {count} {}", kind.as_key()),
            PendingKind::Defence { kind, count } => format!("queue {count} {}", kind.as_key()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingSpend {
    pub id: u64,
    pub center: CostCenter,
    pub amount: f64,
    pub actor: String,
    pub kind: PendingKind,
    pub requested_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Treasury {
    pub cash: f64,
    pub reserve_target: f64,
}

impl Treasury {
    pub fn debt(&self) -> f64 {
        (-self.cash).max(0.0)
    }

    pub fn reserve_gap(&self) -> f64 {
        (self.reserve_target - self.cash.max(0.0)).max(0.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinanceState {
    pub treasury: Treasury,
    pub budgets: Vec<Budget>,
    pub pending: Vec<PendingSpend>,
    pub period_started_at: u64,
    pub next_period_at: u64,
    pub income: Ledger,
    pub expense: Ledger,
    pub next_spend_id: u64,
    /// When true, the next spend applies even if it is over budget.
    pub(crate) bypass: bool,
}

impl FinanceState {
    pub fn new(now: u64, cfg: &FinanceConfig) -> Self {
        let period = cfg.period_secs.max(1);
        let budgets = Department::ALL
            .iter()
            .map(|&dept| Budget {
                center: CostCenter::Department(dept),
                limit: None,
                actual: 0.0,
                forecast: 0.0,
                last_actual: 0.0,
                last_forecast: 0.0,
            })
            .collect();
        Self {
            treasury: Treasury {
                cash: cfg.starting_cash,
                reserve_target: cfg.reserve_target,
            },
            budgets,
            pending: Vec::new(),
            period_started_at: now,
            next_period_at: now.saturating_add(period),
            income: Ledger::default(),
            expense: Ledger::default(),
            next_spend_id: 0,
            bypass: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinanceReport {
    pub cash: f64,
    pub debt: f64,
    pub reserve_target: f64,
    pub reserve_gap: f64,
    pub period_started_at: u64,
    pub next_period_at: u64,
    pub elapsed_secs: u64,
    pub income: Ledger,
    pub expense: Ledger,
    pub budgets: Vec<Budget>,
    pub pending: Vec<PendingSpend>,
}

impl GameState {
    pub fn finance_config(&self) -> FinanceConfig {
        FinanceConfig::bundled()
    }

    pub fn value_resources(&self, resources: Resources) -> f64 {
        ResourceKind::ALL
            .iter()
            .map(|&kind| resources.get(kind) * self.market.price(kind).max(0.0))
            .sum()
    }

    pub(crate) fn refund_credits(&self, credits: f64, _paid: Resources) -> f64 {
        if credits.is_finite() && credits > 0.0 {
            credits
        } else {
            0.0
        }
    }

    pub(crate) fn replace_spend(
        &mut self,
        centers: &[CostCenter],
        old_credits: f64,
        old_paid: Resources,
        new_paid: Resources,
    ) -> f64 {
        if !(old_credits.is_finite() && old_credits > 0.0) {
            return 0.0;
        }
        self.refund_spend(centers, self.refund_credits(old_credits, old_paid));
        let amount = self.value_resources(new_paid);
        self.post_spend(centers, amount);
        amount
    }

    pub fn finance_report(&self) -> FinanceReport {
        let elapsed = self
            .last_tick_unix
            .saturating_sub(self.finance.period_started_at);
        FinanceReport {
            cash: self.finance.treasury.cash,
            debt: self.finance.treasury.debt(),
            reserve_target: self.finance.treasury.reserve_target,
            reserve_gap: self.finance.treasury.reserve_gap(),
            period_started_at: self.finance.period_started_at,
            next_period_at: self.finance.next_period_at,
            elapsed_secs: elapsed,
            income: self.finance.income,
            expense: self.finance.expense,
            budgets: self.finance.budgets.clone(),
            pending: self.finance.pending.clone(),
        }
    }

    pub fn set_budget(
        &mut self,
        center: CostCenter,
        limit: Option<f64>,
        actor: Actor,
        now_unix: u64,
    ) -> Result<(), FinanceError> {
        self.catch_up(now_unix);
        if !self.can_set_budget(actor, center) {
            return Err(FinanceError::NotApprover);
        }
        if let Some(limit) = limit
            && (!limit.is_finite() || limit < 0.0)
        {
            return Err(FinanceError::InvalidLimit);
        }
        self.upsert_budget(center).limit = limit;
        self.refresh_forecasts();
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: crate::HistoryKind::Other,
            actor: actor.as_key(),
            action: "set_budget".into(),
            objects: vec![center.as_key()],
            inputs: limit
                .map(|v| format!("{v:.4}"))
                .unwrap_or_else(|| "unlimited".into()),
            outcome: "set".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn approve_spend(
        &mut self,
        id: u64,
        actor: Actor,
        now_unix: u64,
    ) -> Result<(), FinanceError> {
        self.catch_up(now_unix);
        let idx = self
            .finance
            .pending
            .iter()
            .position(|row| row.id == id)
            .ok_or(FinanceError::UnknownSpend(id))?;
        if !self.can_approve_spend(actor, self.finance.pending[idx].center) {
            return Err(FinanceError::NotApprover);
        }
        let pending = self.finance.pending.remove(idx);
        self.finance.bypass = true;
        let applied = self.apply_pending(&pending, now_unix);
        self.finance.bypass = false;
        if applied.is_err() {
            self.finance
                .pending
                .insert(idx.min(self.finance.pending.len()), pending);
            return applied;
        }
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: crate::HistoryKind::Other,
            actor: actor.as_key(),
            action: "approve_spend".into(),
            objects: vec![format!("spend:{}", pending.id), pending.center.as_key()],
            inputs: format!("{:.4}", pending.amount),
            outcome: "applied".into(),
            reason: pending.kind.summary(),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn apply_finance_at(&mut self, at: u64) {
        if at < self.finance.next_period_at {
            return;
        }
        let cfg = self.finance_config();
        let mut next = self.finance.next_period_at;
        while at >= next {
            self.close_finance_period(next, &cfg);
            next = next.saturating_add(cfg.period_secs.max(1));
            if next <= self.finance.period_started_at {
                break;
            }
        }
        self.finance.next_period_at = next;
    }

    pub(crate) fn next_finance_event(&self) -> Option<u64> {
        Some(self.finance.next_period_at).filter(|&at| at > self.last_tick_unix)
    }

    pub(crate) fn accrue_production_income(&mut self, hours: f64, produced: Resources) {
        if hours <= 0.0 {
            return;
        }
        let amount = self.value_resources(produced);
        self.credit_income(amount, IncomeKind::Production);
        let upkeep =
            self.empire_energy_draw().max(0.0) * hours * self.finance_config().energy_credit;
        if upkeep > 0.0 {
            self.finance.treasury.cash -= upkeep;
            self.charge_expense(upkeep, Department::Logistics);
            let _ = self.charge_centers(&[CostCenter::Department(Department::Logistics)], upkeep);
        }
    }

    fn empire_energy_draw(&self) -> f64 {
        self.colonies
            .iter()
            .fold(self.energy_consumed(), |sum, colony| {
                sum + self.config.energy_draw(&colony.buildings)
            })
    }

    pub(crate) fn credit_income(&mut self, amount: f64, kind: IncomeKind) {
        if !amount.is_finite() || amount <= 0.0 {
            return;
        }
        self.finance.treasury.cash += amount;
        match kind {
            IncomeKind::Production => self.finance.income.production += amount,
            IncomeKind::Tax => self.finance.income.tax += amount,
            IncomeKind::Trade => {
                self.finance.income.trade += amount;
                self.kpis.period_trade += amount;
            }
            IncomeKind::Contract => {
                self.finance.income.contract += amount;
                self.kpis.period_contract += amount;
            }
        }
    }

    pub(crate) fn hold_spend(
        &mut self,
        centers: &[CostCenter],
        amount: f64,
        kind: PendingKind,
        actor: Actor,
    ) -> Result<(), u64> {
        if self.finance.bypass || amount <= 0.0 || !amount.is_finite() {
            return Ok(());
        }
        if let Some(id) = self
            .finance
            .pending
            .iter()
            .find(|row| row.kind == kind)
            .map(|row| row.id)
        {
            return Err(id);
        }
        if let Some(center) = self.exceeding_center(centers, amount) {
            return Err(self.push_pending(center, amount, kind, actor));
        }
        Ok(())
    }

    pub(crate) fn commit_spend(
        &mut self,
        centers: &[CostCenter],
        amount: f64,
        kind: &PendingKind,
        actor: Actor,
    ) {
        if amount <= 0.0 || !amount.is_finite() {
            return;
        }
        self.post_spend(centers, amount);
        if !self.finance.bypass {
            self.record_history(crate::HistoryDraft {
                at: self.last_tick_unix,
                kind: crate::HistoryKind::Other,
                actor: actor.as_key(),
                action: "request_spend".into(),
                objects: vec![centers[0].as_key()],
                inputs: format!("{amount:.4}"),
                // What was bought and for how much, readable on its own.
                outcome: format!(
                    "applied: {}, {amount:.0} cash",
                    kind.summary().replace('_', " ")
                ),
                reason: kind.summary(),
                ..crate::HistoryDraft::default()
            });
        }
    }

    fn post_spend(&mut self, centers: &[CostCenter], amount: f64) {
        self.finance.treasury.cash -= amount;
        if let Some(dept) = centers.iter().find_map(|c| match c {
            CostCenter::Department(dept) => Some(*dept),
            _ => None,
        }) {
            self.charge_expense(amount, dept);
        }
        let _ = self.charge_centers(centers, amount);
        self.refresh_forecasts();
    }

    pub(crate) fn refund_spend(&mut self, centers: &[CostCenter], amount: f64) {
        if amount <= 0.0 || !amount.is_finite() {
            return;
        }
        self.finance.treasury.cash += amount;
        if let Some(dept) = centers.iter().find_map(|c| match c {
            CostCenter::Department(dept) => Some(*dept),
            _ => None,
        }) {
            self.uncharge_expense(amount, dept);
        }
        for &center in centers {
            let budget = self.upsert_budget(center);
            budget.actual = (budget.actual - amount).max(0.0);
        }
        self.refresh_forecasts();
    }

    fn charge_expense(&mut self, amount: f64, dept: Department) {
        match dept {
            Department::Industry => self.finance.expense.industry += amount,
            Department::Research => self.finance.expense.research += amount,
            Department::Military => self.finance.expense.military += amount,
            Department::Logistics => self.finance.expense.logistics += amount,
        }
    }

    fn uncharge_expense(&mut self, amount: f64, dept: Department) {
        let slot = match dept {
            Department::Industry => &mut self.finance.expense.industry,
            Department::Research => &mut self.finance.expense.research,
            Department::Military => &mut self.finance.expense.military,
            Department::Logistics => &mut self.finance.expense.logistics,
        };
        *slot = (*slot - amount).max(0.0);
    }

    fn close_finance_period(&mut self, at: u64, cfg: &FinanceConfig) {
        for budget in &mut self.finance.budgets {
            budget.last_actual = budget.actual;
            budget.last_forecast = budget.forecast;
            budget.actual = 0.0;
        }
        self.finance.income = Ledger::default();
        self.finance.expense = Ledger::default();
        if self.finance.treasury.cash < 0.0 && cfg.interest_rate > 0.0 {
            let interest = (-self.finance.treasury.cash) * cfg.interest_rate;
            self.finance.treasury.cash -= interest;
            self.finance.expense.interest += interest;
        }
        self.finance.period_started_at = at;
        self.refresh_forecasts();
    }

    fn refresh_forecasts(&mut self) {
        let home = self.home;
        let home_paid: Vec<Resources> = self.build_queue.iter().map(|job| job.paid).collect();
        let colony_paid: Vec<(Coord, Resources)> = self
            .colonies
            .iter()
            .flat_map(|colony| {
                colony
                    .build_queue
                    .iter()
                    .map(|job| (colony.coord, job.paid))
            })
            .collect();
        let research_paid = self.research.as_ref().map(|job| job.paid);
        let ship_paid: Vec<Resources> = self
            .ship_queue
            .iter()
            .map(|job| {
                let n = f64::from(job.remaining);
                Resources::from_parts(
                    job.unit_paid.iron * n,
                    job.unit_paid.silicon * n,
                    job.unit_paid.deuterium * n,
                )
            })
            .collect();
        let pending: Vec<(CostCenter, f64, PendingKind)> = self
            .finance
            .pending
            .iter()
            .map(|row| (row.center, row.amount, row.kind.clone()))
            .collect();
        for budget in &mut self.finance.budgets {
            budget.forecast = 0.0;
        }
        let mut jobs: Vec<(CostCenter, f64)> = Vec::new();
        for paid in home_paid {
            let amount = self.value_resources(paid);
            jobs.push((CostCenter::Department(Department::Industry), amount));
            jobs.push((CostCenter::Colony(home), amount));
        }
        for (coord, paid) in colony_paid {
            let amount = self.value_resources(paid);
            jobs.push((CostCenter::Department(Department::Industry), amount));
            jobs.push((CostCenter::Colony(coord), amount));
        }
        if let Some(paid) = research_paid {
            jobs.push((
                CostCenter::Department(Department::Research),
                self.value_resources(paid),
            ));
        }
        for paid in ship_paid {
            let amount = self.value_resources(paid);
            jobs.push((CostCenter::Department(Department::Military), amount));
            jobs.push((CostCenter::Fleet, amount));
        }
        for (center, amount, kind) in pending {
            let centers = match kind {
                PendingKind::Build { coord, .. } => build_centers(coord).to_vec(),
                PendingKind::Research { .. } => research_centers().to_vec(),
                PendingKind::Ships { .. } | PendingKind::Defence { .. } => {
                    military_centers().to_vec()
                }
            };
            for extra in centers {
                if extra != center {
                    jobs.push((extra, amount));
                }
            }
            jobs.push((center, amount));
        }
        for (center, amount) in jobs {
            self.upsert_budget(center).forecast += amount;
        }
    }

    fn pending_reserved(&self, center: CostCenter) -> f64 {
        self.finance
            .pending
            .iter()
            .filter(|row| pending_applies(row, center))
            .map(|row| row.amount)
            .sum()
    }

    fn exceeding_center(&mut self, centers: &[CostCenter], amount: f64) -> Option<CostCenter> {
        for &center in centers {
            let reserved = self.pending_reserved(center);
            let budget = self.upsert_budget(center);
            if budget
                .limit
                .is_some_and(|limit| budget.actual + reserved + amount > limit + 1e-9)
            {
                return Some(center);
            }
        }
        None
    }

    fn charge_centers(&mut self, centers: &[CostCenter], amount: f64) -> bool {
        for &center in centers {
            self.upsert_budget(center).actual += amount;
        }
        true
    }

    fn upsert_budget(&mut self, center: CostCenter) -> &mut Budget {
        if let Some(idx) = self.finance.budgets.iter().position(|b| b.center == center) {
            return &mut self.finance.budgets[idx];
        }
        self.finance.budgets.push(Budget {
            center,
            limit: None,
            actual: 0.0,
            forecast: 0.0,
            last_actual: 0.0,
            last_forecast: 0.0,
        });
        self.finance.budgets.last_mut().expect("just pushed")
    }

    fn push_pending(
        &mut self,
        center: CostCenter,
        amount: f64,
        kind: PendingKind,
        actor: Actor,
    ) -> u64 {
        self.finance.next_spend_id = self.finance.next_spend_id.saturating_add(1);
        let id = self.finance.next_spend_id;
        self.finance.pending.push(PendingSpend {
            id,
            center,
            amount,
            actor: actor.as_key(),
            kind: kind.clone(),
            requested_at: self.last_tick_unix,
        });
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: crate::HistoryKind::Other,
            actor: actor.as_key(),
            action: "request_spend".into(),
            objects: vec![format!("spend:{id}"), center.as_key()],
            inputs: format!("{amount:.4}"),
            outcome: format!(
                "held: {}, {amount:.0} cash",
                kind.summary().replace('_', " ")
            ),
            reason: kind.summary(),
            ..crate::HistoryDraft::default()
        });
        self.refresh_forecasts();
        id
    }

    fn apply_pending(&mut self, pending: &PendingSpend, now_unix: u64) -> Result<(), FinanceError> {
        match &pending.kind {
            PendingKind::Build { coord, building } => self
                .enqueue_build_as(*coord, *building, now_unix, Actor::Player)
                .map_err(|e| FinanceError::CannotApply(e.to_string())),
            PendingKind::Research { tech } => self
                .start_research(tech, now_unix)
                .map_err(|e| FinanceError::CannotApply(e.to_string())),
            PendingKind::Ships { kind, count } => self
                .enqueue_ships(*kind, *count, now_unix)
                .map_err(|e| FinanceError::CannotApply(e.to_string())),
            PendingKind::Defence { kind, count } => self
                .enqueue_defence(*kind, *count, now_unix)
                .map_err(|e| FinanceError::CannotApply(e.to_string())),
        }
    }

    fn can_set_budget(&self, actor: Actor, center: CostCenter) -> bool {
        match actor {
            Actor::Player => true,
            Actor::Leader(id) => self.leader_owns(id, center),
        }
    }

    fn can_approve_spend(&self, actor: Actor, center: CostCenter) -> bool {
        match actor {
            Actor::Player => true,
            Actor::Leader(id) => self.leader_owns(id, center),
        }
    }

    fn leader_owns(&self, leader_id: u64, center: CostCenter) -> bool {
        let Some(row) = self.assignment_of(leader_id) else {
            return false;
        };
        if !row.ready(self.last_tick_unix) {
            return false;
        }
        match (row.role, center, row.target) {
            (RoleKind::Minister, CostCenter::Department(_), crate::AssignmentTarget::Empire) => {
                true
            }
            (RoleKind::Governor, CostCenter::Colony(coord), crate::AssignmentTarget::Planet(p)) => {
                p == coord
            }
            (RoleKind::Admiral, CostCenter::Fleet, crate::AssignmentTarget::HomeFleet) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncomeKind {
    Production,
    Tax,
    Trade,
    Contract,
}

fn pending_applies(row: &PendingSpend, center: CostCenter) -> bool {
    if row.center == center {
        return true;
    }
    match &row.kind {
        PendingKind::Build { coord, .. } => build_centers(*coord).contains(&center),
        PendingKind::Research { .. } => research_centers().contains(&center),
        PendingKind::Ships { .. } | PendingKind::Defence { .. } => {
            military_centers().contains(&center)
        }
    }
}

pub(crate) fn build_centers(coord: Coord) -> [CostCenter; 2] {
    [
        CostCenter::Department(Department::Industry),
        CostCenter::Colony(coord),
    ]
}

pub(crate) fn research_centers() -> [CostCenter; 1] {
    [CostCenter::Department(Department::Research)]
}

pub(crate) fn military_centers() -> [CostCenter; 2] {
    [
        CostCenter::Department(Department::Military),
        CostCenter::Fleet,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::state::{GameState, Resources};

    fn rich(now: u64) -> GameState {
        let mut state = GameState::with_config(now, BalanceConfig::default());
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 50_000.0;
        state
    }

    #[test]
    fn bundled_finance_parses() {
        let cfg = FinanceConfig::bundled();
        assert_eq!(cfg.period_secs, 3600);
        assert!(cfg.interest_rate > 0.0);
    }

    #[test]
    fn over_budget_spend_is_held() {
        let mut state = rich(1_000);
        state
            .set_budget(
                CostCenter::Department(Department::Industry),
                Some(1.0),
                Actor::Player,
                1_000,
            )
            .unwrap();
        let err = state
            .enqueue_build(BuildingKind::MetalMine, 1_000)
            .unwrap_err();
        assert!(
            matches!(err, crate::BuildError::NeedsApproval { id } if id == 1),
            "{err:?}"
        );
        assert!(state.build_queue.is_empty());
        assert_eq!(state.finance.pending.len(), 1);
        assert_eq!(state.finance.pending[0].id, 1);
        assert_eq!(state.resources.iron, 50_000.0);
    }

    #[test]
    fn approved_spend_applies() {
        let mut state = rich(1_000);
        state
            .set_budget(
                CostCenter::Department(Department::Industry),
                Some(1.0),
                Actor::Player,
                1_000,
            )
            .unwrap();
        let err = state
            .enqueue_build(BuildingKind::MetalMine, 1_000)
            .unwrap_err();
        let crate::BuildError::NeedsApproval { id } = err else {
            panic!("{err:?}");
        };
        state
            .approve_spend(id, Actor::Player, 1_000)
            .expect("approve");
        assert_eq!(state.build_queue.len(), 1);
        assert!(state.finance.pending.is_empty());
        assert!(state.resources.iron < 50_000.0);
        let industry = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Department(Department::Industry))
            .unwrap();
        assert!(industry.actual > 0.0);
    }

    #[test]
    fn forecast_matches_actual_for_a_fixed_scenario() {
        let mut state = rich(1_000);
        state.enqueue_build(BuildingKind::MetalMine, 1_000).unwrap();
        let cost = state.build_queue[0].paid;
        let expected = state.value_resources(cost);
        let industry = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Department(Department::Industry))
            .unwrap();
        assert!(
            (industry.actual - expected).abs() < 1e-9,
            "actual {} expected {expected}",
            industry.actual
        );
        assert!(
            (industry.forecast - expected).abs() < 1e-9,
            "forecast {} expected {expected}",
            industry.forecast
        );
    }

    #[test]
    fn negative_cash_accrues_simple_interest() {
        let mut state = rich(1_000);
        state.finance.treasury.cash = -100.0;
        state.finance.next_period_at = 1_000;
        state.apply_finance_at(1_000);
        assert!(
            state.finance.treasury.cash < -100.0,
            "cash {}",
            state.finance.treasury.cash
        );
        assert!(state.finance.treasury.debt() > 100.0);
        assert!(
            state.finance.expense.interest > 0.0,
            "interest {}",
            state.finance.expense.interest
        );
    }

    #[test]
    fn pending_colony_hold_forecasts_once_per_center() {
        let mut state = rich(1_000);
        state
            .set_budget(
                CostCenter::Colony(state.home),
                Some(1.0),
                Actor::Player,
                1_000,
            )
            .unwrap();
        let err = state
            .enqueue_build(BuildingKind::MetalMine, 1_000)
            .unwrap_err();
        assert!(matches!(err, crate::BuildError::NeedsApproval { .. }));
        let amount = state.finance.pending[0].amount;
        let colony = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Colony(state.home))
            .unwrap();
        let industry = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Department(Department::Industry))
            .unwrap();
        assert!(
            (colony.forecast - amount).abs() < 1e-9,
            "colony forecast {} amount {amount}",
            colony.forecast
        );
        assert!(
            (industry.forecast - amount).abs() < 1e-9,
            "industry forecast {} amount {amount}",
            industry.forecast
        );
    }

    #[test]
    fn colony_budget_hold_is_attributed_to_the_colony() {
        let mut state = rich(1_000);
        state
            .set_budget(
                CostCenter::Colony(state.home),
                Some(1.0),
                Actor::Player,
                1_000,
            )
            .unwrap();
        let err = state
            .enqueue_build(BuildingKind::MetalMine, 1_000)
            .unwrap_err();
        assert!(matches!(err, crate::BuildError::NeedsApproval { .. }));
        assert_eq!(
            state.finance.pending[0].center,
            CostCenter::Colony(state.home)
        );
    }

    #[test]
    fn minister_can_approve_held_spend_unassigned_leader_cannot() {
        let mut state = rich(1_000);
        let leader = state
            .recruit_leader("Nara", "reformer", 1_000)
            .expect("recruit");
        state
            .set_budget(
                CostCenter::Department(Department::Industry),
                Some(1.0),
                Actor::Player,
                1_000,
            )
            .unwrap();
        let err = state
            .enqueue_build(BuildingKind::MetalMine, 1_000)
            .unwrap_err();
        let crate::BuildError::NeedsApproval { id } = err else {
            panic!("{err:?}");
        };
        let denied = state.approve_spend(id, Actor::Leader(leader), 1_000);
        assert!(
            matches!(denied, Err(FinanceError::NotApprover)),
            "{denied:?}"
        );
        state
            .assign_leader(
                leader,
                RoleKind::Minister,
                crate::AssignmentTarget::Empire,
                1_000,
            )
            .unwrap();
        state
            .approve_spend(id, Actor::Leader(leader), 1_000)
            .expect("minister approve");
        assert_eq!(state.build_queue.len(), 1);
        assert!(state.finance.pending.is_empty());
    }

    #[test]
    fn pending_spend_reserves_the_budget() {
        let mut state = rich(1_000);
        let industry = CostCenter::Department(Department::Industry);
        state
            .set_budget(industry, Some(100.0), Actor::Player, 1_000)
            .unwrap();
        let centers = [industry];
        let first = PendingKind::Build {
            coord: state.home,
            building: BuildingKind::MetalMine,
        };
        let second = PendingKind::Build {
            coord: state.home,
            building: BuildingKind::CrystalMine,
        };
        assert!(
            state
                .hold_spend(&centers, 150.0, first, Actor::Player)
                .is_err()
        );
        let second = state.hold_spend(&centers, 80.0, second, Actor::Player);
        assert!(second.is_err(), "second request applied: {second:?}");
        assert_eq!(state.finance.pending.len(), 2);
    }

    #[test]
    fn trade_overflow_is_refused_and_moves_no_cash() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market = crate::MarketConfig::default_active();
        cfg.market.tick_secs = 10_000;
        cfg.market.cargo_free = 1_000.0;
        let mut state = GameState::with_config(1_000, cfg);
        let cap = state.storage_cap(ResourceKind::Silicon);
        state.resources.set(ResourceKind::Silicon, cap);
        state.resources.iron = 500.0;
        let cash = state.finance.treasury.cash;
        let refused = state.trade_market(ResourceKind::Iron, ResourceKind::Silicon, 100.0, 1_000);
        assert!(
            matches!(refused, Err(crate::MarketError::NoRoom { .. })),
            "{refused:?}"
        );
        assert_eq!(state.resources.iron, 500.0);
        assert!((state.finance.treasury.cash - cash).abs() < 1e-9);
    }

    #[test]
    fn cancel_does_not_mint_credits_for_uncharged_jobs() {
        let mut state = rich(1_000);
        let cash = state.finance.treasury.cash;
        let paid = Resources::from_parts(400.0, 0.0, 0.0);
        assert!(state.value_resources(paid) > 0.0);
        state.build_queue.push(crate::BuildJob {
            kind: crate::buildings::BuildJobKind::Build,
            work_left: 0.0,
            building: BuildingKind::MetalMine,
            from_level: 1,
            paid,
            credits: 0.0,
            started_at: None,
            completes_at: None,
        });
        state.cancel_build(0, 1_000).unwrap();
        assert!(
            (state.finance.treasury.cash - cash).abs() < 1e-9,
            "cash {} -> {}",
            cash,
            state.finance.treasury.cash
        );
        let industry = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Department(Department::Industry));
        assert!(
            industry.is_none_or(|b| b.actual.abs() < 1e-9),
            "industry actual {:?}",
            industry.map(|b| b.actual)
        );
    }

    #[test]
    fn cancel_refunds_the_charged_credits_after_a_price_move() {
        let mut state = rich(1_000);
        state.enqueue_build(BuildingKind::MetalMine, 1_000).unwrap();
        let charged = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Department(Department::Industry))
            .unwrap()
            .actual;
        let cash_after_queue = state.finance.treasury.cash;
        state.market.set_price(ResourceKind::Iron, 1_000.0);
        state.cancel_build(0, 1_000).unwrap();
        let industry = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Department(Department::Industry))
            .unwrap();
        assert!(industry.actual.abs() < 1e-9, "actual {}", industry.actual);
        assert!(
            (state.finance.treasury.cash - (cash_after_queue + charged)).abs() < 1e-9,
            "cash {} expected {}",
            state.finance.treasury.cash,
            cash_after_queue + charged
        );
    }

    #[test]
    fn cancel_build_refunds_treasury_and_actual() {
        let mut state = rich(1_000);
        state.enqueue_build(BuildingKind::MetalMine, 1_000).unwrap();
        let industry = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Department(Department::Industry))
            .unwrap();
        let spent = industry.actual;
        assert!(spent > 0.0);
        let cash_after_queue = state.finance.treasury.cash;
        state.cancel_build(0, 1_000).unwrap();
        let industry = state
            .finance
            .budgets
            .iter()
            .find(|b| b.center == CostCenter::Department(Department::Industry))
            .unwrap();
        assert!(industry.actual.abs() < 1e-9, "actual {}", industry.actual);
        assert!(
            (state.finance.treasury.cash - (cash_after_queue + spent)).abs() < 1e-9,
            "cash {} expected {}",
            state.finance.treasury.cash,
            cash_after_queue + spent
        );
    }

    #[test]
    fn full_storage_does_not_mint_production_credits() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 50_000.0;
        for kind in ResourceKind::ALL {
            state.resources.set(kind, state.storage_cap(kind));
        }
        let before = state.finance.income.production;
        let cash = state.finance.treasury.cash;
        // Stay inside the finance period so the rolled ledger is not wiped.
        state.catch_up(1_000 + 1_800);
        assert!(
            (state.finance.income.production - before).abs() < 1e-9,
            "production income {} -> {}",
            before,
            state.finance.income.production
        );
        assert!(
            state.finance.expense.logistics > 0.0,
            "logistics {}",
            state.finance.expense.logistics
        );
        assert!(
            (state.finance.treasury.cash - (cash - state.finance.expense.logistics)).abs() < 1e-9,
            "cash {} logistics {} expected {}",
            state.finance.treasury.cash,
            state.finance.expense.logistics,
            cash - state.finance.expense.logistics
        );
    }

    #[test]
    fn energy_upkeep_debits_cash_and_includes_colonies() {
        fn idle() -> GameState {
            let mut cfg = BalanceConfig::default();
            cfg.raid.enabled = false;
            cfg.market.enabled = false;
            let mut state = GameState::with_config(1_000, cfg);
            for kind in ResourceKind::ALL {
                state.resources.set(kind, state.storage_cap(kind));
            }
            state
        }

        let mut home = idle();
        let hours = 0.5;
        let credit = home.finance_config().energy_credit;
        let expected_home = home.energy_consumed() * hours * credit;
        assert!(expected_home > 0.0);
        home.catch_up(1_000 + 1_800);
        assert!(
            (home.finance.expense.logistics - expected_home).abs() < 1e-9,
            "home logistics {} expected {expected_home}",
            home.finance.expense.logistics
        );
        assert!(
            (home.finance.treasury.cash + expected_home).abs() < 1e-9,
            "home cash {}",
            home.finance.treasury.cash
        );

        let mut empire = idle();
        let buildings = empire.buildings;
        empire.colonies.push(crate::Colony {
            coord: crate::Coord {
                galaxy: 1,
                system: 1,
                position: 4,
            },
            kind: crate::PlanetType::Rocky,
            size: 120,
            resources: Resources::from_parts(50_000.0, 50_000.0, 50_000.0),
            buildings,
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: std::collections::BTreeMap::new(),
        });
        let expected = (empire.energy_consumed()
            + empire.config.energy_draw(&empire.colonies[0].buildings))
            * hours
            * credit;
        empire.catch_up(1_000 + 1_800);
        assert!(
            (empire.finance.expense.logistics - expected).abs() < 1e-9,
            "empire logistics {} expected {expected}",
            empire.finance.expense.logistics
        );
        let expected_cash = empire.finance.income.production - expected;
        assert!(
            (empire.finance.treasury.cash - expected_cash).abs() < 1e-9,
            "empire cash {} expected {expected_cash} income {}",
            empire.finance.treasury.cash,
            empire.finance.income.production
        );
    }

    #[test]
    fn unaffordable_over_budget_build_is_refused_not_held() {
        let mut state = GameState::with_config(1_000, BalanceConfig::default());
        state.resources = Resources::ZERO;
        state
            .set_budget(
                CostCenter::Department(Department::Industry),
                Some(1.0),
                Actor::Player,
                1_000,
            )
            .unwrap();
        let err = state
            .enqueue_build(BuildingKind::MetalMine, 1_000)
            .unwrap_err();
        assert!(
            matches!(err, crate::BuildError::InsufficientResources { .. }),
            "{err:?}"
        );
        assert!(state.finance.pending.is_empty());
    }
}
