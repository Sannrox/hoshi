//! Authoritative gate for a Chisei-hosted shard. No network.
//!
//! Local play never goes through this module. A shard host owns the clock,
//! the MAC key, and per-identity command windows, then calls the same
//! `propose` / `apply` path as the CLI.
//!
//! The host call order is in `docs/chisei.md` ("Host contract"). Host
//! duties on restart: persist [`ShardSession::export_bookkeeping`] next
//! to the tenant saves and hand it back through
//! [`ShardSession::restore_bookkeeping`], so a retried submit after reconnect
//! still hits its idempotency id. Load tenants with [`load_tenants`], one at a
//! time through the migrating save loader; a tenant that fails to load is
//! reported and never stops or writes the others.

use crate::activity::{ActivityApply, ActivityPulse};
use crate::classification::ClassifiedKind;
use crate::clock::{Clock, scale_unix};
use crate::combat::{
    CombatWinner, fill_hold, plunder_from, rapid_fire_table, resolve_combat, stats_from_catalog,
};
use crate::decision::{ExpiringDecision, expiring_decision_with_inbound};
use crate::fleets::{Coord, Mission, MissionPhase};
use crate::frontier::FrontierConfig;
use crate::intel::IntelFact;
use crate::intent::{Intent, IntentError, apply, propose};
use crate::leaders::Actor;
use crate::market::{MarketConfig, OrderTicket};
use crate::shard_market::{BookDoc, ShardBook, ShardMarket};
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, Resources};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

type HmacSha256 = Hmac<Sha256>;

/// Commands admitted per identity inside `command_window_secs`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardConfig {
    pub command_window_secs: u64,
    pub command_limit: u32,
    /// Defender run younger than this cannot be raided.
    pub newbie_secs: u64,
    pub attack_window_secs: u64,
    pub attack_limit: u32,
    /// Most tenants one shard seats; ADR 0005 caps the player list. A new
    /// identity past it is refused by [`ShardSession::join`].
    pub tenant_cap: u32,
    /// Rules of the host's regional book. A host passes the same
    /// `[market]` its tenant saves load with.
    pub market: MarketConfig,
}

impl ShardConfig {
    /// How long an idempotency id is remembered: the longer of the command
    /// and attack windows, so a reconnect inside either window still hits the
    /// same id. An older id is forgotten and may apply again.
    pub fn seen_ttl_secs(&self) -> u64 {
        self.command_window_secs.max(self.attack_window_secs).max(1)
    }
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            command_window_secs: 60,
            command_limit: 30,
            newbie_secs: 24 * 3600,
            attack_window_secs: 24 * 3600,
            attack_limit: 3,
            tenant_cap: 16,
            market: MarketConfig::default_active(),
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

/// Raid fleet from `identity` to another tenant's home at `dest`. See
/// [`ShardSession::launch_raid`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShardRaid {
    pub identity: String,
    pub idempotency_id: String,
    pub ships: Fleet,
    pub dest: Coord,
    pub speed_percent: u8,
}

/// Probe wave from `identity` to `dest`. See [`ShardSession::spy`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShardSpy {
    pub identity: String,
    pub idempotency_id: String,
    pub ships: Fleet,
    pub dest: Coord,
    pub speed_percent: u8,
}

/// Recyclers from `identity` to the shared debris at `dest`. See
/// [`ShardSession::recycle`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShardRecycle {
    pub identity: String,
    pub idempotency_id: String,
    pub ships: Fleet,
    pub dest: Coord,
    pub speed_percent: u8,
}

/// An order from `identity` for the shard's regional book. See
/// [`ShardSession::place_order`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShardOrder {
    pub identity: String,
    pub idempotency_id: String,
    pub ticket: OrderTicket,
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

/// One raid on the tenant's home, read from its own combat log. It names
/// what the tenant lost and never what the raider kept.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardRaidAftermath {
    pub at: u64,
    /// Own ships destroyed, by hull. Ships are never rebuilt.
    pub lost: Fleet,
    /// Stock carried off. Zero when the raid was held.
    pub plunder: Resources,
    /// Debris the fight left at home. Zero when no hull was lost.
    pub debris: f64,
}

/// Another tenant's raid flying at the viewer's home. It carries no
/// attacker identity or origin: the row is the clock. `ships` is the hull
/// count the viewer's own phalanx reads, with its sensor noise, and `None`
/// when no phalanx covers the fleet. A probe reads a parked fleet, never one
/// in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardInbound {
    pub dest: Coord,
    /// The raid's arrival on the flight clock.
    pub arrives_at: u64,
    pub ships: Option<u32>,
}

/// Pirate outposts, open expedition sites, and the shard's one expedition
/// coord, borrowed from [`ShardCommons`]. Sites stay empty until that
/// catalog exists. These lists are the shard map, not each save's private
/// outpost table.
#[derive(Debug, Clone, Copy)]
pub struct ShardSites<'a> {
    pub outposts: &'a [Coord],
    pub sites: &'a [Coord],
    /// Reads as a `Site` for every tenant.
    pub expedition: Option<Coord>,
}

impl ShardSites<'static> {
    /// No pirate outposts, no open sites, and no expedition coord.
    pub fn none() -> Self {
        Self {
            outposts: &[],
            sites: &[],
            expedition: None,
        }
    }
}

/// What sits on the shard map and belongs to no tenant: pirate outposts,
/// open sites, the expedition coord, battle debris per coord, and the
/// regional market book.
///
/// The [`ShardSession`] holds the shard's one copy, so every tenant reads
/// the same outpost and the same debris stock. A tenant save keeps no shard
/// debris: each catch-up lends the stock to one tenant at a time and takes
/// it back with that tenant's harvests and new wreckage. The host persists
/// [`Self::to_bytes`] beside the tenant saves and puts it back with
/// [`ShardSession::commons_mut`] on restart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShardCommons {
    pub outposts: Vec<Coord>,
    pub sites: Vec<Coord>,
    /// Where every shard expedition without a site flies. `None` until the
    /// host calls [`Self::place_expedition`]; until then
    /// [`ShardSession::submit`] refuses such a launch.
    pub expedition: Option<Coord>,
    pub debris: BTreeMap<Coord, f64>,
    /// One NPC book per region that every tenant trades on. See
    /// [`ShardSession::place_order`].
    pub market: ShardMarket,
}

/// Version 3 added the market book, version 2 `expedition`. Older bytes
/// still read, with a closed book and no expedition coord.
const COMMONS_VERSION: u32 = 3;
const COMMONS_READS: [u32; 3] = [1, 2, COMMONS_VERSION];

#[derive(Serialize, Deserialize)]
struct CommonsDoc {
    version: u32,
    outposts: Vec<[u16; 3]>,
    sites: Vec<[u16; 3]>,
    #[serde(default)]
    expedition: Option<[u16; 3]>,
    debris: Vec<([u16; 3], f64)>,
    #[serde(default)]
    market: Option<BookDoc>,
}

fn coord_row(coord: Coord) -> [u16; 3] {
    [coord.galaxy, coord.system, coord.position]
}

fn row_coord([galaxy, system, position]: [u16; 3]) -> Coord {
    Coord {
        galaxy,
        system,
        position,
    }
}

impl ShardCommons {
    /// Occupancy view of the outposts, sites, and expedition coord.
    pub fn sites(&self) -> ShardSites<'_> {
        ShardSites {
            outposts: &self.outposts,
            sites: &self.sites,
            expedition: self.expedition,
        }
    }

    /// The shard's one expedition coord, placed on the first call.
    ///
    /// It is the last `Empty` slot on `map`'s home walk outside the
    /// `frontier` galaxy, which a fresh tenant cannot fly to. A host that
    /// places it before any join gets a coord from the seed and bounds alone,
    /// and homes would reach it last. A placed coord is kept, so a restart or
    /// a retry returns the same one. Restored version 1 bytes have none; the
    /// host calls this after restoring them.
    pub fn place_expedition(
        &mut self,
        map: ShardMap,
        frontier: &FrontierConfig,
        empires: &BTreeMap<String, GameState>,
    ) -> Result<Coord, ShardError> {
        if let Some(coord) = self.expedition {
            return Ok(coord);
        }
        let sites = self.sites();
        let coord = map
            .home_walk()
            .rev()
            .filter(|coord| coord.galaxy != frontier.galaxy)
            .find(|&coord| occupancy("", empires, sites, coord).join_slot())
            .ok_or(ShardError::NoEmptySlot)?;
        self.expedition = Some(coord);
        Ok(coord)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ShardError> {
        let doc = CommonsDoc {
            version: COMMONS_VERSION,
            outposts: self.outposts.iter().copied().map(coord_row).collect(),
            sites: self.sites.iter().copied().map(coord_row).collect(),
            expedition: self.expedition.map(coord_row),
            debris: self
                .debris
                .iter()
                .map(|(&coord, &amount)| (coord_row(coord), amount))
                .collect(),
            market: self.market.to_doc(),
        };
        serde_json::to_vec(&doc).map_err(|e| ShardError::Commons(e.to_string()))
    }

    /// Read bytes from [`Self::to_bytes`], or version 1 or 2 bytes from a
    /// build before the expedition coord or the market book. Another
    /// version is refused.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ShardError> {
        let doc: CommonsDoc =
            serde_json::from_slice(bytes).map_err(|e| ShardError::Commons(e.to_string()))?;
        if !COMMONS_READS.contains(&doc.version) {
            return Err(ShardError::Commons(format!(
                "version {} is not {COMMONS_VERSION}",
                doc.version
            )));
        }
        Ok(Self {
            outposts: doc.outposts.into_iter().map(row_coord).collect(),
            sites: doc.sites.into_iter().map(row_coord).collect(),
            expedition: doc.expedition.map(row_coord),
            debris: doc
                .debris
                .into_iter()
                .map(|(row, amount)| (row_coord(row), amount))
                .collect(),
            market: ShardMarket::from_doc(doc.market).map_err(ShardError::Commons)?,
        })
    }
}

/// Bounds of the shard map and the seed its home walk starts from.
///
/// The map is galaxies `1..=galaxies`, systems `1..=systems` in each galaxy,
/// and positions `1..=positions` in each system. The host picks these once
/// per shard and keeps them stable, so [`place_home`] stays deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardMap {
    pub seed: u64,
    pub galaxies: u16,
    pub systems: u16,
    pub positions: u16,
}

impl ShardMap {
    /// Every slot once, in home-walk order.
    ///
    /// Systems are numbered galaxy-major (`1:1`, `1:2`, ..., `2:1`, ...). The
    /// walk starts at system `seed % systems` and position
    /// `(seed / systems) % positions`, both zero-based. It visits that
    /// position in every system, wrapping, before moving to the next
    /// position, so homes spread one per system before any system takes a
    /// second.
    fn home_walk(self) -> impl DoubleEndedIterator<Item = Coord> {
        let per_galaxy = u64::from(self.systems);
        let systems = u64::from(self.galaxies) * per_galaxy;
        let positions = u64::from(self.positions);
        let start_system = self.seed.checked_rem(systems).unwrap_or(0);
        let start_position = (self.seed / systems.max(1))
            .checked_rem(positions)
            .unwrap_or(0);
        (0..systems * positions).map(move |n| {
            let system = (start_system + n % systems) % systems;
            let position = (start_position + n / systems) % positions;
            // Each quotient is below its u16 bound, so the casts are exact.
            Coord {
                galaxy: (system / per_galaxy) as u16 + 1,
                system: (system % per_galaxy) as u16 + 1,
                position: position as u16 + 1,
            }
        })
    }
}

/// One coord on a tenant's target list and who holds it.
///
/// Spy and shard raids take a `Tenant` coord from this list; `You` is not a
/// target and `Empty` is for colonize only. The kind carries no identity or
/// stockpile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardTarget {
    pub coord: Coord,
    pub kind: Occupancy,
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
    /// Shard identity: game unix is `epoch + factor * (wall - epoch)`.
    pub epoch: u64,
    /// At least 1. Factor 1 is inert.
    pub factor: u32,
    pub home: Coord,
    pub planets: Vec<ShardPlanet>,
    pub fleet: Fleet,
    pub missions: Vec<ShardMission>,
    pub resources: Resources,
    pub intel: Vec<IntelFact>,
    /// Coords the player's clearance can see, tagged `you`, `occupied`, or `empty`.
    pub occupancy: Vec<ShardSlot>,
    /// Raids on home still in the combat log, oldest first. Reads do not
    /// consume them; newer fights push them out of the capped log.
    pub raids: Vec<ShardRaidAftermath>,
    /// Other tenants' raids still flying at home, earliest arrival first.
    pub inbound: Vec<ShardInbound>,
    /// The earliest choice still open, an inbound raid included.
    pub decision: Option<ExpiringDecision>,
    /// Present after the shard ends. Hidden boards stay off the view until then.
    pub standing: Option<ShardStanding>,
    /// Live ranked scores when the creation option is `visible`.
    pub scoreboard: Option<Vec<ShardScoreRow>>,
}

/// Creation option: live scores in every tenant view, or only after the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreboardVisibility {
    Hidden,
    Visible,
}

/// How a finish-line shard named its winner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShardWinPath {
    Race,
    Deadline,
}

impl ShardWinPath {
    fn as_key(self) -> &'static str {
        match self {
            Self::Race => "race",
            Self::Deadline => "deadline",
        }
    }
}

/// Final standing of a finished shard.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardStanding {
    pub winner: String,
    pub path: ShardWinPath,
    pub scores: Vec<ShardScoreRow>,
}

/// One tenant's arena score. No stockpile or fleet.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardScoreRow {
    pub identity: String,
    pub score: f64,
}

/// Arena scoring version 1, pinned on a finish-line shard at creation.
pub const ARENA_SCORING_VERSION: u32 = 1;

/// `prestige_score + 10*metal_mine + 5*crystal_mine + 3*solar_plant + 50*colonies + iron/1000`.
pub fn arena_score(state: &GameState) -> f64 {
    f64::from(state.prestige_score())
        + 10.0 * f64::from(state.buildings.metal_mine)
        + 5.0 * f64::from(state.buildings.crystal_mine)
        + 3.0 * f64::from(state.buildings.solar_plant)
        + 50.0 * state.colonies.len() as f64
        + state.resources.iron / 1000.0
}

impl ShardView {
    /// JSON the host can hand to a tenant. Occupancy tags do not carry
    /// another tenant's identity or stockpile.
    pub fn to_json(&self) -> serde_json::Value {
        let mut json = serde_json::json!({
            "identity": self.identity,
            "at": self.at,
            "epoch": self.epoch,
            "factor": self.factor,
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
            "raids": self.raids.iter().map(|raid| {
                let lost: serde_json::Map<String, serde_json::Value> = ShipKind::ALL
                    .iter()
                    .filter(|&&kind| raid.lost.get(kind) > 0)
                    .map(|&kind| (kind.as_key().to_string(), raid.lost.get(kind).into()))
                    .collect();
                serde_json::json!({
                    "at": raid.at,
                    "lost": lost,
                    "plunder": {
                        "iron": raid.plunder.iron,
                        "silicon": raid.plunder.silicon,
                        "deuterium": raid.plunder.deuterium,
                    },
                    "debris": raid.debris,
                })
            }).collect::<Vec<_>>(),
            "inbound": self.inbound.iter().map(|inbound| {
                serde_json::json!({
                    "dest": inbound.dest.to_string(),
                    "arrives_at": inbound.arrives_at,
                    "ships": inbound.ships,
                })
            }).collect::<Vec<_>>(),
            "decision": self.decision.map(|decision| {
                serde_json::json!({
                    "id": decision.id.as_key(),
                    "deadline": decision.deadline,
                    "action": decision.action.as_key(),
                })
            }),
        });
        if let Some(obj) = json.as_object_mut() {
            if let Some(standing) = &self.standing {
                obj.insert(
                    "standing".into(),
                    serde_json::json!({
                        "winner": standing.winner,
                        "path": standing.path.as_key(),
                        "scores": standing.scores.iter().map(|row| {
                            serde_json::json!({
                                "identity": row.identity,
                                "score": row.score,
                            })
                        }).collect::<Vec<_>>(),
                    }),
                );
            }
            if let Some(board) = &self.scoreboard {
                obj.insert(
                    "scoreboard".into(),
                    board
                        .iter()
                        .map(|row| {
                            serde_json::json!({
                                "identity": row.identity,
                                "score": row.score,
                            })
                        })
                        .collect::<Vec<_>>()
                        .into(),
                );
            }
        }
        json
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
    /// A spy dest is `Empty` or a `Site`: nothing there to read.
    NothingToSpy {
        coord: Coord,
        kind: Occupancy,
    },
    /// Colonize dest is `Tenant`, `Outpost`, or `Site`, or a join seat is
    /// no longer `Empty`.
    Occupied {
        coord: Coord,
        kind: Occupancy,
    },
    /// A raid dest is not another tenant's home: `Empty`, an `Outpost`, a
    /// `Site`, or a tenant colony.
    NotATenantHome {
        coord: Coord,
        kind: Occupancy,
    },
    /// `submit` cannot see the shard map. Colonize goes through [`ShardSession::colonize`].
    ColonizeNeedsMap,
    /// Restored bookkeeping bytes are not a version this build reads.
    Bookkeeping(String),
    /// Restored commons bytes are not a version this build reads.
    Commons(String),
    /// A colony, recycler, or raid of `identity` arrives by the host clock,
    /// or the regional book has a tick or expiry due while `identity` has an
    /// order on it, or a finish-line shard has reached its deadline or this
    /// tenant already recorded a victory. A lone catch-up cannot see the
    /// shard map, the book, or the other tenants, so the host calls
    /// [`ShardSession::settle`] first and retries.
    Unsettled {
        identity: String,
    },
    /// The home walk has no `Empty` slot left.
    NoEmptySlot,
    /// A shard expedition without a site needs the shard's expedition
    /// coord, and the host has not placed it with
    /// [`ShardCommons::place_expedition`].
    NoExpeditionCoord,
    /// `identity` already has a home elsewhere on the shard.
    AlreadySeated {
        identity: String,
    },
    /// The shard already seats [`ShardConfig::tenant_cap`] tenants.
    ShardFull {
        cap: u32,
    },
    /// `submit` cannot see the shard's regional book. Orders go through
    /// [`ShardSession::place_order`].
    OrderNeedsBook,
    /// The regional book opens at the next [`ShardSession::join`]; restored
    /// bytes from a build before it have none yet.
    NoBook,
    /// Scaled clock `factor` was 0.
    ZeroClockFactor,
    /// Gameplay after a finish-line shard named a winner.
    Ended,
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
            ShardError::SameEmpire => write!(f, "cannot raid or spy on your own empire"),
            ShardError::NothingToSpy { coord, kind } => {
                write!(f, "{coord} is {}, nothing to spy", kind.as_key())
            }
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
            ShardError::NotATenantHome { coord, kind } => {
                write!(f, "{coord} is {}, not a tenant home to raid", kind.as_key())
            }
            ShardError::ColonizeNeedsMap => {
                write!(f, "colonize needs the shard map")
            }
            ShardError::Bookkeeping(msg) => write!(f, "shard bookkeeping: {msg}"),
            ShardError::Commons(msg) => write!(f, "shard commons: {msg}"),
            ShardError::Unsettled { identity } => {
                write!(f, "{identity} needs the shard settled first")
            }
            ShardError::NoEmptySlot => write!(f, "shard map has no empty slot"),
            ShardError::NoExpeditionCoord => {
                write!(f, "shard map has no expedition coord yet")
            }
            ShardError::AlreadySeated { identity } => {
                write!(f, "{identity} already has a home on the shard")
            }
            ShardError::ShardFull { cap } => write!(f, "shard is full at {cap} tenants"),
            ShardError::OrderNeedsBook => write!(f, "market orders need the shard book"),
            ShardError::NoBook => write!(f, "shard market book opens at the next join"),
            ShardError::ZeroClockFactor => write!(f, "clock factor must be at least 1"),
            ShardError::Ended => write!(f, "shard has ended"),
        }
    }
}

