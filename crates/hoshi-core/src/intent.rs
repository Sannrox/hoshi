//! Propose then apply. A refused proposal never mutates live state.

use crate::buildings::{BuildError, BuildingKind};
use crate::fleets::{Coord, FleetError};
use crate::leaders::Actor;
use crate::market::{MarketError, OrderSide};
use crate::research::ResearchError;
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, ResourceKind, Resources};
use std::fmt;

/// One governed move. Names match the ontology action list.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Cheat {
        code: String,
        actor: Actor,
    },
    Dismantle {
        planet: Coord,
        building: BuildingKind,
        actor: Actor,
    },
    Build {
        planet: Coord,
        building: BuildingKind,
        actor: Actor,
    },
    FusionMode {
        planet: Coord,
        mode: crate::FusionMode,
        actor: Actor,
    },
    Research {
        tech: String,
    },
    Launch {
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
        hold_secs: u64,
    },
    Colonize {
        dest: Coord,
        speed_percent: u8,
    },
    Invade {
        ships: Fleet,
        dest: Coord,
        speed_percent: u8,
    },
    PlaceOrder {
        region: Coord,
        side: OrderSide,
        good: ResourceKind,
        amount: f64,
        limit: Option<f64>,
        ttl_secs: u64,
    },
    AcceptContract {
        id: u64,
    },
    FulfilContract {
        id: u64,
    },
    ShipGoods {
        origin: Coord,
        dest: Coord,
        cargo: Resources,
        contract: Option<u64>,
    },
    SetBudget {
        center: String,
        limit: Option<f64>,
        actor: Actor,
    },
    RequestSpend {
        center: String,
        amount: f64,
    },
    ApproveSpend {
        id: u64,
        actor: Actor,
    },
    SetTarget {
        kpi: String,
        owner: String,
        value: f64,
        actor: Actor,
    },
    GenerateReport {
        actor: Actor,
    },
    OpenIncident {
        kind: String,
        severity: String,
        actor: Actor,
    },
    AssignIncident {
        id: u64,
        leader: u64,
        actor: Actor,
    },
    ResolveIncident {
        id: u64,
        note: String,
        actor: Actor,
    },
    ReviewIncident {
        id: u64,
        actor: Actor,
    },
    ScheduleMaintenance {
        asset_id: u64,
        plan: String,
        actor: Actor,
    },
    CompleteWorkOrder {
        order_id: u64,
        actor: Actor,
    },
}

impl Intent {
    pub fn as_key(&self) -> &'static str {
        match self {
            Intent::Cheat { .. } => "cheat",
            Intent::Build { .. } => "build",
            Intent::Dismantle { .. } => "dismantle",
            Intent::FusionMode { .. } => "fusion_mode",
            Intent::Research { .. } => "research",
            Intent::Launch { .. } => "launch",
            Intent::Colonize { .. } => "colonize",
            Intent::Invade { .. } => "invade",
            Intent::PlaceOrder { .. } => "place_order",
            Intent::AcceptContract { .. } => "accept_contract",
            Intent::FulfilContract { .. } => "fulfil_contract",
            Intent::ShipGoods { .. } => "ship_goods",
            Intent::SetBudget { .. } => "set_budget",
            Intent::RequestSpend { .. } => "request_spend",
            Intent::ApproveSpend { .. } => "approve_spend",
            Intent::SetTarget { .. } => "set_target",
            Intent::GenerateReport { .. } => "generate_report",
            Intent::OpenIncident { .. } => "open_incident",
            Intent::AssignIncident { .. } => "assign_incident",
            Intent::ResolveIncident { .. } => "resolve_incident",
            Intent::ReviewIncident { .. } => "review_incident",
            Intent::ScheduleMaintenance { .. } => "schedule_maintenance",
            Intent::CompleteWorkOrder { .. } => "complete_work_order",
        }
    }
}

/// Dry-run result. Apply with the same intent to mutate.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub id: String,
    pub intent: Intent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IntentError {
    Cheat(crate::CheatError),
    Build(BuildError),
    Fusion(crate::FusionError),
    Research(ResearchError),
    Fleet(FleetError),
    Market(MarketError),
    Finance(crate::FinanceError),
    Kpi(crate::KpiError),
    Incident(crate::IncidentError),
    Maintenance(crate::MaintenanceError),
    Save(String),
}

