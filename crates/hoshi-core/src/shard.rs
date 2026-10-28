//! Authoritative gate for a Chisei-hosted shard. No network.
//!
//! Local play never goes through this module. A shard host owns the clock,
//! the MAC key, and per-identity command windows, then calls the same
//! `propose` / `apply` path as the CLI.

use crate::activity::{ActivityApply, ActivityPulse};
use crate::classification::ClassifiedKind;
use crate::clock::Clock;
use crate::combat::{
    CombatReport, CombatWinner, plunder_from, rapid_fire_table, resolve_combat, stats_from_catalog,
};
use crate::fleets::{Coord, MissionPhase};
use crate::intel::IntelFact;
use crate::intent::{Intent, IntentError, apply, propose};
use crate::leaders::Actor;
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

/// One owned world on a tenant read. Other empires never appear here.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardPlanet {
    pub coord: Coord,
    pub resources: Resources,
    pub fleet: Fleet,
}

/// In-flight ships the tenant already launched. Omits expedition seeds.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardMission {
    pub id: u64,
    pub ships: Fleet,
    pub origin: Coord,
    pub dest: Coord,
    pub phase: MissionPhase,
    pub phase_ends_at: u64,
}

/// Pirate outposts and open expedition sites. Sites stay empty until that
/// catalog exists. These lists are the shard map, not each save's private
/// outpost table.
#[derive(Debug, Clone, Copy)]
pub struct ShardSites<'a> {
    pub outposts: &'a [Coord],
    pub sites: &'a [Coord],
}

impl ShardSites<'static> {
    /// No pirate outposts and no open sites.
    pub fn none() -> Self {
        Self {
            outposts: &[],
            sites: &[],
        }
    }
}

/// One coord the tenant may see. `tag` is `you`, `occupied`, or `empty`.
/// Another tenant's name and stockpile are not on this row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardSlot {
    pub coord: Coord,
    pub tag: String,
}

/// Who holds a coord on the shard map. At most one kind.
///
/// `You` beats `Tenant`, then a pirate `Outpost`, then an open `Site`.
/// Pirate outposts and sites are the lists the host passes. They are not
/// read from each save's private outpost table. A conquered outpost is a
/// colony, so it is `You` or `Tenant`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occupancy {
    You,
    Tenant,
    Outpost,
    Site,
    Empty,
}

impl Occupancy {
    pub fn as_key(self) -> &'static str {
        match self {
            Occupancy::You => "you",
            Occupancy::Tenant => "tenant",
            Occupancy::Outpost => "outpost",
            Occupancy::Site => "site",
            Occupancy::Empty => "empty",
        }
    }

    /// Tenant-view label. A taken slot that is not yours is `occupied`.
    pub fn view_tag(self) -> &'static str {
        match self {
            Occupancy::You => "you",
            Occupancy::Empty => "empty",
            Occupancy::Tenant | Occupancy::Outpost | Occupancy::Site => "occupied",
        }
    }

    /// Join may place a home only on `Empty`.
    pub fn join_slot(self) -> bool {
        matches!(self, Occupancy::Empty)
    }

    /// Colonize, and a snapshot raid aimed at a planet, refuse these.
    pub fn blocks_planet_dest(self) -> bool {
        matches!(
            self,
            Occupancy::Tenant | Occupancy::Outpost | Occupancy::Site
        )
    }
}

/// Per-tenant read. Hidden simulation and other empires stay off this type.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardView {
    pub identity: String,
    pub at: u64,
    pub home: Coord,
    pub planets: Vec<ShardPlanet>,
    pub fleet: Fleet,
    pub missions: Vec<ShardMission>,
    pub resources: Resources,
    pub intel: Vec<IntelFact>,
    /// Coords the player's clearance can see, tagged `you`, `occupied`, or `empty`.
    pub occupancy: Vec<ShardSlot>,
}

impl ShardView {
    /// JSON the host can hand to a tenant. Occupancy tags do not carry
    /// another tenant's identity or stockpile.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "identity": self.identity,
            "at": self.at,
            "home": self.home.to_string(),
            "occupancy": self.occupancy.iter().map(|slot| {
                serde_json::json!({
                    "coord": slot.coord.to_string(),
                    "tag": slot.tag,
                })
            }).collect::<Vec<_>>(),
            "resources": {
                "iron": self.resources.iron,
                "silicon": self.resources.silicon,
                "deuterium": self.resources.deuterium,
            },
            "planets": self.planets.iter().map(|planet| planet.coord.to_string()).collect::<Vec<_>>(),
        })
    }
}