impl std::error::Error for ShardError {}

const BOOKKEEPING_VERSION: u32 = 3;
const BOOKKEEPING_READS: [u32; 3] = [1, 2, BOOKKEEPING_VERSION];

/// Host-owned shard bookkeeping: idempotency ids, command windows and attack
/// stamps. Opaque bytes the host persists beside the tenant saves. It holds
/// identity strings, so it is never part of a tenant view, a `GameState`, or
/// a bug report; `Debug` prints only its size.
#[derive(Clone, PartialEq, Eq)]
pub struct ShardBookkeeping(Vec<u8>);

impl ShardBookkeeping {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for ShardBookkeeping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShardBookkeeping({} bytes)", self.0.len())
    }
}

fn default_clock_factor() -> u32 {
    1
}

fn join_rank(order: &[String], identity: &str) -> usize {
    order
        .iter()
        .position(|id| id == identity)
        .unwrap_or(usize::MAX)
}

#[derive(Serialize, Deserialize)]
struct BookkeepingDoc {
    version: u32,
    seen: Vec<SeenRow>,
    windows: BTreeMap<String, Vec<u64>>,
    attacks: BTreeMap<String, Vec<u64>>,
    /// Absent in bytes from a build before shard raids flew; the next
    /// settle rebuilds it.
    #[serde(default)]
    inbound: Vec<([u16; 3], u64)>,
    /// Absent in bytes from a factor-1 host; `0` and `1` are the identity.
    #[serde(default)]
    epoch: u64,
    #[serde(default = "default_clock_factor")]
    factor: u32,
    #[serde(default)]
    deadline: Option<u64>,
    #[serde(default)]
    scoreboard: Option<ScoreboardVisibility>,
    #[serde(default)]
    scoring_version: Option<u32>,
    #[serde(default)]
    ended_winner: Option<String>,
    #[serde(default)]
    ended_path: Option<ShardWinPath>,
    #[serde(default)]
    ended_scores: Vec<ShardScoreRowDoc>,
    #[serde(default)]
    join_order: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct ShardScoreRowDoc {
    identity: String,
    score: f64,
}

#[derive(Serialize, Deserialize)]
struct SeenRow {
    identity: String,
    id: String,
    at: u64,
}

/// Live shard session. The clock and MAC key stay on the host.
pub struct ShardSession<C> {
    clock: C,
    /// Shard identity. Game unix is `epoch + factor * (wall - epoch)`.
    epoch: u64,
    /// At least 1. Factor 1 is the identity. Never changes on a live session.
    factor: u32,
    config: ShardConfig,
    mac_key: [u8; 32],
    windows: BTreeMap<String, Vec<u64>>,
    /// `(identity, idempotency id)` to the host time it applied.
    seen: BTreeMap<(String, String), u64>,
    attacks: BTreeMap<String, Vec<u64>>,
    /// Host time of the last full prune. See [`Self::prune_due`].
    pruned_at: u64,
    commons: ShardCommons,
    /// Earliest unresolved raid arrival per target home. A lone `submit`
    /// on that home cannot see the attacker's mission, so it reads this.
    inbound: BTreeMap<Coord, u64>,
    /// Game unix. `None` is an ordinary shard with no finish line.
    deadline: Option<u64>,
    scoreboard: ScoreboardVisibility,
    scoring_version: u32,
    ended: Option<ShardStanding>,
    /// First-join order; deadline ties break this way.
    join_order: Vec<String>,
    /// Game unix of the last finish-line [`Self::catch_up_shard`]. Not
    /// persisted: after restore the next command unsettles so the host
    /// settles before anyone moves.
    settled_at: Option<u64>,
}

impl<C: Clock> ShardSession<C> {
    /// Factor 1, epoch 0: game unix equals the inner clock.
    pub fn new(clock: C, mac_key: [u8; 32], config: ShardConfig) -> Self {
        Self {
            clock,
            epoch: 0,
            factor: 1,
            config,
            mac_key,
            windows: BTreeMap::new(),
            seen: BTreeMap::new(),
            attacks: BTreeMap::new(),
            pruned_at: 0,
            commons: ShardCommons::default(),
            inbound: BTreeMap::new(),
            deadline: None,
            scoreboard: ScoreboardVisibility::Hidden,
            scoring_version: ARENA_SCORING_VERSION,
            ended: None,
            join_order: Vec::new(),
            settled_at: None,
        }
    }

    /// `epoch` and `factor` are identity and never change. `factor` must be
    /// at least 1.
    pub fn new_scaled(
        clock: C,
        mac_key: [u8; 32],
        config: ShardConfig,
        epoch: u64,
        factor: u32,
    ) -> Result<Self, ShardError> {
        if factor == 0 {
            return Err(ShardError::ZeroClockFactor);
        }
        Ok(Self {
            clock,
            epoch,
            factor,
            config,
            mac_key,
            windows: BTreeMap::new(),
            seen: BTreeMap::new(),
            attacks: BTreeMap::new(),
            pruned_at: 0,
            commons: ShardCommons::default(),
            inbound: BTreeMap::new(),
            deadline: None,
            scoreboard: ScoreboardVisibility::Hidden,
            scoring_version: ARENA_SCORING_VERSION,
            ended: None,
            join_order: Vec::new(),
            settled_at: None,
        })
    }

    /// Finish line identity, fixed at creation. Ordinary `new` has none.
    pub fn with_finish_line(mut self, deadline: u64, scoreboard: ScoreboardVisibility) -> Self {
        self.deadline = Some(deadline);
        self.scoreboard = scoreboard;
        self.scoring_version = ARENA_SCORING_VERSION;
        self
    }

    pub fn deadline(&self) -> Option<u64> {
        self.deadline
    }

    pub fn standing(&self) -> Option<&ShardStanding> {
        self.ended.as_ref()
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn factor(&self) -> u32 {
        self.factor
    }

    fn real_now(&self) -> u64 {
        self.clock.now_unix()
    }

    fn game_now(&self) -> u64 {
        scale_unix(self.real_now(), self.epoch, self.factor)
    }

    /// The shard map's outposts, sites, and debris.
    pub fn commons(&self) -> &ShardCommons {
        &self.commons
    }

    /// Place outposts and sites, or restore the commons on restart.
    pub fn commons_mut(&mut self) -> &mut ShardCommons {
        &mut self.commons
    }

    /// Game unix on this shard (`epoch + factor * (wall - epoch)`).
    pub fn now_unix(&self) -> u64 {
        self.game_now()
    }

    fn require_open(&self) -> Result<(), ShardError> {
        if self.ended.is_some() {
            Err(ShardError::Ended)
        } else {
            Ok(())
        }
    }

    fn finish_line(&self) -> bool {
        self.deadline.is_some()
    }

    fn needs_finish_settle(&self, now: u64) -> bool {
        self.finish_line() && self.ended.is_none() && self.settled_at != Some(now)
    }

    fn finish_unsettled(&self, state: &GameState, now: u64) -> bool {
        self.needs_finish_settle(now)
            || self.deadline.is_some_and(|d| now >= d)
            || (self.finish_line() && finish_due(state, now))
    }

    /// Catch-up bound for a finish-line shard: the deadline if it is already
    /// due, else the earliest recorded victory, last-stage victory-wonder, or
    /// alliance hold, else `now`. Ordinary shards pass `now` through.
    /// A conquest that appears only inside this catch-up is named at the end
    /// of the same settle (interaction-time, ADR 0022).
    fn freeze_at(&self, empires: &BTreeMap<String, GameState>, now: u64) -> u64 {
        let mut target = self.deadline.filter(|&d| d < now).unwrap_or(now);
        if !self.finish_line() {
            return target;
        }
        for state in empires.values() {
            if let Some(victory) = state.victory.as_ref() {
                target = target.min(victory.at);
            }
            if !state.config.victory.enabled {
                continue;
            }
            if let Some(at) = state.next_victory_at().filter(|&at| at <= target) {
                target = at;
            }
            if let Some(at) = victory_wonder_at(state).filter(|&at| at <= target) {
                target = at;
            }
        }
        target
    }

    fn ranked_scores(&self, empires: &BTreeMap<String, GameState>) -> Vec<ShardScoreRow> {
        let mut rows: Vec<ShardScoreRow> = empires
            .iter()
            .map(|(id, state)| ShardScoreRow {
                identity: id.clone(),
                score: arena_score(state),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    join_rank(&self.join_order, &a.identity)
                        .cmp(&join_rank(&self.join_order, &b.identity))
                })
                .then_with(|| a.identity.cmp(&b.identity))
        });
        rows
    }

    fn resolve_finish(&mut self, empires: &BTreeMap<String, GameState>, now: u64) {
        if !self.finish_line() {
            return;
        }
        if self.ended.is_some() {
            return;
        }
        let mut race: Option<(u64, usize, String)> = None;
        for (id, state) in empires {
            let Some(victory) = state.victory.as_ref() else {
                continue;
            };
            let rank = join_rank(&self.join_order, id);
            let cand = (victory.at, rank, id.clone());
            if race.as_ref().is_none_or(|cur| cand < *cur) {
                race = Some(cand);
            }
        }
        if let Some((_, _, winner)) = race {
            self.ended = Some(ShardStanding {
                winner,
                path: ShardWinPath::Race,
                scores: self.ranked_scores(empires),
            });
            return;
        }
        let Some(deadline) = self.deadline else {
            return;
        };
        if now < deadline || empires.is_empty() {
            return;
        }
        let scores = self.ranked_scores(empires);
        let Some(winner) = scores.first().map(|row| row.identity.clone()) else {
            return;
        };
        self.ended = Some(ShardStanding {
            winner,
            path: ShardWinPath::Deadline,
            scores,
        });
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
    /// Hosts call [`Self::colonize`]. `Intent::Expedition` without a site
    /// flies to the shard's one expedition coord, not the tenant's own slot,
    /// and is refused as [`ShardError::NoExpeditionCoord`] until the host
    /// places it. A colony, recycler, or raid arrival due by the host clock,
    /// another tenant's raid due on this home, a market tick or expiry
    /// due while the tenant has an order on the regional book, or a
    /// finish-line deadline or recorded victory, is refused as
    /// [`ShardError::Unsettled`] before anything moves.
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
        if matches!(cmd.intent, Intent::PlaceOrder { .. }) {
            return Err(ShardError::OrderNeedsBook);
        }
        let expedition = match cmd.intent {
            Intent::Expedition { site: None, .. } => Some(
                self.commons
                    .expedition
                    .ok_or(ShardError::NoExpeditionCoord)?,
            ),
            _ => None,
        };
        if cmd.identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if cmd.idempotency_id.is_empty() {
            return Err(ShardError::EmptyIdempotency);
        }
        let _ = cmd.client_now_unix;
        let real = self.real_now();
        let now = self.game_now();
        self.prune_due(real);
        let seen_key = (cmd.identity.clone(), cmd.idempotency_id.clone());
        if self.is_seen(&seen_key, real) {
            return Ok(());
        }
        self.require_open()?;
        if self.finish_unsettled(state, now) {
            return Err(ShardError::Unsettled {
                identity: cmd.identity,
            });
        }
        let raid_due = self.inbound.get(&state.home).is_some_and(|&at| at <= now);
        let book_due = self
            .commons
            .market
            .due_for(&cmd.identity, now, &self.config.market);
        if raid_due || book_due || race_due(state, now) {
            return Err(ShardError::Unsettled {
                identity: cmd.identity,
            });
        }
        self.admit(&cmd.identity, real)?;
        let finish = self.finish_line();
        let applied = with_shared_debris(&mut self.commons.debris, state, |state| {
            state.catch_up(now);
            if finish && state.victory.is_some() {
                return Ok(false);
            }
            match (&cmd.intent, expedition) {
                (
                    Intent::Expedition {
                        origin,
                        ships,
                        hold_secs,
                        speed_percent,
                        ..
                    },
                    Some(dest),
                ) => state
                    .launch_expedition_to(
                        origin.unwrap_or(state.home),
                        dest,
                        *ships,
                        *hold_secs,
                        *speed_percent,
                        now,
                    )
                    .map(|_| ())
                    .map_err(IntentError::Fleet)?,
                (intent, _) => {
                    let proposal = propose(state, intent, now)?;
                    apply(state, &proposal, now)?;
                }
            }
            Ok(true)
        })
        .map_err(ShardError::Intent)?;
        if !applied {
            return Err(ShardError::Unsettled {
                identity: cmd.identity,
            });
        }
        self.seen.insert(seen_key, real);
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
        self.require_open()?;
        let real = self.real_now();
        let now = self.game_now();
        let pulse = scale_pulse(event.pulse, self.epoch, self.factor);
        if self.deadline.is_some_and(|d| now >= d) || (self.finish_line() && finish_due(state, now))
        {
            return Err(ShardError::Unsettled {
                identity: identity.to_string(),
            });
        }
        // Delayed pulses (ts < now) apply at their stamp. A pulse at/after
        // the host clock needs shard-wide settle first so another tenant's
        // victory can close the shard; settle-then-retry would skip a
        // delayed stamp.
        if self.needs_finish_settle(now) && pulse.ts() >= now {
            return Err(ShardError::Unsettled {
                identity: identity.to_string(),
            });
        }
        self.admit(identity, real)?;
        Ok(state.apply_activity(pulse, now))
    }

    /// Launch a raid from `cmd.identity` at another tenant's home after
    /// catching every empire up at the host clock.
    ///
    /// The fleet flies on the current fleet rules (flight time, fuel, speed
    /// percent) with a hold of zero, and the attacker may recall it while
    /// outbound. Nothing fights at launch. At arrival the host catches the
    /// target up and resolves combat against its parked home fleet (see
    /// [`catch_up_empires`]); survivors fly home with the plunder as cargo.
    /// `You` is refused as [`ShardError::SameEmpire`]; `Empty`, `Outpost`,
    /// `Site`, and a tenant colony as [`ShardError::NotATenantHome`]. Newbie
    /// protection and the attack cap gate the launch. A refusal launches
    /// nothing, and a retried id launches once.
    pub fn launch_raid(
        &mut self,
        empires: &mut BTreeMap<String, GameState>,
        cmd: ShardRaid,
    ) -> Result<(), ShardError> {
        let identity = cmd.identity.as_str();
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if cmd.idempotency_id.is_empty() {
            return Err(ShardError::EmptyIdempotency);
        }
        let Some(state) = empires.get(identity) else {
            return Err(ShardError::UnknownIdentity {
                identity: cmd.identity,
            });
        };
        state
            .require_fair_play()
            .map_err(|e| ShardError::Intent(IntentError::Cheat(e)))?;
        let real = self.real_now();
        let now = self.game_now();
        self.prune_due(real);
        let seen_key = (cmd.identity.clone(), cmd.idempotency_id.clone());
        if self.is_seen(&seen_key, real) {
            return Ok(());
        }
        self.catch_up_shard(empires, now);
        self.require_open()?;
        let kind = occupancy(identity, empires, self.commons.sites(), cmd.dest);
        if kind == Occupancy::You {
            return Err(ShardError::SameEmpire);
        }
        let Some((defender_id, defender)) = empires
            .iter()
            .find(|(id, state)| *id != identity && state.home == cmd.dest)
        else {
            return Err(ShardError::NotATenantHome {
                coord: cmd.dest,
                kind,
            });
        };
        defender
            .require_fair_play()
            .map_err(|e| ShardError::Intent(IntentError::Cheat(e)))?;
        if now.saturating_sub(defender.prestige.current.started_at) < self.config.newbie_secs {
            return Err(ShardError::NewbieProtected {
                identity: defender_id.clone(),
            });
        }
        self.admit(identity, real)?;
        self.check_attack(identity, now)?;
        let state = empires
            .get_mut(identity)
            .ok_or_else(|| ShardError::UnknownIdentity {
                identity: identity.to_string(),
            })?;
        state
            .launch_tenant_raid(cmd.ships, cmd.dest, cmd.speed_percent, now)
            .map_err(|e| ShardError::Intent(IntentError::Fleet(e)))?;
        self.attacks
            .entry(cmd.identity.clone())
            .or_default()
            .push(now);
        self.inbound = inbound_raids(empires);
        self.seen.insert(seen_key, real);
        Ok(())
    }

    /// Join `identity` to the shard and return its home.
    ///
    /// The first join, or the first after restoring commons bytes from a
    /// build before the market book, opens the regional book at the host
    /// clock with the RNG seeded by `map.seed`.
    ///
    /// A seated identity resumes: its home comes back and nothing is built
    /// or written. A new identity past [`ShardConfig::tenant_cap`] is refused
    /// as [`ShardError::ShardFull`] before anything is written. Otherwise the
    /// host settles, places the home with [`place_home`], builds the empire
    /// for that coord with `build`, and seats it with [`seat_home`].
    pub fn join(
        &mut self,
        identity: &str,
        map: ShardMap,
        empires: &mut BTreeMap<String, GameState>,
        build: impl FnOnce(Coord) -> GameState,
    ) -> Result<Coord, ShardError> {
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        let now = self.game_now();
        if let Some(seated) = empires.get(identity) {
            let home = seated.home;
            self.commons.market.open(map.seed, now, &self.config.market);
            return Ok(home);
        }
        let cap = self.config.tenant_cap;
        if empires.len() >= cap as usize {
            return Err(ShardError::ShardFull { cap });
        }
        self.settle(empires);
        self.require_open()?;
        let coord = place_home(map, empires, self.commons.sites())?;
        seat_home(identity, build(coord), coord, empires, self.commons.sites())?;
        if !self.join_order.iter().any(|id| id == identity) {
            self.join_order.push(identity.to_string());
        }
        if self.factor > 1
            && let Some(state) = empires.get_mut(identity)
        {
            state.isolate_prestige(now);
        }
        self.commons.market.open(map.seed, now, &self.config.market);
        Ok(coord)
    }

