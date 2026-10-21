//! Optional move policies for a governed player. Live play ignores this module.

use crate::advisor::advisor_endpoint_allowed;
use crate::fleets::Coord;
use crate::intent::Intent;
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, Resources};
use std::collections::BTreeMap;
use std::fmt;

/// Caps, budgets and model names. Empty model URL means recorded local replies.
#[derive(Debug, Clone, PartialEq)]
pub struct GovernanceConfig {
    pub spend_cap_per_turn: f64,
    pub spend_cap_per_day: f64,
    pub token_budget_per_turn: u64,
    pub token_budget_per_day: u64,
    pub model_call_budget_per_turn: u32,
    pub model_call_budget_per_day: u32,
    pub cheap_model: String,
    pub expensive_model: String,
    pub cheap_tokens: u64,
    pub expensive_tokens: u64,
    pub model_url: String,
}

impl Default for GovernanceConfig {
    fn default() -> Self {
        Self {
            spend_cap_per_turn: 50.0,
            spend_cap_per_day: 200.0,
            token_budget_per_turn: 80,
            token_budget_per_day: 400,
            model_call_budget_per_turn: 8,
            model_call_budget_per_day: 40,
            cheap_model: "local".into(),
            expensive_model: "local-large".into(),
            cheap_tokens: 32,
            expensive_tokens: 256,
            model_url: String::new(),
        }
    }
}

impl GovernanceConfig {
    /// Remote hosts need an explicit URL; public endpoints stay refused.
    pub fn refuse_remote(&self, allow_lan: bool) -> Option<String> {
        if self.model_url.is_empty() {
            return None;
        }
        if advisor_endpoint_allowed(&self.model_url, allow_lan) {
            None
        } else {
            Some("remote model URL is not a local endpoint".into())
        }
    }
}

/// Cheap local model versus the expensive one used for war, invasion and wonders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    Cheap,
    Expensive,
}

impl ModelTier {
    pub fn as_key(self) -> &'static str {
        match self {
            ModelTier::Cheap => "cheap",
            ModelTier::Expensive => "expensive",
        }
    }
}

/// One recorded (or named) model call.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUse {
    pub tier: ModelTier,
    pub model: String,
    pub tokens: u64,
    pub response: String,
}

/// Headless runs auto-deny; the TUI can grant after a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvasionApproval {
    Granted,
    Denied,
    AutoDeny,
}

/// Running spend and token totals. Hitting a budget sets `halted`.
#[derive(Debug, Clone, PartialEq)]
pub struct GovernanceLedger {
    pub turn: u32,
    pub day: u32,
    pub spent_turn: f64,
    pub spent_day: f64,
    pub tokens_turn: u64,
    pub tokens_day: u64,
    pub calls_turn: u32,
    pub calls_day: u32,
    pub halted: bool,
}

impl Default for GovernanceLedger {
    fn default() -> Self {
        Self {
            turn: 1,
            day: 1,
            spent_turn: 0.0,
            spent_day: 0.0,
            tokens_turn: 0,
            tokens_day: 0,
            calls_turn: 0,
            calls_day: 0,
            halted: false,
        }
    }
}

impl GovernanceLedger {
    pub fn record_spend(&mut self, spent: f64) {
        self.spent_turn += spent;
        self.spent_day += spent;
    }
}

/// Named rule plus the inputs that tripped it.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyBlock {
    pub rule: &'static str,
    pub inputs: BTreeMap<String, String>,
    pub message: String,
}

impl PolicyBlock {
    fn new(rule: &'static str, inputs: BTreeMap<String, String>, message: String) -> Self {
        Self {
            rule,
            inputs,
            message,
        }
    }
}

impl fmt::Display for PolicyBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule, self.message)
    }
}

/// One proposal, decision and outcome for the governed audit trail.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub at: u64,
    pub proposal_id: Option<String>,
    pub intent: String,
    pub decision: String,
    pub rule: Option<String>,
    pub inputs: BTreeMap<String, String>,
    pub model: Option<String>,
    pub tokens: u64,
    pub outcome: String,
}

