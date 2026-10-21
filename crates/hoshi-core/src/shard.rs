//! Authoritative gate for a Chisei-hosted shard. No network.
//!
//! Local play never goes through this module. A shard host owns the clock,
//! the MAC key, and per-identity command windows, then calls the same
//! `propose` / `apply` path as the CLI.

use crate::activity::{ActivityApply, ActivityPulse};
use crate::clock::Clock;
use crate::combat::{
    CombatReport, CombatWinner, plunder_from, rapid_fire_table, resolve_combat, stats_from_catalog,
};
use crate::intent::{Intent, IntentError, apply, propose};
use crate::ships::Fleet;
use crate::state::{GameState, Resources};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

type HmacSha256 = Hmac<Sha256>;

/// Commands admitted per identity inside `command_window_secs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardConfig {
    pub command_window_secs: u64,
    pub command_limit: u32,
    /// Defender run younger than this cannot be raided.
    pub newbie_secs: u64,
    pub attack_window_secs: u64,
    pub attack_limit: u32,
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            command_window_secs: 60,
            command_limit: 30,
            newbie_secs: 24 * 3600,
            attack_window_secs: 24 * 3600,
            attack_limit: 3,
        }
    }
}

/// One player intent submitted to a shard. `client_now_unix` is ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardCommand {
    pub identity: String,
    pub idempotency_id: String,
    pub intent: Intent,
    pub client_now_unix: Option<u64>,
}

/// Bridge pulse plus HMAC-SHA256 over its canonical bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedBridgeEvent {
    pub pulse: ActivityPulse,
    pub mac: [u8; 32],
}

/// Snapshot raid: attacker and defender after catch-up at the host clock.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardRaid {
    pub attacker_id: String,
    pub defender_id: String,
    pub idempotency_id: String,
    pub client_now_unix: Option<u64>,
}

/// Combat report plus plunder moved from defender to attacker.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardRaidResult {
    pub report: CombatReport,
    pub plunder: Resources,
}

/// Why a shard refused a command, a bridge pulse, or a raid.
#[derive(Debug, Clone, PartialEq)]
pub enum ShardError {
    EmptyIdentity,
    EmptyIdempotency,
    RateLimited {
        identity: String,
        retry_after_secs: u64,
    },
    ForgedBridge,
    Intent(IntentError),
    SameEmpire,
    NewbieProtected {
        identity: String,
    },
    AttackCapped {
        identity: String,
        retry_after_secs: u64,
    },
}

impl fmt::Display for ShardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShardError::EmptyIdentity => write!(f, "shard identity is empty"),
            ShardError::EmptyIdempotency => write!(f, "shard idempotency id is empty"),
            ShardError::RateLimited {
                identity,
                retry_after_secs,
            } => write!(
                f,
                "rate limited {identity}, retry after {retry_after_secs}s"
            ),
            ShardError::ForgedBridge => write!(f, "forged bridge event"),
            ShardError::Intent(err) => write!(f, "{err}"),
            ShardError::SameEmpire => write!(f, "cannot raid your own empire"),
            ShardError::NewbieProtected { identity } => {
                write!(f, "{identity} is under newbie protection")
            }
            ShardError::AttackCapped {
                identity,
                retry_after_secs,
            } => write!(
                f,
                "attack cap for {identity}, retry after {retry_after_secs}s"
            ),
        }
    }
}

impl std::error::Error for ShardError {}

/// Live shard session. The clock and MAC key stay on the host.
pub struct ShardSession<C> {
    clock: C,
    config: ShardConfig,
    mac_key: [u8; 32],
    windows: BTreeMap<String, Vec<u64>>,
    seen: BTreeSet<(String, String)>,
    attacks: BTreeMap<String, Vec<u64>>,
}

impl<C: Clock> ShardSession<C> {
    pub fn new(clock: C, mac_key: [u8; 32], config: ShardConfig) -> Self {
        Self {
            clock,
            config,
            mac_key,
            windows: BTreeMap::new(),
            seen: BTreeSet::new(),
            attacks: BTreeMap::new(),
        }
    }

