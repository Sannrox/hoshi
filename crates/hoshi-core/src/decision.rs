//! One choosable deadline, derived from the save. Not stored.

use crate::diplomacy::FactionSpec;
use crate::ships::ShipKind;
use crate::state::GameState;
use crate::victory::VictoryPath;
use crate::{Formation, Stance};

/// Which clock the decision reads. Lower rank wins a deadline tie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionId {
    Story,
    Tactical,
    Raid,
    /// Another tenant's raid flying at a shard home. Local play never has one.
    Inbound,
    /// A diplomacy tick that would leave a standing tier. Only when focus is
    /// diplomacy; a story, fight, or warned raid still wins an earlier or tied
    /// deadline.
    Diplomacy,
}

impl DecisionId {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Story => "story",
            Self::Tactical => "tactical",
            Self::Raid => "raid",
            Self::Inbound => "inbound",
            Self::Diplomacy => "diplomacy",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Story => 0,
            Self::Tactical => 1,
            Self::Raid => 2,
            Self::Inbound => 3,
            Self::Diplomacy => 4,
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
    Trade,
    Tribute,
}

impl DecisionAction {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::ChooseStory => "choose_story",
            Self::QueueTacticalAction => "queue_tactical_action",
            Self::SetStance => "set_stance",
            Self::ArmHome => "arm",
            Self::Trade => "trade",
            Self::Tribute => "tribute",
        }
    }
}

/// Faction and the tier its next crossing would leave. Present when `id` is
/// diplomacy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiplomacyDeadline {
    pub faction_id: String,
    pub faction_name: String,
    pub tier_name: String,
}

/// The earliest choice still open. Deadlines are the story, fight, raid, or
/// (with diplomacy focus) the next standing-tier crossing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiringDecision {
    pub id: DecisionId,
    pub deadline: u64,
    pub action: DecisionAction,
    pub diplomacy: Option<DiplomacyDeadline>,
}

impl ExpiringDecision {
    fn new(id: DecisionId, deadline: u64, action: DecisionAction) -> Self {
        Self {
            id,
            deadline,
            action,
            diplomacy: None,
        }
    }
}

/// At most one decision. The earliest deadline wins. A tie keeps story, then
/// a live fight, then the raid, then an inbound, then a diplomacy crossing. An
/// idle queue and an in-flight recall are not decisions. Wonder and conquest
/// focus do not add a countdown.
pub fn expiring_decision(
    state: &GameState,
    focus: Option<VictoryPath>,
) -> Option<ExpiringDecision> {
    let mut best: Option<ExpiringDecision> = None;
    if let Some(pending) = &state.story_pending
        && state.last_tick_unix < pending.timeout_at
    {
        consider(
            &mut best,
            ExpiringDecision::new(
                DecisionId::Story,
                pending.timeout_at,
                DecisionAction::ChooseStory,
            ),
        );
    }
    if state.interactive_tactics
        && let Some(pending) = &state.pending_tactical
        && state.last_tick_unix < pending.next_at
    {
        consider(
            &mut best,
            ExpiringDecision::new(
                DecisionId::Tactical,
                pending.next_at,
                DecisionAction::QueueTacticalAction,
            ),
        );
    }
    if let Some(raid) = raid_decision(state) {
        consider(&mut best, raid);
    }
    if focus == Some(VictoryPath::Diplomacy)
        && let Some(row) = diplomacy_decision(state)
    {
        consider(&mut best, row);
    }
    best
}

/// [`expiring_decision`] for a shard tenant whose home another tenant's
/// raid reaches at `inbound_at`, the earliest such arrival.
///
/// The inbound is one more candidate, so an earlier story, fight, or pirate
/// warning still wins and ties go to them. Its action is always arming the
/// home: the shard fight reads the parked home fleet at arrival and ignores
/// stance, formation, and defences.
pub(crate) fn expiring_decision_with_inbound(
    state: &GameState,
    inbound_at: Option<u64>,
) -> Option<ExpiringDecision> {
    let mut best = expiring_decision(state, None);
    if let Some(deadline) = inbound_at.filter(|&at| state.last_tick_unix < at) {
        consider(
            &mut best,
            ExpiringDecision::new(DecisionId::Inbound, deadline, DecisionAction::ArmHome),
        );
    }
    best
}