/// Destination of a move that can hit another world.
pub fn dest_of(intent: &Intent) -> Option<Coord> {
    match intent {
        Intent::Launch { dest, .. }
        | Intent::Colonize { dest, .. }
        | Intent::Invade { dest, .. } => Some(*dest),
        Intent::Build { planet, .. } | Intent::Dismantle { planet, .. } => Some(*planet),
        Intent::PlaceOrder { region, .. } => Some(*region),
        Intent::ShipGoods { dest, .. } => Some(*dest),
        Intent::Research { .. }
        | Intent::AcceptContract { .. }
        | Intent::FulfilContract { .. }
        | Intent::SetBudget { .. }
        | Intent::RequestSpend { .. }
        | Intent::ApproveSpend { .. }
        | Intent::SetTarget { .. }
        | Intent::GenerateReport { .. }
        | Intent::OpenIncident { .. }
        | Intent::AssignIncident { .. }
        | Intent::ResolveIncident { .. }
        | Intent::ReviewIncident { .. }
        | Intent::ScheduleMaintenance { .. }
        | Intent::CompleteWorkOrder { .. } => None,
    }
}

/// Catalog cost of `intent` (build/research paid resources, or launch fuel).
/// Does not apply the move, so production during catch-up cannot hide spend.
pub fn spend_of(state: &GameState, intent: &Intent, _now: u64) -> f64 {
    match intent {
        Intent::Build {
            planet, building, ..
        } => stock_of(state.build_cost_on(*planet, *building)),
        Intent::Research { tech } => state
            .config
            .techs
            .get(tech)
            .map(|spec| stock_of(spec.cost(state.tech_level(tech))))
            .unwrap_or(0.0),
        Intent::Launch {
            ships,
            dest,
            speed_percent,
            ..
        }
        | Intent::Invade {
            ships,
            dest,
            speed_percent,
        } => fuel_of(state, *ships, *dest, *speed_percent),
        Intent::Colonize {
            dest,
            speed_percent,
        } => {
            let mut ships = Fleet::default();
            ships.add(ShipKind::ColonyShip, 1);
            fuel_of(state, ships, *dest, *speed_percent)
        }
        Intent::PlaceOrder {
            amount,
            limit,
            side,
            good,
            ..
        } => match side {
            crate::OrderSide::Buy => state.buy_order_escrow(*good, *amount, *limit),
            crate::OrderSide::Sell => *amount,
        },
        Intent::ShipGoods {
            origin,
            dest,
            cargo,
            ..
        } => cargo.total() + state.shipment_fuel(*origin, *dest, *cargo),
        Intent::Dismantle { .. }
        | Intent::AcceptContract { .. }
        | Intent::FulfilContract { .. }
        | Intent::SetBudget { .. }
        | Intent::ApproveSpend { .. }
        | Intent::SetTarget { .. }
        | Intent::GenerateReport { .. }
        | Intent::OpenIncident { .. }
        | Intent::AssignIncident { .. }
        | Intent::ResolveIncident { .. }
        | Intent::ReviewIncident { .. }
        | Intent::CompleteWorkOrder { .. } => 0.0,
        Intent::ScheduleMaintenance { .. } => {
            let cfg = state.maintenance_config();
            cfg.iron_cost + cfg.silicon_cost
        }
        Intent::RequestSpend { amount, .. } => *amount,
    }
}

fn stock_of(resources: Resources) -> f64 {
    resources.total()
}

fn fuel_of(state: &GameState, ships: Fleet, dest: Coord, speed_percent: u8) -> f64 {
    state
        .flight_time_and_fuel(ships, dest, speed_percent)
        .map(|(_, fuel)| fuel)
        .unwrap_or(0.0)
}

/// War, invasion and wonder research use the expensive model; everything else is cheap.
pub fn route_model(state: &GameState, intent: &Intent) -> ModelTier {
    match intent {
        Intent::Invade { .. } | Intent::Launch { .. } => ModelTier::Expensive,
        Intent::Research { tech } if is_wonder_tech(state, tech) => ModelTier::Expensive,
        _ => ModelTier::Cheap,
    }
}

fn is_wonder_tech(state: &GameState, tech: &str) -> bool {
    state.config.wonders.iter().any(|spec| spec.id == tech)
}

/// Deterministic local reply. The demo never opens a socket.
pub fn recorded_response(tier: ModelTier, intent: &Intent) -> String {
    match tier {
        ModelTier::Cheap => format!("routine:{}", intent.as_key()),
        ModelTier::Expensive => format!("strategic:{}", intent.as_key()),
    }
}

