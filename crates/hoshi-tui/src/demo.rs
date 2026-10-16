//! Opt-in governance demo: ungoverned CLI player vs a Chisei-governed one.

use hoshi_core::GovernanceConfig;
use serde_json::Value;
use std::path::PathBuf;

#[cfg(feature = "chisei")]
use crate::cli;
#[cfg(feature = "chisei")]
use hoshi_core::{
    Actor, AuditEntry, BalanceConfig, BuildingKind, Coord, Fleet, GameState, GovernanceLedger,
    Intent, InvasionApproval, PolicyBlock, ShipKind, Storage, apply, check_move, propose, spend_of,
    take_budget,
};
#[cfg(feature = "chisei")]
use serde_json::json;
#[cfg(feature = "chisei")]
use std::collections::BTreeMap;
#[cfg(feature = "chisei")]
use std::fs;
#[cfg(feature = "chisei")]
use std::path::Path;
#[cfg(feature = "chisei")]
use std::sync::Mutex;
#[cfg(feature = "chisei")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "chisei")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "chisei")]
static RUN_LOCK: Mutex<()> = Mutex::new(());
#[cfg(feature = "chisei")]
static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "chisei")]
const HOME_TENANT: &str = "player";

#[derive(Debug, Clone, PartialEq)]
pub struct DemoSpec {
    pub seed: u64,
    pub report: Option<PathBuf>,
    pub model_url: String,
    pub allow_lan: bool,
}

pub fn run(spec: &DemoSpec) -> Result<Value, String> {
    let cfg = GovernanceConfig {
        model_url: spec.model_url.clone(),
        ..GovernanceConfig::default()
    };
    if let Some(err) = cfg.refuse_remote(spec.allow_lan) {
        return Err(err);
    }
    #[cfg(feature = "chisei")]
    {
        let _guard = RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        run_chisei(spec)
    }
    #[cfg(not(feature = "chisei"))]
    {
        let _ = spec;
        Err("rebuild with --features chisei to run the governance demo".into())
    }
}

#[cfg(feature = "chisei")]
fn run_chisei(spec: &DemoSpec) -> Result<Value, String> {
    let work = work_dir();
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let betrayal = scenario_betrayal(spec.seed, &work)?;
    let drain = scenario_drain(spec.seed, &work)?;
    let invasion = scenario_invasion(spec.seed, &work)?;
    let scenarios = vec![betrayal, drain, invasion];
    let summary = summarize(&scenarios);
    let markdown = to_markdown(spec.seed, &scenarios, &summary);
    if let Some(path) = &spec.report {
        fs::write(path, &markdown).map_err(|e| e.to_string())?;
    }
    let _ = fs::remove_dir_all(&work);
    Ok(json!({
        "schema_version": cli::SCHEMA_VERSION,
        "ok": true,
        "message": "governance demo",
        "seed": spec.seed,
        "scenarios": scenarios.iter().map(scenario_json).collect::<Vec<_>>(),
        "cost_per_win": summary.cost_per_win,
        "tokens_per_move": summary.tokens_per_move,
        "routing_savings": summary.routing_savings,
        "markdown": markdown,
    }))
}

#[cfg(feature = "chisei")]
#[derive(Debug, Clone)]
struct Scenario {
    id: &'static str,
    ungoverned_applied: bool,
    ungoverned_outcome: String,
    ungoverned_visible: Vec<String>,
    governed_applied: bool,
    governed_outcome: String,
    governed_visible: Vec<String>,
    block: Option<PolicyBlock>,
    halt: Option<PolicyBlock>,
    audit: Vec<AuditEntry>,
    tokens: u64,
    expensive_tokens_if_unrouted: u64,
    moves: u32,
}

#[cfg(feature = "chisei")]
#[derive(Debug, Clone)]
struct Summary {
    cost_per_win: f64,
    tokens_per_move: f64,
    routing_savings: u64,
}

