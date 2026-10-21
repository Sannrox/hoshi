//! Local experimentation state. Deterministic simulations remain a separate concern.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CheatState {
    pub modified: bool,
    pub enabled: bool,
    pub god_mode: bool,
    pub reveal_map: bool,
    pub time_offset: u64,
    pub audit: Vec<CheatAudit>,
}

impl CheatState {
    pub(crate) fn preserves(&self, previous: &Self) -> bool {
        (!previous.modified || self.modified) && self.audit.starts_with(&previous.audit)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheatAudit {
    pub at: u64,
    pub actor: String,
    pub action: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheatError {
    Disabled,
    Invalid,
    Forbidden,
}

impl std::fmt::Display for CheatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Disabled => "cheats are disabled",
            Self::Invalid => "invalid cheat or arguments",
            Self::Forbidden => "cheats are unavailable in fair play",
        })
    }
}

impl std::error::Error for CheatError {}

enum Command {
    Grant(crate::ResourceKind, f64),
    FinishBuild,
    FinishResearch,
    Reveal,
    God(bool),
    Skip(u64),
}

impl Command {
    fn parse(code: &str) -> Result<Self, CheatError> {
        let words: Vec<_> = code.split_whitespace().collect();
        let invalid = CheatError::Invalid;
        match words.as_slice() {
            ["grant", resource, amount] => {
                let resource = crate::ResourceKind::from_key(resource).ok_or(invalid.clone())?;
                let amount: f64 = amount.parse().map_err(|_| invalid.clone())?;
                if !amount.is_finite() || amount <= 0.0 {
                    return Err(invalid);
                }
                Ok(Self::Grant(resource, amount))
            }
            ["finish-build"] => Ok(Self::FinishBuild),
            ["finish-research"] => Ok(Self::FinishResearch),
            ["reveal"] => Ok(Self::Reveal),
            ["god", "on"] => Ok(Self::God(true)),
            ["god", "off"] => Ok(Self::God(false)),
            ["skip", seconds] => {
                let seconds = seconds.parse::<u64>().map_err(|_| invalid.clone())?;
                if seconds == 0 || seconds > 365 * 24 * 3600 {
                    return Err(invalid);
                }
                Ok(Self::Skip(seconds))
            }
            _ => Err(invalid),
        }
    }
}

impl crate::GameState {
    pub fn cheat_audit(&self) -> &[CheatAudit] {
        &self.cheats.audit
    }

    pub fn require_fair_play(&self) -> Result<(), CheatError> {
        if self.is_modified() {
            Err(CheatError::Forbidden)
        } else {
            Ok(())
        }
    }

    pub(crate) fn apply_cheat(
        &mut self,
        code: &str,
        actor: crate::Actor,
        at: u64,
    ) -> Result<(), CheatError> {
        // Check permission before parsing, including for unknown codes.
        if !self.cheats.enabled {
            return Err(CheatError::Disabled);
        }
        if actor != crate::Actor::Player {
            return Err(CheatError::Forbidden);
        }
        let command = Command::parse(code)?;
        if let Command::Grant(resource, amount) = command
            && !(self.resources.get(resource) + amount).is_finite()
        {
            return Err(CheatError::Invalid);
        }
        let at = at.max(self.last_tick_unix);
        let advance = match command {
            Command::Skip(seconds) => seconds,
            Command::FinishBuild | Command::FinishResearch => 1,
            _ => 0,
        };
        let until = at.checked_add(advance).ok_or(CheatError::Invalid)?;
        let offset = self
            .cheats
            .time_offset
            .checked_add(advance)
            .ok_or(CheatError::Invalid)?;
        self.cheats.modified = true;
        self.cheats.audit.push(CheatAudit {
            at,
            actor: actor.as_key(),
            action: "cheat".into(),
            arguments: code.into(),
        });
        self.catch_up(at);
        match command {
            Command::Grant(resource, amount) => self
                .resources
                .set(resource, self.resources.get(resource) + amount),
            Command::FinishBuild => {
                if let Some(job) = self.build_queue.first_mut() {
                    job.started_at = Some(at);
                    job.completes_at = Some(until);
                    job.work_left = 0.0;
                }
                for colony in &mut self.colonies {
                    if let Some(job) = colony.build_queue.first_mut() {
                        job.started_at = Some(at);
                        job.completes_at = Some(until);
                        job.work_left = 0.0;
                    }
                }
            }
            Command::FinishResearch => {
                if let Some(job) = &mut self.research {
                    job.completes_at = until;
                }
            }
            Command::Reveal => {
                self.cheats.reveal_map = true;
                self.frontier.revealed = self.config.frontier.sectors;
            }
            Command::God(enabled) => self.cheats.god_mode = enabled,
            Command::Skip(_) => {}
        }
        self.cheats.time_offset = offset;
        self.catch_up(until);
        Ok(())
    }

    /// Once set, this marker survives turning cheats off, prestige and saving.
    pub fn is_modified(&self) -> bool {
        self.cheats.modified
    }

    pub fn cheats_enabled(&self) -> bool {
        self.cheats.enabled
    }

    /// Convert injected wall time at the application boundary, once per operation.
    pub fn effective_time(&self, wall_time: u64) -> u64 {
        wall_time.saturating_add(self.cheats.time_offset)
    }