/// Charge the model budget. A hard stop idles the player; it does not overspend.
pub fn take_budget(
    state: &GameState,
    intent: &Intent,
    ledger: &mut GovernanceLedger,
    cfg: &GovernanceConfig,
) -> Result<ModelUse, PolicyBlock> {
    if ledger.halted {
        return Err(halted_block(ledger, cfg, "already halted"));
    }
    let tier = route_model(state, intent);
    let (model, tokens) = match tier {
        ModelTier::Cheap => (cfg.cheap_model.clone(), cfg.cheap_tokens),
        ModelTier::Expensive => (cfg.expensive_model.clone(), cfg.expensive_tokens),
    };
    let next_tokens_turn = ledger.tokens_turn.saturating_add(tokens);
    let next_tokens_day = ledger.tokens_day.saturating_add(tokens);
    let next_calls_turn = ledger.calls_turn.saturating_add(1);
    let next_calls_day = ledger.calls_day.saturating_add(1);
    if next_tokens_turn > cfg.token_budget_per_turn
        || next_tokens_day > cfg.token_budget_per_day
        || next_calls_turn > cfg.model_call_budget_per_turn
        || next_calls_day > cfg.model_call_budget_per_day
    {
        ledger.halted = true;
        return Err(halted_block(ledger, cfg, "budget exhausted"));
    }
    ledger.tokens_turn = next_tokens_turn;
    ledger.tokens_day = next_tokens_day;
    ledger.calls_turn = next_calls_turn;
    ledger.calls_day = next_calls_day;
    Ok(ModelUse {
        tier,
        model,
        tokens,
        response: recorded_response(tier, intent),
    })
}

fn halted_block(ledger: &GovernanceLedger, cfg: &GovernanceConfig, why: &str) -> PolicyBlock {
    let mut inputs = BTreeMap::new();
    inputs.insert("why".into(), why.into());
    inputs.insert("tokens_turn".into(), ledger.tokens_turn.to_string());
    inputs.insert(
        "token_cap_turn".into(),
        cfg.token_budget_per_turn.to_string(),
    );
    inputs.insert("calls_turn".into(), ledger.calls_turn.to_string());
    PolicyBlock::new(
        "token-budget",
        inputs,
        format!("hard stop: {why}; governed player idles"),
    )
}