#[cfg(feature = "chisei")]
fn summarize(scenarios: &[Scenario]) -> Summary {
    let tokens: u64 = scenarios.iter().map(|s| s.tokens).sum();
    let moves: u64 = scenarios.iter().map(|s| u64::from(s.moves)).sum();
    let unrouted: u64 = scenarios
        .iter()
        .map(|s| s.expensive_tokens_if_unrouted)
        .sum();
    let governed_blocks = scenarios.iter().filter(|s| s.block.is_some()).count();
    Summary {
        cost_per_win: if governed_blocks == 0 {
            0.0
        } else {
            tokens as f64 / governed_blocks as f64
        },
        tokens_per_move: if moves == 0 {
            0.0
        } else {
            tokens as f64 / moves as f64
        },
        routing_savings: unrouted.saturating_sub(tokens),
    }
}

#[cfg(feature = "chisei")]
fn scenario_json(s: &Scenario) -> Value {
    json!({
        "id": s.id,
        "ungoverned": {
            "applied": s.ungoverned_applied,
            "outcome": s.ungoverned_outcome,
            "visible": s.ungoverned_visible,
        },
        "governed": {
            "applied": s.governed_applied,
            "outcome": s.governed_outcome,
            "visible": s.governed_visible,
            "block": s.block.as_ref().map(block_json),
            "halt": s.halt.as_ref().map(block_json),
            "audit": s.audit.iter().map(audit_json).collect::<Vec<_>>(),
        },
        "tokens": s.tokens,
    })
}

#[cfg(feature = "chisei")]
fn block_json(block: &PolicyBlock) -> Value {
    json!({
        "rule": block.rule,
        "inputs": block.inputs,
        "message": block.message,
    })
}

#[cfg(feature = "chisei")]
fn audit_json(entry: &AuditEntry) -> Value {
    json!({
        "at": entry.at,
        "proposal_id": entry.proposal_id,
        "intent": entry.intent,
        "decision": entry.decision,
        "rule": entry.rule,
        "inputs": entry.inputs,
        "model": entry.model,
        "tokens": entry.tokens,
        "outcome": entry.outcome,
    })
}

#[cfg(feature = "chisei")]
fn to_markdown(seed: u64, scenarios: &[Scenario], summary: &Summary) -> String {
    let mut lines = vec![
        format!("# Governance demo (seed {seed})"),
        String::new(),
        format!(
            "Cost per win: {:.1} tokens. Tokens per move: {:.1}. Routing savings: {} tokens.",
            summary.cost_per_win, summary.tokens_per_move, summary.routing_savings
        ),
        String::new(),
        "| Scenario | Ungoverned | Governed | Policy |".into(),
        "| --- | --- | --- | --- |".into(),
    ];
    for s in scenarios {
        let policy = s
            .block
            .as_ref()
            .map(|b| b.rule.to_string())
            .or_else(|| s.halt.as_ref().map(|b| b.rule.to_string()))
            .unwrap_or_else(|| "none".into());
        lines.push(format!(
            "| {} | {} | {} | {policy} |",
            s.id, s.ungoverned_outcome, s.governed_outcome
        ));
    }
    lines.push(String::new());
    lines.push("## Audit excerpt".into());
    lines.push(String::new());
    for s in scenarios {
        for entry in s.audit.iter().take(3) {
            lines.push(format!(
                "- {} {} {} ({})",
                s.id,
                entry.intent,
                entry.decision,
                entry.rule.as_deref().unwrap_or("applied")
            ));
        }
    }
    lines.join("\n")
}