    pub fn set_cheats_enabled(&mut self, enabled: bool, actor: crate::Actor, at: u64) {
        if self.cheats.enabled == enabled {
            return;
        }
        self.cheats.enabled = enabled;
        self.cheats.audit.push(CheatAudit {
            at,
            actor: actor.as_key(),
            action: "enable".into(),
            arguments: enabled.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::{Actor, BalanceConfig, GameState, load_state, open_save, save_state};

    fn run(state: &mut GameState, code: &str) -> Result<(), crate::IntentError> {
        let at = state.last_tick_unix;
        let intent = crate::Intent::Cheat {
            code: code.into(),
            actor: Actor::Player,
        };
        let proposal = crate::propose(state, &intent, at)?;
        crate::apply(state, &proposal, at)
    }

    #[test]
    fn god_mode_preserves_fleet_and_planet_across_raid_layers() {
        for layered in [false, true] {
            let mut state = GameState::with_config(100, BalanceConfig::default());
            state.fleet.add(crate::ShipKind::LightFighter, 1);
            if layered {
                state.defences.add(crate::DefenceKind::Garrison, 1);
            }
            state.set_cheats_enabled(true, Actor::Player, 100);
            run(&mut state, "god on").unwrap();
            let fleet = state.fleet;
            let defences = state.defences;
            let resources = state.resources;
            let report = state.trigger_raid(100);
            assert_eq!(state.fleet, fleet);
            assert_eq!(state.defences, defences);
            assert_eq!(state.resources, resources);
            assert_eq!(report.defender_after, report.defender_before);
            assert_ne!(report.winner, crate::CombatWinner::Attacker);
        }
    }

    #[test]
    fn finishing_build_uses_completion_path_and_blocks_achievements() {
        let mut state = GameState::with_config(100, BalanceConfig::default());
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        state
            .enqueue_build(crate::BuildingKind::MetalMine, 100)
            .unwrap();
        let before = state.buildings.get(crate::BuildingKind::MetalMine);
        state.set_cheats_enabled(true, Actor::Player, 100);
        run(&mut state, "finish-build").unwrap();
        assert!(state.build_queue.is_empty());
        assert_eq!(
            state.buildings.get(crate::BuildingKind::MetalMine),
            before + 1
        );
        assert!(state.achievements.is_empty());
        assert!(
            state
                .history
                .events
                .iter()
                .any(|event| event.action == "complete")
        );
    }

    #[test]
    fn disabled_console_does_not_reveal_codes_or_mutate_state() {
        let mut state = GameState::with_config(100, BalanceConfig::default());
        let before = state.clone();
        let known = run(&mut state, "grant iron 100").unwrap_err();
        let unknown = run(&mut state, "unknown-code").unwrap_err();
        assert_eq!(known, unknown);
        assert_eq!(state, before);
    }

    #[test]
    fn skip_uses_normal_catch_up_and_remains_modified_after_disable() {
        let mut state = GameState::with_config(100, BalanceConfig::default());
        state.set_cheats_enabled(true, Actor::Player, 100);
        let mut ordinary = state.clone();
        ordinary.cheats.modified = true;
        ordinary.catch_up(160);
        run(&mut state, "skip 60").unwrap();
        assert_eq!(state.resources, ordinary.resources);
        assert_eq!(state.events, ordinary.events);
        assert_eq!(state.last_tick_unix, 160);
        assert_eq!(state.effective_time(101), 161);
        state.set_cheats_enabled(false, Actor::Player, 160);
        assert!(state.require_fair_play().is_err());
        assert!(state.is_modified());
        assert_eq!(state.cheat_audit()[1].arguments, "skip 60");
    }

    #[test]
    fn refused_arguments_do_not_mark_or_audit_a_save() {
        let mut state = GameState::with_config(100, BalanceConfig::default());
        state.set_cheats_enabled(true, Actor::Player, 100);
        let before = state.clone();
        for code in [
            "grant iron NaN",
            "grant iron -1",
            "skip 0",
            "skip 18446744073709551615",
            "god maybe",
        ] {
            assert!(run(&mut state, code).is_err());
            assert_eq!(state, before);
        }
    }

    // Independent SQLite connections ensure this test has no process-global files.
    #[test]
    fn save_round_trip_preserves_modified_and_refuses_stale_audit() {
        let mut conn = open_save(std::path::Path::new(":memory:")).unwrap();
        let mut state = GameState::with_config(100, BalanceConfig::default());
        save_state(&mut conn, &state).unwrap();
        let stale = state.clone();
        state.set_cheats_enabled(true, Actor::Player, 100);
        state.cheats.modified = true;
        state.cheats.time_offset = 60;
        state.set_cheats_enabled(false, Actor::Player, 101);
        save_state(&mut conn, &state).unwrap();
        assert!(save_state(&mut conn, &stale).is_err());
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert!(loaded.is_modified());
        assert!(!loaded.cheats_enabled());
        assert_eq!(loaded.effective_time(102), 162);
        assert_eq!(loaded.cheats.audit, state.cheats.audit);
    }

    #[test]
    fn enabling_alone_keeps_save_unmodified_and_records_actor() {
        let mut state = GameState::with_config(100, BalanceConfig::default());
        assert!(!state.cheats_enabled());
        state.set_cheats_enabled(true, Actor::Player, 100);
        assert!(!state.is_modified());
        assert_eq!(state.cheats.audit[0].actor, "player");
        assert_eq!(state.cheats.audit[0].at, 100);
    }
}