/// Why a shard refused a command, a bridge pulse, a raid, or a tenant read.
#[derive(Debug, Clone, PartialEq)]
pub enum ShardError {
    EmptyIdentity,
    EmptyIdempotency,
    UnknownIdentity {
        identity: String,
    },
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
    /// Colonize or a planet raid dest is `Tenant`, `Outpost`, or `Site`.
    Occupied {
        coord: Coord,
        kind: Occupancy,
    },
    /// `submit` cannot see the shard map. Colonize goes through [`ShardSession::colonize`].
    ColonizeNeedsMap,
}

impl fmt::Display for ShardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShardError::EmptyIdentity => write!(f, "shard identity is empty"),
            ShardError::EmptyIdempotency => write!(f, "shard idempotency id is empty"),
            ShardError::UnknownIdentity { identity } => {
                write!(f, "unknown shard identity {identity}")
            }
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
            ShardError::Occupied { coord, kind } => {
                write!(f, "{coord} is {}", kind.as_key())
            }
            ShardError::ColonizeNeedsMap => {
                write!(f, "colonize needs the shard map")
            }
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
    /// and is never read. `Intent::Colonize` is refused: this method has one
    /// empire, so it cannot see another tenant, a pirate outpost, or a site.
    /// Hosts call [`Self::colonize`].
    pub fn submit(&mut self, state: &mut GameState, cmd: ShardCommand) -> Result<(), ShardError> {
        state
            .require_fair_play()
            .map_err(|e| ShardError::Intent(crate::IntentError::Cheat(e)))?;
        if matches!(cmd.intent, crate::Intent::Cheat { .. }) {
            return Err(ShardError::Intent(crate::IntentError::Cheat(
                crate::CheatError::Forbidden,
            )));
        }
        if matches!(cmd.intent, Intent::Colonize { .. }) {
            return Err(ShardError::ColonizeNeedsMap);
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
                defender.plunderable(),
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

    /// Snapshot raid aimed at a planet coord.
    ///
    /// `Tenant`, `Outpost`, and `Site` are refused before combat, so fleets
    /// stay put. `Empty` and `You` continue into [`Self::raid`].
    pub fn raid_planet(
        &mut self,
        attacker: &mut GameState,
        defender: &mut GameState,
        dest: Coord,
        empires: &BTreeMap<String, GameState>,
        sites: ShardSites<'_>,
        cmd: ShardRaid,
    ) -> Result<ShardRaidResult, ShardError> {
        let kind = occupancy(&cmd.attacker_id, empires, sites, dest);
        if kind.blocks_planet_dest() {
            return Err(ShardError::Occupied { coord: dest, kind });
        }
        self.raid(attacker, defender, cmd)
    }

    /// Colonize `dest` for `identity` when occupancy allows it.
    ///
    /// Refusal happens before `propose` / `apply`, so a taken coord does not
    /// launch a colony ship. An empty coord uses the same colonize intent as
    /// local play. The check does not reserve the dest: a second colonize of
    /// that still-empty coord also launches, and each empire settles its own
    /// mission. Join is not this method: a host calls [`occupancy`] and
    /// [`Occupancy::join_slot`] before placing a home.
    pub fn colonize(
        &mut self,
        identity: &str,
        empires: &mut BTreeMap<String, GameState>,
        sites: ShardSites<'_>,
        dest: Coord,
        speed_percent: u8,
        idempotency_id: &str,
    ) -> Result<(), ShardError> {
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if idempotency_id.is_empty() {
            return Err(ShardError::EmptyIdempotency);
        }
        if !empires.contains_key(identity) {
            return Err(ShardError::UnknownIdentity {
                identity: identity.to_string(),
            });
        }
        let seen_key = (identity.to_string(), idempotency_id.to_string());
        if self.seen.contains(&seen_key) {
            return Ok(());
        }
        {
            let state = empires
                .get_mut(identity)
                .ok_or_else(|| ShardError::UnknownIdentity {
                    identity: identity.to_string(),
                })?;
            state
                .require_fair_play()
                .map_err(|e| ShardError::Intent(IntentError::Cheat(e)))?;
        }
        let now = self.clock.now_unix();
        for state in empires.values_mut() {
            state.catch_up(now);
        }
        let kind = occupancy(identity, empires, sites, dest);
        if kind.blocks_planet_dest() {
            return Err(ShardError::Occupied { coord: dest, kind });
        }
        self.admit(identity, now)?;
        let state = empires
            .get_mut(identity)
            .ok_or_else(|| ShardError::UnknownIdentity {
                identity: identity.to_string(),
            })?;
        let intent = Intent::Colonize {
            dest,
            speed_percent,
        };
        let proposal = propose(state, &intent, now).map_err(ShardError::Intent)?;
        apply(state, &proposal, now).map_err(ShardError::Intent)?;
        self.seen.insert(seen_key);
        Ok(())
    }

    /// Catch every empire up at the host clock, then return `identity`'s view.
    ///
    /// The view is a whitelist: own planets, fleets, and stock, plus spy intel
    /// that has already arrived, plus occupancy tags for coords the player
    /// clearance allows. It has no RNG, pirate timer, or other tenant's
    /// parked fleet and stockpile. Local play does not call this.
    /// `sites` is the shard's pirate outposts and open expedition sites.
    /// The site list stays empty until those sites exist.
    pub fn view(
        &self,
        identity: &str,
        empires: &mut BTreeMap<String, GameState>,
        sites: ShardSites<'_>,
    ) -> Result<ShardView, ShardError> {
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if !empires.contains_key(identity) {
            return Err(ShardError::UnknownIdentity {
                identity: identity.to_string(),
            });
        }
        let now = self.clock.now_unix();
        for (id, state) in empires.iter_mut() {
            if id == identity {
                state
                    .require_fair_play()
                    .map_err(|e| ShardError::Intent(crate::IntentError::Cheat(e)))?;
            }
            state.catch_up(now);
        }
        match shard_view(identity, empires, sites) {
            Some(view) => Ok(view),
            None => Err(ShardError::UnknownIdentity {
                identity: identity.to_string(),
            }),
        }
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

/// Kind at `coord` for `viewer`. Hosts call this before join and colonize.
///
/// This is the map at the call. A colony mission in flight does not hold
/// the coord, so two ships may still launch at one `Empty` slot. Arrival
/// order is a later check, when a colony exists and this function returns
/// `You` or `Tenant`.
pub fn occupancy(
    viewer: &str,
    empires: &BTreeMap<String, GameState>,
    sites: ShardSites<'_>,
    coord: Coord,
) -> Occupancy {
    if empires
        .get(viewer)
        .is_some_and(|own| tenant_holds(own, coord))
    {
        return Occupancy::You;
    }
    for (id, state) in empires {
        if id == viewer {
            continue;
        }
        if tenant_holds(state, coord) {
            return Occupancy::Tenant;
        }
    }
    if sites.outposts.contains(&coord) {
        return Occupancy::Outpost;
    }
    if sites.sites.contains(&coord) {
        return Occupancy::Site;
    }
    Occupancy::Empty
}

fn tenant_holds(state: &GameState, coord: Coord) -> bool {
    state.home == coord || state.colonies.iter().any(|colony| colony.coord == coord)
}

fn shard_view(
    identity: &str,
    empires: &BTreeMap<String, GameState>,
    sites: ShardSites<'_>,
) -> Option<ShardView> {
    if !empires.contains_key(identity) {
        return None;
    }
    let occupancy_rows = occupancy_slots(identity, empires, sites);
    let own = empires.get(identity)?;
    let mut planets = Vec::with_capacity(1 + own.colonies.len());
    planets.push(ShardPlanet {
        coord: own.home,
        resources: own.resources,
        fleet: own.fleet,
    });
    for colony in &own.colonies {
        planets.push(ShardPlanet {
            coord: colony.coord,
            resources: colony.resources,
            fleet: colony.fleet,
        });
    }
    Some(ShardView {
        identity: identity.to_string(),
        at: own.last_tick_unix,
        home: own.home,
        planets,
        fleet: own.fleet,
        missions: own
            .missions
            .iter()
            .map(|mission| ShardMission {
                id: mission.id,
                ships: mission.ships,
                origin: mission.origin,
                dest: mission.dest,
                phase: mission.phase,
                phase_ends_at: mission.phase_ends_at,
            })
            .collect(),
        resources: own.resources,
        intel: own.intel_facts_for(Actor::Player, own.last_tick_unix),
        occupancy: occupancy_rows,
    })
}

fn occupancy_slots(
    viewer: &str,
    empires: &BTreeMap<String, GameState>,
    sites: ShardSites<'_>,
) -> Vec<ShardSlot> {
    let (home, end) = match empires.get(viewer) {
        Some(own) => (own.home, own.config.expedition.dest_position),
        None => return Vec::new(),
    };
    let mut coords = BTreeSet::new();
    for position in 1..end {
        coords.insert(Coord {
            galaxy: home.galaxy,
            system: home.system,
            position,
        });
    }
    for state in empires.values() {
        coords.insert(state.home);
        for colony in &state.colonies {
            coords.insert(colony.coord);
        }
    }
    coords.extend(sites.outposts.iter().copied());
    coords.extend(sites.sites.iter().copied());
    coords
        .into_iter()
        .filter(|coord| {
            empires.get(viewer).is_some_and(|own| {
                own.can_see(Actor::Player, ClassifiedKind::Fact, &coord.to_string())
            })
        })
        .map(|coord| {
            let kind = occupancy(viewer, empires, sites, coord);
            ShardSlot {
                coord,
                tag: kind.view_tag().to_string(),
            }
        })
        .collect()
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
    use crate::espionage::SpyReport;
    use crate::fleets::{Coord, Mission, MissionPhase};
    use crate::intel::IntelValue;
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
        defender.config.raid.protected_share = 0.0;
        let stock = defender.resources;
        let reachable = defender.plunderable();
        let before = attacker.resources;
        let result = shard
            .raid(&mut attacker, &mut defender, raid_cmd("a", "b", "loot"))
            .unwrap();
        assert_eq!(result.report.winner, CombatWinner::Attacker);
        let expected = plunder_from(reachable, 0.5, defender.config.raid.plunder_cap);
        assert_eq!(result.plunder, expected);
        assert_eq!(result.plunder.iron, 40.0);
        assert_eq!(attacker.resources, before.saturating_add(expected));
        assert_eq!(defender.resources, stock.saturating_sub(expected));
    }

    const B_HOME: Coord = Coord {
        galaxy: 1,
        system: 9,
        position: 9,
    };
    const B_IRON: f64 = 888_777.0;
    const B_BATTLESHIPS: u32 = 64;
    const B_RAID_AT: u64 = 9_001_337;
    const B_RNG: u64 = 0x1111_2222_3333_4444;
    const SPY_IRON: f64 = 12.5;

    fn tenant_a(now: u64) -> GameState {
        let mut state = test_state(now);
        state.resources.iron = 111.0;
        state.fleet = Fleet::default();
        state.fleet.add(ShipKind::LightFighter, 4);
        state.rng_seed = 7;
        state.next_raid_at = Some(now + 8_000);
        state.missions.push(Mission {
            id: 1,
            ships: {
                let mut ships = Fleet::default();
                ships.add(ShipKind::SmallCargo, 1);
                ships
            },
            origin: Coord::HOME,
            dest: Coord {
                galaxy: 1,
                system: 1,
                position: 3,
            },
            speed_percent: 100,
            phase: MissionPhase::Outbound,
            launched_at: now,
            outbound_secs: 120,
            hold_secs: 0,
            phase_ends_at: now + 120,
            fuel_paid: 1.0,
            colonize: false,
            expedition_seed: Some(0xFEED_FACE_FEED_FACE),
            expedition_applied: false,
            cargo: Resources::ZERO,
            transport: false,
            attack: false,
            invade: false,
            spy: false,
            site_id: None,
        });
        state
    }

    fn tenant_b(now: u64) -> GameState {
        let mut state = test_state(now);
        state.home = B_HOME;
        state.resources.iron = B_IRON;
        state.fleet = Fleet::default();
        state.fleet.add(ShipKind::Battleship, B_BATTLESHIPS);
        state.rng_seed = B_RNG;
        state.next_raid_at = Some(B_RAID_AT);
        state
    }

    fn spy_on_b(now: u64, arrives_at: u64) -> SpyReport {
        let mut fleet = Fleet::default();
        fleet.add(ShipKind::LightFighter, 3);
        SpyReport {
            id: 1,
            target: B_HOME,
            created_at: now,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 2.0,
            tier: "fleet".into(),
            resources: Some(Resources::from_parts(SPY_IRON, 0.0, 0.0)),
            fleet: Some(fleet),
            defences: None,
            garrison: None,
            kind: None,
            size: None,
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: now,
            arrives_at,
            confidence: 0.8,
            deceived: false,
        }
    }

    fn two_empires(a: GameState, b: GameState) -> BTreeMap<String, GameState> {
        let mut empires = BTreeMap::new();
        empires.insert("tenant-a".into(), a);
        empires.insert("tenant-b".into(), b);
        empires
    }

    fn dump_hides(view: &ShardView, needle: &str) {
        let dump = format!("{view:?}");
        assert!(
            !dump.contains(needle),
            "tenant view leaked {needle}: {dump}"
        );
    }

    #[test]
    fn tenant_view_hides_the_other_empire_and_simulation_secrets() {
        let shard = session(1_000);
        let mut a = tenant_a(1_000);
        a.spy_reports.push(spy_on_b(1_000, 1_000));
        let mut empires = two_empires(a, tenant_b(1_000));
        let view = shard
            .view("tenant-a", &mut empires, ShardSites::none())
            .unwrap();
        assert_eq!(view.identity, "tenant-a");
        assert_eq!(view.home, Coord::HOME);
        assert_eq!(view.resources.iron, 111.0);
        assert_eq!(view.fleet.get(ShipKind::LightFighter), 4);
        assert_eq!(view.fleet.get(ShipKind::Battleship), 0);
        assert!(view.planets.iter().all(|planet| planet.coord != B_HOME));
        assert_eq!(view.missions.len(), 1);
        assert_eq!(view.missions[0].ships.get(ShipKind::SmallCargo), 1);
        let iron = view
            .intel
            .iter()
            .find(|fact| fact.object == B_HOME.to_string() && fact.field == "resources.iron")
            .expect("spy iron");
        assert_eq!(iron.observations[0].value, IntelValue::Number(SPY_IRON));
        dump_hides(&view, "rng_seed");
        dump_hides(&view, "next_raid");
        dump_hides(&view, "expedition_seed");
        dump_hides(&view, &format!("{B_IRON}"));
        dump_hides(&view, &B_RNG.to_string());
        dump_hides(&view, &B_RAID_AT.to_string());
        assert_eq!(view.fleet.get(ShipKind::Battleship), 0);
        assert!(
            view.planets
                .iter()
                .all(|planet| planet.fleet.get(ShipKind::Battleship) == 0)
        );
    }

    #[test]
    fn tenant_view_spy_intel_waits_for_light_delay() {
        let shard = session(1_000);
        let mut a = tenant_a(1_000);
        a.spy_reports.push(spy_on_b(1_000, 1_000 + 50_000));
        let mut empires = two_empires(a, tenant_b(1_000));
        let view = shard
            .view("tenant-a", &mut empires, ShardSites::none())
            .unwrap();
        assert!(
            view.intel
                .iter()
                .all(|fact| fact.object != B_HOME.to_string())
        );
    }

    #[test]
    fn tenant_view_catch_up_does_not_leak_hidden_fields() {
        let shard = session(1_000);
        let a = tenant_a(1_000);
        let b = tenant_b(1_000);
        shard.clock.advance(3_600);
        let mut empires = two_empires(a, b);
        let view = shard
            .view("tenant-a", &mut empires, ShardSites::none())
            .unwrap();
        assert_eq!(view.at, 4_600);
        assert_eq!(empires["tenant-a"].last_tick_unix, 4_600);
        assert_eq!(empires["tenant-b"].last_tick_unix, 4_600);
        assert_eq!(view.home, Coord::HOME);
        dump_hides(&view, &format!("{B_IRON}"));
        dump_hides(&view, "rng_seed");
        dump_hides(&view, "next_raid");
        assert_eq!(view.fleet.get(ShipKind::Battleship), 0);
        assert!(view.planets.iter().all(|planet| planet.coord != B_HOME));
    }

    #[test]
    fn tenant_view_rejects_empty_and_unknown_identity() {
        let shard = session(1_000);
        let mut empires = two_empires(tenant_a(1_000), tenant_b(1_000));
        assert_eq!(
            shard
                .view("", &mut empires, ShardSites::none())
                .unwrap_err(),
            ShardError::EmptyIdentity
        );
        assert_eq!(
            shard
                .view("tenant-z", &mut empires, ShardSites::none())
                .unwrap_err(),
            ShardError::UnknownIdentity {
                identity: "tenant-z".into()
            }
        );
    }

    fn pirate_coord() -> Coord {
        Coord {
            galaxy: 4,
            system: 4,
            position: 4,
        }
    }

    fn site_coord() -> Coord {
        Coord {
            galaxy: 5,
            system: 5,
            position: 5,
        }
    }

    fn empty_slot() -> Coord {
        Coord {
            galaxy: 1,
            system: 1,
            position: 14,
        }
    }

    fn with_colony(mut state: GameState, coord: Coord) -> GameState {
        state.colonies.push(crate::Colony {
            coord,
            kind: crate::PlanetType::Rocky,
            size: 8,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
        state
    }

    #[test]
    fn occupancy_names_you_and_tenant_and_skips_outposts_and_sites_for_join() {
        let colony = Coord {
            galaxy: 1,
            system: 2,
            position: 2,
        };
        let a = with_colony(tenant_a(1_000), colony);
        let empires = two_empires(a, tenant_b(1_000));
        let outposts = [pirate_coord()];
        let sites = [site_coord()];
        assert_eq!(
            occupancy(
                "tenant-a",
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites
                },
                Coord::HOME
            ),
            Occupancy::You
        );
        assert_eq!(
            occupancy(
                "tenant-b",
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites
                },
                Coord::HOME
            ),
            Occupancy::Tenant
        );
        assert_eq!(
            occupancy(
                "tenant-a",
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites
                },
                colony
            ),
            Occupancy::You
        );
        assert_eq!(
            occupancy(
                "tenant-b",
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites
                },
                colony
            ),
            Occupancy::Tenant
        );
        assert_eq!(
            occupancy(
                "tenant-a",
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites
                },
                B_HOME
            ),
            Occupancy::Tenant
        );
        assert_eq!(
            occupancy(
                "tenant-b",
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites
                },
                B_HOME
            ),
            Occupancy::You
        );
        assert_eq!(
            occupancy(
                "tenant-a",
                &empires,
                ShardSites {
                    outposts: &[B_HOME],
                    sites: &sites,
                },
                B_HOME,
            ),
            Occupancy::Tenant,
            "a tenant home stays a tenant when the host also lists that coord"
        );
        for viewer in ["tenant-a", "tenant-b"] {
            let kind = occupancy(
                viewer,
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites,
                },
                pirate_coord(),
            );
            assert_eq!(kind, Occupancy::Outpost);
            assert!(!kind.join_slot());
            let kind = occupancy(
                viewer,
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites,
                },
                site_coord(),
            );
            assert_eq!(kind, Occupancy::Site);
            assert!(!kind.join_slot());
        }
        let before_sites = occupancy(
            "tenant-a",
            &empires,
            ShardSites {
                outposts: &outposts,
                sites: &[],
            },
            site_coord(),
        );
        assert_eq!(before_sites, Occupancy::Empty);
        assert!(before_sites.join_slot());
        let empty = occupancy(
            "tenant-a",
            &empires,
            ShardSites {
                outposts: &outposts,
                sites: &sites,
            },
            empty_slot(),
        );
        assert_eq!(empty, Occupancy::Empty);
        assert!(empty.join_slot());
    }

    #[test]
    fn colonize_refuses_taken_coords_and_still_launches_to_empty() {
        let mut shard = session(1_000);
        let mut a = test_state(1_000);
        a.resources.deuterium = 10_000.0;
        a.fleet.add(ShipKind::ColonyShip, 1);
        let mut empires = two_empires(a, tenant_b(1_000));
        let outposts = [pirate_coord()];
        let sites = [site_coord()];
        let refused = [
            (B_HOME, "b-home", Occupancy::Tenant),
            (pirate_coord(), "outpost", Occupancy::Outpost),
            (site_coord(), "site", Occupancy::Site),
        ];
        for (dest, id, kind) in refused {
            let err = shard
                .colonize(
                    "tenant-a",
                    &mut empires,
                    ShardSites {
                        outposts: &outposts,
                        sites: &sites,
                    },
                    dest,
                    100,
                    id,
                )
                .unwrap_err();
            assert_eq!(err, ShardError::Occupied { coord: dest, kind });
            assert_eq!(empires["tenant-a"].fleet.get(ShipKind::ColonyShip), 1);
            assert!(empires["tenant-a"].missions.is_empty());
        }
        shard
            .colonize(
                "tenant-a",
                &mut empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites,
                },
                empty_slot(),
                100,
                "empty",
            )
            .unwrap();
        assert_eq!(empires["tenant-a"].missions.len(), 1);
        let mission = &empires["tenant-a"].missions[0];
        assert!(mission.colonize);
        assert_eq!(mission.dest, empty_slot());
        assert_eq!(empires["tenant-a"].fleet.get(ShipKind::ColonyShip), 0);
    }

    #[test]
    fn submit_refuses_colonize_without_the_shard_map() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        state.resources.deuterium = 10_000.0;
        state.fleet.add(ShipKind::ColonyShip, 1);
        let before = state.clone();
        let err = shard
            .submit(
                &mut state,
                ShardCommand {
                    identity: "tenant-a".into(),
                    idempotency_id: "col".into(),
                    intent: Intent::Colonize {
                        dest: empty_slot(),
                        speed_percent: 100,
                    },
                    client_now_unix: None,
                },
            )
            .unwrap_err();
        assert_eq!(err, ShardError::ColonizeNeedsMap);
        assert_eq!(state, before);
    }

    #[test]
    fn planet_raid_dest_refuses_tenant_outpost_and_site() {
        let mut shard = raid_session(1_000, 0, 3);
        let mut attacker = armed(1_000, 20);
        let mut defender = armed(1_000, 1);
        defender.home = B_HOME;
        let atk = attacker.fleet.get(ShipKind::LightFighter);
        let def = defender.fleet.get(ShipKind::LightFighter);
        let mut empires = BTreeMap::new();
        empires.insert("a".into(), attacker.clone());
        empires.insert("b".into(), defender.clone());
        let outposts = [pirate_coord()];
        let sites = [site_coord()];
        for (dest, id, kind) in [
            (B_HOME, "home", Occupancy::Tenant),
            (pirate_coord(), "outpost", Occupancy::Outpost),
            (site_coord(), "site", Occupancy::Site),
        ] {
            let err = shard
                .raid_planet(
                    &mut attacker,
                    &mut defender,
                    dest,
                    &empires,
                    ShardSites {
                        outposts: &outposts,
                        sites: &sites,
                    },
                    raid_cmd("a", "b", id),
                )
                .unwrap_err();
            assert_eq!(err, ShardError::Occupied { coord: dest, kind });
            assert_eq!(attacker.fleet.get(ShipKind::LightFighter), atk);
            assert_eq!(defender.fleet.get(ShipKind::LightFighter), def);
        }
        let result = shard
            .raid_planet(
                &mut attacker,
                &mut defender,
                empty_slot(),
                &empires,
                ShardSites {
                    outposts: &outposts,
                    sites: &sites,
                },
                raid_cmd("a", "b", "empty"),
            )
            .unwrap();
        assert_eq!(result.report.winner, CombatWinner::Attacker);
        assert!(defender.fleet.get(ShipKind::LightFighter) < def);
    }

    #[test]
    fn tenant_view_json_tags_occupancy_without_the_other_identity() {
        let shard = session(1_000);
        let mut empires = two_empires(tenant_a(1_000), tenant_b(1_000));
        let view = shard
            .view(
                "tenant-a",
                &mut empires,
                ShardSites {
                    outposts: &[pirate_coord()],
                    sites: &[site_coord()],
                },
            )
            .unwrap();
        let json = view.to_json();
        let text = serde_json::to_string(&json).unwrap();
        assert!(!text.contains("tenant-b"), "{text}");
        assert!(!text.contains("888777"), "{text}");
        let slots = json["occupancy"].as_array().unwrap();
        assert!(
            slots
                .iter()
                .any(|slot| slot["coord"] == "1:1:1" && slot["tag"] == "you")
        );
        assert!(
            slots
                .iter()
                .any(|slot| { slot["coord"] == B_HOME.to_string() && slot["tag"] == "occupied" })
        );
        assert!(slots.iter().any(|slot| slot["tag"] == "empty"));
        assert!(slots.iter().any(|slot| {
            slot["coord"] == pirate_coord().to_string() && slot["tag"] == "occupied"
        }));
        assert!(slots.iter().any(|slot| {
            slot["coord"] == site_coord().to_string() && slot["tag"] == "occupied"
        }));
        for slot in slots {
            let obj = slot.as_object().unwrap();
            assert_eq!(obj.len(), 2);
            assert!(obj.contains_key("coord"));
            assert!(obj.contains_key("tag"));
        }
        let planets = json["planets"].as_array().unwrap();
        let other_home = B_HOME.to_string();
        assert!(
            planets
                .iter()
                .all(|coord| coord.as_str() != Some(other_home.as_str()))
        );
    }
}