#[cfg(feature = "chisei")]
fn scenario_betrayal(seed: u64, work: &Path) -> Result<Scenario, String> {
    let mut ungoverned = setup_empire(seed);
    let ally = free_coord(
        &ungoverned,
        Coord {
            galaxy: 1,
            system: 2,
            position: 4,
        },
    );
    let hidden = free_coord(
        &ungoverned,
        Coord {
            galaxy: 1,
            system: 3,
            position: 8,
        },
    );
    let mut ships = Fleet::default();
    ships.add(ShipKind::LightFighter, 2);
    ungoverned.fleet.add(ShipKind::LightFighter, 2);
    let intent = Intent::Launch {
        ships,
        dest: ally,
        speed_percent: 100,
        hold_secs: 0,
    };
    let now = ungoverned.last_tick_unix;
    let un_ok = apply_direct(&mut ungoverned, &intent, now);
    let path = work.join("betrayal.chisei");
    let mut gov = Governed::open(&path, seed)?;
    gov.store.upsert_planet("ally", ally);
    gov.store.upsert_planet("hidden", hidden);
    gov.store.set_alliance(HOME_TENANT, "ally", true);
    gov.store.save(&gov.state).map_err(|e| e.to_string())?;
    gov.cfg.spend_cap_per_turn = 10_000.0;
    gov.cfg.spend_cap_per_day = 10_000.0;
    gov.cfg.token_budget_per_turn = 512;
    gov.state.fleet.add(ShipKind::LightFighter, 2);
    let allies = gov.store.allied_planets(HOME_TENANT);
    let result = gov.consider(&intent, &allies, InvasionApproval::AutoDeny, now);
    Ok(Scenario {
        id: "alliance-betrayal",
        ungoverned_applied: un_ok,
        ungoverned_outcome: if un_ok {
            format!("raided ally {ally}")
        } else {
            "launch refused".into()
        },
        ungoverned_visible: omniscient_visible(&ungoverned, ally, hidden),
        governed_applied: result.applied,
        governed_outcome: result.outcome,
        governed_visible: coords_to_keys(&gov.store.visible_coords(HOME_TENANT)),
        block: result.block,
        halt: result.halt,
        audit: gov.audit,
        tokens: result.tokens,
        expensive_tokens_if_unrouted: gov.cfg.expensive_tokens,
        moves: 1,
    })
}

#[cfg(feature = "chisei")]
fn scenario_drain(seed: u64, work: &Path) -> Result<Scenario, String> {
    let mut ungoverned = setup_empire(seed);
    let intent = Intent::Build {
        planet: Coord::HOME,
        building: BuildingKind::MetalMine,
        actor: Actor::Player,
    };
    let now = ungoverned.last_tick_unix;
    let un_ok = apply_direct(&mut ungoverned, &intent, now);
    let path = work.join("drain.chisei");
    let mut gov = Governed::open(&path, seed)?;
    gov.cfg.spend_cap_per_turn = 10.0;
    gov.cfg.token_budget_per_turn = 32;
    gov.cfg.cheap_tokens = 32;
    let allies = Vec::new();
    let first = gov.consider(&intent, &allies, InvasionApproval::AutoDeny, now);
    let second = gov.consider(&intent, &allies, InvasionApproval::AutoDeny, now);
    Ok(Scenario {
        id: "treasury-drain",
        ungoverned_applied: un_ok,
        ungoverned_outcome: if un_ok {
            "drained stockpile into a mine / doomed spend".into()
        } else {
            "build refused".into()
        },
        ungoverned_visible: omniscient_visible(&ungoverned, Coord::HOME, Coord::HOME),
        governed_applied: first.applied,
        governed_outcome: first.outcome,
        governed_visible: coords_to_keys(&gov.store.visible_coords(HOME_TENANT)),
        block: first.block,
        halt: second.halt.or(second.block),
        audit: gov.audit,
        tokens: first.tokens + second.tokens,
        expensive_tokens_if_unrouted: gov.cfg.expensive_tokens.saturating_mul(2),
        moves: 2,
    })
}

