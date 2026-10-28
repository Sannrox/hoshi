//! One choosable deadline, derived from the save. Not stored.

use crate::ships::ShipKind;
use crate::state::GameState;
use crate::{Formation, Stance};

/// Which clock the decision reads. Lower rank wins a deadline tie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionId {
    Story,
    Tactical,
    Raid,
}

impl DecisionId {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Story => "story",
            Self::Tactical => "tactical",
            Self::Raid => "raid",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Story => 0,
            Self::Tactical => 1,
            Self::Raid => 2,
        }
    }
}

/// Core action the player can still take before the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionAction {
    ChooseStory,
    QueueTacticalAction,
    SetStance,
    ArmHome,
}

impl DecisionAction {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::ChooseStory => "choose_story",
            Self::QueueTacticalAction => "queue_tactical_action",
            Self::SetStance => "set_stance",
            Self::ArmHome => "arm",
        }
    }
}

/// The earliest choice still open. Deadlines are the story, fight, or raid clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpiringDecision {
    pub id: DecisionId,
    pub deadline: u64,
    pub action: DecisionAction,
}

/// At most one decision. The earliest deadline wins. A tie keeps story, then
/// a live fight, then the raid. An idle queue and an in-flight recall are not
/// decisions.
pub fn expiring_decision(state: &GameState) -> Option<ExpiringDecision> {
    let mut best: Option<ExpiringDecision> = None;
    if let Some(pending) = &state.story_pending
        && state.last_tick_unix < pending.timeout_at
    {
        consider(
            &mut best,
            ExpiringDecision {
                id: DecisionId::Story,
                deadline: pending.timeout_at,
                action: DecisionAction::ChooseStory,
            },
        );
    }
    if state.interactive_tactics
        && let Some(pending) = &state.pending_tactical
        && state.last_tick_unix < pending.next_at
    {
        consider(
            &mut best,
            ExpiringDecision {
                id: DecisionId::Tactical,
                deadline: pending.next_at,
                action: DecisionAction::QueueTacticalAction,
            },
        );
    }
    if let Some(raid) = raid_decision(state) {
        consider(&mut best, raid);
    }
    best
}

fn consider(best: &mut Option<ExpiringDecision>, next: ExpiringDecision) {
    let replace = match *best {
        None => true,
        Some(current) => {
            next.deadline < current.deadline
                || (next.deadline == current.deadline && next.id.rank() < current.id.rank())
        }
    };
    if replace {
        *best = Some(next);
    }
}

fn raid_decision(state: &GameState) -> Option<ExpiringDecision> {
    let lands = state.next_raid_at?;
    state.raid_countdown_secs()?;
    let armed = home_armed(state);
    // A defence grid resolves through the planet battle, which ignores stance
    // and formation. Only an empty grid still has a fleet-tactics choice.
    let action = if !armed && state.defences.is_empty() {
        DecisionAction::ArmHome
    } else if armed
        && state.defences.is_empty()
        && state.fleet_tactics.formation == Formation::Neutral
        && state.fleet_tactics.stance == Stance::Neutral
    {
        DecisionAction::SetStance
    } else {
        return None;
    };
    Some(ExpiringDecision {
        id: DecisionId::Raid,
        deadline: lands,
        action,
    })
}