fn consider(best: &mut Option<ExpiringDecision>, next: ExpiringDecision) {
    let replace = match best.as_ref() {
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
    let lands = state.upcoming_raid_at()?;
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
    Some(ExpiringDecision::new(DecisionId::Raid, lands, action))
}

fn diplomacy_decision(state: &GameState) -> Option<ExpiringDecision> {
    if !state.config.diplomacy.enabled {
        return None;
    }
    let first = state.next_diplomacy_at()?;
    let tick = state.config.diplomacy.tick_secs;
    let decay = state.config.diplomacy.decay;
    let mut best: Option<ExpiringDecision> = None;
    for spec in state.config.diplomacy.iter() {
        if let Some(row) = crossing_at(state, spec, first, tick, decay) {
            consider(&mut best, row);
        }
    }
    best
}

fn crossing_at(
    state: &GameState,
    spec: &FactionSpec,
    first: u64,
    tick: u64,
    decay: f64,
) -> Option<ExpiringDecision> {
    let mut standing = state.diplomacy.standing(&spec.id);
    let current = spec.tier(standing)?;
    let leaving_id = current.id.clone();
    let leaving_name = current.name.clone();
    let mut at = first;
    for _ in 0..10_000 {
        if at <= state.last_tick_unix {
            return None;
        }
        let next = clamp_standing(state, standing + decay * (0.0 - standing));
        let next_id = spec.tier(next).map(|tier| tier.id.as_str());
        if next_id != Some(leaving_id.as_str()) {
            return Some(ExpiringDecision {
                id: DecisionId::Diplomacy,
                deadline: at,
                action: standing_action(spec),
                diplomacy: Some(DiplomacyDeadline {
                    faction_id: spec.id.clone(),
                    faction_name: spec.name.clone(),
                    tier_name: leaving_name,
                }),
            });
        }
        if tick == 0 || (next - standing).abs() < 1e-15 {
            return None;
        }
        standing = next;
        let next_at = at.saturating_add(tick);
        if next_at <= at {
            return None;
        }
        at = next_at;
    }
    None
}

fn standing_action(spec: &FactionSpec) -> DecisionAction {
    if spec.trade_delta.abs() > 1e-12 {
        DecisionAction::Trade
    } else if spec.tribute_delta.abs() > 1e-12 && spec.tribute_metal > 0.0 {
        DecisionAction::Tribute
    } else {
        DecisionAction::ChooseStory
    }
}

fn clamp_standing(state: &GameState, value: f64) -> f64 {
    value
        .max(state.config.diplomacy.min_standing)
        .min(state.config.diplomacy.max_standing)
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
        let decision = expiring_decision(&state, None).expect("story");
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
        let decision = expiring_decision(&state, None).expect("story");
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
        let decision = expiring_decision(&state, None).expect("raid");
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
        let decision = expiring_decision(&state, None).expect("arm");
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
        let decision = expiring_decision(&state, None).expect("story");
        assert_eq!(decision.id, DecisionId::Story);
        assert_eq!(decision.action, DecisionAction::ChooseStory);
        assert_eq!(decision.deadline, 1_800);

        state.story_pending = None;
        assert!(
            expiring_decision(&state, None).is_none(),
            "stance cannot change a raid that lands on a defence grid"
        );
    }

    #[test]
    fn a_chosen_stance_or_a_defence_is_not_a_raid_decision() {
        let mut armed = quiet(1_000);
        warn(&mut armed, 1_200);
        armed.fleet.add(ShipKind::LightFighter, 1);
        armed.set_stance("aggressive").unwrap();
        assert!(expiring_decision(&armed, None).is_none());

        let mut held = quiet(1_000);
        warn(&mut held, 1_200);
        held.defences.add(DefenceKind::LightCannon, 1);
        assert!(expiring_decision(&held, None).is_none());
    }

    #[test]
    fn a_raid_outside_the_warning_window_is_not_a_decision() {
        let mut state = quiet(1_000);
        state.config.raid.enabled = true;
        state.config.raid.min_warning_secs = 100;
        state.next_raid_at = Some(1_000 + 101);
        assert!(state.raid_countdown_secs().is_none());
        assert!(expiring_decision(&state, None).is_none());
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
            survey: false,
            site_id: None,
            recycle: false,
            deploy: false,
            station: false,
        });
        assert!(expiring_decision(&state, None).is_none());
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
            expiring_decision(&state, None).unwrap().id,
            DecisionId::Story,
            "a saved fight is not waiting on the player"
        );
        state.interactive_tactics = true;
        let decision = expiring_decision(&state, None).unwrap();
        assert_eq!(decision.id, DecisionId::Tactical);
        assert_eq!(decision.action, DecisionAction::QueueTacticalAction);
        assert_eq!(decision.deadline, 1_100);
        state.queue_tactical_action(RoundAction::Hold);
        assert_eq!(expiring_decision(&state, None).unwrap().deadline, 1_100);
    }

    #[test]
    fn story_stops_being_the_decision_at_its_timeout() {
        let mut state = quiet(1_000);
        state.story_pending = Some(pending_story(1_000));
        warn(&mut state, 1_500);
        let decision = expiring_decision(&state, None).expect("raid");
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
        let gap_decision = expiring_decision(&gap, None).expect("still the story");
        assert_eq!(gap_decision, expiring_decision(&step, None).unwrap());
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
        let after = expiring_decision(&gap, None).expect("raid remains");
        assert_eq!(after, expiring_decision(&step, None).unwrap());
        assert_eq!(after.id, DecisionId::Raid);
        assert_eq!(after.action, DecisionAction::SetStance);
        assert_eq!(after.deadline, gap.next_raid_at.unwrap());
    }

    fn diplomacy_live(now: u64) -> GameState {
        let mut state = GameState::new(now);
        state.config.story = crate::StoryCatalog::empty();
        state.config.market.enabled = false;
        state.config.raid.enabled = false;
        state.next_raid_at = None;
        state.story_pending = None;
        state.config.diplomacy = crate::DiplomacyConfig::default();
        state.config.diplomacy.tick_secs = 10;
        state.last_diplomacy_at = now;
        state
    }

    #[test]
    fn diplomacy_focus_names_a_tier_that_crosses_on_the_next_tick() {
        let mut state = diplomacy_live(1_000);
        state.diplomacy.set_standing("traders", 20.0);
        assert!(expiring_decision(&state, None).is_none());
        assert!(expiring_decision(&state, Some(VictoryPath::Conquest)).is_none());
        assert!(expiring_decision(&state, Some(VictoryPath::Wonder)).is_none());
        let decision = expiring_decision(&state, Some(VictoryPath::Diplomacy)).expect("crossing");
        assert_eq!(decision.id, DecisionId::Diplomacy);
        assert_eq!(decision.action, DecisionAction::Trade);
        assert_eq!(decision.deadline, 1_010);
        let row = decision.diplomacy.expect("faction");
        assert_eq!(row.faction_id, "traders");
        assert_eq!(row.faction_name, "Traders' guild");
        assert_eq!(row.tier_name, "Favored");
    }

    #[test]
    fn a_standing_inside_its_tier_is_not_a_decision() {
        let mut state = diplomacy_live(1_000);
        state.diplomacy.set_standing("traders", 5.0);
        assert!(expiring_decision(&state, Some(VictoryPath::Diplomacy)).is_none());
    }

    #[test]
    fn a_sooner_story_beats_a_diplomacy_crossing() {
        let mut state = diplomacy_live(1_000);
        state.diplomacy.set_standing("traders", 20.0);
        state.story_pending = Some(pending_story(1_005));
        let decision = expiring_decision(&state, Some(VictoryPath::Diplomacy)).expect("story");
        assert_eq!(decision.id, DecisionId::Story);
        assert_eq!(decision.deadline, 1_005);
    }

    #[test]
    fn catch_up_and_a_stepped_tick_agree_on_the_crossing() {
        let mut gap = diplomacy_live(1_000);
        gap.diplomacy.set_standing("traders", 30.0);
        let mut step = gap.clone();
        let focus = Some(VictoryPath::Diplomacy);
        let before = expiring_decision(&gap, focus).expect("crossing");
        assert_eq!(before.id, DecisionId::Diplomacy);
        assert!(before.deadline > 1_010, "{}", before.deadline);
        assert_eq!(before.diplomacy.as_ref().unwrap().tier_name, "Favored");

        let just_before = before.deadline - 1;
        gap.catch_up(just_before);
        for t in 1_001..=just_before {
            step.catch_up(t);
        }
        let gap_decision = expiring_decision(&gap, focus).expect("still crossing");
        assert_eq!(gap_decision, expiring_decision(&step, focus).unwrap());
        assert_eq!(gap_decision.deadline, before.deadline);
        assert_eq!(
            gap.config
                .diplomacy
                .get("traders")
                .unwrap()
                .tier(gap.diplomacy.standing("traders"))
                .unwrap()
                .id,
            "favored"
        );

        gap.catch_up(before.deadline);
        for t in just_before + 1..=before.deadline {
            step.catch_up(t);
        }
        assert_eq!(
            expiring_decision(&gap, focus),
            expiring_decision(&step, focus)
        );
        assert_ne!(
            gap.config
                .diplomacy
                .get("traders")
                .unwrap()
                .tier(gap.diplomacy.standing("traders"))
                .unwrap()
                .id,
            "favored"
        );
        assert!(expiring_decision(&gap, focus).is_none());
    }
}