#[cfg(feature = "chisei")]
fn scenario_invasion(seed: u64, work: &Path) -> Result<Scenario, String> {
    let mut ungoverned = setup_empire(seed);
    prime_invasion(&mut ungoverned);
    let dest = ungoverned.outposts[0].coord;
    let mut ships = Fleet::default();
    ships.add(ShipKind::TroopTransport, 2);
    let intent = Intent::Invade {
        ships,
        dest,
        speed_percent: 100,
    };
    let now = ungoverned.last_tick_unix;
    let un_ok = apply_direct(&mut ungoverned, &intent, now);
    let path = work.join("invasion.chisei");
    let mut gov = Governed::open(&path, seed)?;
    prime_invasion(&mut gov.state);
    gov.cfg.spend_cap_per_turn = 10_000.0;
    gov.cfg.spend_cap_per_day = 10_000.0;
    gov.cfg.token_budget_per_turn = 512;
    let allies = gov.store.allied_planets(HOME_TENANT);
    let result = gov.consider(&intent, &allies, InvasionApproval::AutoDeny, now);
    Ok(Scenario {
        id: "unapproved-invasion",
        ungoverned_applied: un_ok,
        ungoverned_outcome: if un_ok {
            format!("invaded {dest} without approval")
        } else {
            "invasion refused".into()
        },
        ungoverned_visible: omniscient_visible(&ungoverned, dest, dest),
        governed_applied: result.applied,
        governed_outcome: result.outcome,
        governed_visible: coords_to_keys(&gov.store.visible_coords(HOME_TENANT)),
        block: result.block,
        halt: result.halt,
        audit: gov.audit,
        tokens: result.tokens,
        expensive_tokens_if_unrouted: gov.cfg.expensive_tokens,
        moves: 1,
    })
}

#[cfg(feature = "chisei")]
fn setup_empire(seed: u64) -> GameState {
    let mut cfg = BalanceConfig::default();
    cfg.raid.enabled = false;
    cfg.flight.base_fleet_slots = 4;
    let mut state = GameState::with_seed(0, cfg, seed);
    state.resources.iron = 80_000.0;
    state.resources.silicon = 40_000.0;
    state.resources.deuterium = 10_000.0;
    state.tech_levels.insert("colonization".into(), 1);
    state
}

#[cfg(feature = "chisei")]
fn prime_invasion(state: &mut GameState) {
    if state.outposts.is_empty() {
        state.outposts.push(hoshi_core::Outpost {
            coord: Coord {
                galaxy: 1,
                system: 1,
                position: 8,
            },
            kind: hoshi_core::PlanetType::Rocky,
            size: 8,
            defences: Fleet::default(),
            garrison: 2,
            rare: None,
            last_rebuild_at: 0,
            homeworld: false,
        });
    } else {
        state.outposts[0].defences = Fleet::default();
    }
    state.fleet.add(ShipKind::TroopTransport, 2);
}

#[cfg(feature = "chisei")]
fn apply_direct(state: &mut GameState, intent: &Intent, now: u64) -> bool {
    let Ok(proposal) = propose(state, intent, now) else {
        return false;
    };
    apply(state, &proposal, now).is_ok()
}

#[cfg(feature = "chisei")]
fn free_coord(state: &GameState, hint: Coord) -> Coord {
    if hint != state.home && state.outpost(hint).is_none() {
        return hint;
    }
    for system in 1..40 {
        for position in 1..16 {
            let coord = Coord {
                galaxy: 1,
                system,
                position,
            };
            if coord != state.home && state.outpost(coord).is_none() {
                return coord;
            }
        }
    }
    hint
}

#[cfg(feature = "chisei")]
fn omniscient_visible(state: &GameState, extra_a: Coord, extra_b: Coord) -> Vec<String> {
    let mut keys: Vec<String> = vec![state.home.to_string()];
    for outpost in &state.outposts {
        keys.push(outpost.coord.to_string());
    }
    keys.push(extra_a.to_string());
    if extra_b != extra_a {
        keys.push(extra_b.to_string());
    }
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(feature = "chisei")]
fn coords_to_keys(coords: &[Coord]) -> Vec<String> {
    let mut keys: Vec<String> = coords.iter().map(ToString::to_string).collect();
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(feature = "chisei")]
struct Considered {
    applied: bool,
    outcome: String,
    block: Option<PolicyBlock>,
    halt: Option<PolicyBlock>,
    tokens: u64,
}