fn home_armed(state: &GameState) -> bool {
    ShipKind::ALL
        .iter()
        .any(|&kind| state.fleet.get(kind) > 0 && state.config.ships.get(kind).attack > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Coord;
    use crate::ships::ShipKind;
    use crate::state::GameState;
    use crate::{
        DefenceKind, Fleet, Mission, MissionPhase, PendingStory, PendingTactical, Resources,
        RoundAction, TacticalPlan,
    };

    fn quiet(now: u64) -> GameState {
        let mut state = GameState::new(now);
        state.config.story = crate::StoryCatalog::empty();
        state.config.market.enabled = false;
        state.config.diplomacy.enabled = false;
        state.story_pending = None;
        state
    }

    fn pending_story(timeout_at: u64) -> PendingStory {
        PendingStory {
            event_id: "not-in-catalog".into(),
            offered_at: 0,
            timeout_at,
        }
    }

    fn warn(state: &mut GameState, lands: u64) {
        state.config.raid.enabled = true;
        state.config.raid.min_warning_secs = 10_000;
        state.next_raid_at = Some(lands);
    }

    #[test]
    fn sooner_story_beats_a_warned_raid() {
        let mut state = quiet(1_000);
        state.story_pending = Some(pending_story(1_200));
        warn(&mut state, 1_800);
        state.fleet.add(ShipKind::LightFighter, 3);
        let decision = expiring_decision(&state).expect("story");
        assert_eq!(decision.id, DecisionId::Story);
        assert_eq!(decision.action, DecisionAction::ChooseStory);
        assert_eq!(
            decision.deadline,
            state.story_pending.as_ref().unwrap().timeout_at
        );
    }

    #[test]
    fn equal_deadlines_keep_the_story() {
        let mut state = quiet(1_000);
        state.story_pending = Some(pending_story(1_500));
        warn(&mut state, 1_500);
        state.fleet.add(ShipKind::LightFighter, 1);
        let decision = expiring_decision(&state).expect("story");
        assert_eq!(decision.id, DecisionId::Story);
        assert_eq!(decision.deadline, 1_500);
    }

    #[test]
    fn armed_neutral_raid_is_a_stance_decision() {
        let mut state = quiet(1_000);
        warn(&mut state, 1_500);
        state.fleet.add(ShipKind::LightFighter, 2);
        assert_eq!(state.fleet_tactics.stance, Stance::Neutral);
        assert_eq!(state.fleet_tactics.formation, Formation::Neutral);
        let decision = expiring_decision(&state).expect("raid");
        assert_eq!(decision.id, DecisionId::Raid);
        assert_eq!(decision.action, DecisionAction::SetStance);
        assert_eq!(decision.deadline, state.next_raid_at.unwrap());
    }

    #[test]
    fn undefended_home_inside_the_warning_is_an_arm_decision() {
        let mut state = quiet(1_000);
        warn(&mut state, 1_400);
        state.fleet.add(ShipKind::ColonyShip, 2);
        assert!(state.defences.is_empty());
        let decision = expiring_decision(&state).expect("arm");
        assert_eq!(decision.id, DecisionId::Raid);
        assert_eq!(decision.action, DecisionAction::ArmHome);
        assert_eq!(decision.deadline, 1_400);
    }

    #[test]
    fn armed_defences_keep_a_later_story_instead_of_set_stance() {
        let mut state = quiet(1_000);
        state.story_pending = Some(pending_story(1_800));
        warn(&mut state, 1_200);
        state.fleet.add(ShipKind::LightFighter, 2);
        state.defences.add(DefenceKind::LightCannon, 1);
        assert_eq!(state.fleet_tactics.stance, Stance::Neutral);
        assert_eq!(state.fleet_tactics.formation, Formation::Neutral);
        let decision = expiring_decision(&state).expect("story");
        assert_eq!(decision.id, DecisionId::Story);
        assert_eq!(decision.action, DecisionAction::ChooseStory);
        assert_eq!(decision.deadline, 1_800);

        state.story_pending = None;
        assert!(
            expiring_decision(&state).is_none(),
            "stance cannot change a raid that lands on a defence grid"
        );
    }

    #[test]
    fn a_chosen_stance_or_a_defence_is_not_a_raid_decision() {
        let mut armed = quiet(1_000);
        warn(&mut armed, 1_200);
        armed.fleet.add(ShipKind::LightFighter, 1);
        armed.set_stance("aggressive").unwrap();
        assert!(expiring_decision(&armed).is_none());

        let mut held = quiet(1_000);
        warn(&mut held, 1_200);
        held.defences.add(DefenceKind::LightCannon, 1);
        assert!(expiring_decision(&held).is_none());
    }

    #[test]
    fn a_raid_outside_the_warning_window_is_not_a_decision() {
        let mut state = quiet(1_000);
        state.config.raid.enabled = true;
        state.config.raid.min_warning_secs = 100;
        state.next_raid_at = Some(1_000 + 101);
        assert!(state.raid_countdown_secs().is_none());
        assert!(expiring_decision(&state).is_none());
    }

    #[test]
    fn an_idle_queue_and_an_in_flight_mission_are_not_decisions() {
        let mut state = quiet(1_000);
        state.config.raid.enabled = false;
        state.next_raid_at = None;
        assert!(state.build_queue.is_empty());
        state.missions.push(Mission {
            id: 1,
            ships: Fleet::default(),
            origin: Coord::HOME,
            dest: Coord {
                galaxy: 1,
                system: 1,
                position: 4,
            },
            speed_percent: 100,
            phase: MissionPhase::Outbound,
            launched_at: 1_000,
            outbound_secs: 400,
            hold_secs: 0,
            phase_ends_at: 1_400,
            fuel_paid: 1.0,
            colonize: false,
            expedition_seed: None,
            expedition_applied: false,
            cargo: Resources::ZERO,
            transport: true,
            attack: false,
            invade: false,
            spy: false,
            site_id: None,
            recycle: false,
            deploy: false,
        });
        assert!(expiring_decision(&state).is_none());
    }

    #[test]
    fn a_live_fight_uses_its_round_clock_only_while_interactive() {
        let mut state = quiet(1_000);
        state.story_pending = Some(pending_story(1_800));
        state.pending_tactical = Some(PendingTactical {
            seed: 1,
            attacker: Fleet::default(),
            defender: Fleet::default(),
            reserves: Fleet::default(),
            atk_plan: TacticalPlan::default(),
            def_plan: TacticalPlan::default(),
            actions: Vec::new(),
            next_at: 1_100,
            queued: None,
        });
        assert_eq!(
            expiring_decision(&state).unwrap().id,
            DecisionId::Story,
            "a saved fight is not waiting on the player"
        );
        state.interactive_tactics = true;
        let decision = expiring_decision(&state).unwrap();
        assert_eq!(decision.id, DecisionId::Tactical);
        assert_eq!(decision.action, DecisionAction::QueueTacticalAction);
        assert_eq!(decision.deadline, 1_100);
        state.queue_tactical_action(RoundAction::Hold);
        assert_eq!(expiring_decision(&state).unwrap().deadline, 1_100);
    }

    #[test]
    fn story_stops_being_the_decision_at_its_timeout() {
        let mut state = quiet(1_000);
        state.story_pending = Some(pending_story(1_000));
        warn(&mut state, 1_500);
        let decision = expiring_decision(&state).expect("raid");
        assert_eq!(decision.id, DecisionId::Raid);
    }

    #[test]
    fn catch_up_follows_the_story_and_raid_clocks() {
        let mut gap = quiet(1_000);
        gap.story_pending = Some(pending_story(1_040));
        warn(&mut gap, 1_080);
        gap.fleet.add(ShipKind::LightFighter, 2);
        let mut step = gap.clone();

        gap.catch_up(1_020);
        for t in 1_001..=1_020 {
            step.catch_up(t);
        }
        let gap_decision = expiring_decision(&gap).expect("still the story");
        assert_eq!(gap_decision, expiring_decision(&step).unwrap());
        assert_eq!(
            gap_decision.deadline,
            gap.story_pending.as_ref().unwrap().timeout_at
        );
        assert!(gap_decision.deadline - gap.last_tick_unix < 40);

        gap.catch_up(1_050);
        for t in 1_021..=1_050 {
            step.catch_up(t);
        }
        assert!(gap.story_pending.is_none());
        let after = expiring_decision(&gap).expect("raid remains");
        assert_eq!(after, expiring_decision(&step).unwrap());
        assert_eq!(after.id, DecisionId::Raid);
        assert_eq!(after.action, DecisionAction::SetStance);
        assert_eq!(after.deadline, gap.next_raid_at.unwrap());
    }
}