    /// Colonize `dest` for `identity` when occupancy allows it.
    ///
    /// Refusal happens before `propose` / `apply`, so a taken coord does not
    /// launch a colony ship. An empty coord uses the same colonize intent as
    /// local play. The check does not reserve the dest: a second colonize of
    /// that still-empty coord also launches. At arrival the host checks
    /// again (see [`Self::settle`]): the first arrival founds, a later one
    /// turns back with its colony ship. Join is not this method: a host
    /// calls [`Self::join`].
    pub fn colonize(
        &mut self,
        identity: &str,
        empires: &mut BTreeMap<String, GameState>,
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
        let real = self.real_now();
        let now = self.game_now();
        self.prune_due(real);
        let seen_key = (identity.to_string(), idempotency_id.to_string());
        if self.is_seen(&seen_key, real) {
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
        self.catch_up_shard(empires, now);
        self.require_open()?;
        let kind = occupancy(identity, empires, self.commons.sites(), dest);
        if kind.blocks_planet_dest() {
            return Err(ShardError::Occupied { coord: dest, kind });
        }
        self.admit(identity, real)?;
        let state = empires
            .get_mut(identity)
            .ok_or_else(|| ShardError::UnknownIdentity {
                identity: identity.to_string(),
            })?;
        let intent = Intent::Colonize {
            origin: None,
            dest,
            speed_percent,
        };
        let proposal = propose(state, &intent, now).map_err(ShardError::Intent)?;
        apply(state, &proposal, now).map_err(ShardError::Intent)?;
        self.seen.insert(seen_key, real);
        Ok(())
    }

    /// Launch probes from `cmd.identity` at `cmd.dest` after catching every
    /// empire up at the host clock.
    ///
    /// A `Tenant` dest flies the probe hull with a hold of zero. On arrival
    /// the host reads the target planet's stock and parked fleet at that
    /// moment and writes a noisy report with the current sensor error,
    /// delivered after light-time from the spy's home (see
    /// [`catch_up_empires`]). An `Outpost` dest is today's local spy.
    /// `You` is refused as [`ShardError::SameEmpire`]; `Empty` and `Site` as
    /// [`ShardError::NothingToSpy`]. A refusal launches nothing.
    pub fn spy(
        &mut self,
        empires: &mut BTreeMap<String, GameState>,
        cmd: ShardSpy,
    ) -> Result<(), ShardError> {
        let identity = cmd.identity.as_str();
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if cmd.idempotency_id.is_empty() {
            return Err(ShardError::EmptyIdempotency);
        }
        let Some(state) = empires.get(identity) else {
            return Err(ShardError::UnknownIdentity {
                identity: cmd.identity,
            });
        };
        state
            .require_fair_play()
            .map_err(|e| ShardError::Intent(IntentError::Cheat(e)))?;
        let real = self.real_now();
        let now = self.game_now();
        self.prune_due(real);
        let seen_key = (cmd.identity.clone(), cmd.idempotency_id.clone());
        if self.is_seen(&seen_key, real) {
            return Ok(());
        }
        self.catch_up_shard(empires, now);
        self.require_open()?;
        let kind = occupancy(identity, empires, self.commons.sites(), cmd.dest);
        match kind {
            Occupancy::You => return Err(ShardError::SameEmpire),
            Occupancy::Empty | Occupancy::Site => {
                return Err(ShardError::NothingToSpy {
                    coord: cmd.dest,
                    kind,
                });
            }
            Occupancy::Tenant | Occupancy::Outpost => {}
        }
        self.admit(identity, real)?;
        let state = empires
            .get_mut(identity)
            .ok_or_else(|| ShardError::UnknownIdentity {
                identity: identity.to_string(),
            })?;
        if kind == Occupancy::Tenant {
            state
                .launch_tenant_spy(cmd.ships, cmd.dest, cmd.speed_percent, now)
                .map_err(|e| ShardError::Intent(IntentError::Fleet(e)))?;
        } else {
            let intent = Intent::Spy {
                origin: None,
                ships: cmd.ships,
                dest: cmd.dest,
                speed_percent: cmd.speed_percent,
            };
            let proposal = propose(state, &intent, now).map_err(ShardError::Intent)?;
            apply(state, &proposal, now).map_err(ShardError::Intent)?;
        }
        self.seen.insert(seen_key, real);
        Ok(())
    }

    /// Fly `cmd.identity`'s recyclers to the shared debris at `cmd.dest`
    /// after catching every empire up at the host clock.
    ///
    /// The stock stays on the shard map until arrival, so two tenants may
    /// launch at one pile. Arrivals harvest in order (see [`Self::settle`]):
    /// each takes at most its hold from what is left, so the takes sum to
    /// at most the stock. No debris at `dest` launches nothing.
    pub fn recycle(
        &mut self,
        empires: &mut BTreeMap<String, GameState>,
        cmd: ShardRecycle,
    ) -> Result<(), ShardError> {
        let identity = cmd.identity.as_str();
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if cmd.idempotency_id.is_empty() {
            return Err(ShardError::EmptyIdempotency);
        }
        let Some(state) = empires.get(identity) else {
            return Err(ShardError::UnknownIdentity {
                identity: cmd.identity,
            });
        };
        state
            .require_fair_play()
            .map_err(|e| ShardError::Intent(IntentError::Cheat(e)))?;
        let real = self.real_now();
        let now = self.game_now();
        self.prune_due(real);
        let seen_key = (cmd.identity.clone(), cmd.idempotency_id.clone());
        if self.is_seen(&seen_key, real) {
            return Ok(());
        }
        self.catch_up_shard(empires, now);
        self.require_open()?;
        self.admit(identity, real)?;
        let state = empires
            .get_mut(identity)
            .ok_or_else(|| ShardError::UnknownIdentity {
                identity: identity.to_string(),
            })?;
        with_shared_debris(&mut self.commons.debris, state, |state| {
            state.launch_recycle(state.home, cmd.dest, cmd.ships, cmd.speed_percent, now)
        })
        .map_err(|e| ShardError::Intent(IntentError::Fleet(e)))?;
        self.seen.insert(seen_key, real);
        Ok(())
    }

    /// Place `cmd.ticket` for `cmd.identity` on the shard's regional book
    /// after catching every empire up at the host clock.
    ///
    /// One book per region serves every tenant. The escrow comes from the
    /// tenant's stock in that region (the planet there plus its own
    /// warehouse), and the order matches price-time against the NPC and
    /// other tenants' resting orders, never the tenant's own. A fill pays
    /// the seller's iron and the buyer's goods into each tenant's save in
    /// that region at the host clock; goods still move between regions only
    /// by shipment. A resting order expires back to its tenant. A market
    /// order that trades nothing is refused and returns its escrow. A
    /// retried idempotency id applies once. Refused as [`ShardError::NoBook`]
    /// until a join opens the book.
    pub fn place_order(
        &mut self,
        empires: &mut BTreeMap<String, GameState>,
        cmd: ShardOrder,
    ) -> Result<(), ShardError> {
        let identity = cmd.identity.as_str();
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if cmd.idempotency_id.is_empty() {
            return Err(ShardError::EmptyIdempotency);
        }
        let Some(state) = empires.get(identity) else {
            return Err(ShardError::UnknownIdentity {
                identity: cmd.identity,
            });
        };
        state
            .require_fair_play()
            .map_err(|e| ShardError::Intent(IntentError::Cheat(e)))?;
        if !self.commons.market.is_open() {
            return Err(ShardError::NoBook);
        }
        let real = self.real_now();
        let now = self.game_now();
        self.prune_due(real);
        let seen_key = (cmd.identity.clone(), cmd.idempotency_id.clone());
        if self.is_seen(&seen_key, real) {
            return Ok(());
        }
        self.catch_up_shard(empires, now);
        self.require_open()?;
        self.admit(identity, real)?;
        self.commons
            .market
            .place(identity, cmd.ticket, empires, now, &self.config.market)?;
        self.seen.insert(seen_key, real);
        Ok(())
    }

    /// Catch every empire up at the host clock, then return `region`'s
    /// book as `identity` reads it: the shard's mids, the region's price
    /// history, and its resting orders with the tenant's own marked. Other
    /// orders carry no identity, and NPC warehouses stay hidden.
    pub fn book(
        &mut self,
        identity: &str,
        region: Coord,
        empires: &mut BTreeMap<String, GameState>,
    ) -> Result<ShardBook, ShardError> {
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if !empires.contains_key(identity) {
            return Err(ShardError::UnknownIdentity {
                identity: identity.to_string(),
            });
        }
        let now = self.game_now();
        self.catch_up_shard(empires, now);
        self.commons
            .market
            .read(identity, region, now)
            .ok_or(ShardError::NoBook)
    }

    /// Catch every empire up at the host clock on the shared map.
    ///
    /// Empires step together to each colony, recycler, and raid arrival, in
    /// time order and, at one instant, in identity order, except that a
    /// raid's target steps to its arrival first. A colony ship whose
    /// dest another tenant, an outpost, or a site holds by then turns back
    /// with the ship; the first arrival founds. A recycler harvests the
    /// shared stock that earlier arrivals left. A raid fights the target's
    /// home as it is at arrival. The regional book runs its NPC ticks and
    /// order expiries at their instants. [`Self::view`], [`Self::book`],
    /// [`Self::colonize`], [`Self::spy`], [`Self::recycle`],
    /// [`Self::place_order`] and [`Self::launch_raid`] settle first; a host
    /// calls this before [`Self::submit`] when it refuses with
    /// [`ShardError::Unsettled`].
    pub fn settle(&mut self, empires: &mut BTreeMap<String, GameState>) {
        self.catch_up_shard(empires, self.game_now());
    }

    /// Catch every empire up at the host clock, then return `identity`'s view.
    ///
    /// The view is a whitelist: own planets, fleets, and stock, plus spy intel
    /// that has already arrived, plus occupancy tags for coords the player
    /// clearance allows, plus what each logged raid on home took, plus each
    /// other tenant's raid still flying at home and the expiring decision
    /// with those inbounds as candidates. It has no RNG, pirate timer, or
    /// other tenant's parked fleet and stockpile, and an inbound names no
    /// attacker. Local play does not call this.
    /// Occupancy reads the session's [`ShardCommons`]. The site list stays
    /// empty until those sites exist.
    pub fn view(
        &mut self,
        identity: &str,
        empires: &mut BTreeMap<String, GameState>,
    ) -> Result<ShardView, ShardError> {
        if identity.is_empty() {
            return Err(ShardError::EmptyIdentity);
        }
        if !empires.contains_key(identity) {
            return Err(ShardError::UnknownIdentity {
                identity: identity.to_string(),
            });
        }
        let now = self.game_now();
        if let Some(state) = empires.get(identity) {
            state
                .require_fair_play()
                .map_err(|e| ShardError::Intent(crate::IntentError::Cheat(e)))?;
        }
        self.catch_up_shard(empires, now);
        match shard_view(
            identity,
            empires,
            self.commons.sites(),
            self.epoch,
            self.factor,
        ) {
            Some(mut view) => {
                view.standing = self.ended.clone();
                if self.ended.is_none() && self.scoreboard == ScoreboardVisibility::Visible {
                    view.scoreboard = Some(self.ranked_scores(empires));
                }
                Ok(view)
            }
            None => Err(ShardError::UnknownIdentity {
                identity: identity.to_string(),
            }),
        }
    }

    /// Prune at the host clock, then serialise ids, windows and attack stamps.
    /// Factor-1 sessions stay on version 1 so an older host can resume them.
    /// Scaled sessions write version 2 so an older host fails closed.
    /// Finish-line sessions write version 3.
    pub fn export_bookkeeping(&mut self) -> Result<ShardBookkeeping, ShardError> {
        self.prune(self.real_now());
        let doc = BookkeepingDoc {
            version: if self.deadline.is_some() || self.ended.is_some() {
                BOOKKEEPING_VERSION
            } else if self.factor > 1 {
                2
            } else {
                1
            },
            seen: self
                .seen
                .iter()
                .map(|((identity, id), &at)| SeenRow {
                    identity: identity.clone(),
                    id: id.clone(),
                    at,
                })
                .collect(),
            windows: self.windows.clone(),
            attacks: self.attacks.clone(),
            inbound: self
                .inbound
                .iter()
                .map(|(&coord, &at)| (coord_row(coord), at))
                .collect(),
            epoch: self.epoch,
            factor: self.factor,
            deadline: self.deadline,
            scoreboard: Some(self.scoreboard),
            scoring_version: Some(self.scoring_version),
            ended_winner: self.ended.as_ref().map(|s| s.winner.clone()),
            ended_path: self.ended.as_ref().map(|s| s.path),
            ended_scores: self
                .ended
                .as_ref()
                .map(|s| {
                    s.scores
                        .iter()
                        .map(|row| ShardScoreRowDoc {
                            identity: row.identity.clone(),
                            score: row.score,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            join_order: self.join_order.clone(),
        };
        serde_json::to_vec(&doc)
            .map(ShardBookkeeping)
            .map_err(|e| ShardError::Bookkeeping(e.to_string()))
    }

    /// Replace this session's bookkeeping with `saved`, then prune at the
    /// host clock. Unreadable bytes or a version other than 1 or 2 leave
    /// the session untouched. Version 1 is a factor-1 document.
    pub fn restore_bookkeeping(&mut self, saved: &ShardBookkeeping) -> Result<(), ShardError> {
        let doc: BookkeepingDoc = serde_json::from_slice(saved.as_bytes())
            .map_err(|e| ShardError::Bookkeeping(e.to_string()))?;
        if !BOOKKEEPING_READS.contains(&doc.version) {
            return Err(ShardError::Bookkeeping(format!(
                "version {} is not {BOOKKEEPING_VERSION}",
                doc.version
            )));
        }
        if doc.factor == 0 {
            return Err(ShardError::Bookkeeping(
                "clock factor must be at least 1".into(),
            ));
        }
        if doc.epoch != self.epoch || doc.factor != self.factor {
            return Err(ShardError::Bookkeeping(format!(
                "clock identity {0}+{1}x is not {2}+{3}x",
                doc.epoch, doc.factor, self.epoch, self.factor
            )));
        }
        let doc_board = doc.scoreboard.unwrap_or(ScoreboardVisibility::Hidden);
        let doc_scoring = doc.scoring_version.unwrap_or(ARENA_SCORING_VERSION);
        if doc.deadline != self.deadline
            || doc_board != self.scoreboard
            || (self.deadline.is_some() && doc_scoring != self.scoring_version)
        {
            return Err(ShardError::Bookkeeping(
                "finish-line identity does not match the session".into(),
            ));
        }
        self.seen = doc
            .seen
            .into_iter()
            .map(|row| ((row.identity, row.id), row.at))
            .collect();
        self.windows = doc.windows;
        self.attacks = doc.attacks;
        self.inbound = doc
            .inbound
            .into_iter()
            .map(|(row, at)| (row_coord(row), at))
            .collect();
        self.join_order = doc.join_order;
        self.ended = match (doc.ended_winner, doc.ended_path) {
            (Some(winner), Some(path)) => Some(ShardStanding {
                winner,
                path,
                scores: doc
                    .ended_scores
                    .into_iter()
                    .map(|row| ShardScoreRow {
                        identity: row.identity,
                        score: row.score,
                    })
                    .collect(),
            }),
            _ => None,
        };
        self.prune(self.real_now());
        Ok(())
    }

    /// Catch every empire up to `now` on the shared map, then note the raids
    /// still in flight for [`Self::submit`].
    fn catch_up_shard(&mut self, empires: &mut BTreeMap<String, GameState>, now: u64) {
        if self.ended.is_some() {
            self.inbound = inbound_raids(empires);
            return;
        }
        if empires.is_empty() {
            self.settled_at = Some(now);
            return;
        }
        loop {
            let target = self.freeze_at(empires, now);
            catch_up_empires(empires, &mut self.commons, &self.config.market, target);
            self.inbound = inbound_raids(empires);
            self.resolve_finish(empires, target);
            self.settled_at = Some(target);
            if self.ended.is_some() || target >= now {
                break;
            }
        }
    }

    /// Whether `key` applied less than [`ShardConfig::seen_ttl_secs`] ago.
    /// Expiry is checked here, so a stale id between full prunes is forgotten.
    fn is_seen(&self, key: &(String, String), now: u64) -> bool {
        self.seen
            .get(key)
            .is_some_and(|&at| now.saturating_sub(at) < self.config.seen_ttl_secs())
    }

    /// Full prune at most once per command window, so a request does not
    /// scan every remembered id on the shard.
    fn prune_due(&mut self, now: u64) {
        if now.saturating_sub(self.pruned_at) >= self.config.command_window_secs.max(1) {
            self.prune(now);
        }
    }

    /// Forget ids past [`ShardConfig::seen_ttl_secs`] and stamps outside
    /// their window, so a long-lived host does not grow without bound.
    /// `real` is wall unix: seen ids and command stamps live there. Attack
    /// stamps are game unix, so the attack window shrinks with `factor`.
    fn prune(&mut self, real: u64) {
        self.pruned_at = real;
        let ttl = self.config.seen_ttl_secs();
        self.seen.retain(|_, at| real.saturating_sub(*at) < ttl);
        let command = real.saturating_sub(self.config.command_window_secs.max(1));
        self.windows.retain(|_, stamps| {
            stamps.retain(|&t| t > command);
            !stamps.is_empty()
        });
        let game = scale_unix(real, self.epoch, self.factor);
        let attack = game.saturating_sub(self.config.attack_window_secs.max(1));
        self.attacks.retain(|_, stamps| {
            stamps.retain(|&t| t > attack);
            !stamps.is_empty()
        });
    }

    /// Refuse when `identity` has used its attack cap in the window. Records
    /// nothing: the caller stamps the attack once the fleet launched.
    fn check_attack(&mut self, identity: &str, now: u64) -> Result<(), ShardError> {
        let window = self.config.attack_window_secs.max(1);
        let limit = self.config.attack_limit;
        if limit == 0 {
            return Err(ShardError::AttackCapped {
                identity: identity.to_string(),
                retry_after_secs: window,
            });
        }
        let start = now.saturating_sub(window);
        let Some(stamps) = self.attacks.get_mut(identity) else {
            return Ok(());
        };
        stamps.retain(|&t| t > start);
        if stamps.len() as u32 >= limit {
            let oldest = stamps.first().copied().unwrap_or(now);
            let retry_after_secs = oldest.saturating_add(window).saturating_sub(now).max(1);
            return Err(ShardError::AttackCapped {
                identity: identity.to_string(),
                retry_after_secs,
            });
        }
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

/// Load each tenant one at a time through `load`, which the host backs with
/// the migrating save loader. A tenant that fails is returned with its error
/// and never stops or writes the others. Nothing is saved here.
pub fn load_tenants<E>(
    identities: impl IntoIterator<Item = String>,
    mut load: impl FnMut(&str) -> Result<GameState, E>,
) -> (BTreeMap<String, GameState>, Vec<(String, E)>) {
    let mut loaded = BTreeMap::new();
    let mut failed = Vec::new();
    for identity in identities {
        match load(&identity) {
            Ok(state) => {
                loaded.insert(identity, state);
            }
            Err(err) => failed.push((identity, err)),
        }
    }
    (loaded, failed)
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
    if sites.sites.contains(&coord) || sites.expedition == Some(coord) {
        return Occupancy::Site;
    }
    Occupancy::Empty
}

/// First `Empty` slot on `map`'s home walk, for the next tenant to join.
///
/// Deterministic from the seed and join order: each seated home fills the
/// earliest free slot, so the next place returns the one after it. This is a
/// read and reserves nothing. The host seats the result with [`seat_home`]
/// before the next place, under the same serialisation as other Actions. A
/// place computed from a stale map can return a coord that a parallel join
/// already took; [`seat_home`] refuses it and the host places again.
pub fn place_home(
    map: ShardMap,
    empires: &BTreeMap<String, GameState>,
    sites: ShardSites<'_>,
) -> Result<Coord, ShardError> {
    // No tenant has an empty identity, so every held coord reads as taken.
    map.home_walk()
        .find(|&coord| occupancy("", empires, sites, coord).join_slot())
        .ok_or(ShardError::NoEmptySlot)
}

/// Seat `state` as `identity`'s empire with its home on `coord`.
///
/// Join writes the home only here. A coord that is no longer `Empty` is
/// refused with [`ShardError::Occupied`] and nothing is written. A retried
/// seat whose home already landed on `coord` is a no-op.
///
/// Only `home` moves. A state built by `GameState::with_seed` rolled its home
/// planet profile and private pirate outposts around `Coord::HOME`; join
/// builds `state` for `coord` before seating it.
pub fn seat_home(
    identity: &str,
    mut state: GameState,
    coord: Coord,
    empires: &mut BTreeMap<String, GameState>,
    sites: ShardSites<'_>,
) -> Result<(), ShardError> {
    if identity.is_empty() {
        return Err(ShardError::EmptyIdentity);
    }
    if let Some(seated) = empires.get(identity) {
        if seated.home == coord {
            return Ok(());
        }
        return Err(ShardError::AlreadySeated {
            identity: identity.to_string(),
        });
    }
    let kind = occupancy(identity, empires, sites, coord);
    if !kind.join_slot() {
        return Err(ShardError::Occupied { coord, kind });
    }
    state.home = coord;
    empires.insert(identity.to_string(), state);
    Ok(())
}

/// `viewer`'s target list: coords its clearance can see, each with its kind.
///
/// Covers the viewer's home system up to the expedition point, every tenant
/// home and colony, and the host's outposts and sites. Reads the map as
/// given; [`ShardSession::view`] catches empires up first. The tenant view
/// shows the same coords with [`Occupancy::view_tag`].
pub fn targets(
    viewer: &str,
    empires: &BTreeMap<String, GameState>,
    sites: ShardSites<'_>,
) -> Vec<ShardTarget> {
    let Some(own) = empires.get(viewer) else {
        return Vec::new();
    };
    let home = own.home;
    let mut coords = BTreeSet::new();
    for position in 1..own.config.expedition.dest_position {
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
    coords.extend(sites.expedition);
    coords
        .into_iter()
        .filter(|coord| own.can_see(Actor::Player, ClassifiedKind::Fact, &coord.to_string()))
        .map(|coord| ShardTarget {
            coord,
            kind: occupancy(viewer, empires, sites, coord),
        })
        .collect()
}

/// Catch every empire up to `now` on the shared map, stopping at each
/// colony, recycler, and raid arrival (see [`ShardSession::settle`]) and at
/// each probe arrival at another tenant's planet, so the host reads that
/// planet as it was then. The regional book runs at each NPC tick and
/// tenant order expiry after every empire has stepped there.
///
/// One long gap and many short ones read the same numbers. A lone
/// catch-up (a `submit`) can run the target past a probe arrival: the
/// report is then filled from the target as it is now and stamped with the
/// target's time, never back-dated to the arrival. A raid on a target that
/// ran ahead fights it as it is now in the same way. A lone catch-up never
/// runs an empire past its own colony, recycler, or raid arrival; it is
/// refused as [`ShardError::Unsettled`].
fn catch_up_empires(
    empires: &mut BTreeMap<String, GameState>,
    commons: &mut ShardCommons,
    market: &MarketConfig,
    now: u64,
) {
    let next = |empires: &BTreeMap<String, GameState>, commons: &ShardCommons| {
        let stops = [
            next_stop(empires, now),
            commons.market.next_due(now, market),
        ];
        stops.into_iter().flatten().min()
    };
    while let Some(at) = next(empires, commons) {
        resolve_raids(empires, commons, at);
        step_empires(empires, commons, at);
        commons.market.advance(at, empires, market);
        fill_tenant_spies(empires);
    }
    step_empires(empires, commons, now);
    commons.market.advance(now, empires, market);
    fill_tenant_spies(empires);
}

/// Step each empire to `at` in identity order on the shared map.
fn step_empires(empires: &mut BTreeMap<String, GameState>, commons: &mut ShardCommons, at: u64) {
    let identities: Vec<String> = empires.keys().cloned().collect();
    for identity in &identities {
        step_one(identity, empires, commons, at);
    }
}

/// Step `identity` to `at` on the shared map.
///
/// First its colony ships arriving by `at` at a coord another tenant, an
/// outpost, or a site holds turn back, so a tenant earlier in order that
/// founds at the same instant wins the coord. The shared debris is lent to
/// the tenant for its step and taken back with its harvests and new
/// wreckage, so the next tenant sees what is left.
fn step_one(
    identity: &str,
    empires: &mut BTreeMap<String, GameState>,
    commons: &mut ShardCommons,
    at: u64,
) {
    let taken: Vec<u64> = empires
        .get(identity)
        .map(|state| {
            state
                .missions
                .iter()
                .filter(|mission| mission.colonize && due_by(state, mission, at))
                .filter(|mission| {
                    occupancy(identity, empires, commons.sites(), mission.dest).blocks_planet_dest()
                })
                .map(|mission| mission.id)
                .collect()
        })
        .unwrap_or_default();
    let Some(state) = empires.get_mut(identity) else {
        return;
    };
    for id in taken {
        state.turn_back_colony(id, at);
    }
    with_shared_debris(&mut commons.debris, state, |state| state.catch_up(at));
}

/// Fight every raid arriving at `at`, before its attacker steps there.
///
/// The target steps to `at` first, ahead of identity order, so the fight
/// reads its home as it is at the arrival. The fight draws from the
/// target's RNG, which is at the arrival on every path, while the attacker
/// may still be at an earlier stop. Its own step then only turns the
/// survivors home with the plunder. A raid at a coord no tenant holds as
/// home any more flies home with nothing.
fn resolve_raids(empires: &mut BTreeMap<String, GameState>, commons: &mut ShardCommons, at: u64) {
    let raids: Vec<(String, u64, Coord)> = empires
        .iter()
        .flat_map(|(id, state)| {
            state
                .missions
                .iter()
                .filter(|mission| tenant_raid(state, mission) && due_by(state, mission, at))
                .map(|mission| (id.clone(), mission.id, mission.dest))
        })
        .collect();
    for (attacker_id, mission_id, dest) in raids {
        let Some(defender_id) = empires
            .iter()
            .find(|(id, state)| **id != attacker_id && state.home == dest)
            .map(|(id, _)| id.clone())
        else {
            continue;
        };
        step_one(&defender_id, empires, commons, at);
        let Some(mut defender) = empires.remove(&defender_id) else {
            continue;
        };
        if let Some(attacker) = empires.get_mut(&attacker_id) {
            fight_raid(attacker, mission_id, &mut defender, &mut commons.debris, at);
        }
        empires.insert(defender_id, defender);
    }
}

/// Resolve raid mission `mission_id` against `defender`'s parked home fleet
/// with the same fleet combat as local play.
///
/// A won fight carries off the defender's plunder share then cap, no more
/// than the survivors' holds take. The defender logs the fight as a raid on
/// home with that plunder and takes the debris at home on the shared map.
fn fight_raid(
    attacker: &mut GameState,
    mission_id: u64,
    defender: &mut GameState,
    debris: &mut BTreeMap<Coord, f64>,
    at: u64,
) {
    let Some(ships) = attacker
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
        .map(|mission| mission.ships)
    else {
        return;
    };
    let fought_at = defender.last_tick_unix;
    let mut rng = defender.rng_seed;
    let report = resolve_combat(
        ships,
        defender.fleet,
        &stats_from_catalog(&attacker.config.ships),
        &stats_from_catalog(&defender.config.ships),
        attacker.config.raid.max_rounds,
        &rapid_fire_table(&attacker.config.ships),
        &mut rng,
    );
    defender.rng_seed = rng;
    defender.fleet = report.defender_after;
    let plunder = if report.winner == CombatWinner::Attacker {
        fill_hold(
            plunder_from(
                defender.plunderable(),
                defender.config.raid.plunder_share,
                defender.config.raid.plunder_cap,
            ),
            attacker.cargo_capacity(report.attacker_after),
        )
    } else {
        Resources::ZERO
    };
    defender.resources = defender.resources.saturating_sub(plunder);
    defender.push_combat_log(fought_at, report.clone());
    if let Some(log) = defender.combat_logs.last_mut() {
        log.raid_plunder = Some(plunder);
    }
    with_shared_debris(debris, defender, |defender| {
        defender.note_fight_debris(fought_at, defender.home, &report);
    });
    attacker.land_tenant_raid(mission_id, report, plunder, at);
}

/// Run `f` on `state` with the shared debris stock in its debris map, then
/// take the whole map back, harvests and new wreckage included. Moon rolls
/// and recyclers inside `f` see the full pile at a coord.
fn with_shared_debris<T>(
    shared: &mut BTreeMap<Coord, f64>,
    state: &mut GameState,
    f: impl FnOnce(&mut GameState) -> T,
) -> T {
    for (coord, amount) in std::mem::take(shared) {
        *state.debris.entry(coord).or_insert(0.0) += amount;
    }
    let out = f(state);
    *shared = std::mem::take(&mut state.debris);
    out
}

/// Earliest stop due by `now` and not yet reached: a colony, recycler, or
/// raid Action, or a probe Action at a planet its owner does not hold.
fn next_stop(empires: &BTreeMap<String, GameState>, now: u64) -> Option<u64> {
    empires
        .values()
        .flat_map(|state| {
            state
                .missions
                .iter()
                .filter(|mission| {
                    mission.colonize
                        || mission.recycle
                        || tenant_raid(state, mission)
                        || (mission.spy && aims_at_tenant(state, mission))
                })
                .filter_map(action_at)
                .filter(|&at| at > state.last_tick_unix && at <= now)
        })
        .min()
}

/// Earliest arrival of each raid still outbound, by target home.
fn inbound_raids(empires: &BTreeMap<String, GameState>) -> BTreeMap<Coord, u64> {
    let mut inbound = BTreeMap::new();
    for state in empires.values() {
        for mission in state.missions.iter().filter(|m| tenant_raid(state, m)) {
            if let Some(at) = action_at(mission) {
                inbound
                    .entry(mission.dest)
                    .and_modify(|earliest: &mut u64| *earliest = (*earliest).min(at))
                    .or_insert(at);
            }
        }
    }
    inbound
}

/// Whether `state` has a colony, recycler, or raid Action due by `now` that
/// its own catch-up would run without the shard map.
fn race_due(state: &GameState, now: u64) -> bool {
    state.missions.iter().any(|mission| {
        (mission.colonize || mission.recycle || tenant_raid(state, mission))
            && due_by(state, mission, now)
    })
}

/// Whether this tenant has already won or would declare during a lone catch-up.
fn finish_due(state: &GameState, now: u64) -> bool {
    if state.victory.is_some() {
        return true;
    }
    if !state.config.victory.enabled {
        return false;
    }
    state.next_victory_at().is_some_and(|at| at <= now)
        || victory_wonder_at(state).is_some_and(|at| at <= now)
}

/// Last-stage timer of the victory wonder, if that stage is already paid.
fn victory_wonder_at(state: &GameState) -> Option<u64> {
    let job = state.wonder.as_ref()?;
    if job.id != state.config.victory.wonder_id {
        return None;
    }
    let spec = state.config.wonders.iter().find(|spec| spec.id == job.id)?;
    if job.stage + 1 < spec.stages.len() {
        return None;
    }
    job.completes_at.filter(|&at| at > state.last_tick_unix)
}

/// `mission` flies at a coord that is neither an outpost of `state` nor its
/// own planet, so only another tenant can be there.
fn aims_at_tenant(state: &GameState, mission: &Mission) -> bool {
    state.outpost(mission.dest).is_none() && !tenant_holds(state, mission.dest)
}

/// A raid from [`ShardSession::launch_raid`]: local attacks only launch at
/// outposts, and an outpost leaves the map only as the attacker's colony.
fn tenant_raid(state: &GameState, mission: &Mission) -> bool {
    mission.attack && aims_at_tenant(state, mission)
}

/// `mission`'s Action is after `state`'s clock and no later than `at`.
fn due_by(state: &GameState, mission: &Mission, at: u64) -> bool {
    action_at(mission).is_some_and(|t| t > state.last_tick_unix && t <= at)
}

/// When `mission`'s Action ends: founding, harvest, or a probe read.
fn action_at(mission: &Mission) -> Option<u64> {
    match mission.phase {
        MissionPhase::Outbound => Some(mission.phase_ends_at.saturating_add(mission.hold_secs)),
        MissionPhase::Action => Some(mission.phase_ends_at),
        MissionPhase::Return => None,
    }
}

/// Fill every pending probe report from the tenant that holds its target,
/// once that tenant has reached the arrival.
fn fill_tenant_spies(empires: &mut BTreeMap<String, GameState>) {
    let mut fills = Vec::new();
    for (id, state) in empires.iter() {
        for report in state.spy_reports.iter().filter(|report| report.pending()) {
            let target =
                empires
                    .iter()
                    .filter(|(other, _)| *other != id)
                    .find_map(|(_, target)| {
                        tenant_planet(target, report.target)
                            .map(|planet| (planet, target.last_tick_unix))
                    });
            let (planet, seen_at) = match target {
                // Behind the arrival: wait for a later stop, at the latest `now`.
                Some((_, seen_at)) if seen_at < report.observed_at => continue,
                Some((planet, seen_at)) => (Some(planet), seen_at),
                None => (None, report.observed_at),
            };
            fills.push((id.clone(), report.id, planet, seen_at));
        }
    }
    for (id, report, planet, seen_at) in fills {
        if let Some(state) = empires.get_mut(&id) {
            state.fill_tenant_spy(report, planet, seen_at);
        }
    }
}

/// Stock and parked fleet on `state`'s home or colony at `coord`.
fn tenant_planet(state: &GameState, coord: Coord) -> Option<(Resources, Fleet)> {
    if state.home == coord {
        return Some((state.resources, state.fleet));
    }
    state
        .colonies
        .iter()
        .find(|colony| colony.coord == coord)
        .map(|colony| (colony.resources, colony.fleet))
}

fn tenant_holds(state: &GameState, coord: Coord) -> bool {
    state.home == coord || state.colonies.iter().any(|colony| colony.coord == coord)
}

fn shard_view(
    identity: &str,
    empires: &BTreeMap<String, GameState>,
    sites: ShardSites<'_>,
    epoch: u64,
    factor: u32,
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
    let inbound = inbound_at_home(own, identity, empires);
    let decision =
        expiring_decision_with_inbound(own, inbound.first().map(|inbound| inbound.arrives_at));
    Some(ShardView {
        identity: identity.to_string(),
        at: own.last_tick_unix,
        epoch,
        factor,
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
        raids: own
            .combat_logs
            .iter()
            .filter_map(|log| {
                Some(ShardRaidAftermath {
                    at: log.at,
                    lost: log.raid_losses()?.ships_lost,
                    plunder: log.raid_plunder?,
                    debris: own.fight_debris(&log.report),
                })
            })
            .collect(),
        inbound,
        decision,
        standing: None,
        scoreboard: None,
    })
}

/// Other tenants' raids that still land on `own`'s home after its clock,
/// earliest first, each with the hull count `own`'s phalanx reads.
fn inbound_at_home(
    own: &GameState,
    identity: &str,
    empires: &BTreeMap<String, GameState>,
) -> Vec<ShardInbound> {
    let mut inbound: Vec<ShardInbound> = empires
        .iter()
        .filter(|(id, _)| id.as_str() != identity)
        .flat_map(|(_, attacker)| {
            attacker
                .missions
                .iter()
                .filter(|mission| mission.dest == own.home && tenant_raid(attacker, mission))
                .filter_map(|mission| {
                    let arrives_at = action_at(mission).filter(|&at| at > own.last_tick_unix)?;
                    Some(ShardInbound {
                        dest: mission.dest,
                        arrives_at,
                        ships: own.phalanx_foreign_ships(mission, arrives_at),
                    })
                })
        })
        .collect();
    inbound.sort_by_key(|inbound| inbound.arrives_at);
    inbound
}

fn occupancy_slots(
    viewer: &str,
    empires: &BTreeMap<String, GameState>,
    sites: ShardSites<'_>,
) -> Vec<ShardSlot> {
    targets(viewer, empires, sites)
        .into_iter()
        .map(|target| ShardSlot {
            coord: target.coord,
            tag: target.kind.view_tag().to_string(),
        })
        .collect()
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

fn scale_pulse(pulse: ActivityPulse, epoch: u64, factor: u32) -> ActivityPulse {
    let ts = scale_unix(pulse.ts(), epoch, factor);
    match pulse {
        ActivityPulse::PrMerged { .. } => ActivityPulse::PrMerged { ts },
        ActivityPulse::WorkflowFinished { success, .. } => {
            ActivityPulse::WorkflowFinished { ts, success }
        }
        ActivityPulse::AgentFinished { duration_secs, .. } => {
            ActivityPulse::AgentFinished { ts, duration_secs }
        }
        ActivityPulse::AgentWaiting { waiting_secs, .. } => {
            ActivityPulse::AgentWaiting { ts, waiting_secs }
        }
    }
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
    use crate::decision::{DecisionAction, DecisionId};
    use crate::espionage::SpyReport;
    use crate::fleets::{Coord, Mission, MissionPhase};
    use crate::intel::{IntelValue, light_delay_secs};
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
    fn scaled_bridge_pulse_applies_at_game_time() {
        let mut shard = scaled_session(1_000, 1_000, 10);
        let mut state = test_state(1_000);
        shard.clock.advance(10);
        shard
            .submit(&mut state, build_cmd("tenant-a", "catch", None))
            .unwrap();
        assert_eq!(state.last_tick_unix, 1_100);
        let iron = state.resources.iron;
        let event = shard.sign_bridge(ActivityPulse::PrMerged { ts: 1_010 });
        let result = shard.apply_bridge(&mut state, "tenant-a", &event).unwrap();
        assert_eq!(result, ActivityApply::Applied);
        assert!(state.resources.iron > iron);
    }

    #[test]
    fn scaled_bridge_pulse_before_epoch_is_stale() {
        let mut shard = scaled_session(1_000, 1_000, 10);
        let mut state = test_state(1_000);
        let iron = state.resources.iron;
        let event = shard.sign_bridge(ActivityPulse::PrMerged { ts: 990 });
        let result = shard.apply_bridge(&mut state, "tenant-a", &event).unwrap();
        assert_eq!(result, ActivityApply::Skipped);
        assert_eq!(state.resources.iron, iron);
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
    fn shipyard_intent_applies_once_at_the_host_clock() {
        let mut shard = session(1_000);
        let mut state = test_state(0);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        let command = ShardCommand {
            identity: "tenant-a".into(),
            idempotency_id: "fighters".into(),
            intent: Intent::QueueShips {
                planet: state.home,
                kind: ShipKind::LightFighter,
                count: 3,
            },
            client_now_unix: Some(5_000),
        };
        shard.submit(&mut state, command.clone()).unwrap();
        shard.submit(&mut state, command).unwrap();
        assert_eq!(state.ship_queue.len(), 1);
        assert_eq!(state.ship_queue[0].remaining, 3);
        assert_eq!(state.last_tick_unix, 1_000);
    }

    #[test]
    fn restored_bookkeeping_keeps_a_retry_from_applying_twice() {
        let mut before_restart = session(1_000);
        let mut state = test_state(1_000);
        before_restart
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        let saved = before_restart.export_bookkeeping().unwrap();
        let iron = state.resources.iron;

        let mut after_restart = session(1_030);
        after_restart.restore_bookkeeping(&saved).unwrap();
        after_restart
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        assert_eq!(state.build_queue.len(), 1);
        assert!(state.resources.iron >= iron, "no second build was paid");
    }

    #[test]
    fn restored_bookkeeping_keeps_the_clock_identity() {
        let mut scaled = scaled_session(1_000, 1_000, 10);
        let mut state = test_state(1_000);
        scaled
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        let saved = scaled.export_bookkeeping().unwrap();
        let scaled_doc: serde_json::Value = serde_json::from_slice(saved.as_bytes()).unwrap();
        assert_eq!(scaled_doc["version"], 2, "older hosts refuse scaled bytes");
        let mut same = scaled_session(1_030, 1_000, 10);
        same.restore_bookkeeping(&saved).unwrap();
        same.submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        assert_eq!(state.build_queue.len(), 1);
        let err = session(1_030).restore_bookkeeping(&saved).unwrap_err();
        assert!(
            matches!(err, ShardError::Bookkeeping(ref msg) if msg.contains("clock identity")),
            "{err}"
        );
        let legacy = ShardBookkeeping::from_bytes(
            br#"{"version":1,"seen":[],"windows":{},"attacks":{}}"#.to_vec(),
        );
        let mut ordinary = session(1_000);
        ordinary.restore_bookkeeping(&legacy).unwrap();
        let factor_one = ordinary.export_bookkeeping().unwrap();
        let factor_one_doc: serde_json::Value =
            serde_json::from_slice(factor_one.as_bytes()).unwrap();
        assert_eq!(factor_one_doc["version"], 1);
        let err = scaled_session(1_000, 1_000, 10)
            .restore_bookkeeping(&legacy)
            .unwrap_err();
        assert!(
            matches!(err, ShardError::Bookkeeping(ref msg) if msg.contains("clock identity")),
            "{err}"
        );
        let unknown = ShardBookkeeping::from_bytes(
            br#"{"version":4,"seen":[],"windows":{},"attacks":{}}"#.to_vec(),
        );
        let err = session(1_000).restore_bookkeeping(&unknown).unwrap_err();
        assert!(
            matches!(err, ShardError::Bookkeeping(ref msg) if msg.contains("version 4")),
            "{err}"
        );
    }

    #[test]
    fn an_id_older_than_the_ttl_is_forgotten_and_applies_again() {
        let ttl = ShardConfig::default().seen_ttl_secs();
        assert!(ttl >= ShardConfig::default().command_window_secs);
        assert!(ttl >= ShardConfig::default().attack_window_secs);
        let zero = ShardConfig {
            command_window_secs: 0,
            attack_window_secs: 0,
            ..ShardConfig::default()
        };
        assert_eq!(zero.seen_ttl_secs(), 1, "zero windows keep dedup on");
        let mut first = session(1_000);
        let mut state = test_state(1_000);
        state.resources.iron = 1_000_000.0;
        state.resources.silicon = 1_000_000.0;
        first
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        let saved = first.export_bookkeeping().unwrap();

        let mut later = session(1_000 + ttl);
        later.restore_bookkeeping(&saved).unwrap();
        state.catch_up(1_000 + ttl);
        assert!(state.build_queue.is_empty(), "the first build finished");
        later
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        assert_eq!(state.build_queue.len(), 1, "the forgotten id applied again");
    }

    #[test]
    fn restored_windows_still_rate_limit() {
        let mut first = tight_session(1_000, 2);
        let mut state = test_state(1_000);
        for id in ["a", "b"] {
            first
                .submit(&mut state, build_cmd("tenant-a", id, None))
                .unwrap();
        }
        let saved = first.export_bookkeeping().unwrap();

        let mut restarted = tight_session(1_010, 2);
        restarted.restore_bookkeeping(&saved).unwrap();
        let err = restarted
            .submit(&mut state, build_cmd("tenant-a", "c", None))
            .unwrap_err();
        assert!(matches!(err, ShardError::RateLimited { .. }), "{err:?}");
    }

    #[test]
    fn unreadable_bookkeeping_leaves_the_session_untouched() {
        let mut shard = session(1_000);
        let mut state = test_state(1_000);
        shard
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        for bad in [
            b"not json".to_vec(),
            br#"{"version":4,"seen":[],"windows":{},"attacks":{}}"#.to_vec(),
        ] {
            let err = shard
                .restore_bookkeeping(&ShardBookkeeping::from_bytes(bad))
                .unwrap_err();
            assert!(matches!(err, ShardError::Bookkeeping(_)), "{err:?}");
        }
        shard
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        assert_eq!(state.build_queue.len(), 1);
    }

    #[test]
    fn bookkeeping_stays_out_of_debug_and_the_tenant_view() {
        let mut shard = session(1_000);
        let mut empires = two_empires(tenant_a(1_000), tenant_b(1_000));
        let state = empires.get_mut("tenant-a").unwrap();
        shard
            .submit(state, build_cmd("tenant-a", "secret-retry-id", None))
            .unwrap();
        let saved = shard.export_bookkeeping().unwrap();
        let debug = format!("{saved:?}");
        assert!(!debug.contains("tenant-a"), "{debug}");
        assert!(!debug.contains("secret-retry-id"), "{debug}");
        let json = shard
            .view("tenant-a", &mut empires)
            .unwrap()
            .to_json()
            .to_string();
        assert!(!json.contains("seen"), "{json}");
        assert!(!json.contains("secret-retry-id"), "{json}");
    }

    #[test]
    fn a_tenant_that_fails_to_load_does_not_stop_the_others() {
        let ids = ["tenant-a", "broken", "tenant-b"].map(String::from);
        let mut calls = Vec::new();
        let (loaded, failed) = load_tenants(ids, |id| {
            calls.push(id.to_string());
            match id {
                "broken" => Err("schema too new"),
                _ => Ok(test_state(1_000)),
            }
        });
        assert_eq!(calls, ["tenant-a", "broken", "tenant-b"]);
        assert_eq!(
            loaded.keys().map(String::as_str).collect::<Vec<_>>(),
            ["tenant-a", "tenant-b"]
        );
        assert_eq!(failed, [("broken".to_string(), "schema too new")]);
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
            recycle: false,
            deploy: false,
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
        let mut shard = session(1_000);
        let mut a = tenant_a(1_000);
        a.spy_reports.push(spy_on_b(1_000, 1_000));
        let mut empires = two_empires(a, tenant_b(1_000));
        let view = shard.view("tenant-a", &mut empires).unwrap();
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
        let mut shard = session(1_000);
        let mut a = tenant_a(1_000);
        a.spy_reports.push(spy_on_b(1_000, 1_000 + 50_000));
        let mut empires = two_empires(a, tenant_b(1_000));
        let view = shard.view("tenant-a", &mut empires).unwrap();
        assert!(
            view.intel
                .iter()
                .all(|fact| fact.object != B_HOME.to_string())
        );
    }

    #[test]
    fn tenant_view_catch_up_does_not_leak_hidden_fields() {
        let mut shard = session(1_000);
        let a = tenant_a(1_000);
        let b = tenant_b(1_000);
        shard.clock.advance(3_600);
        let mut empires = two_empires(a, b);
        let view = shard.view("tenant-a", &mut empires).unwrap();
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
        let mut shard = session(1_000);
        let mut empires = two_empires(tenant_a(1_000), tenant_b(1_000));
        assert_eq!(
            shard.view("", &mut empires).unwrap_err(),
            ShardError::EmptyIdentity
        );
        assert_eq!(
            shard.view("tenant-z", &mut empires).unwrap_err(),
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

    /// Puts `pirate_coord` and `site_coord` on `shard`'s map.
    fn place_sites(shard: &mut ShardSession<FakeClock>) {
        let commons = shard.commons_mut();
        commons.outposts = vec![pirate_coord()];
        commons.sites = vec![site_coord()];
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
            ship_queue: Vec::new(),
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
                    sites: &sites,
                    expedition: None,
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
                    sites: &sites,
                    expedition: None,
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
                    sites: &sites,
                    expedition: None,
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
                    sites: &sites,
                    expedition: None,
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
                    sites: &sites,
                    expedition: None,
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
                    sites: &sites,
                    expedition: None,
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
                    expedition: None,
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
                    expedition: None,
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
                    expedition: None,
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
                expedition: None,
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
                expedition: None,
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
        place_sites(&mut shard);
        let refused = [
            (B_HOME, "b-home", Occupancy::Tenant),
            (pirate_coord(), "outpost", Occupancy::Outpost),
            (site_coord(), "site", Occupancy::Site),
        ];
        for (dest, id, kind) in refused {
            let err = shard
                .colonize("tenant-a", &mut empires, dest, 100, id)
                .unwrap_err();
            assert_eq!(err, ShardError::Occupied { coord: dest, kind });
            assert_eq!(empires["tenant-a"].fleet.get(ShipKind::ColonyShip), 1);
            assert!(empires["tenant-a"].missions.is_empty());
        }
        shard
            .colonize("tenant-a", &mut empires, empty_slot(), 100, "empty")
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
                        origin: None,
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
    fn tenant_view_json_tags_occupancy_without_the_other_identity() {
        let mut shard = session(1_000);
        place_sites(&mut shard);
        let mut empires = two_empires(tenant_a(1_000), tenant_b(1_000));
        let view = shard.view("tenant-a", &mut empires).unwrap();
        let json = view.to_json();
        let text = serde_json::to_string(&json).unwrap();
        assert_eq!(json["epoch"], 0);
        assert_eq!(json["factor"], 1);
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

    /// One galaxy, four systems, three positions. Seed 7 starts the walk at
    /// system 4 (7 % 4 = 3) and position 2 ((7 / 4) % 3 = 1).
    const WALK: ShardMap = ShardMap {
        seed: 7,
        galaxies: 1,
        systems: 4,
        positions: 3,
    };

    fn walk_slot(system: u16, position: u16) -> Coord {
        Coord {
            galaxy: 1,
            system,
            position,
        }
    }

    #[test]
    fn place_home_walks_empty_slots_in_join_order() {
        let outposts = [walk_slot(1, 2)];
        let sites = ShardSites {
            outposts: &outposts,
            sites: &[],
            expedition: None,
        };
        let mut empires = BTreeMap::new();
        let first = place_home(WALK, &empires, sites).unwrap();
        assert_eq!(first, walk_slot(4, 2));
        seat_home("tenant-a", test_state(1_000), first, &mut empires, sites).unwrap();
        let second = place_home(WALK, &empires, sites).unwrap();
        assert_eq!(second, walk_slot(2, 2), "skips the outpost at 1:1:2");
        assert_eq!(
            occupancy("tenant-b", &empires, sites, second),
            Occupancy::Empty
        );
        seat_home("tenant-b", test_state(1_000), second, &mut empires, sites).unwrap();
        assert_eq!(empires["tenant-a"].home, first);
        assert_eq!(empires["tenant-b"].home, second);
    }

    #[test]
    fn seat_home_refuses_a_parallel_place_that_lost_the_slot() {
        let sites = ShardSites::none();
        let mut empires = BTreeMap::new();
        let a_slot = place_home(WALK, &empires, sites).unwrap();
        let b_slot = place_home(WALK, &empires, sites).unwrap();
        assert_eq!(a_slot, b_slot, "both places read the same map");
        seat_home("tenant-a", test_state(1_000), a_slot, &mut empires, sites).unwrap();
        assert_eq!(
            seat_home("tenant-b", test_state(1_000), b_slot, &mut empires, sites).unwrap_err(),
            ShardError::Occupied {
                coord: a_slot,
                kind: Occupancy::Tenant
            }
        );
        assert!(!empires.contains_key("tenant-b"));
        seat_home("tenant-a", test_state(1_000), a_slot, &mut empires, sites).unwrap();
        assert_eq!(empires.len(), 1, "a retried seat is a no-op");
        let retry = place_home(WALK, &empires, sites).unwrap();
        assert_ne!(retry, a_slot);
        seat_home("tenant-b", test_state(1_000), retry, &mut empires, sites).unwrap();
        assert_eq!(empires["tenant-b"].home, retry);
    }

    #[test]
    fn place_home_refuses_a_full_map() {
        let tiny = ShardMap {
            seed: 0,
            galaxies: 1,
            systems: 1,
            positions: 1,
        };
        let mut empires = BTreeMap::new();
        let slot = place_home(tiny, &empires, ShardSites::none()).unwrap();
        seat_home(
            "tenant-a",
            test_state(1_000),
            slot,
            &mut empires,
            ShardSites::none(),
        )
        .unwrap();
        assert_eq!(
            place_home(tiny, &empires, ShardSites::none()).unwrap_err(),
            ShardError::NoEmptySlot
        );
    }

    #[test]
    fn join_past_the_tenant_cap_is_refused_and_a_seated_tenant_resumes() {
        let mut shard = ShardSession::new(
            FakeClock::new(1_000),
            KEY,
            ShardConfig {
                tenant_cap: 1,
                ..ShardConfig::default()
            },
        );
        let mut empires = BTreeMap::new();
        let home = shard
            .join("tenant-a", WALK, &mut empires, |_| test_state(1_000))
            .unwrap();
        assert_eq!(
            home,
            place_home(WALK, &BTreeMap::new(), ShardSites::none()).unwrap()
        );
        assert_eq!(empires["tenant-a"].home, home);
        let before = empires.clone();
        assert_eq!(
            shard.join("tenant-b", WALK, &mut empires, |_| unreachable!(
                "refused before build"
            )),
            Err(ShardError::ShardFull { cap: 1 })
        );
        assert_eq!(empires, before, "a refused join writes nothing");
        assert_eq!(
            shard.join("tenant-a", WALK, &mut empires, |_| unreachable!(
                "resume builds nothing"
            )),
            Ok(home)
        );
        assert_eq!(empires, before);
    }

    #[test]
    fn target_list_tags_kinds_without_the_other_identity() {
        let empires = two_empires(tenant_a(1_000), tenant_b(1_000));
        let outposts = [pirate_coord()];
        let sites = [site_coord()];
        let list = targets(
            "tenant-a",
            &empires,
            ShardSites {
                outposts: &outposts,
                sites: &sites,
                expedition: None,
            },
        );
        let kind_at = |coord: Coord| {
            list.iter()
                .find(|target| target.coord == coord)
                .map(|target| target.kind)
        };
        assert_eq!(kind_at(Coord::HOME), Some(Occupancy::You));
        assert_eq!(kind_at(B_HOME), Some(Occupancy::Tenant));
        assert_eq!(kind_at(pirate_coord()), Some(Occupancy::Outpost));
        assert_eq!(kind_at(site_coord()), Some(Occupancy::Site));
        assert_eq!(kind_at(empty_slot()), Some(Occupancy::Empty));
        let dump = format!("{list:?}");
        assert!(!dump.contains("tenant-b"), "{dump}");
        assert!(!dump.contains(&format!("{B_IRON}")), "{dump}");
    }

    const SPY_START: u64 = 1_000;

    fn spy_empires() -> BTreeMap<String, GameState> {
        let mut a = test_state(SPY_START);
        a.resources.deuterium = 10_000.0;
        a.fleet.add(ShipKind::EspionageProbe, 4);
        a.config.espionage.error_base = 0.3;
        two_empires(a, tenant_b(SPY_START))
    }

    fn spy_cmd(dest: Coord, id: &str) -> ShardSpy {
        let mut ships = Fleet::default();
        ships.add(ShipKind::EspionageProbe, 4);
        ShardSpy {
            identity: "tenant-a".into(),
            idempotency_id: id.into(),
            ships,
            dest,
            speed_percent: 100,
        }
    }

    fn intel_value<'a>(view: &'a ShardView, field: &str) -> &'a IntelValue {
        &view
            .intel
            .iter()
            .find(|fact| fact.object == B_HOME.to_string() && fact.field == field)
            .unwrap_or_else(|| panic!("no {field} intel"))
            .observations[0]
            .value
    }

    #[test]
    fn tenant_spy_reads_noisy_stock_and_fleet_after_light_delay() {
        let mut shard = session(SPY_START);
        let mut empires = spy_empires();
        shard.spy(&mut empires, spy_cmd(B_HOME, "spy")).unwrap();
        let mission = &empires["tenant-a"].missions[0];
        assert!(mission.spy);
        assert_eq!(mission.hold_secs, 0);
        let arrive = mission.phase_ends_at;
        let delay = light_delay_secs(Coord::HOME, B_HOME, &empires["tenant-a"].config.orbits);
        assert!(delay > 0);

        shard.clock.set(arrive + delay - 1);
        let early = shard.view("tenant-a", &mut empires).unwrap();
        assert!(
            early
                .intel
                .iter()
                .all(|fact| fact.object != B_HOME.to_string())
        );
        let report = &empires["tenant-a"].spy_reports[0];
        assert_eq!(report.observed_at, arrive);
        assert_eq!(report.arrives_at, arrive + delay);
        assert_eq!(
            empires["tenant-a"].marking_of(ClassifiedKind::Report, &report.id.to_string()),
            crate::Marking::new(
                crate::Classification::Restricted,
                [crate::classification::COMPARTMENT_INTEL.to_string()],
            )
        );

        shard.clock.advance(1);
        let view = shard.view("tenant-a", &mut empires).unwrap();
        let mut truth = tenant_b(SPY_START);
        truth.catch_up(arrive);
        let IntelValue::Number(iron) = *intel_value(&view, "resources.iron") else {
            panic!("iron is a number");
        };
        assert!(iron > 0.0 && iron != truth.resources.iron);
        let IntelValue::Count(battleships) = *intel_value(&view, "fleet.battleship") else {
            panic!("battleships are a count");
        };
        assert!(battleships > 0 && battleships != B_BATTLESHIPS);
        dump_hides(&view, "rng_seed");
        dump_hides(&view, &B_RNG.to_string());
        dump_hides(&view, "next_raid");
        dump_hides(&view, &B_RAID_AT.to_string());
        assert!(view.planets.iter().all(|planet| planet.coord != B_HOME));
        assert_eq!(view.fleet.get(ShipKind::Battleship), 0);
    }

    /// A's spy reports and RNG after viewing at each of `steps`.
    fn spy_run(steps: &[u64]) -> (Vec<SpyReport>, u64) {
        let mut shard = session(SPY_START);
        let mut empires = spy_empires();
        shard.spy(&mut empires, spy_cmd(B_HOME, "spy")).unwrap();
        for &at in steps {
            shard.clock.set(at);
            shard.view("tenant-a", &mut empires).unwrap();
        }
        let a = &empires["tenant-a"];
        (a.spy_reports.clone(), a.rng_seed)
    }

    #[test]
    fn tenant_spy_repeats_by_seed_and_catch_up_matches_stepped_views() {
        let end = SPY_START + 48 * 3_600;
        let (once, rng) = spy_run(&[end]);
        assert_eq!(once.len(), 1);
        assert!(once[0].delivered(end));
        assert_eq!((once.clone(), rng), spy_run(&[end]));
        let steps: Vec<u64> = (SPY_START + 7..end).step_by(397).chain([end]).collect();
        let (stepped, stepped_rng) = spy_run(&steps);
        assert_eq!(stepped_rng, rng);
        // Production splits float sums at each step; stock may drift in the last bits.
        let (mut a, mut b) = (once[0].clone(), stepped[0].clone());
        let (stock_a, stock_b) = (a.resources.take(), b.resources.take());
        assert_eq!(a, b);
        let (stock_a, stock_b) = (stock_a.unwrap(), stock_b.unwrap());
        for kind in crate::ResourceKind::ALL {
            assert!((stock_a.get(kind) - stock_b.get(kind)).abs() < 1e-6);
        }
    }

    #[test]
    fn tenant_spy_on_a_target_that_ran_ahead_is_stamped_with_its_time() {
        let mut shard = session(SPY_START);
        let mut empires = spy_empires();
        shard.spy(&mut empires, spy_cmd(B_HOME, "spy")).unwrap();
        let arrive = empires["tenant-a"].missions[0].phase_ends_at;
        let ahead = arrive + 600;
        empires.get_mut("tenant-b").unwrap().catch_up(ahead);
        shard.clock.set(ahead + 60);
        shard.view("tenant-a", &mut empires).unwrap();
        let report = &empires["tenant-a"].spy_reports[0];
        assert_eq!(report.created_at, arrive);
        assert_eq!(report.observed_at, ahead);
        let delay = light_delay_secs(Coord::HOME, B_HOME, &empires["tenant-a"].config.orbits);
        assert_eq!(report.arrives_at, ahead + delay);
    }

    #[test]
    fn spy_refuses_own_home_empty_and_site_and_keeps_the_outpost_path() {
        let mut shard = session(SPY_START);
        let mut empires = spy_empires();
        place_sites(&mut shard);
        let refused = [
            (Coord::HOME, ShardError::SameEmpire),
            (
                empty_slot(),
                ShardError::NothingToSpy {
                    coord: empty_slot(),
                    kind: Occupancy::Empty,
                },
            ),
            (
                site_coord(),
                ShardError::NothingToSpy {
                    coord: site_coord(),
                    kind: Occupancy::Site,
                },
            ),
            (
                pirate_coord(),
                ShardError::Intent(IntentError::Fleet(crate::FleetError::UnknownOutpost)),
            ),
        ];
        for (dest, err) in refused {
            let id = dest.to_string();
            assert_eq!(shard.spy(&mut empires, spy_cmd(dest, &id)), Err(err));
            assert!(empires["tenant-a"].missions.is_empty());
            assert_eq!(empires["tenant-a"].fleet.get(ShipKind::EspionageProbe), 4);
        }
        let a = empires.get_mut("tenant-a").unwrap();
        assert_eq!(
            a.launch_spy(
                a.home,
                spy_cmd(B_HOME, "local").ships,
                B_HOME,
                100,
                SPY_START
            ),
            Err(crate::FleetError::UnknownOutpost)
        );
    }

    /// Tenant-a at home and tenant-b at `B_HOME`, each with fuel, `ships`, and
    /// room for one colony.
    fn race_empires(ships: Fleet) -> BTreeMap<String, GameState> {
        let mut a = test_state(1_000);
        let mut b = test_state(1_000);
        b.home = B_HOME;
        for state in [&mut a, &mut b] {
            state.resources.deuterium = 100_000.0;
            state.fleet = ships;
            state.tech_levels.insert("colonization".into(), 1);
        }
        two_empires(a, b)
    }

    fn one(kind: ShipKind) -> Fleet {
        let mut ships = Fleet::default();
        ships.add(kind, 1);
        ships
    }

    #[test]
    fn first_colony_arrival_founds_and_the_second_ship_turns_back() {
        let mut shard = session(1_000);
        let mut empires = race_empires(one(ShipKind::ColonyShip));
        // In tenant-b's system, so tenant-b arrives first though it steps second.
        let dest = Coord {
            position: 14,
            ..B_HOME
        };
        for identity in ["tenant-a", "tenant-b"] {
            shard
                .colonize(identity, &mut empires, dest, 100, "col")
                .unwrap();
        }
        let arrival = |id: &str| action_at(&empires[id].missions[0]).unwrap();
        let (a_at, b_at) = (arrival("tenant-a"), arrival("tenant-b"));
        assert!(b_at < a_at);

        shard.clock.set(a_at);
        shard.settle(&mut empires);
        let a = &empires["tenant-a"];
        let b = &empires["tenant-b"];
        assert_eq!(b.colonies.len(), 1);
        assert_eq!(b.colonies[0].coord, dest);
        assert!(a.colonies.is_empty());
        let [mission] = a.missions.as_slice() else {
            panic!("tenant-a mission: {:?}", a.missions);
        };
        assert_eq!(mission.phase, MissionPhase::Return);
        assert_eq!(mission.ships.get(ShipKind::ColonyShip), 1);
        assert_eq!(
            occupancy("tenant-a", &empires, shard.commons().sites(), dest),
            Occupancy::Tenant
        );
        assert_eq!(
            occupancy("tenant-b", &empires, shard.commons().sites(), dest),
            Occupancy::You
        );
    }

    #[test]
    fn two_recyclers_split_one_debris_stock() {
        let mut shard = session(1_000);
        let mut empires = race_empires(one(ShipKind::Recycler));
        let hold = empires["tenant-a"].cargo_capacity(one(ShipKind::Recycler));
        let stock = 1.5 * hold;
        let dest = Coord {
            position: 14,
            ..B_HOME
        };
        shard.commons_mut().debris.insert(dest, stock);
        for identity in ["tenant-a", "tenant-b"] {
            let cmd = ShardRecycle {
                identity: identity.into(),
                idempotency_id: "rec".into(),
                ships: one(ShipKind::Recycler),
                dest,
                speed_percent: 100,
            };
            shard.recycle(&mut empires, cmd).unwrap();
        }
        assert_eq!(shard.commons().debris[&dest], stock);
        assert!(empires.values().all(|state| state.debris.is_empty()));
        let a_at = action_at(&empires["tenant-a"].missions[0]).unwrap();

        shard.clock.set(a_at);
        shard.settle(&mut empires);
        let taken = |id: &str| empires[id].missions[0].cargo.total();
        assert_eq!(taken("tenant-b"), hold);
        assert!((taken("tenant-a") - (stock - hold)).abs() < 1e-9);
        assert!((taken("tenant-a") + taken("tenant-b") - stock).abs() < 1e-9);
        assert!(shard.commons().debris[&dest].abs() < 1e-9);
    }

    const RAID_START: u64 = 1_000;

    /// Tenant "a" at home with `atk_battleships` and fuel; tenant "b" at
    /// `B_HOME` with `def_fighters` and no moon roll.
    fn raid_empires(atk_battleships: u32, def_fighters: u32) -> BTreeMap<String, GameState> {
        let mut a = armed(RAID_START, 0);
        a.fleet.add(ShipKind::Battleship, atk_battleships);
        a.resources.deuterium = 5_000.0;
        let mut b = armed(RAID_START, def_fighters);
        b.home = B_HOME;
        b.config.moons.form_chance_per_debris = 0.0;
        let mut empires = BTreeMap::new();
        empires.insert("a".to_string(), a);
        empires.insert("b".to_string(), b);
        empires
    }

    fn raid_cmd(battleships: u32, dest: Coord, id: &str) -> ShardRaid {
        let mut ships = Fleet::default();
        ships.add(ShipKind::Battleship, battleships);
        ShardRaid {
            identity: "a".into(),
            idempotency_id: id.into(),
            ships,
            dest,
            speed_percent: 100,
        }
    }

    /// Launch all of a's battleships at b's home. Returns the arrival.
    fn launch(
        shard: &mut ShardSession<FakeClock>,
        empires: &mut BTreeMap<String, GameState>,
    ) -> u64 {
        let battleships = empires["a"].fleet.get(ShipKind::Battleship);
        shard
            .launch_raid(empires, raid_cmd(battleships, B_HOME, "r1"))
            .unwrap();
        action_at(&empires["a"].missions[0]).unwrap()
    }

    #[test]
    fn raid_fights_once_at_arrival_and_the_defender_view_lists_it() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        let arrive = launch(&mut shard, &mut empires);
        let mission = &empires["a"].missions[0];
        assert!(mission.attack);
        assert_eq!(mission.hold_secs, 0);
        assert!(mission.fuel_paid > 0.0);
        assert!(arrive > RAID_START);

        shard.clock.set(arrive - 1);
        let early = shard.view("b", &mut empires).unwrap();
        assert!(early.raids.is_empty());
        assert_eq!(early.fleet.get(ShipKind::LightFighter), 2);

        shard.clock.set(arrive);
        let a = empires.get_mut("a").unwrap();
        assert_eq!(
            shard.submit(a, build_cmd("a", "build", None)),
            Err(ShardError::Unsettled {
                identity: "a".into()
            })
        );
        let view = shard.view("b", &mut empires).unwrap();
        let [raid] = view.raids.as_slice() else {
            panic!("one raid: {:?}", view.raids);
        };
        assert_eq!(raid.at, arrive);
        assert_eq!(raid.lost.get(ShipKind::LightFighter), 2);
        assert_eq!(raid.lost.total(), 2);
        assert!(raid.plunder.iron > 0.0);
        assert_eq!(raid.plunder, empires["a"].missions[0].cargo);
        assert!(raid.debris > 0.0);
        assert_eq!(Some(&raid.debris), shard.commons().debris.get(&B_HOME));
        assert!(empires["b"].debris.is_empty());
        let text = serde_json::to_string(&view.to_json()).unwrap();
        assert!(text.contains("\"light_fighter\":2"), "{text}");
        assert!(!text.contains("battleship"), "{text}");
        assert_eq!(shard.view("b", &mut empires).unwrap().raids, view.raids);
        assert!(shard.view("a", &mut empires).unwrap().raids.is_empty());
    }

    #[test]
    fn raid_matches_the_single_player_resolver_at_arrival() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(8, 30);
        let arrive = launch(&mut shard, &mut empires);
        let mut target = empires["b"].clone();
        target.catch_up(arrive);
        let a = &empires["a"];
        let mut rng = target.rng_seed;
        let expected = resolve_combat(
            a.missions[0].ships,
            target.fleet,
            &stats_from_catalog(&a.config.ships),
            &stats_from_catalog(&target.config.ships),
            a.config.raid.max_rounds,
            &rapid_fire_table(&a.config.ships),
            &mut rng,
        );
        shard.clock.set(arrive);
        shard.settle(&mut empires);
        let b = &empires["b"];
        assert_eq!(b.combat_logs.last().map(|log| &log.report), Some(&expected));
        assert_eq!(b.fleet, expected.defender_after);
        assert_eq!(b.rng_seed, rng);
        let survivors = empires["a"].missions.first().map(|mission| mission.ships);
        let expected_survivors =
            (!expected.attacker_after.is_empty()).then_some(expected.attacker_after);
        assert_eq!(survivors, expected_survivors);
    }

    #[test]
    fn held_raid_loses_ships_and_takes_nothing() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(1, 40);
        let arrive = launch(&mut shard, &mut empires);
        shard.clock.set(arrive);
        let view = shard.view("b", &mut empires).unwrap();
        let [raid] = view.raids.as_slice() else {
            panic!("one raid: {:?}", view.raids);
        };
        let log = empires["b"].combat_logs.last().unwrap();
        assert_eq!(log.report.winner, CombatWinner::Defender);
        assert_eq!(
            raid.lost,
            log.report
                .defender_before
                .losses_to(log.report.defender_after)
        );
        assert_eq!(raid.plunder, Resources::ZERO);
        assert!(empires["a"].missions.is_empty());
    }

    #[test]
    fn survivors_land_the_plunder_at_home_on_return() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        let arrive = launch(&mut shard, &mut empires);
        shard.clock.set(arrive);
        shard.settle(&mut empires);
        let mission = &empires["a"].missions[0];
        assert_eq!(mission.phase, MissionPhase::Return);
        let (plunder, back) = (mission.cargo, mission.phase_ends_at);
        assert!(plunder.iron > 0.0);

        shard.clock.set(back - 1);
        shard.settle(&mut empires);
        let mut unladen = empires["a"].clone();
        unladen.missions[0].cargo = Resources::ZERO;
        unladen.catch_up(back);
        shard.clock.set(back);
        shard.settle(&mut empires);
        let a = &empires["a"];
        assert!(a.missions.is_empty());
        assert_eq!(a.fleet.get(ShipKind::Battleship), 20);
        for kind in crate::ResourceKind::ALL {
            let landed = a.resources.get(kind) - unladen.resources.get(kind);
            assert!((landed - plunder.get(kind)).abs() < 1e-6, "{kind:?}");
        }
    }

    /// Both empires after launching at `RAID_START` and viewing at `steps`.
    fn raid_run(steps: &[u64]) -> BTreeMap<String, GameState> {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        launch(&mut shard, &mut empires);
        for &at in steps {
            shard.clock.set(at);
            shard.view("a", &mut empires).unwrap();
        }
        empires
    }

    #[test]
    fn raid_catch_up_across_the_flight_matches_stepped_views() {
        let end = RAID_START + 48 * 3_600;
        let once = raid_run(&[end]);
        let steps: Vec<u64> = (RAID_START + 7..end).step_by(397).chain([end]).collect();
        let stepped = raid_run(&steps);
        assert!(once["a"].missions.is_empty());
        for id in ["a", "b"] {
            let (x, y) = (&once[id], &stepped[id]);
            assert_eq!(x.fleet, y.fleet, "{id}");
            assert_eq!(x.rng_seed, y.rng_seed, "{id}");
            assert_eq!(x.combat_logs.len(), 1, "{id}");
            assert_eq!(x.combat_logs[0].report, y.combat_logs[0].report, "{id}");
            // Production splits float sums at each step; stock may drift in the last bits.
            for kind in crate::ResourceKind::ALL {
                assert!(
                    (x.resources.get(kind) - y.resources.get(kind)).abs() < 1e-6,
                    "{id} {kind:?}"
                );
            }
        }
    }

    #[test]
    fn the_defender_decision_is_the_inbound_on_the_flight_clock() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        let arrive = launch(&mut shard, &mut empires);

        let b = shard.view("b", &mut empires).unwrap();
        assert_eq!(
            b.inbound,
            vec![ShardInbound {
                dest: B_HOME,
                arrives_at: arrive,
                ships: None,
            }]
        );
        let decision = b.decision.expect("inbound");
        assert_eq!(decision.id, DecisionId::Inbound);
        assert_eq!(decision.action, DecisionAction::ArmHome);
        assert_eq!(decision.deadline, arrive);
        let text = serde_json::to_string(&b.to_json()).unwrap();
        assert!(text.contains("\"ships\":null"), "{text}");
        assert!(!text.contains("battleship"), "{text}");

        let a = shard.view("a", &mut empires).unwrap();
        assert!(a.inbound.is_empty());
        assert_ne!(a.decision.map(|d| d.id), Some(DecisionId::Inbound));
        assert_eq!(a.missions.len(), 1);
        assert_eq!(a.missions[0].dest, B_HOME);

        shard.clock.set(arrive);
        let after = shard.view("b", &mut empires).unwrap();
        assert!(after.inbound.is_empty());
        assert_eq!(after.decision, None);
    }

    #[test]
    fn a_sooner_pirate_warning_beats_the_inbound() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 0);
        let arrive = launch(&mut shard, &mut empires);
        let b = empires.get_mut("b").unwrap();
        b.config.raid.enabled = true;
        b.config.raid.min_warning_secs = arrive;
        for (pirate, winner) in [
            (arrive - 10, DecisionId::Raid),
            (arrive + 10, DecisionId::Inbound),
        ] {
            empires.get_mut("b").unwrap().next_raid_at = Some(pirate);
            let decision = shard.view("b", &mut empires).unwrap().decision.unwrap();
            assert_eq!(decision.id, winner);
            assert_eq!(decision.deadline, pirate.min(arrive));
        }
    }

    #[test]
    fn a_home_phalanx_reads_the_inbound_hull_count() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        let b = empires.get_mut("b").unwrap();
        let mut moon = crate::moons::Moon::new(B_HOME);
        moon.phalanx = 1;
        b.moons.push(moon);
        b.config.espionage.error_base = 0.0;
        b.config.espionage.error_per_au = 0.0;
        launch(&mut shard, &mut empires);
        let view = shard.view("b", &mut empires).unwrap();
        assert_eq!(view.inbound[0].ships, Some(20));
    }

    #[test]
    fn catch_up_keeps_one_inbound_on_the_flight_clock() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        let arrive = launch(&mut shard, &mut empires);
        for at in (RAID_START + 1..arrive).step_by(397) {
            shard.clock.set(at);
            shard.settle(&mut empires);
            let view = shard.view("b", &mut empires).unwrap();
            let [inbound] = view.inbound.as_slice() else {
                panic!("one inbound at {at}: {:?}", view.inbound);
            };
            assert_eq!(inbound.arrives_at, arrive);
            assert_eq!(view.decision.map(|d| d.deadline), Some(arrive));
        }
    }

    #[test]
    fn a_target_submit_waits_for_an_inbound_raid_across_a_restart() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        let arrive = launch(&mut shard, &mut empires);
        let saved = shard.export_bookkeeping().unwrap();
        let mut shard = raid_session(RAID_START, 0, 3);
        shard.restore_bookkeeping(&saved).unwrap();
        shard.clock.set(arrive);
        let build = ShardCommand {
            intent: Intent::Build {
                planet: B_HOME,
                building: BuildingKind::MetalMine,
                actor: Actor::Player,
            },
            ..build_cmd("b", "build", None)
        };
        let b = empires.get_mut("b").unwrap();
        let before = b.clone();
        assert_eq!(
            shard.submit(b, build.clone()),
            Err(ShardError::Unsettled {
                identity: "b".into()
            })
        );
        assert_eq!(*b, before);
        shard.settle(&mut empires);
        assert_eq!(empires["b"].combat_logs.len(), 1);
        let b = empires.get_mut("b").unwrap();
        shard.submit(b, build).unwrap();
    }

    #[test]
    fn recalled_raid_comes_home_without_a_fight() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        let arrive = launch(&mut shard, &mut empires);
        let id = empires["a"].missions[0].id;
        shard.clock.set((RAID_START + arrive) / 2);
        let recall = ShardCommand {
            identity: "a".into(),
            idempotency_id: "recall".into(),
            intent: Intent::Recall { mission: id },
            client_now_unix: None,
        };
        shard.submit(empires.get_mut("a").unwrap(), recall).unwrap();
        shard.clock.set(arrive + 3_600);
        let view = shard.view("b", &mut empires).unwrap();
        assert!(view.raids.is_empty());
        assert_eq!(view.fleet.get(ShipKind::LightFighter), 2);
        let a = &empires["a"];
        assert!(a.missions.is_empty());
        assert_eq!(a.fleet.get(ShipKind::Battleship), 20);
        assert!(a.combat_logs.is_empty());
        assert!(empires["b"].combat_logs.is_empty());
    }

    #[test]
    fn raid_refuses_you_empty_outpost_site_and_colony_and_launches_nothing() {
        let mut shard = raid_session(RAID_START, 0, 1);
        place_sites(&mut shard);
        let mut empires = raid_empires(20, 2);
        let colony = Coord {
            position: 3,
            ..B_HOME
        };
        let b = empires.remove("b").unwrap();
        empires.insert("b".into(), with_colony(b, colony));
        let not_home = |coord, kind| ShardError::NotATenantHome { coord, kind };
        let refused = [
            (Coord::HOME, ShardError::SameEmpire),
            (empty_slot(), not_home(empty_slot(), Occupancy::Empty)),
            (pirate_coord(), not_home(pirate_coord(), Occupancy::Outpost)),
            (site_coord(), not_home(site_coord(), Occupancy::Site)),
            (colony, not_home(colony, Occupancy::Tenant)),
        ];
        for (dest, err) in refused {
            let cmd = raid_cmd(20, dest, &dest.to_string());
            assert_eq!(shard.launch_raid(&mut empires, cmd), Err(err));
            assert!(empires["a"].missions.is_empty());
            assert_eq!(empires["a"].fleet.get(ShipKind::Battleship), 20);
        }
        // Refusals spent no attack: the one raid the cap allows still launches.
        shard
            .launch_raid(&mut empires, raid_cmd(20, B_HOME, "home"))
            .unwrap();
    }

    #[test]
    fn newbie_protection_blocks_the_launch_then_expires() {
        let mut shard = raid_session(RAID_START, 3_600, 3);
        let mut empires = raid_empires(20, 2);
        assert_eq!(
            shard.launch_raid(&mut empires, raid_cmd(20, B_HOME, "early")),
            Err(ShardError::NewbieProtected {
                identity: "b".into()
            })
        );
        assert!(empires["a"].missions.is_empty());
        assert_eq!(empires["a"].fleet.get(ShipKind::Battleship), 20);
        shard.clock.advance(3_600);
        shard
            .launch_raid(&mut empires, raid_cmd(20, B_HOME, "later"))
            .unwrap();
        assert_eq!(empires["a"].missions.len(), 1);
    }

    #[test]
    fn a_retried_raid_launches_once_and_a_new_one_hits_the_attack_cap() {
        let mut shard = raid_session(RAID_START, 0, 1);
        let mut empires = raid_empires(20, 2);
        for _ in 0..2 {
            shard
                .launch_raid(&mut empires, raid_cmd(10, B_HOME, "r1"))
                .unwrap();
        }
        assert_eq!(empires["a"].missions.len(), 1);
        assert_eq!(empires["a"].fleet.get(ShipKind::Battleship), 10);
        let err = shard
            .launch_raid(&mut empires, raid_cmd(10, B_HOME, "r2"))
            .unwrap_err();
        assert!(matches!(
            err,
            ShardError::AttackCapped { identity, .. } if identity == "a"
        ));
        assert_eq!(empires["a"].missions.len(), 1);
    }

    #[test]
    fn raid_plunder_takes_the_share_then_cap_into_the_hold() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 1);
        let b = empires.get_mut("b").unwrap();
        b.resources.iron = 200.0;
        b.config.raid.plunder_share = 0.5;
        b.config.raid.plunder_cap = Resources::from_parts(40.0, 0.0, 0.0);
        b.config.raid.protected_share = 0.0;
        let arrive = launch(&mut shard, &mut empires);
        let mut target = empires["b"].clone();
        target.catch_up(arrive);
        shard.clock.set(arrive);
        shard.settle(&mut empires);
        let mission = &empires["a"].missions[0];
        let expected = fill_hold(
            plunder_from(target.plunderable(), 0.5, target.config.raid.plunder_cap),
            empires["a"].cargo_capacity(mission.ships),
        );
        assert_eq!(mission.cargo, expected);
        assert_eq!(expected.iron, 40.0);
        assert_eq!(
            empires["b"].resources,
            target.resources.saturating_sub(expected)
        );
    }

    #[test]
    fn raid_wreckage_joins_the_shared_pile_for_the_moon_roll() {
        let mut shard = raid_session(RAID_START, 0, 3);
        let mut empires = raid_empires(20, 2);
        let moons = &mut empires.get_mut("b").unwrap().config.moons;
        moons.form_chance_per_debris = 1.0;
        moons.form_chance_cap = 1.0;
        let pile = moons.form_threshold;
        shard.commons_mut().debris.insert(B_HOME, pile);
        let arrive = launch(&mut shard, &mut empires);
        shard.clock.set(arrive);
        shard.settle(&mut empires);
        assert!(empires["b"].moon_at(B_HOME).is_some());
        assert_eq!(shard.commons().debris[&B_HOME], 0.0);
        assert!(empires["b"].debris.is_empty());
    }

    #[test]
    fn two_tenants_see_the_shard_outpost() {
        let commons = ShardCommons {
            outposts: vec![pirate_coord()],
            ..ShardCommons::default()
        };
        let empires = two_empires(tenant_a(1_000), tenant_b(1_000));
        for identity in ["tenant-a", "tenant-b"] {
            assert!(
                targets(identity, &empires, commons.sites()).contains(&ShardTarget {
                    coord: pirate_coord(),
                    kind: Occupancy::Outpost,
                }),
                "{identity}"
            );
        }
    }

    #[test]
    fn lone_catch_up_past_an_arrival_waits_for_settle() {
        let mut shard = session(1_000);
        let mut empires = race_empires(one(ShipKind::ColonyShip));
        shard
            .colonize("tenant-a", &mut empires, empty_slot(), 100, "col")
            .unwrap();
        shard
            .clock
            .set(action_at(&empires["tenant-a"].missions[0]).unwrap());
        let a = empires.get_mut("tenant-a").unwrap();
        let before = a.clone();
        assert_eq!(
            shard.submit(a, build_cmd("tenant-a", "build", None)),
            Err(ShardError::Unsettled {
                identity: "tenant-a".into()
            })
        );
        assert_eq!(*a, before);
        shard.settle(&mut empires);
        assert_eq!(empires["tenant-a"].colonies[0].coord, empty_slot());
        let a = empires.get_mut("tenant-a").unwrap();
        shard
            .submit(a, build_cmd("tenant-a", "build", None))
            .unwrap();
    }

    #[test]
    fn commons_bytes_round_trip_and_refuse_another_version() {
        let mut commons = ShardCommons {
            outposts: vec![pirate_coord()],
            sites: vec![site_coord()],
            expedition: Some(walk_slot(3, 1)),
            ..ShardCommons::default()
        };
        commons.debris.insert(empty_slot(), 42.5);
        let bytes = commons.to_bytes().unwrap();
        assert_eq!(ShardCommons::from_bytes(&bytes).unwrap(), commons);
        let other = String::from_utf8(bytes)
            .unwrap()
            .replace("\"version\":3", "\"version\":4");
        assert!(matches!(
            ShardCommons::from_bytes(other.as_bytes()),
            Err(ShardError::Commons(_))
        ));
    }

    #[test]
    fn version_1_commons_read_with_no_expedition_coord() {
        let v1 = br#"{"version":1,"outposts":[[4,4,4]],"sites":[],"debris":[[[1,1,14],42.5]]}"#;
        let commons = ShardCommons::from_bytes(v1).unwrap();
        assert_eq!(commons.outposts, vec![pirate_coord()]);
        assert_eq!(commons.debris[&empty_slot()], 42.5);
        assert_eq!(commons.expedition, None);
    }

    fn frontier() -> FrontierConfig {
        BalanceConfig::default().frontier
    }

    #[test]
    fn expedition_coord_skips_the_frontier_galaxy() {
        let map = ShardMap {
            seed: 0,
            galaxies: 2,
            systems: 2,
            positions: 2,
        };
        let frontier = frontier();
        assert_eq!(frontier.galaxy, 2);
        let coord = ShardCommons::default()
            .place_expedition(map, &frontier, &BTreeMap::new())
            .unwrap();
        assert_eq!(
            coord,
            Coord {
                galaxy: 1,
                system: 2,
                position: 2,
            },
            "the walk ends in galaxy 2, so the last slot before it"
        );
    }

    #[test]
    fn expedition_coord_is_placed_once_from_the_seed_and_bounds() {
        let mut empires = BTreeMap::new();
        let mut commons = ShardCommons::default();
        let coord = commons
            .place_expedition(WALK, &frontier(), &empires)
            .unwrap();
        assert_eq!(coord, walk_slot(3, 1), "last slot on the home walk");
        assert_eq!(commons.expedition, Some(coord));
        assert_eq!(
            ShardCommons::default()
                .place_expedition(WALK, &frontier(), &empires)
                .unwrap(),
            coord,
            "same seed and bounds, same coord"
        );
        seat_home(
            "tenant-a",
            test_state(1_000),
            walk_slot(2, 3),
            &mut empires,
            commons.sites(),
        )
        .unwrap();
        assert_eq!(
            commons
                .place_expedition(WALK, &frontier(), &empires)
                .unwrap(),
            coord
        );
        let mut late = ShardCommons::default();
        let mut seated = BTreeMap::new();
        seat_home(
            "tenant-a",
            test_state(1_000),
            coord,
            &mut seated,
            late.sites(),
        )
        .unwrap();
        assert_eq!(
            late.place_expedition(WALK, &frontier(), &seated).unwrap(),
            walk_slot(2, 1),
            "a host placing after joins skips a held slot"
        );
    }

    #[test]
    fn submit_refuses_a_shard_expedition_before_the_coord_is_placed() {
        let mut shard = session(1_000);
        let mut empires = race_empires(one(ShipKind::LightFighter));
        let a = empires.get_mut("tenant-a").unwrap();
        let before = a.clone();
        let hold_secs = a.config.expedition.min_hold_secs;
        assert_eq!(
            shard.submit(a, expedition_cmd("tenant-a", hold_secs)),
            Err(ShardError::NoExpeditionCoord)
        );
        assert_eq!(*a, before);
    }

    fn expedition_cmd(identity: &str, hold_secs: u64) -> ShardCommand {
        ShardCommand {
            identity: identity.into(),
            idempotency_id: "exp".into(),
            intent: Intent::Expedition {
                origin: None,
                ships: one(ShipKind::LightFighter),
                site: None,
                hold_secs,
                speed_percent: 100,
            },
            client_now_unix: None,
        }
    }

    #[test]
    fn two_tenants_fly_expeditions_to_one_shard_coord_at_their_own_distance() {
        let mut shard = session(1_000);
        let mut ships = one(ShipKind::LightFighter);
        ships.add(ShipKind::ColonyShip, 1);
        let mut empires = race_empires(ships);
        let local = empires["tenant-b"].clone();
        let coord = shard
            .commons_mut()
            .place_expedition(WALK, &frontier(), &empires)
            .unwrap();
        let mut outbound = Vec::new();
        for identity in ["tenant-a", "tenant-b"] {
            let state = empires.get_mut(identity).unwrap();
            let (secs, fuel) = state
                .flight_time_and_fuel(one(ShipKind::LightFighter), coord, 100)
                .unwrap();
            let hold_secs = state.config.expedition.min_hold_secs;
            shard
                .submit(state, expedition_cmd(identity, hold_secs))
                .unwrap();
            let mission = &state.missions[0];
            assert_eq!(mission.dest, coord, "{identity}");
            assert!(mission.expedition_seed.is_some());
            assert_eq!(mission.outbound_secs, secs, "{identity}");
            assert_eq!(mission.fuel_paid, fuel, "{identity}");
            outbound.push(secs);
        }
        assert_ne!(outbound[0], outbound[1], "each pays its own distance");
        for identity in ["tenant-a", "tenant-b"] {
            assert_eq!(
                occupancy(identity, &empires, shard.commons().sites(), coord),
                Occupancy::Site
            );
            assert!(
                targets(identity, &empires, shard.commons().sites()).contains(&ShardTarget {
                    coord,
                    kind: Occupancy::Site,
                })
            );
        }
        assert_eq!(
            shard.colonize("tenant-a", &mut empires, coord, 100, "col"),
            Err(ShardError::Occupied {
                coord,
                kind: Occupancy::Site,
            })
        );

        let mut local = local;
        local
            .launch_expedition(
                local.home,
                one(ShipKind::LightFighter),
                local.config.expedition.min_hold_secs,
                100,
                1_000,
            )
            .unwrap();
        assert_eq!(
            local.missions[0].dest,
            Coord {
                position: local.config.expedition.dest_position,
                ..B_HOME
            },
            "a local expedition still flies to its home-system slot"
        );
    }

    fn scaled_session(now: u64, epoch: u64, factor: u32) -> ShardSession<FakeClock> {
        ShardSession::new_scaled(
            FakeClock::new(now),
            KEY,
            ShardConfig::default(),
            epoch,
            factor,
        )
        .expect("factor >= 1")
    }

    fn close_resources(a: &Resources, b: &Resources) {
        assert!(
            (a.iron - b.iron).abs() < 1e-6,
            "iron {} vs {}",
            a.iron,
            b.iron
        );
        assert!(
            (a.silicon - b.silicon).abs() < 1e-6,
            "silicon {} vs {}",
            a.silicon,
            b.silicon
        );
        assert!(
            (a.deuterium - b.deuterium).abs() < 1e-6,
            "deuterium {} vs {}",
            a.deuterium,
            b.deuterium
        );
    }

    #[test]
    fn zero_clock_factor_is_refused() {
        let err = match ShardSession::new_scaled(
            FakeClock::new(1_000),
            KEY,
            ShardConfig::default(),
            1_000,
            0,
        ) {
            Ok(_) => panic!("factor 0 must be refused"),
            Err(err) => err,
        };
        assert_eq!(err, ShardError::ZeroClockFactor);
    }

    #[test]
    fn catch_up_over_t_real_seconds_at_factor_k_matches_factor_one() {
        const EPOCH: u64 = 1_000;
        const K: u32 = 10;
        const T: u64 = 12;
        let mut slow = session(EPOCH);
        let mut fast = scaled_session(EPOCH, EPOCH, K);
        let mut slow_empires = two_empires(tenant_a(EPOCH), tenant_b(EPOCH));
        let mut fast_empires = two_empires(tenant_a(EPOCH), tenant_b(EPOCH));
        slow.clock.advance(u64::from(K) * T);
        fast.clock.advance(T);
        let slow_view = slow.view("tenant-a", &mut slow_empires).unwrap();
        let fast_view = fast.view("tenant-a", &mut fast_empires).unwrap();
        assert_eq!(slow_view.at, fast_view.at);
        assert_eq!(slow_view.at, EPOCH + u64::from(K) * T);
        close_resources(&slow_view.resources, &fast_view.resources);
        assert_eq!(slow_view.missions, fast_view.missions);
        assert_eq!(
            slow_empires["tenant-a"].build_queue,
            fast_empires["tenant-a"].build_queue
        );
        assert_eq!(slow_view.factor, 1);
        assert_eq!(fast_view.epoch, EPOCH);
        assert_eq!(fast_view.factor, K);
        let slow_b = slow.view("tenant-b", &mut slow_empires).unwrap();
        let fast_b = fast.view("tenant-b", &mut fast_empires).unwrap();
        assert_eq!(slow_b.at, fast_b.at);
        assert_eq!(slow_b.at, fast_view.at);
    }

    #[test]
    fn duplicate_idempotency_id_on_a_scaled_shard_applies_once() {
        let mut shard = scaled_session(1_000, 1_000, 10);
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
        shard.clock.advance(7);
        shard
            .submit(&mut state, build_cmd("tenant-a", "same", None))
            .unwrap();
        assert_eq!(state.build_queue.len(), 1);
        shard
            .submit(&mut state, build_cmd("tenant-a", "next", None))
            .unwrap();
        assert_eq!(state.last_tick_unix, 1_070);
    }

    #[test]
    fn command_limit_stays_on_wall_time_at_factor_ten() {
        let mut shard = ShardSession::new_scaled(
            FakeClock::new(1_000),
            KEY,
            ShardConfig {
                command_window_secs: 60,
                command_limit: 2,
                ..ShardConfig::default()
            },
            1_000,
            10,
        )
        .unwrap();
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
        shard.clock.advance(6);
        let err = shard
            .submit(&mut state, build_cmd("tenant-a", "c", None))
            .unwrap_err();
        assert!(matches!(err, ShardError::RateLimited { .. }));
        shard.clock.advance(54);
        shard
            .submit(&mut state, build_cmd("tenant-a", "c", None))
            .unwrap();
        assert_eq!(state.last_tick_unix, 1_000 + 60 * 10);
    }

    #[test]
    fn attack_window_expires_k_times_sooner_in_real_time() {
        const WINDOW: u64 = 100;
        let cfg = ShardConfig {
            newbie_secs: 0,
            attack_limit: 1,
            attack_window_secs: WINDOW,
            ..ShardConfig::default()
        };
        let mut slow = ShardSession::new(FakeClock::new(RAID_START), KEY, cfg.clone());
        let mut fast =
            ShardSession::new_scaled(FakeClock::new(RAID_START), KEY, cfg, RAID_START, 10).unwrap();
        let mut slow_empires = raid_empires(20, 2);
        let mut fast_empires = raid_empires(20, 2);
        for empires in [&mut slow_empires, &mut fast_empires] {
            for state in empires.values_mut() {
                state.config.flight.base_fleet_slots = 2;
            }
        }
        slow.launch_raid(&mut slow_empires, raid_cmd(10, B_HOME, "r1"))
            .unwrap();
        fast.launch_raid(&mut fast_empires, raid_cmd(10, B_HOME, "r1"))
            .unwrap();
        assert!(matches!(
            slow.launch_raid(&mut slow_empires, raid_cmd(10, B_HOME, "r2")),
            Err(ShardError::AttackCapped { .. })
        ));
        assert!(matches!(
            fast.launch_raid(&mut fast_empires, raid_cmd(10, B_HOME, "r2")),
            Err(ShardError::AttackCapped { .. })
        ));
        fast.clock.advance(WINDOW / 10);
        fast.launch_raid(&mut fast_empires, raid_cmd(10, B_HOME, "r2"))
            .unwrap();
        assert!(matches!(
            slow.launch_raid(&mut slow_empires, raid_cmd(10, B_HOME, "r2")),
            Err(ShardError::AttackCapped { .. })
        ));
        slow.clock.advance(WINDOW);
        slow.launch_raid(&mut slow_empires, raid_cmd(10, B_HOME, "r2"))
            .unwrap();
    }

    #[test]
    fn raid_at_game_t_matches_factor_one() {
        let mut slow = raid_session(RAID_START, 0, 3);
        let mut fast = ShardSession::new_scaled(
            FakeClock::new(RAID_START),
            KEY,
            ShardConfig {
                newbie_secs: 0,
                attack_limit: 3,
                ..ShardConfig::default()
            },
            RAID_START,
            10,
        )
        .unwrap();
        let mut slow_empires = raid_empires(8, 30);
        let mut fast_empires = raid_empires(8, 30);
        let arrive = launch(&mut slow, &mut slow_empires);
        launch(&mut fast, &mut fast_empires);
        assert_eq!(
            slow_empires["a"].missions[0].phase_ends_at,
            fast_empires["a"].missions[0].phase_ends_at
        );
        let mut real_arrive = crate::wall_unix(arrive, RAID_START, 10);
        if crate::scale_unix(real_arrive, RAID_START, 10) < arrive {
            real_arrive += 1;
        }
        let game_arrive = crate::scale_unix(real_arrive, RAID_START, 10);
        slow.clock.set(game_arrive);
        fast.clock.set(real_arrive);
        slow.settle(&mut slow_empires);
        fast.settle(&mut fast_empires);
        assert_eq!(
            slow_empires["b"].combat_logs.last().map(|log| &log.report),
            fast_empires["b"].combat_logs.last().map(|log| &log.report)
        );
        assert_eq!(slow_empires["b"].fleet, fast_empires["b"].fleet);
        assert_eq!(
            slow_empires["a"].missions.first().map(|m| m.ships),
            fast_empires["a"].missions.first().map(|m| m.ships)
        );
    }

    #[test]
    fn tenant_view_json_carries_epoch_and_factor() {
        let mut shard = scaled_session(1_000, 1_000, 10);
        place_sites(&mut shard);
        let mut empires = two_empires(tenant_a(1_000), tenant_b(1_000));
        let json = shard.view("tenant-a", &mut empires).unwrap().to_json();
        assert_eq!(json["epoch"], 1_000);
        assert_eq!(json["factor"], 10);
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        let mut without_clock = keys.clone();
        without_clock.retain(|k| k != "epoch" && k != "factor");
        assert_eq!(
            without_clock,
            [
                "at",
                "decision",
                "home",
                "identity",
                "inbound",
                "occupancy",
                "planets",
                "raids",
                "resources"
            ]
        );
    }

    #[test]
    fn blitz_join_isolates_prestige_and_resume_keeps_it() {
        let mut shard = scaled_session(1_000, 1_000, 10);
        let mut empires = BTreeMap::new();
        shard
            .join("tenant-a", WALK, &mut empires, |_| {
                let mut state = test_state(1_000);
                state.prestige.currency = 9;
                state.prestige.lifetime = 4;
                state
            })
            .unwrap();
        assert_eq!(empires["tenant-a"].prestige.currency, 0);
        assert_eq!(empires["tenant-a"].prestige.lifetime, 0);
        let run = empires["tenant-a"]
            .season
            .as_ref()
            .expect("isolated overlay");
        assert!(!run.prestige);
        assert_eq!(run.ruleset_id, crate::ISOLATED_PRESTIGE);
        {
            let seated = empires.get_mut("tenant-a").unwrap();
            seated.config.victory.enabled = true;
            seated.config.victory.wonder_id = "dyson_ring".into();
            seated.completed_wonders.push("dyson_ring".into());
            seated.maybe_declare_victory(seated.last_tick_unix);
            let rec = seated.victory.as_ref().expect("victory");
            assert_eq!(rec.prestige, 0);
            assert_eq!(seated.prestige.currency, 0);
            seated.victory = None;
            seated.completed_wonders.clear();
        }
        empires.get_mut("tenant-a").unwrap().prestige.currency = 3;
        let before = empires.clone();
        shard
            .join("tenant-a", WALK, &mut empires, |_| {
                unreachable!("resume builds nothing")
            })
            .unwrap();
        assert_eq!(empires["tenant-a"].prestige.currency, 3);
        assert_eq!(empires, before);
    }

    fn finish_session(
        now: u64,
        deadline: u64,
        board: ScoreboardVisibility,
    ) -> ShardSession<FakeClock> {
        session(now).with_finish_line(deadline, board)
    }

    fn join_pair(shard: &mut ShardSession<FakeClock>, now: u64) -> BTreeMap<String, GameState> {
        let mut empires = BTreeMap::new();
        shard
            .join("tenant-a", WALK, &mut empires, |_| test_state(now))
            .unwrap();
        shard
            .join("tenant-b", WALK, &mut empires, |_| test_state(now))
            .unwrap();
        empires
    }

    fn grant_wonder(state: &mut GameState) {
        state.config.victory.enabled = true;
        state.config.victory.wonder_id = "dyson_ring".into();
        state.completed_wonders.push("dyson_ring".into());
        state.maybe_declare_victory(state.last_tick_unix);
    }

    fn build_on(identity: &str, id: &str, planet: Coord) -> ShardCommand {
        let mut cmd = build_cmd(identity, id, None);
        cmd.intent = Intent::Build {
            planet,
            building: BuildingKind::MetalMine,
            actor: Actor::Player,
        };
        cmd
    }

    #[test]
    fn wonder_first_ends_the_shard_and_replay_is_a_no_op() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        let home_a = empires["tenant-a"].home;
        let home_b = empires["tenant-b"].home;
        grant_wonder(empires.get_mut("tenant-a").unwrap());
        let err = shard
            .submit(
                empires.get_mut("tenant-a").unwrap(),
                build_on("tenant-a", "win", home_a),
            )
            .unwrap_err();
        assert_eq!(
            err,
            ShardError::Unsettled {
                identity: "tenant-a".into()
            }
        );
        shard.settle(&mut empires);
        let standing = shard.standing().expect("race");
        assert_eq!(standing.winner, "tenant-a");
        assert_eq!(standing.path, ShardWinPath::Race);
        let err = shard
            .submit(
                empires.get_mut("tenant-a").unwrap(),
                build_on("tenant-a", "win", home_a),
            )
            .unwrap_err();
        assert_eq!(err, ShardError::Ended);
        assert_eq!(shard.standing().unwrap().winner, "tenant-a");
        let err = shard
            .submit(
                empires.get_mut("tenant-b").unwrap(),
                build_on("tenant-b", "late", home_b),
            )
            .unwrap_err();
        assert_eq!(err, ShardError::Ended);
        shard.settle(&mut empires);
        let json = shard.view("tenant-b", &mut empires).unwrap().to_json();
        assert_eq!(json["standing"]["winner"], "tenant-a");
        assert_eq!(json["standing"]["path"], "race");
        assert!(json.get("scoreboard").is_none());
        let scores = json["standing"]["scores"].as_array().unwrap();
        assert_eq!(scores.len(), 2);
        assert!(scores.iter().all(|row| {
            row.get("identity").is_some()
                && row.get("score").is_some()
                && row.get("iron").is_none()
                && row.get("fleet").is_none()
        }));
    }

    #[test]
    fn race_catch_up_stops_at_the_victory_instant() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        grant_wonder(empires.get_mut("tenant-a").unwrap());
        assert_eq!(empires["tenant-a"].last_tick_unix, 1_000);
        shard.clock.set(1_500);
        shard.settle(&mut empires);
        assert_eq!(empires["tenant-a"].last_tick_unix, 1_000);
        assert_eq!(empires["tenant-b"].last_tick_unix, 1_000);
        assert_eq!(shard.standing().unwrap().path, ShardWinPath::Race);
    }

    #[test]
    fn other_tenant_submit_unsettles_until_the_race_is_named() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        grant_wonder(empires.get_mut("tenant-a").unwrap());
        shard.clock.set(1_200);
        let home_b = empires["tenant-b"].home;
        let err = shard
            .submit(
                empires.get_mut("tenant-b").unwrap(),
                build_on("tenant-b", "late", home_b),
            )
            .unwrap_err();
        assert_eq!(
            err,
            ShardError::Unsettled {
                identity: "tenant-b".into()
            }
        );
        assert_eq!(empires["tenant-b"].last_tick_unix, 1_000);
        shard.settle(&mut empires);
        assert_eq!(shard.standing().unwrap().winner, "tenant-a");
        assert_eq!(empires["tenant-b"].last_tick_unix, 1_000);
        let err = shard
            .submit(
                empires.get_mut("tenant-b").unwrap(),
                build_on("tenant-b", "late", home_b),
            )
            .unwrap_err();
        assert_eq!(err, ShardError::Ended);
    }

    #[test]
    fn bridge_pulse_after_the_deadline_is_unsettled() {
        let mut shard = finish_session(1_000, 1_100, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        shard.clock.set(1_200);
        let iron = empires["tenant-a"].resources.iron;
        let event = shard.sign_bridge(ActivityPulse::PrMerged { ts: 1_200 });
        let err = shard
            .apply_bridge(empires.get_mut("tenant-a").unwrap(), "tenant-a", &event)
            .unwrap_err();
        assert_eq!(
            err,
            ShardError::Unsettled {
                identity: "tenant-a".into()
            }
        );
        assert!((empires["tenant-a"].resources.iron - iron).abs() < 1e-9);
        shard.settle(&mut empires);
        assert_eq!(shard.standing().unwrap().path, ShardWinPath::Deadline);
        let iron = empires["tenant-a"].resources.iron;
        let err = shard
            .apply_bridge(empires.get_mut("tenant-a").unwrap(), "tenant-a", &event)
            .unwrap_err();
        assert_eq!(err, ShardError::Ended);
        assert!((empires["tenant-a"].resources.iron - iron).abs() < 1e-9);
    }

    #[test]
    fn delayed_bridge_pulse_applies_before_the_deadline() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        shard.clock.set(1_100);
        let iron = empires["tenant-a"].resources.iron;
        let event = shard.sign_bridge(ActivityPulse::PrMerged { ts: 1_050 });
        let result = shard
            .apply_bridge(empires.get_mut("tenant-a").unwrap(), "tenant-a", &event)
            .unwrap();
        assert_eq!(result, ActivityApply::Applied);
        assert!(empires["tenant-a"].resources.iron > iron);
        assert!(shard.standing().is_none());
    }

    #[test]
    fn current_bridge_pulse_unsettles_until_the_race_is_named() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        grant_wonder(empires.get_mut("tenant-a").unwrap());
        shard.clock.set(1_200);
        let iron = empires["tenant-b"].resources.iron;
        let event = shard.sign_bridge(ActivityPulse::PrMerged { ts: 1_200 });
        let err = shard
            .apply_bridge(empires.get_mut("tenant-b").unwrap(), "tenant-b", &event)
            .unwrap_err();
        assert_eq!(
            err,
            ShardError::Unsettled {
                identity: "tenant-b".into()
            }
        );
        assert!((empires["tenant-b"].resources.iron - iron).abs() < 1e-9);
        shard.settle(&mut empires);
        assert_eq!(shard.standing().unwrap().winner, "tenant-a");
        let err = shard
            .apply_bridge(empires.get_mut("tenant-b").unwrap(), "tenant-b", &event)
            .unwrap_err();
        assert_eq!(err, ShardError::Ended);
        assert!((empires["tenant-b"].resources.iron - iron).abs() < 1e-9);
    }

    #[test]
    fn first_join_after_the_deadline_still_seats() {
        let mut shard = finish_session(1_000, 1_100, ScoreboardVisibility::Hidden);
        shard.clock.set(1_200);
        let mut empires = BTreeMap::new();
        shard
            .join("tenant-a", WALK, &mut empires, |_| test_state(1_000))
            .unwrap();
        assert_eq!(empires.len(), 1);
        shard.settle(&mut empires);
        assert_eq!(shard.standing().unwrap().winner, "tenant-a");
        assert_eq!(shard.standing().unwrap().path, ShardWinPath::Deadline);
    }

    #[test]
    fn earlier_victory_wins_even_if_the_later_tenant_is_touched_first() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        {
            let a = empires.get_mut("tenant-a").unwrap();
            a.config.victory.enabled = true;
            a.config.victory.wonder_id = "dyson_ring".into();
            a.completed_wonders.push("dyson_ring".into());
            a.maybe_declare_victory(1_100);
        }
        {
            let b = empires.get_mut("tenant-b").unwrap();
            b.config.victory.enabled = true;
            b.config.victory.wonder_id = "dyson_ring".into();
            b.completed_wonders.push("dyson_ring".into());
            b.maybe_declare_victory(1_200);
        }
        shard.clock.set(1_300);
        shard.settle(&mut empires);
        let standing = shard.standing().expect("race");
        assert_eq!(standing.winner, "tenant-a");
        assert_eq!(standing.path, ShardWinPath::Race);
    }

    #[test]
    fn deadline_picks_the_higher_score_at_the_next_interaction() {
        let mut shard = finish_session(1_000, 1_100, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        empires.get_mut("tenant-a").unwrap().resources.iron = 2_000_000.0;
        empires.get_mut("tenant-b").unwrap().resources.iron = 10.0;
        shard.clock.set(1_100);
        let json = shard.view("tenant-a", &mut empires).unwrap().to_json();
        assert_eq!(json["standing"]["winner"], "tenant-a");
        assert_eq!(json["standing"]["path"], "deadline");
        let home_b = empires["tenant-b"].home;
        let err = shard
            .submit(
                empires.get_mut("tenant-b").unwrap(),
                build_on("tenant-b", "after", home_b),
            )
            .unwrap_err();
        assert_eq!(err, ShardError::Ended);
        let frozen = shard.standing().unwrap().scores.clone();
        empires.get_mut("tenant-b").unwrap().resources.iron = 9_000_000.0;
        let later = shard.view("tenant-a", &mut empires).unwrap();
        let standing = later.standing.as_ref().unwrap();
        assert_eq!(standing.scores, frozen);
        assert_eq!(standing.winner, "tenant-a");
    }

    #[test]
    fn hidden_scoreboard_stays_off_the_view_until_end() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        let json = shard.view("tenant-a", &mut empires).unwrap().to_json();
        assert!(json.get("scoreboard").is_none());
        assert!(json.get("standing").is_none());
    }

    #[test]
    fn visible_scoreboard_is_identity_and_score_only() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Visible);
        let mut empires = join_pair(&mut shard, 1_000);
        let json = shard.view("tenant-a", &mut empires).unwrap().to_json();
        assert!(json.get("standing").is_none());
        let row = &json["scoreboard"][0];
        let keys: Vec<_> = row.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["identity", "score"]);
        assert_ne!(row["identity"], "");
    }

    #[test]
    fn ordinary_shard_does_not_end_on_victory() {
        let mut shard = session(1_000);
        let mut empires = join_pair(&mut shard, 1_000);
        grant_wonder(empires.get_mut("tenant-a").unwrap());
        shard.settle(&mut empires);
        assert!(shard.standing().is_none());
        let home_b = empires["tenant-b"].home;
        shard
            .submit(
                empires.get_mut("tenant-b").unwrap(),
                build_on("tenant-b", "still", home_b),
            )
            .unwrap();
    }

    #[test]
    fn finish_bookkeeping_is_version_3_and_keeps_the_winner() {
        let mut shard = finish_session(1_000, 5_000, ScoreboardVisibility::Hidden);
        let mut empires = join_pair(&mut shard, 1_000);
        grant_wonder(empires.get_mut("tenant-a").unwrap());
        shard.settle(&mut empires);
        assert!(shard.standing().is_some());
        let saved = shard.export_bookkeeping().unwrap();
        let doc: serde_json::Value = serde_json::from_slice(saved.as_bytes()).unwrap();
        assert_eq!(doc["version"], 3);
        let mut restored = finish_session(1_030, 5_000, ScoreboardVisibility::Hidden);
        restored.restore_bookkeeping(&saved).unwrap();
        assert_eq!(restored.standing().unwrap().winner, "tenant-a");
        let err = session(1_030).restore_bookkeeping(&saved).unwrap_err();
        assert!(
            matches!(err, ShardError::Bookkeeping(ref msg) if msg.contains("finish-line identity")),
            "{err}"
        );
    }
}