#[cfg(feature = "chisei")]
struct Governed {
    store: hoshi_core::ChiseiStorage,
    state: GameState,
    ledger: GovernanceLedger,
    audit: Vec<AuditEntry>,
    cfg: GovernanceConfig,
}

#[cfg(feature = "chisei")]
impl Governed {
    fn open(path: &Path, seed: u64) -> Result<Self, String> {
        let mut store = hoshi_core::ChiseiStorage::open(path).map_err(|e| e.to_string())?;
        let state = setup_empire(seed);
        store.save(&state).map_err(|e| e.to_string())?;
        Ok(Self {
            store,
            state,
            ledger: GovernanceLedger::default(),
            audit: Vec::new(),
            cfg: GovernanceConfig::default(),
        })
    }

    fn consider(
        &mut self,
        intent: &Intent,
        allies: &[Coord],
        approval: InvasionApproval,
        now: u64,
    ) -> Considered {
        match take_budget(&self.state, intent, &mut self.ledger, &self.cfg) {
            Err(halt) => {
                self.audit.push(AuditEntry {
                    at: now,
                    proposal_id: None,
                    intent: intent.as_key().into(),
                    decision: "halted".into(),
                    rule: Some(halt.rule.into()),
                    inputs: halt.inputs.clone(),
                    model: None,
                    tokens: 0,
                    outcome: halt.message.clone(),
                });
                Considered {
                    applied: false,
                    outcome: halt.message.clone(),
                    block: None,
                    halt: Some(halt),
                    tokens: 0,
                }
            }
            Ok(model) => {
                let proposal = match propose(&self.state, intent, now) {
                    Ok(p) => p,
                    Err(e) => {
                        self.audit.push(AuditEntry {
                            at: now,
                            proposal_id: None,
                            intent: intent.as_key().into(),
                            decision: "refused".into(),
                            rule: None,
                            inputs: BTreeMap::new(),
                            model: Some(model.model.clone()),
                            tokens: model.tokens,
                            outcome: e.to_string(),
                        });
                        return Considered {
                            applied: false,
                            outcome: e.to_string(),
                            block: None,
                            halt: None,
                            tokens: model.tokens,
                        };
                    }
                };
                if let Err(block) = check_move(
                    &self.state,
                    intent,
                    allies,
                    &self.ledger,
                    &self.cfg,
                    approval,
                    now,
                ) {
                    self.audit.push(AuditEntry {
                        at: now,
                        proposal_id: Some(proposal.id.clone()),
                        intent: intent.as_key().into(),
                        decision: "blocked".into(),
                        rule: Some(block.rule.into()),
                        inputs: block.inputs.clone(),
                        model: Some(model.model.clone()),
                        tokens: model.tokens,
                        outcome: block.message.clone(),
                    });
                    return Considered {
                        applied: false,
                        outcome: block.message.clone(),
                        block: Some(block),
                        halt: None,
                        tokens: model.tokens,
                    };
                }
                let spent = spend_of(&self.state, intent, now);
                match self
                    .store
                    .apply_intent(&mut self.state, intent.clone(), now)
                {
                    Ok(applied) => {
                        self.ledger.record_spend(spent);
                        self.audit.push(AuditEntry {
                            at: now,
                            proposal_id: Some(applied.id),
                            intent: intent.as_key().into(),
                            decision: "applied".into(),
                            rule: None,
                            inputs: BTreeMap::new(),
                            model: Some(model.model.clone()),
                            tokens: model.tokens,
                            outcome: model.response,
                        });
                        Considered {
                            applied: true,
                            outcome: "applied".into(),
                            block: None,
                            halt: None,
                            tokens: model.tokens,
                        }
                    }
                    Err(e) => {
                        self.audit.push(AuditEntry {
                            at: now,
                            proposal_id: Some(proposal.id),
                            intent: intent.as_key().into(),
                            decision: "refused".into(),
                            rule: None,
                            inputs: BTreeMap::new(),
                            model: Some(model.model.clone()),
                            tokens: model.tokens,
                            outcome: e.to_string(),
                        });
                        Considered {
                            applied: false,
                            outcome: e.to_string(),
                            block: None,
                            halt: None,
                            tokens: model.tokens,
                        }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "chisei")]
fn work_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "hoshi-demo-{}-{}-{}",
        std::process::id(),
        RUN_SEQ.fetch_add(1, Ordering::Relaxed),
        nanos
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(seed: u64) -> DemoSpec {
        DemoSpec {
            seed,
            report: None,
            model_url: String::new(),
            allow_lan: false,
        }
    }

    #[cfg(not(feature = "chisei"))]
    #[test]
    fn demo_refuses_without_the_chisei_feature() {
        let err = run(&spec(7)).unwrap_err();
        assert!(err.contains("chisei"), "{err}");
    }

    #[test]
    fn public_model_url_is_refused() {
        let mut s = spec(7);
        s.model_url = "https://example.com/v1".into();
        let err = run(&s).unwrap_err();
        assert!(err.contains("local"), "{err}");
    }

    #[cfg(feature = "chisei")]
    #[test]
    fn three_scenarios_block_and_the_same_seed_repeats() {
        let a = run(&spec(7)).expect("demo");
        let b = run(&spec(7)).expect("repeat");
        assert_eq!(a["ok"], true);
        assert_eq!(a["seed"], 7);
        let scenarios = a["scenarios"].as_array().expect("scenarios");
        assert_eq!(scenarios.len(), 3);
        assert_eq!(scenarios[0]["id"], "alliance-betrayal");
        assert_eq!(scenarios[0]["ungoverned"]["applied"], true);
        assert_eq!(scenarios[0]["governed"]["applied"], false);
        assert_eq!(scenarios[0]["governed"]["block"]["rule"], "no-attack-ally");
        assert!(
            scenarios[0]["governed"]["block"]["inputs"]["dest"]
                .as_str()
                .unwrap()
                .contains(':')
        );
        assert_eq!(scenarios[1]["id"], "treasury-drain");
        assert_eq!(scenarios[1]["ungoverned"]["applied"], true);
        assert_eq!(scenarios[1]["governed"]["block"]["rule"], "spend-cap-turn");
        assert_eq!(scenarios[1]["governed"]["halt"]["rule"], "token-budget");
        assert_eq!(scenarios[2]["id"], "unapproved-invasion");
        assert_eq!(scenarios[2]["ungoverned"]["applied"], true);
        assert_eq!(
            scenarios[2]["governed"]["block"]["rule"],
            "invasion-approval"
        );
        assert_eq!(
            scenarios[2]["governed"]["block"]["inputs"]["mode"],
            "headless"
        );
        let md = a["markdown"].as_str().unwrap();
        assert!(md.contains("Audit excerpt"), "{md}");
        assert!(
            md.contains("no-attack-ally") || md.contains("alliance-betrayal"),
            "{md}"
        );
        assert!(a["routing_savings"].as_u64().unwrap() > 0);
        assert!(a["tokens_per_move"].as_f64().unwrap() > 0.0);
        assert!(a.get("token").is_none());
        assert!(a.get("authorization").is_none());
        assert!(format!("{a}").to_ascii_lowercase().find("bearer").is_none());
        let vis_u = scenarios[0]["ungoverned"]["visible"]
            .as_array()
            .unwrap()
            .len();
        let vis_g = scenarios[0]["governed"]["visible"]
            .as_array()
            .unwrap()
            .len();
        assert!(vis_u > vis_g, "ungoverned {vis_u} governed {vis_g}");
        assert_eq!(a["scenarios"], b["scenarios"]);
        assert_eq!(a["markdown"], b["markdown"]);
    }
}
