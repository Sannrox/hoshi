//! In-memory what-if sandbox. Never written to the save.
//!
//! A [`Scenario`] forks a [`Baseline`] (a clone of live state at one tick),
//! holds [`PlannedAction`]s, and yields an [`Outcome`] of public fields only.
//! [`advance`](Scenario::advance) fast-forwards the copy with the same
//! [`GameState::catch_up`] as play so the player can compare outcomes.
//! [`promote`](Scenario::promote) re-validates the action list against
//! **current** live state through `propose` then `apply`. Catch-up stays in
//! the sandbox; a refused proposal leaves live untouched.

use crate::Risk;
use crate::intel::IntelFact;
use crate::intent::{Intent, IntentError, apply, propose};
use crate::kpi::Measurement;
use crate::leaders::Actor;
use crate::state::{GameState, Resources};
use std::fmt;

/// Live tick the scenario forked from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Baseline {
    pub at: u64,
}

/// One intent queued inside the sandbox, in order.
///
/// The queue clock is not stored: [`Scenario::promote`] re-validates against
/// live `now`, not the time used while planning.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedAction {
    pub intent: Intent,
}

/// Public comparison of a sandbox after queued actions and catch-up.
///
/// Hidden simulation (RNG, unrevealed map, pending intel, `next_raid_at`,
/// the deceived flag) stays off this snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub at: u64,
    pub resources: Resources,
    pub kpis: Vec<Measurement>,
    pub risks: Vec<Risk>,
    pub intel: Vec<IntelFact>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScenarioError {
    Intent(IntentError),
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenarioError::Intent(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ScenarioError {}

impl From<IntentError> for ScenarioError {
    fn from(value: IntentError) -> Self {
        Self::Intent(value)
    }
}

/// Sandboxed copy of the world. The live [`GameState`] is not stored here.
pub struct Scenario {
    baseline: Baseline,
    sandbox: GameState,
    planned: Vec<PlannedAction>,
}

impl Scenario {
    /// `CreateScenario`: fork live into an isolated copy at `live.last_tick_unix`.
    pub fn create(live: &GameState) -> Self {
        Self {
            baseline: Baseline {
                at: live.last_tick_unix,
            },
            sandbox: live.clone(),
            planned: Vec::new(),
        }
    }

    pub fn baseline(&self) -> Baseline {
        self.baseline
    }

    pub fn planned(&self) -> &[PlannedAction] {
        &self.planned
    }

    /// `QueuePlannedAction`: propose and apply on the copy only.
    pub fn queue(&mut self, intent: Intent, now_unix: u64) -> Result<(), ScenarioError> {
        let proposal = propose(&self.sandbox, &intent, now_unix)?;
        apply(&mut self.sandbox, &proposal, now_unix)?;
        self.planned.push(PlannedAction { intent });
        Ok(())
    }

    /// `AdvanceScenario`: the same [`GameState::catch_up`] as live play.
    pub fn advance(&mut self, until_unix: u64) {
        self.sandbox.catch_up(until_unix);
    }

    pub fn outcome(&self) -> Outcome {
        public_outcome(&self.sandbox)
    }

    /// `PromoteScenario`: replay planned intents on a trial of `live`.
    ///
    /// Sandbox catch-up is not copied onto live. Each intent is proposed at
    /// `now_unix` (current live time). The first refused proposal drops the
    /// trial; `live` is not assigned.
    pub fn promote(&self, live: &mut GameState, now_unix: u64) -> Result<(), ScenarioError> {
        let mut trial = live.clone();
        for action in &self.planned {
            let proposal = propose(&trial, &action.intent, now_unix)?;
            apply(&mut trial, &proposal, now_unix)?;
        }
        *live = trial;
        Ok(())
    }
}

fn public_outcome(state: &GameState) -> Outcome {
    Outcome {
        at: state.last_tick_unix,
        resources: state.resources,
        kpis: state.kpi_snapshot().measurements,
        risks: state.incidents.risks.clone(),
        intel: state.intel_facts_for(Actor::Player, state.last_tick_unix),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::buildings::BuildingKind;
    use crate::espionage::SpyReport;
    use crate::fleets::Coord;
    use crate::planets::PlanetType;

    fn raidless() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg
    }

    fn funded(now: u64) -> GameState {
        let mut state = GameState::with_config(now, raidless());
        state.resources.iron = 40_000.0;
        state.resources.silicon = 20_000.0;
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

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "left={a} right={b}");
    }

    #[test]
    fn scenario_actions_do_not_change_live() {
        let live = funded(1_000);
        let before = live.clone();
        let mut scenario = Scenario::create(&live);
        scenario.queue(build_mine(), 1_000).unwrap();
        scenario.advance(1_000 + 3_600);
        assert_eq!(live, before);
        assert_eq!(scenario.baseline().at, 1_000);
        assert_eq!(scenario.planned().len(), 1);
        assert_ne!(scenario.outcome().resources, before.resources);
    }

    #[test]
    fn scenario_advance_matches_step_by_step_catch_up() {
        let live = funded(0);
        let mut one_shot = Scenario::create(&live);
        one_shot.queue(build_mine(), 0).unwrap();
        one_shot.advance(7_200);

        let mut stepped = Scenario::create(&live);
        stepped.queue(build_mine(), 0).unwrap();
        for t in 1..=7_200 {
            stepped.advance(t);
        }

        let a = one_shot.outcome();
        let b = stepped.outcome();
        assert_eq!(a.at, 7_200);
        assert_eq!(b.at, 7_200);
        close(a.resources.iron, b.resources.iron);
        close(a.resources.silicon, b.resources.silicon);
        close(a.resources.deuterium, b.resources.deuterium);
    }

    #[test]
    fn promote_applies_planned_intents_to_live() {
        let mut live = funded(1_000);
        let mut scenario = Scenario::create(&live);
        scenario.queue(build_mine(), 1_000).unwrap();
        scenario.promote(&mut live, 1_000).unwrap();
        assert_eq!(live.build_queue.len(), 1);
        assert_eq!(live.build_queue[0].building, BuildingKind::MetalMine);
    }

    #[test]
    fn promote_does_not_copy_sandbox_clock() {
        let mut live = funded(1_000);
        let mut scenario = Scenario::create(&live);
        scenario.queue(build_mine(), 1_000).unwrap();
        scenario.advance(1_000 + 3_600);
        assert_eq!(scenario.outcome().at, 4_600);
        scenario.promote(&mut live, 1_000).unwrap();
        assert_eq!(live.last_tick_unix, 1_000);
        assert_eq!(live.build_queue.len(), 1);
    }

    #[test]
    fn refused_promote_does_not_write_live() {
        let mut live = funded(1_000);
        let mut scenario = Scenario::create(&live);
        scenario.queue(build_mine(), 1_000).unwrap();
        live.resources.iron = 0.0;
        live.resources.silicon = 0.0;
        let before = live.clone();
        let err = scenario.promote(&mut live, 1_000).unwrap_err();
        assert!(matches!(err, ScenarioError::Intent(_)), "{err}");
        assert_eq!(live, before);
        assert!(live.build_queue.is_empty());
    }

    #[test]
    fn outcome_hides_pending_intel_and_simulation_secrets() {
        let mut live = funded(1_000);
        live.next_raid_at = Some(9_999);
        live.spy_reports.push(SpyReport {
            id: 1,
            target: Coord {
                galaxy: 1,
                system: 1,
                position: 8,
            },
            created_at: 1_000,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 1.0,
            tier: "resources".into(),
            resources: Some(Resources {
                iron: 99_999.0,
                silicon: 0.0,
                rare_earths: 0.0,
                water_ice: 0.0,
                helium_3: 0.0,
                deuterium: 0.0,
            }),
            fleet: None,
            defences: None,
            garrison: None,
            kind: Some(PlanetType::Rocky),
            size: Some(100),
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 1_000,
            arrives_at: 50_000,
            confidence: 0.9,
            deceived: true,
        });
        live.spy_reports.push(SpyReport {
            id: 2,
            target: Coord {
                galaxy: 1,
                system: 1,
                position: 9,
            },
            created_at: 500,
            probes_sent: 1,
            probes_lost: 0,
            detected: true,
            score: 1.0,
            tier: "kind".into(),
            resources: Some(Resources {
                iron: 12.0,
                silicon: 3.0,
                rare_earths: 0.0,
                water_ice: 0.0,
                helium_3: 0.0,
                deuterium: 0.0,
            }),
            fleet: None,
            defences: None,
            garrison: None,
            kind: Some(PlanetType::Rocky),
            size: Some(80),
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 500,
            arrives_at: 1_000,
            confidence: 0.4,
            deceived: true,
        });
        let scenario = Scenario::create(&live);
        let outcome = scenario.outcome();
        assert_eq!(outcome.at, 1_000);
        assert!(!outcome.risks.is_empty());
        assert!(!outcome.kpis.is_empty());
        let dump = format!("{outcome:?}");
        assert!(!dump.contains("next_raid_at"), "{dump}");
        assert!(!dump.contains("deceived"), "{dump}");
        assert!(!dump.contains("rng"), "{dump}");
        assert!(!dump.contains("unrevealed"), "{dump}");
        assert!(
            outcome.intel.iter().all(|fact| fact.object != "1:1:8"),
            "{:?}",
            outcome.intel
        );
        assert!(
            outcome
                .intel
                .iter()
                .any(|fact| fact.object == "1:1:9" && fact.field == "kind"),
            "{:?}",
            outcome.intel
        );
        assert!(
            outcome
                .intel
                .iter()
                .any(|fact| fact.field == "resources.iron"
                    && fact.observations.iter().any(|o| match o.value {
                        crate::IntelValue::Number(v) => (v - 12.0).abs() < 1e-9,
                        _ => false,
                    })),
            "{:?}",
            outcome.intel
        );
    }

    #[test]
    fn outcome_intel_ages_confidence_like_live() {
        let mut live = funded(1_000);
        live.spy_reports.push(SpyReport {
            id: 2,
            target: Coord {
                galaxy: 1,
                system: 1,
                position: 9,
            },
            created_at: 500,
            probes_sent: 1,
            probes_lost: 0,
            detected: true,
            score: 1.0,
            tier: "kind".into(),
            resources: None,
            fleet: None,
            defences: None,
            garrison: None,
            kind: Some(PlanetType::Rocky),
            size: Some(80),
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 500,
            arrives_at: 1_000,
            confidence: 0.4,
            deceived: true,
        });
        let mut scenario = Scenario::create(&live);
        let until = 1_000 + 36_000;
        scenario.advance(until);
        let mut expected = live.clone();
        expected.catch_up(until);
        let outcome = scenario.outcome();
        assert_eq!(
            outcome.intel,
            expected.intel_facts_for(Actor::Player, until)
        );
        let aged = outcome
            .intel
            .iter()
            .find(|fact| fact.object == "1:1:9" && fact.field == "kind")
            .and_then(|fact| fact.observations.first())
            .map(|obs| obs.confidence)
            .expect("kind fact");
        assert!(
            (aged - 0.4).abs() > 1e-6,
            "confidence should decay with age, got {aged}"
        );
    }
}