    pub fn now_unix(&self) -> u64 {
        self.clock.now_unix()
    }

    /// Sign a pulse the host itself ingested. Clients never hold the key.
    pub fn sign_bridge(&self, pulse: ActivityPulse) -> SignedBridgeEvent {
        SignedBridgeEvent {
            mac: mac_pulse(&self.mac_key, pulse),
            pulse,
        }
    }

    /// Catch up and apply `cmd.intent` at the shard clock.
    ///
    /// `cmd.client_now_unix` is accepted so a caller can send a wall clock
    /// and is never read.
    pub fn submit(&mut self, state: &mut GameState, cmd: ShardCommand) -> Result<(), ShardError> {
        state
            .require_fair_play()
            .map_err(|e| ShardError::Intent(crate::IntentError::Cheat(e)))?;
        if matches!(cmd.intent, crate::Intent::Cheat { .. }) {
            return Err(ShardError::Intent(crate::IntentError::Cheat(
                crate::CheatError::Forbidden,
            )));
        }
        if cmd.identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if cmd.idempotency_id.is_empty() {
            return Err(ShardError::EmptyIdempotency);
        }
        let _ = cmd.client_now_unix;
        let seen_key = (cmd.identity.clone(), cmd.idempotency_id.clone());
        if self.seen.contains(&seen_key) {
            return Ok(());
        }
        let now = self.clock.now_unix();
        self.admit(&cmd.identity, now)?;
        state.catch_up(now);
        let proposal = propose(state, &cmd.intent, now).map_err(ShardError::Intent)?;
        apply(state, &proposal, now).map_err(ShardError::Intent)?;
        self.seen.insert(seen_key);
        Ok(())
    }

    /// Verify the MAC, then apply the pulse at the shard clock.
    pub fn apply_bridge(
        &mut self,
        state: &mut GameState,
        identity: &str,
        event: &SignedBridgeEvent,
    ) -> Result<ActivityApply, ShardError> {
        state
            .require_fair_play()
            .map_err(|e| ShardError::Intent(crate::IntentError::Cheat(e)))?;
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if !verify_pulse(&self.mac_key, event.pulse, &event.mac) {
            return Err(ShardError::ForgedBridge);
        }
        let now = self.clock.now_unix();
        self.admit(identity, now)?;
        Ok(state.apply_activity(event.pulse, now))
    }

    /// Catch up both empires on the host clock, then resolve a snapshot raid.
    pub fn raid(
        &mut self,
        attacker: &mut GameState,
        defender: &mut GameState,
        cmd: ShardRaid,
    ) -> Result<ShardRaidResult, ShardError> {
        attacker
            .require_fair_play()
            .and_then(|()| defender.require_fair_play())
            .map_err(|e| ShardError::Intent(crate::IntentError::Cheat(e)))?;
        if cmd.attacker_id.is_empty() || cmd.defender_id.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if cmd.idempotency_id.is_empty() {
            return Err(ShardError::EmptyIdempotency);
        }
        if cmd.attacker_id == cmd.defender_id {
            return Err(ShardError::SameEmpire);
        }
        let _ = cmd.client_now_unix;
        let seen_key = (cmd.attacker_id.clone(), cmd.idempotency_id.clone());
        if self.seen.contains(&seen_key) {
            return Ok(ShardRaidResult {
                report: empty_report(attacker.fleet, defender.fleet),
                plunder: Resources::ZERO,
            });
        }
        let now = self.clock.now_unix();
        attacker.catch_up(now);
        defender.catch_up(now);
        if now.saturating_sub(defender.prestige.current.started_at) < self.config.newbie_secs {
            return Err(ShardError::NewbieProtected {
                identity: cmd.defender_id,
            });
        }
        self.admit(&cmd.attacker_id, now)?;
        self.admit_attack(&cmd.attacker_id, now)?;
        let mut rng = attacker.rng_seed;
        let report = resolve_combat(
            attacker.fleet,
            defender.fleet,
            &stats_from_catalog(&attacker.config.ships),
            &stats_from_catalog(&defender.config.ships),
            attacker.config.raid.max_rounds,
            &rapid_fire_table(&attacker.config.ships),
            &mut rng,
        );
        attacker.rng_seed = rng;
        attacker.fleet = report.attacker_after;
        defender.fleet = report.defender_after;
        let plunder = if report.winner == CombatWinner::Attacker {
            plunder_from(
                defender.resources,
                defender.config.raid.plunder_share,
                defender.config.raid.plunder_cap,
            )
        } else {
            Resources::ZERO
        };
        defender.resources = defender.resources.saturating_sub(plunder);
        attacker.resources = attacker.resources.saturating_add(plunder);
        self.seen.insert(seen_key);
        Ok(ShardRaidResult { report, plunder })
    }