/// Policy gate after a proposal exists. Does not mutate `state`.
pub fn check_move(
    state: &GameState,
    intent: &Intent,
    allies: &[Coord],
    ledger: &GovernanceLedger,
    cfg: &GovernanceConfig,
    approval: InvasionApproval,
    now: u64,
) -> Result<(), PolicyBlock> {
    let spent = spend_of(state, intent, now);
    if spent > cfg.spend_cap_per_turn + 1e-9 {
        let mut inputs = BTreeMap::new();
        inputs.insert("spent".into(), format!("{spent:.4}"));
        inputs.insert("cap".into(), format!("{:.4}", cfg.spend_cap_per_turn));
        inputs.insert("window".into(), "turn".into());
        return Err(PolicyBlock::new(
            "spend-cap-turn",
            inputs,
            format!(
                "spend {spent:.1} exceeds per-turn cap {}",
                cfg.spend_cap_per_turn
            ),
        ));
    }
    if ledger.spent_day + spent > cfg.spend_cap_per_day + 1e-9 {
        let mut inputs = BTreeMap::new();
        inputs.insert("spent".into(), format!("{spent:.4}"));
        inputs.insert("already".into(), format!("{:.4}", ledger.spent_day));
        inputs.insert("cap".into(), format!("{:.4}", cfg.spend_cap_per_day));
        inputs.insert("window".into(), "day".into());
        return Err(PolicyBlock::new(
            "spend-cap-day",
            inputs,
            format!(
                "spend {spent:.1} plus {:.1} exceeds per-day cap {}",
                ledger.spent_day, cfg.spend_cap_per_day
            ),
        ));
    }
    if let Some(dest) = dest_of(intent)
        && !matches!(intent, Intent::Build { .. })
        && allies.contains(&dest)
    {
        let mut inputs = BTreeMap::new();
        inputs.insert("dest".into(), dest.to_string());
        inputs.insert(
            "allies".into(),
            allies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
        inputs.insert("intent".into(), intent.as_key().into());
        return Err(PolicyBlock::new(
            "no-attack-ally",
            inputs,
            format!("attack on allied world {dest} is refused"),
        ));
    }
    if let Intent::Invade { dest, .. } = intent {
        match approval {
            InvasionApproval::Granted => {}
            InvasionApproval::Denied | InvasionApproval::AutoDeny => {
                let mode = match approval {
                    InvasionApproval::AutoDeny => "headless",
                    _ => "denied",
                };
                let mut inputs = BTreeMap::new();
                inputs.insert("dest".into(), dest.to_string());
                inputs.insert("mode".into(), mode.into());
                return Err(PolicyBlock::new(
                    "invasion-approval",
                    inputs,
                    format!("invasion of {dest} needs human approval ({mode})"),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::buildings::BuildingKind;
    use crate::leaders::Actor;
    use crate::ships::{Fleet, ShipKind};

    fn quiet_state() -> GameState {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 80_000.0;
        state.resources.silicon = 40_000.0;
        state.resources.deuterium = 10_000.0;
        state
    }

    fn build_mine() -> Intent {
        Intent::Build {
            planet: Coord::HOME,
            building: BuildingKind::MetalMine,
            actor: Actor::Player,
        }
    }

    #[test]
    fn spend_cap_names_the_rule_and_the_spent_amount() {
        let state = quiet_state();
        let intent = build_mine();
        let spent = spend_of(&state, &intent, 1_000);
        assert!(spent > 10.0, "{spent}");
        let catalog = state.build_cost(BuildingKind::MetalMine);
        assert!((spent - stock_of(catalog)).abs() < 1e-9);
        let cfg = GovernanceConfig {
            spend_cap_per_turn: 10.0,
            ..GovernanceConfig::default()
        };
        let err = check_move(
            &state,
            &intent,
            &[],
            &GovernanceLedger::default(),
            &cfg,
            InvasionApproval::Granted,
            1_000,
        )
        .unwrap_err();
        assert_eq!(err.rule, "spend-cap-turn");
        assert_eq!(err.inputs["window"], "turn");
        assert!(err.inputs["spent"].parse::<f64>().unwrap() > 10.0);
        assert!(err.message.contains("spend-cap-turn") || err.message.contains("per-turn"));
    }

    #[test]
    fn spend_ignores_production_that_catch_up_would_add() {
        let mut state = quiet_state();
        state.last_tick_unix = 0;
        let intent = build_mine();
        let catalog = stock_of(state.build_cost(BuildingKind::MetalMine));
        let spent = spend_of(&state, &intent, 86_400);
        assert!(
            (spent - catalog).abs() < 1e-9,
            "spend {spent} drifted from catalog {catalog}"
        );
        assert!(catalog > 10.0);
    }

    #[test]
    fn ally_attack_is_blocked_with_the_dest() {
        let state = quiet_state();
        let dest = Coord {
            galaxy: 1,
            system: 2,
            position: 4,
        };
        let intent = Intent::Launch {
            ships: Fleet::default(),
            dest,
            speed_percent: 100,
            hold_secs: 0,
        };
        let err = check_move(
            &state,
            &intent,
            &[dest],
            &GovernanceLedger::default(),
            &GovernanceConfig::default(),
            InvasionApproval::Granted,
            1_000,
        )
        .unwrap_err();
        assert_eq!(err.rule, "no-attack-ally");
        assert_eq!(err.inputs["dest"], "1:2:4");
        assert!(err.message.contains("1:2:4"));
    }

    #[test]
    fn headless_invasion_auto_denies() {
        let state = quiet_state();
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        let mut ships = Fleet::default();
        ships.add(ShipKind::TroopTransport, 1);
        let intent = Intent::Invade {
            ships,
            dest,
            speed_percent: 100,
        };
        let err = check_move(
            &state,
            &intent,
            &[],
            &GovernanceLedger::default(),
            &GovernanceConfig::default(),
            InvasionApproval::AutoDeny,
            1_000,
        )
        .unwrap_err();
        assert_eq!(err.rule, "invasion-approval");
        assert_eq!(err.inputs["mode"], "headless");
        assert_eq!(err.inputs["dest"], dest.to_string());
        check_move(
            &state,
            &intent,
            &[],
            &GovernanceLedger::default(),
            &GovernanceConfig::default(),
            InvasionApproval::Granted,
            1_000,
        )
        .unwrap();
    }

    #[test]
    fn routine_moves_use_the_cheap_model_and_a_hard_stop_idles() {
        let state = quiet_state();
        let intent = build_mine();
        assert_eq!(route_model(&state, &intent), ModelTier::Cheap);
        let invade = Intent::Invade {
            ships: Fleet::default(),
            dest: Coord::HOME,
            speed_percent: 100,
        };
        assert_eq!(route_model(&state, &invade), ModelTier::Expensive);
        let cfg = GovernanceConfig {
            token_budget_per_turn: 32,
            ..GovernanceConfig::default()
        };
        let mut ledger = GovernanceLedger::default();
        let used = take_budget(&state, &intent, &mut ledger, &cfg).unwrap();
        assert_eq!(used.tier, ModelTier::Cheap);
        assert_eq!(used.tokens, 32);
        assert_eq!(used.response, "routine:build");
        let err = take_budget(&state, &intent, &mut ledger, &cfg).unwrap_err();
        assert_eq!(err.rule, "token-budget");
        assert!(ledger.halted);
        assert_eq!(ledger.tokens_turn, 32);
        let again = take_budget(&state, &intent, &mut ledger, &cfg).unwrap_err();
        assert_eq!(again.rule, "token-budget");
        assert_eq!(again.inputs["why"], "already halted");
    }

    #[test]
    fn empty_model_url_is_local_and_a_public_host_is_refused() {
        let cfg = GovernanceConfig::default();
        assert!(cfg.refuse_remote(false).is_none());
        let remote = GovernanceConfig {
            model_url: "https://example.com/v1".into(),
            ..GovernanceConfig::default()
        };
        assert!(remote.refuse_remote(false).unwrap().contains("local"));
        let loopback = GovernanceConfig {
            model_url: "http://127.0.0.1:11434/v1".into(),
            ..GovernanceConfig::default()
        };
        assert!(loopback.refuse_remote(false).is_none());
    }

    #[test]
    fn coord_from_key_reads_three_parts() {
        assert_eq!(
            Coord::from_key("1:2:4"),
            Some(Coord {
                galaxy: 1,
                system: 2,
                position: 4
            })
        );
        assert!(Coord::from_key("player:1:2:4").is_none());
        assert!(Coord::from_key("nope").is_none());
    }

    #[test]
    fn market_buy_spend_uses_escrow_not_a_unit_price() {
        let state = quiet_state();
        let intent = Intent::PlaceOrder {
            region: Coord::HOME,
            side: crate::OrderSide::Buy,
            good: crate::ResourceKind::Silicon,
            amount: 10.0,
            limit: None,
            ttl_secs: 86_400,
        };
        let spent = spend_of(&state, &intent, 1_000);
        let escrow = state.buy_order_escrow(crate::ResourceKind::Silicon, 10.0, None);
        assert!((spent - escrow).abs() < 1e-9);
        assert!(
            spent > 10.0,
            "market buy must not look like 1 metal per unit, got {spent}"
        );
        let cfg = GovernanceConfig {
            spend_cap_per_turn: 10.0,
            ..GovernanceConfig::default()
        };
        let err = check_move(
            &state,
            &intent,
            &[],
            &GovernanceLedger::default(),
            &cfg,
            InvasionApproval::Granted,
            1_000,
        )
        .unwrap_err();
        assert_eq!(err.rule, "spend-cap-turn");
    }

    #[test]
    fn ship_goods_spend_includes_fuel() {
        let state = quiet_state();
        let cargo = Resources {
            iron: 20.0,
            ..Resources::ZERO
        };
        let dest = state.config.market.hub();
        let intent = Intent::ShipGoods {
            origin: Coord::HOME,
            dest,
            cargo,
            contract: None,
        };
        let spent = spend_of(&state, &intent, 1_000);
        let fuel = state.shipment_fuel(Coord::HOME, dest, cargo);
        assert!(fuel > 0.0, "expected flight fuel, got {fuel}");
        assert!((spent - (20.0 + fuel)).abs() < 1e-9);
    }
}
