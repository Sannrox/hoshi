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
    Build {
        planet: Coord,
        building: BuildingKind,
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
}

impl Intent {
    pub fn as_key(&self) -> &'static str {
        match self {
            Intent::Build { .. } => "build",
            Intent::Research { .. } => "research",
            Intent::Launch { .. } => "launch",
            Intent::Colonize { .. } => "colonize",
            Intent::Invade { .. } => "invade",
            Intent::PlaceOrder { .. } => "place_order",
            Intent::AcceptContract { .. } => "accept_contract",
            Intent::FulfilContract { .. } => "fulfil_contract",
            Intent::ShipGoods { .. } => "ship_goods",
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
    Build(BuildError),
    Research(ResearchError),
    Fleet(FleetError),
    Market(MarketError),
    Save(String),
}

impl fmt::Display for IntentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntentError::Build(e) => write!(f, "{e}"),
            IntentError::Research(e) => write!(f, "{e}"),
            IntentError::Fleet(e) => write!(f, "{e}"),
            IntentError::Market(e) => write!(f, "{e}"),
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
        Intent::Build {
            planet,
            building,
            actor,
        } => format!(
            "build:{now_unix}:{tick}:{planet}:{}:{}",
            building.as_key(),
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
            cargo.metal,
            cargo.crystal,
            cargo.deuterium,
            contract.map(|id| id.to_string()).unwrap_or_default(),
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
        Intent::Build {
            planet,
            building,
            actor,
        } => state
            .enqueue_build_as(*planet, *building, now_unix, *actor)
            .map_err(IntentError::Build),
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
                good: ResourceKind::Crystal,
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
    fn distinct_intents_at_the_same_time_get_distinct_ids() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.resources.metal = 80_000.0;
        state.resources.crystal = 40_000.0;
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
        state.resources.metal = 40_000.0;
        state.resources.crystal = 20_000.0;
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
        assert!(state.resources.metal < snapshot.resources.metal);
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