    fn admit_attack(&mut self, identity: &str, now: u64) -> Result<(), ShardError> {
        let window = self.config.attack_window_secs.max(1);
        let limit = self.config.attack_limit;
        if limit == 0 {
            return Err(ShardError::AttackCapped {
                identity: identity.to_string(),
                retry_after_secs: window,
            });
        }
        let stamps = self.attacks.entry(identity.to_string()).or_default();
        let start = now.saturating_sub(window);
        stamps.retain(|&t| t > start);
        if stamps.len() as u32 >= limit {
            let oldest = stamps.first().copied().unwrap_or(now);
            let retry_after_secs = oldest.saturating_add(window).saturating_sub(now).max(1);
            return Err(ShardError::AttackCapped {
                identity: identity.to_string(),
                retry_after_secs,
            });
        }
        stamps.push(now);
        Ok(())
    }

    fn admit(&mut self, identity: &str, now: u64) -> Result<(), ShardError> {
        let window = self.config.command_window_secs.max(1);
        let limit = self.config.command_limit;
        if limit == 0 {
            return Err(ShardError::RateLimited {
                identity: identity.to_string(),
                retry_after_secs: window,
            });
        }
        let stamps = self.windows.entry(identity.to_string()).or_default();
        let start = now.saturating_sub(window);
        stamps.retain(|&t| t > start);
        if stamps.len() as u32 >= limit {
            let oldest = stamps.first().copied().unwrap_or(now);
            let retry_after_secs = oldest.saturating_add(window).saturating_sub(now).max(1);
            return Err(ShardError::RateLimited {
                identity: identity.to_string(),
                retry_after_secs,
            });
        }
        stamps.push(now);
        Ok(())
    }
}

fn empty_report(attacker: Fleet, defender: Fleet) -> CombatReport {
    CombatReport {
        rounds: 0,
        attacker_before: attacker,
        defender_before: defender,
        attacker_after: attacker,
        defender_after: defender,
        winner: CombatWinner::Draw,
        ground: None,
        seed: 0,
        frames: Vec::new(),
        layered: false,
    }
}

fn canonical_pulse(pulse: ActivityPulse) -> Vec<u8> {
    let mut out = Vec::new();
    match pulse {
        ActivityPulse::PrMerged { ts } => {
            out.extend(b"pr_merged\n");
            out.extend(ts.to_le_bytes());
        }
        ActivityPulse::WorkflowFinished { ts, success } => {
            out.extend(b"workflow_finished\n");
            out.extend(ts.to_le_bytes());
            out.push(u8::from(success));
        }
        ActivityPulse::AgentFinished { ts, duration_secs } => {
            out.extend(b"agent_finished\n");
            out.extend(ts.to_le_bytes());
            out.extend(duration_secs.to_le_bytes());
        }
        ActivityPulse::AgentWaiting { ts, waiting_secs } => {
            out.extend(b"agent_waiting\n");
            out.extend(ts.to_le_bytes());
            out.extend(waiting_secs.to_le_bytes());
        }
    }
    out
}

fn mac_pulse(key: &[u8; 32], pulse: ActivityPulse) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a 32-byte key");
    mac.update(&canonical_pulse(pulse));
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