impl fmt::Display for IntentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntentError::Cheat(e) => write!(f, "{e}"),
            IntentError::Build(e) => write!(f, "{e}"),
            IntentError::Fusion(e) => write!(f, "{e}"),
            IntentError::Research(e) => write!(f, "{e}"),
            IntentError::Fleet(e) => write!(f, "{e}"),
            IntentError::Market(e) => write!(f, "{e}"),
            IntentError::Finance(e) => write!(f, "{e}"),
            IntentError::Kpi(e) => write!(f, "{e}"),
            IntentError::Incident(e) => write!(f, "{e}"),
            IntentError::Maintenance(e) => write!(f, "{e}"),
            IntentError::Save(e) => write!(f, "save backend: {e}"),
        }
    }
}

impl std::error::Error for IntentError {}

/// Validate `intent` against a clone. Live `state` is not written.
pub fn propose(state: &GameState, intent: &Intent, now_unix: u64) -> Result<Proposal, IntentError> {
    let mut trial = state.clone();
    apply_inner(&mut trial, intent, now_unix)?;
    Ok(Proposal {
        id: proposal_id(intent, now_unix, trial.last_tick_unix),
        intent: intent.clone(),
    })
}

fn proposal_id(intent: &Intent, now_unix: u64, tick: u64) -> String {
    match intent {
        Intent::Cheat { code, actor } => {
            format!("cheat:{now_unix}:{tick}:{}:{code}", actor.as_key())
        }
        Intent::Dismantle {
            planet,
            building,
            actor,
        } => format!(
            "dismantle:{now_unix}:{tick}:{planet}:{}:{}",
            building.as_key(),
            actor.as_key()
        ),
        Intent::Build {
            planet,
            building,
            actor,
        } => format!(
            "build:{now_unix}:{tick}:{planet}:{}:{}",
            building.as_key(),
            actor.as_key()
        ),
        Intent::FusionMode {
            planet,
            mode,
            actor,
        } => format!(
            "fusion_mode:{now_unix}:{tick}:{planet}:{}:{}",
            mode.as_key(),
            actor.as_key()
        ),
        Intent::Research { tech } => format!("research:{now_unix}:{tick}:{tech}"),
        Intent::Launch {
            ships,
            dest,
            speed_percent,
            hold_secs,
        } => format!(
            "launch:{now_unix}:{tick}:{dest}:{speed_percent}:{hold_secs}:{}",
            fleet_sig(ships)
        ),
        Intent::Colonize {
            dest,
            speed_percent,
        } => format!("colonize:{now_unix}:{tick}:{dest}:{speed_percent}"),
        Intent::Invade {
            ships,
            dest,
            speed_percent,
        } => format!(
            "invade:{now_unix}:{tick}:{dest}:{speed_percent}:{}",
            fleet_sig(ships)
        ),
        Intent::PlaceOrder {
            region,
            side,
            good,
            amount,
            limit,
            ttl_secs,
        } => format!(
            "place_order:{now_unix}:{tick}:{region}:{}:{}:{amount}:{}:{ttl_secs}",
            side.as_key(),
            good.as_key(),
            limit.map(|p| p.to_string()).unwrap_or_else(|| "mkt".into()),
        ),
        Intent::AcceptContract { id } => format!("accept_contract:{now_unix}:{tick}:{id}"),
        Intent::FulfilContract { id } => format!("fulfil_contract:{now_unix}:{tick}:{id}"),
        Intent::ShipGoods {
            origin,
            dest,
            cargo,
            contract,
        } => format!(
            "ship_goods:{now_unix}:{tick}:{origin}:{dest}:{}:{}:{}:{}",
            cargo.iron,
            cargo.silicon,
            cargo.deuterium,
            contract.map(|id| id.to_string()).unwrap_or_default(),
        ),
        Intent::SetBudget {
            center,
            limit,
            actor,
        } => format!(
            "set_budget:{now_unix}:{tick}:{center}:{}:{}",
            limit
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".into()),
            actor.as_key()
        ),
        Intent::RequestSpend { center, amount } => {
            format!("request_spend:{now_unix}:{tick}:{center}:{amount}")
        }
        Intent::ApproveSpend { id, actor } => {
            format!("approve_spend:{now_unix}:{tick}:{id}:{}", actor.as_key())
        }
        Intent::SetTarget {
            kpi,
            owner,
            value,
            actor,
        } => format!(
            "set_target:{now_unix}:{tick}:{kpi}:{owner}:{value}:{}",
            actor.as_key()
        ),
        Intent::GenerateReport { actor } => {
            format!("generate_report:{now_unix}:{tick}:{}", actor.as_key())
        }
        Intent::OpenIncident {
            kind,
            severity,
            actor,
        } => format!(
            "open_incident:{now_unix}:{tick}:{kind}:{severity}:{}",
            actor.as_key()
        ),
        Intent::AssignIncident { id, leader, actor } => {
            format!(
                "assign_incident:{now_unix}:{tick}:{id}:{leader}:{}",
                actor.as_key()
            )
        }
        Intent::ResolveIncident { id, note, actor } => {
            format!(
                "resolve_incident:{now_unix}:{tick}:{id}:{note}:{}",
                actor.as_key()
            )
        }
        Intent::ReviewIncident { id, actor } => {
            format!("review_incident:{now_unix}:{tick}:{id}:{}", actor.as_key())
        }
        Intent::ScheduleMaintenance {
            asset_id,
            plan,
            actor,
        } => format!(
            "schedule_maintenance:{now_unix}:{tick}:{asset_id}:{plan}:{}",
            actor.as_key()
        ),
        Intent::CompleteWorkOrder { order_id, actor } => format!(
            "complete_work_order:{now_unix}:{tick}:{order_id}:{}",
            actor.as_key()
        ),
    }
}