fn verify_pulse(key: &[u8; 32], pulse: ActivityPulse, tag: &[u8; 32]) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(key) else {
        return false;
    };
    mac.update(&canonical_pulse(pulse));
    mac.verify_slice(tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::buildings::BuildingKind;
    use crate::clock::FakeClock;
    use crate::combat::{plunder_from, rapid_fire_table, resolve_combat, stats_from_catalog};
    use crate::fleets::Coord;
    use crate::leaders::Actor;
    use crate::ships::{Fleet, ShipKind};
    use crate::state::GameState;

    const KEY: [u8; 32] = [0x11; 32];

    fn test_state(now: u64) -> GameState {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        GameState::with_config(now, cfg)
    }

    fn session(now: u64) -> ShardSession<FakeClock> {
        ShardSession::new(FakeClock::new(now), KEY, ShardConfig::default())
    }

    fn tight_session(now: u64, limit: u32) -> ShardSession<FakeClock> {
        ShardSession::new(
            FakeClock::new(now),
            KEY,
            ShardConfig {
                command_window_secs: 60,
                command_limit: limit,
                ..ShardConfig::default()
            },
        )
    }

    fn build_cmd(identity: &str, id: &str, client_now: Option<u64>) -> ShardCommand {
        ShardCommand {
            identity: identity.into(),
            idempotency_id: id.into(),
            intent: Intent::Build {
                planet: Coord::HOME,
                building: BuildingKind::MetalMine,
                actor: Actor::Player,
            },
            client_now_unix: client_now,
        }
    }

    #[test]
    fn modified_save_and_cheat_intent_are_refused_before_shared_mutation() {
        let mut session = session(100);
        let mut state = test_state(100);
        let mut command = build_cmd("player", "cheat", None);
        command.intent = crate::Intent::Cheat {
            code: "grant iron 100".into(),
            actor: crate::Actor::Player,
        };
        let before = state.clone();
        assert!(session.submit(&mut state, command).is_err());
        assert_eq!(state, before);
        state.set_cheats_enabled(true, crate::Actor::Player, 100);
        let intent = crate::Intent::Cheat {
            code: "grant iron 100".into(),
            actor: crate::Actor::Player,
        };
        let proposal = crate::propose(&state, &intent, 100).unwrap();
        crate::apply(&mut state, &proposal, 100).unwrap();
        state.set_cheats_enabled(false, crate::Actor::Player, 100);
        let before = state.clone();
        assert!(
            session
                .submit(&mut state, build_cmd("player", "build", None))
                .is_err()
        );
        assert_eq!(state, before);
    }

    #[test]
    fn client_clock_does_not_move_shard_state() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        let iron = state.resources.iron;
        shard
            .submit(
                &mut state,
                build_cmd("tenant-a", "cmd-1", Some(1_000 + 3_600)),
            )
            .unwrap();
        assert_eq!(state.last_tick_unix, 1_000);
        assert!(state.resources.iron < iron);
        assert_eq!(state.build_queue.len(), 1);
    }

    #[test]
    fn client_clock_skew_does_not_speed_production() {
        let mut honest = session(1_000);
        let mut skewed = session(1_000);
        let mut a = test_state(1_000);
        let mut b = test_state(1_000);
        honest
            .submit(&mut a, build_cmd("tenant-a", "h", Some(1_000)))
            .unwrap();
        skewed
            .submit(&mut b, build_cmd("tenant-a", "s", Some(1_000 + 86_400)))
            .unwrap();
        assert_eq!(a.last_tick_unix, b.last_tick_unix);
        assert!((a.resources.iron - b.resources.iron).abs() < 1e-9);
    }

    #[test]
    fn shard_clock_catch_up_is_the_only_time_source() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        shard.clock.advance(3_600);
        shard
            .submit(&mut state, build_cmd("tenant-a", "later", Some(0)))
            .unwrap();
        let mut baseline = test_state(1_000);
        baseline.catch_up(4_600);
        assert_eq!(state.last_tick_unix, 4_600);
        assert_eq!(baseline.last_tick_unix, 4_600);
        assert!(state.resources.iron < baseline.resources.iron);
    }

    #[test]
    fn forged_bridge_event_is_rejected() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        let iron = state.resources.iron;
        let pulse = ActivityPulse::PrMerged { ts: 1_000 };
        let mut event = shard.sign_bridge(pulse);
        event.mac[0] ^= 1;
        let err = shard
            .apply_bridge(&mut state, "tenant-a", &event)
            .unwrap_err();
        assert_eq!(err, ShardError::ForgedBridge);
        assert!((state.resources.iron - iron).abs() < 1e-9);
        let unsigned = SignedBridgeEvent {
            pulse,
            mac: [0; 32],
        };
        assert_eq!(
            shard
                .apply_bridge(&mut state, "tenant-a", &unsigned)
                .unwrap_err(),
            ShardError::ForgedBridge
        );
    }

    #[test]
    fn signed_bridge_event_is_applied() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        let iron = state.resources.iron;
        let event = shard.sign_bridge(ActivityPulse::PrMerged { ts: 1_000 });
        let result = shard.apply_bridge(&mut state, "tenant-a", &event).unwrap();
        assert_eq!(result, ActivityApply::Applied);
        assert!(state.resources.iron > iron);
    }

    #[test]
    fn mutated_pulse_fails_the_mac() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        let mut event = shard.sign_bridge(ActivityPulse::PrMerged { ts: 1_000 });
        event.pulse = ActivityPulse::PrMerged { ts: 2_000 };
        assert_eq!(
            shard
                .apply_bridge(&mut state, "tenant-a", &event)
                .unwrap_err(),
            ShardError::ForgedBridge
        );
    }

    #[test]
    fn rate_limit_rejects_the_next_command() {
        let mut shard = tight_session(1_000, 2);
        let mut state = test_state(1_000);
        shard
            .submit(&mut state, build_cmd("tenant-a", "a", None))
            .unwrap();
        shard
            .submit(&mut state, build_cmd("tenant-a", "b", None))
            .unwrap();
        let err = shard
            .submit(&mut state, build_cmd("tenant-a", "c", None))
            .unwrap_err();
        assert!(matches!(
            err,
            ShardError::RateLimited { identity, .. } if identity == "tenant-a"
        ));
        assert_eq!(state.build_queue.len(), 2);
    }

    #[test]
    fn rate_limit_is_per_identity() {
        let mut shard = tight_session(1_000, 1);
        let mut state = test_state(1_000);
        state.resources.iron = 40_000.0;
        state.resources.silicon = 20_000.0;
        shard
            .submit(&mut state, build_cmd("tenant-a", "a", None))
            .unwrap();
        shard
            .submit(&mut state, build_cmd("tenant-b", "b", None))
            .unwrap();
        assert_eq!(state.build_queue.len(), 2);
    }

    #[test]
    fn dismantle_matches_direct_intent_and_retries_once() {
        let mut state = test_state(1_000);
        let mut direct = state.clone();
        let intent = Intent::Dismantle {
            planet: Coord::HOME,
            building: BuildingKind::MetalMine,
            actor: Actor::Player,
        };
        let proposal = crate::propose(&direct, &intent, 1_000).unwrap();
        crate::apply(&mut direct, &proposal, 1_000).unwrap();
        let mut shard = session(1_000);
        let command = ShardCommand {
            identity: "tenant-a".into(),
            idempotency_id: "remove-level".into(),
            intent,
            client_now_unix: None,
        };
        shard.submit(&mut state, command.clone()).unwrap();
        shard.submit(&mut state, command).unwrap();
        assert_eq!(state.build_queue, direct.build_queue);
        assert_eq!(state.resources, direct.resources);
        assert_eq!(state.buildings, direct.buildings);
    }

    #[test]
    fn duplicate_idempotency_id_does_not_apply_twice() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        shard
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        let iron = state.resources.iron;
        shard
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        assert_eq!(state.build_queue.len(), 1);
        assert!((state.resources.iron - iron).abs() < 1e-9);
    }

    #[test]
    fn idempotency_ids_are_per_identity() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        state.resources.iron = 40_000.0;
        state.resources.silicon = 20_000.0;
        shard
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        shard
            .submit(&mut state, build_cmd("tenant-b", "same", None))
            .unwrap();
        assert_eq!(state.build_queue.len(), 2);
    }

    fn raid_session(now: u64, newbie_secs: u64, attack_limit: u32) -> ShardSession<FakeClock> {
        ShardSession::new(
            FakeClock::new(now),
            KEY,
            ShardConfig {
                newbie_secs,
                attack_limit,
                ..ShardConfig::default()
            },
        )
    }

    fn armed(now: u64, fighters: u32) -> GameState {
        let mut state = test_state(now);
        state.fleet = Fleet::default();
        state.fleet.add(ShipKind::LightFighter, fighters);
        state.resources.iron = 1_000.0;
        state
    }

    fn raid_cmd(attacker: &str, defender: &str, id: &str) -> ShardRaid {
        ShardRaid {
            attacker_id: attacker.into(),
            defender_id: defender.into(),
            idempotency_id: id.into(),
            client_now_unix: Some(99_999),
        }
    }

    #[test]
    fn shard_raid_matches_the_single_player_resolver() {
        let mut shard = raid_session(1_000, 0, 3);
        let mut attacker = armed(1_000, 8);
        let mut defender = armed(1_000, 3);
        let atk_fleet = attacker.fleet;
        let def_fleet = defender.fleet;
        let mut rng = attacker.rng_seed;
        let expected = resolve_combat(
            atk_fleet,
            def_fleet,
            &stats_from_catalog(&attacker.config.ships),
            &stats_from_catalog(&defender.config.ships),
            attacker.config.raid.max_rounds,
            &rapid_fire_table(&attacker.config.ships),
            &mut rng,
        );
        let result = shard
            .raid(&mut attacker, &mut defender, raid_cmd("a", "b", "r1"))
            .unwrap();
        assert_eq!(result.report, expected);
        assert_eq!(attacker.fleet, expected.attacker_after);
        assert_eq!(defender.fleet, expected.defender_after);
    }

    #[test]
    fn newbie_protection_blocks_then_expires() {
        let mut shard = raid_session(1_000, 3_600, 3);
        let mut attacker = armed(1_000, 8);
        let mut defender = armed(1_000, 3);
        let err = shard
            .raid(&mut attacker, &mut defender, raid_cmd("a", "b", "early"))
            .unwrap_err();
        assert!(matches!(
            err,
            ShardError::NewbieProtected { identity } if identity == "b"
        ));
        assert_eq!(attacker.fleet.get(ShipKind::LightFighter), 8);
        shard.clock.advance(3_600);
        shard
            .raid(&mut attacker, &mut defender, raid_cmd("a", "b", "later"))
            .unwrap();
    }

    #[test]
    fn attack_cap_rejects_the_next_raid() {
        let mut shard = raid_session(1_000, 0, 1);
        let mut attacker = armed(1_000, 8);
        let mut defender = armed(1_000, 3);
        shard
            .raid(&mut attacker, &mut defender, raid_cmd("a", "b", "r1"))
            .unwrap();
        let err = shard
            .raid(&mut attacker, &mut defender, raid_cmd("a", "b", "r2"))
            .unwrap_err();
        assert!(matches!(
            err,
            ShardError::AttackCapped { identity, .. } if identity == "a"
        ));
    }

    #[test]
    fn shard_raid_applies_plunder_share_then_cap() {
        let mut shard = raid_session(1_000, 0, 3);
        let mut attacker = armed(1_000, 20);
        let mut defender = armed(1_000, 1);
        defender.resources.iron = 200.0;
        defender.config.raid.plunder_share = 0.5;
        defender.config.raid.plunder_cap = Resources::from_parts(40.0, 0.0, 0.0);
        let stock = defender.resources;
        let before = attacker.resources;
        let result = shard
            .raid(&mut attacker, &mut defender, raid_cmd("a", "b", "loot"))
            .unwrap();
        assert_eq!(result.report.winner, CombatWinner::Attacker);
        let expected = plunder_from(stock, 0.5, defender.config.raid.plunder_cap);
        assert_eq!(result.plunder, expected);
        assert_eq!(attacker.resources, before.saturating_add(expected));
        assert_eq!(defender.resources, stock.saturating_sub(expected));
    }
}