fn fleet_sig(fleet: &Fleet) -> String {
    ShipKind::ALL
        .iter()
        .map(|&kind| format!("{}:{}", kind.as_key(), fleet.get(kind)))
        .collect::<Vec<_>>()
        .join(",")
}

/// Run a previously proposed intent. Does not save.
pub fn apply(state: &mut GameState, proposal: &Proposal, now_unix: u64) -> Result<(), IntentError> {
    apply_inner(state, &proposal.intent, now_unix)
}

fn apply_inner(state: &mut GameState, intent: &Intent, now_unix: u64) -> Result<(), IntentError> {
    match intent {
        Intent::Cheat { code, actor } => state
            .apply_cheat(code, *actor, now_unix)
            .map_err(IntentError::Cheat),
        Intent::Dismantle {
            planet,
            building,
            actor,
        } => state
            .enqueue_dismantle_as(*planet, *building, now_unix, *actor)
            .map_err(IntentError::Build),
        Intent::Build {
            planet,
            building,
            actor,
        } => state
            .enqueue_build_as(*planet, *building, now_unix, *actor)
            .map_err(IntentError::Build),
        Intent::FusionMode {
            planet,
            mode,
            actor,
        } => state
            .set_fusion_mode(*planet, *mode, now_unix, *actor)
            .map_err(IntentError::Fusion),
        Intent::Research { tech } => state
            .start_research(tech, now_unix)
            .map_err(IntentError::Research),
        Intent::Launch {
            ships,
            dest,
            speed_percent,
            hold_secs,
        } => state
            .launch_fleet(*ships, *dest, *speed_percent, *hold_secs, now_unix)
            .map_err(IntentError::Fleet),
        Intent::Colonize {
            dest,
            speed_percent,
        } => state
            .launch_colony(*dest, *speed_percent, now_unix)
            .map(|_| ())
            .map_err(IntentError::Fleet),
        Intent::Invade {
            ships,
            dest,
            speed_percent,
        } => state
            .launch_invasion(*ships, *dest, *speed_percent, now_unix)
            .map(|_| ())
            .map_err(IntentError::Fleet),
        Intent::PlaceOrder {
            region,
            side,
            good,
            amount,
            limit,
            ttl_secs,
        } => state
            .place_order(*region, *side, *good, *amount, *limit, *ttl_secs, now_unix)
            .map(|_| ())
            .map_err(IntentError::Market),
        Intent::AcceptContract { id } => state
            .accept_contract(*id, now_unix)
            .map_err(IntentError::Market),
        Intent::FulfilContract { id } => state
            .fulfil_contract(*id, now_unix)
            .map_err(IntentError::Market),
        Intent::ShipGoods {
            origin,
            dest,
            cargo,
            contract,
        } => state
            .ship_goods(*origin, *dest, *cargo, *contract, now_unix)
            .map(|_| ())
            .map_err(IntentError::Market),
        Intent::SetBudget {
            center,
            limit,
            actor,
        } => {
            let parsed = crate::CostCenter::parse(center)
                .ok_or(IntentError::Finance(crate::FinanceError::UnknownCenter))?;
            state
                .set_budget(parsed, *limit, *actor, now_unix)
                .map_err(IntentError::Finance)
        }
        Intent::RequestSpend { .. } => {
            Err(IntentError::Finance(crate::FinanceError::RequestViaAction))
        }
        Intent::ApproveSpend { id, actor } => state
            .approve_spend(*id, *actor, now_unix)
            .map_err(IntentError::Finance),
        Intent::SetTarget {
            kpi,
            owner,
            value,
            actor,
        } => {
            let kpi =
                crate::KpiId::from_key(kpi).ok_or(IntentError::Kpi(crate::KpiError::UnknownKpi))?;
            let owner = crate::TargetOwner::parse(owner)
                .ok_or(IntentError::Kpi(crate::KpiError::UnknownOwner))?;
            state
                .set_kpi_target(kpi, owner, *value, *actor, now_unix)
                .map_err(IntentError::Kpi)
        }
        Intent::GenerateReport { actor } => state
            .generate_kpi_report(*actor, now_unix)
            .map(|_| ())
            .map_err(IntentError::Kpi),
        Intent::OpenIncident {
            kind,
            severity,
            actor,
        } => {
            let kind = crate::IncidentKind::from_key(kind).ok_or(IntentError::Incident(
                crate::IncidentError::Parse("unknown kind".into()),
            ))?;
            let severity = crate::Severity::from_key(severity).ok_or(IntentError::Incident(
                crate::IncidentError::Parse("unknown severity".into()),
            ))?;
            state
                .open_incident(kind, severity, Vec::new(), *actor, now_unix)
                .map(|_| ())
                .map_err(IntentError::Incident)
        }
        Intent::AssignIncident { id, leader, actor } => state
            .assign_incident(*id, *leader, *actor, now_unix)
            .map_err(IntentError::Incident),
        Intent::ResolveIncident { id, note, actor } => state
            .resolve_incident(*id, note, *actor, now_unix)
            .map_err(IntentError::Incident),
        Intent::ReviewIncident { id, actor } => state
            .review_incident(*id, *actor, now_unix)
            .map(|_| ())
            .map_err(IntentError::Incident),
        Intent::ScheduleMaintenance {
            asset_id,
            plan,
            actor,
        } => {
            let plan = crate::PlanKind::from_key(plan).ok_or(IntentError::Maintenance(
                crate::MaintenanceError::Parse("unknown plan".into()),
            ))?;
            state
                .schedule_maintenance(*asset_id, plan, *actor, now_unix)
                .map(|_| ())
                .map_err(IntentError::Maintenance)
        }
        Intent::CompleteWorkOrder { order_id, actor } => state
            .complete_work_order(*order_id, *actor, now_unix)
            .map_err(IntentError::Maintenance),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::ontology::ACTIONS;
    use crate::ships::ShipKind;

    #[test]
    fn action_names_match_ontology() {
        let names = [
            Intent::Build {
                planet: Coord::HOME,
                building: BuildingKind::MetalMine,
                actor: Actor::Player,
            }
            .as_key(),
            Intent::Research {
                tech: "energy".into(),
            }
            .as_key(),
            Intent::Launch {
                ships: Fleet::default(),
                dest: Coord::HOME,
                speed_percent: 100,
                hold_secs: 0,
            }
            .as_key(),
            Intent::Colonize {
                dest: Coord::HOME,
                speed_percent: 100,
            }
            .as_key(),
            Intent::Invade {
                ships: Fleet::default(),
                dest: Coord::HOME,
                speed_percent: 100,
            }
            .as_key(),
            Intent::PlaceOrder {
                region: Coord::HOME,
                side: OrderSide::Buy,
                good: ResourceKind::Silicon,
                amount: 1.0,
                limit: Some(2.0),
                ttl_secs: 1,
            }
            .as_key(),
            Intent::AcceptContract { id: 1 }.as_key(),
            Intent::FulfilContract { id: 1 }.as_key(),
            Intent::ShipGoods {
                origin: Coord::HOME,
                dest: Coord::HOME,
                cargo: Resources::ZERO,
                contract: None,
            }
            .as_key(),
            Intent::SetBudget {
                center: "industry".into(),
                limit: Some(1.0),
                actor: Actor::Player,
            }
            .as_key(),
            Intent::RequestSpend {
                center: "industry".into(),
                amount: 1.0,
            }
            .as_key(),
            Intent::ApproveSpend {
                id: 1,
                actor: Actor::Player,
            }
            .as_key(),
            Intent::SetTarget {
                kpi: "output".into(),
                owner: "empire".into(),
                value: 1.0,
                actor: Actor::Player,
            }
            .as_key(),
            Intent::GenerateReport {
                actor: Actor::Player,
            }
            .as_key(),
            Intent::OpenIncident {
                kind: "manual".into(),
                severity: "low".into(),
                actor: Actor::Player,
            }
            .as_key(),
            Intent::AssignIncident {
                id: 1,
                leader: 1,
                actor: Actor::Player,
            }
            .as_key(),
            Intent::ResolveIncident {
                id: 1,
                note: "done".into(),
                actor: Actor::Player,
            }
            .as_key(),
            Intent::ReviewIncident {
                id: 1,
                actor: Actor::Player,
            }
            .as_key(),
            Intent::ScheduleMaintenance {
                asset_id: 1,
                plan: "time".into(),
                actor: Actor::Player,
            }
            .as_key(),
            Intent::CompleteWorkOrder {
                order_id: 1,
                actor: Actor::Player,
            }
            .as_key(),
        ];
        assert_eq!(names, ACTIONS);
    }

    #[test]
    fn refused_proposal_leaves_state_unchanged() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let state = GameState::with_config(1_000, cfg);
        let before = state.clone();
        let err = propose(
            &state,
            &Intent::Research {
                tech: "not-a-tech".into(),
            },
            1_000,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            IntentError::Research(ResearchError::UnknownTech { .. })
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn request_spend_intent_is_refused() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let state = GameState::with_config(1_000, cfg);
        let before = state.clone();
        let err = propose(
            &state,
            &Intent::RequestSpend {
                center: "industry".into(),
                amount: 1.0,
            },
            1_000,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                IntentError::Finance(crate::FinanceError::RequestViaAction)
            ),
            "{err:?}"
        );
        assert_eq!(state, before);
    }

    #[test]
    fn distinct_intents_at_the_same_time_get_distinct_ids() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 80_000.0;
        state.resources.silicon = 40_000.0;
        let metal = propose(
            &state,
            &Intent::Build {
                planet: Coord::HOME,
                building: BuildingKind::MetalMine,
                actor: Actor::Player,
            },
            1_000,
        )
        .unwrap();
        let crystal = propose(
            &state,
            &Intent::Build {
                planet: Coord::HOME,
                building: BuildingKind::CrystalMine,
                actor: Actor::Player,
            },
            1_000,
        )
        .unwrap();
        assert_ne!(metal.id, crystal.id);
    }

    #[test]
    fn apply_runs_only_after_propose() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.iron = 40_000.0;
        state.resources.silicon = 20_000.0;
        state.resources.deuterium = 10_000.0;
        let intent = Intent::Build {
            planet: Coord::HOME,
            building: BuildingKind::MetalMine,
            actor: Actor::Player,
        };
        let snapshot = state.clone();
        let proposal = propose(&state, &intent, 1_000).unwrap();
        assert_eq!(state, snapshot, "propose is a dry run");
        apply(&mut state, &proposal, 1_000).unwrap();
        assert_eq!(state.build_queue.len(), 1);
        assert!(state.resources.iron < snapshot.resources.iron);
    }

    #[test]
    fn colonize_intent_needs_a_colony_ship() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let state = GameState::with_config(1_000, cfg);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        let err = propose(
            &state,
            &Intent::Colonize {
                dest,
                speed_percent: 100,
            },
            1_000,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            IntentError::Fleet(FleetError::NeedColonyShip)
        ));
        let mut ready = state;
        ready.tech_levels.insert("colonization".into(), 1);
        ready.resources.deuterium = 10_000.0;
        ready.fleet.add(ShipKind::ColonyShip, 1);
        propose(
            &ready,
            &Intent::Colonize {
                dest,
                speed_percent: 100,
            },
            1_000,
        )
        .unwrap();
    }
}
