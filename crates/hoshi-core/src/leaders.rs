//! Leaders, roles, assignments, permissions and delegation.
//!
//! Traits change scheduling and combat efficiency inside physical limits from
//! `docs/realism.md`. They never beat the energy budget or the rocket equation.

use crate::fleets::Coord;
use serde::Deserialize;
use std::fmt;

const DEFAULT_LEADERS: &str = include_str!("../data/leaders.toml");
pub const PLAYER: &str = "player";

#[derive(Debug, Clone, PartialEq)]
pub enum LeaderError {
    Parse(String),
    DuplicateTrait(String),
    UnknownTrait(String),
    UnknownLeader(u64),
    Retired(u64),
    AlreadyAssigned(u64),
    Reassigning { ready_at: u64 },
    UnknownRole(String),
    UnknownPlanet,
    MissingPermission { actor: String, needed: Permission },
    SpendCap { cap: f64, spent: f64, cost: f64 },
    CannotAfford { missing: crate::state::Resources },
    Unassigned(u64),
}

impl fmt::Display for LeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaderError::Parse(msg) => write!(f, "leaders: {msg}"),
            LeaderError::DuplicateTrait(id) => write!(f, "leaders: duplicate trait {id}"),
            LeaderError::UnknownTrait(id) => write!(f, "unknown trait {id}"),
            LeaderError::UnknownLeader(id) => write!(f, "unknown leader #{id}"),
            LeaderError::Retired(id) => write!(f, "leader #{id} is retired"),
            LeaderError::AlreadyAssigned(id) => write!(f, "leader #{id} already has a role"),
            LeaderError::Reassigning { ready_at } => {
                write!(f, "reassignment is not ready until {ready_at}")
            }
            LeaderError::UnknownRole(id) => write!(f, "unknown role {id}"),
            LeaderError::UnknownPlanet => write!(f, "unknown planet"),
            LeaderError::MissingPermission { actor, needed } => {
                write!(f, "{actor} lacks permission {}", needed.as_key())
            }
            LeaderError::SpendCap { cap, spent, cost } => {
                write!(f, "spend cap {cap:.0} (used {spent:.0}, need {cost:.0})")
            }
            LeaderError::CannotAfford { missing } => write!(
                f,
                "not enough resources (missing metal {:.0}, crystal {:.0}, deuterium {:.0})",
                missing.iron, missing.silicon, missing.deuterium
            ),
            LeaderError::Unassigned(id) => write!(f, "leader #{id} has no assignment"),
        }
    }
}

impl std::error::Error for LeaderError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Actor {
    Player,
    Leader(u64),
}

impl Actor {
    pub fn as_key(self) -> String {
        match self {
            Actor::Player => PLAYER.to_string(),
            Actor::Leader(id) => format!("leader:{id}"),
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        if text == PLAYER {
            return Some(Actor::Player);
        }
        let rest = text.strip_prefix("leader:").unwrap_or(text);
        rest.parse().ok().map(Actor::Leader)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    QueueBuild,
    AcceptBattle,
    ProposeLaw,
    AssignLeader,
    Delegate,
}

impl Permission {
    pub fn as_key(self) -> &'static str {
        match self {
            Permission::QueueBuild => "queue_build",
            Permission::AcceptBattle => "accept_battle",
            Permission::ProposeLaw => "propose_law",
            Permission::AssignLeader => "assign_leader",
            Permission::Delegate => "delegate",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "queue_build" => Permission::QueueBuild,
            "accept_battle" => Permission::AcceptBattle,
            "propose_law" => Permission::ProposeLaw,
            "assign_leader" => Permission::AssignLeader,
            "delegate" => Permission::Delegate,
            _ => return None,
        })
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoleKind {
    Admiral,
    Governor,
    Minister,
}

impl RoleKind {
    pub const ALL: [RoleKind; 3] = [RoleKind::Admiral, RoleKind::Governor, RoleKind::Minister];

    pub fn as_key(self) -> &'static str {
        match self {
            RoleKind::Admiral => "admiral",
            RoleKind::Governor => "governor",
            RoleKind::Minister => "minister",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "admiral" => RoleKind::Admiral,
            "governor" => RoleKind::Governor,
            "minister" => RoleKind::Minister,
            _ => return None,
        })
    }

    pub fn grants(self, permission: Permission) -> bool {
        matches!(
            (self, permission),
            (RoleKind::Governor, Permission::QueueBuild)
                | (RoleKind::Admiral, Permission::AcceptBattle)
                | (RoleKind::Minister, Permission::ProposeLaw)
        )
    }
}

impl fmt::Display for RoleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentTarget {
    HomeFleet,
    Planet(Coord),
    Empire,
}

impl AssignmentTarget {
    pub fn as_key(self) -> String {
        match self {
            AssignmentTarget::HomeFleet => "fleet:home".into(),
            AssignmentTarget::Planet(coord) => format!("planet:{coord}"),
            AssignmentTarget::Empire => "empire".into(),
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        if text == "fleet:home" || text == "home_fleet" {
            return Some(AssignmentTarget::HomeFleet);
        }
        if text == "empire" {
            return Some(AssignmentTarget::Empire);
        }
        let rest = text.strip_prefix("planet:").unwrap_or(text);
        if rest == "home" {
            return Some(AssignmentTarget::Planet(Coord::HOME));
        }
        parse_coord(rest).map(AssignmentTarget::Planet)
    }
}

fn parse_coord(text: &str) -> Option<Coord> {
    let mut parts = text.split(':');
    let galaxy = parts.next()?.parse().ok()?;
    let system = parts.next()?.parse().ok()?;
    let position = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Coord {
        galaxy,
        system,
        position,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitSpec {
    pub id: String,
    pub name: String,
    pub portrait: String,
    pub attack: f64,
    pub shield: f64,
    pub production: f64,
    pub policy: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Leader {
    pub id: u64,
    pub name: String,
    pub species: String,
    pub trait_id: String,
    pub skill_command: u32,
    pub skill_logistics: u32,
    pub skill_policy: u32,
    pub xp: u32,
    pub born_at: u64,
    pub lifespan_secs: u64,
    pub retired: bool,
}

impl Leader {
    pub fn retires_at(&self) -> u64 {
        self.born_at.saturating_add(self.lifespan_secs)
    }

    pub fn as_admiral(&self) -> crate::Admiral {
        crate::Admiral {
            id: self.id,
            name: self.name.clone(),
            trait_id: self.trait_id.clone(),
            xp: self.xp,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub leader_id: u64,
    pub role: RoleKind,
    pub target: AssignmentTarget,
    pub assigned_at: u64,
    pub ready_at: u64,
}

impl Assignment {
    pub fn ready(&self, now: u64) -> bool {
        now >= self.ready_at
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delegation {
    pub leader_id: u64,
    pub permission: Permission,
    pub spend_cap: Option<f64>,
    pub spent: f64,
    pub period_start: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LeaderConfig {
    pub reassign_secs: u64,
    pub lifespan_secs: u64,
    pub period_secs: u64,
    pub empty_governor: f64,
    pub empty_admiral: f64,
    pub recruit_cost: crate::state::Resources,
    traits: Vec<TraitSpec>,
}

impl LeaderConfig {
    pub fn parse(toml: &str) -> Result<Self, LeaderError> {
        let file: File = toml::from_str(toml).map_err(|e| LeaderError::Parse(e.to_string()))?;
        file.into_config()
    }

    pub fn trait_by_id(&self, id: &str) -> Option<&TraitSpec> {
        self.traits.iter().find(|row| row.id == id)
    }

    pub fn production(&self, trait_id: &str) -> f64 {
        self.trait_by_id(trait_id)
            .map(|row| row.production)
            .unwrap_or(1.0)
    }

    pub fn iter_traits(&self) -> impl Iterator<Item = &TraitSpec> {
        self.traits.iter()
    }

    pub fn starter(
        &self,
        now: u64,
    ) -> (
        Vec<Leader>,
        Vec<Assignment>,
        Vec<crate::Admiral>,
        Option<u64>,
        u64,
    ) {
        let admiral = Leader {
            id: 1,
            name: "Home fleet".into(),
            species: "human".into(),
            trait_id: "none".into(),
            skill_command: 1,
            skill_logistics: 0,
            skill_policy: 0,
            xp: 0,
            born_at: now,
            lifespan_secs: self.lifespan_secs,
            retired: false,
        };
        let governor = Leader {
            id: 2,
            name: "Home colony".into(),
            species: "human".into(),
            trait_id: "none".into(),
            skill_command: 0,
            skill_logistics: 1,
            skill_policy: 0,
            xp: 0,
            born_at: now,
            lifespan_secs: self.lifespan_secs,
            retired: false,
        };
        let assignments = vec![
            Assignment {
                leader_id: 1,
                role: RoleKind::Admiral,
                target: AssignmentTarget::HomeFleet,
                assigned_at: now,
                ready_at: now,
            },
            Assignment {
                leader_id: 2,
                role: RoleKind::Governor,
                target: AssignmentTarget::Planet(Coord::HOME),
                assigned_at: now,
                ready_at: now,
            },
        ];
        let admirals = vec![admiral.as_admiral()];
        (vec![admiral, governor], assignments, admirals, Some(1), 3)
    }
}

impl Default for LeaderConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_LEADERS).expect("bundled leaders.toml must be valid")
    }
}

#[derive(Deserialize)]
struct File {
    reassign_secs: u64,
    lifespan_secs: u64,
    period_secs: u64,
    empty_governor: f64,
    empty_admiral: f64,
    recruit_metal: f64,
    recruit_crystal: f64,
    recruit_deuterium: f64,
    traits: Vec<FileTrait>,
}

#[derive(Deserialize)]
struct FileTrait {
    id: String,
    name: String,
    portrait: String,
    attack: f64,
    shield: f64,
    production: f64,
    policy: f64,
}

impl File {
    fn into_config(self) -> Result<LeaderConfig, LeaderError> {
        let mut traits = Vec::with_capacity(self.traits.len());
        for row in self.traits {
            if traits.iter().any(|t: &TraitSpec| t.id == row.id) {
                return Err(LeaderError::DuplicateTrait(row.id));
            }
            traits.push(TraitSpec {
                id: row.id,
                name: row.name,
                portrait: row.portrait,
                attack: row.attack.max(0.0),
                shield: row.shield.max(0.0),
                production: row.production.max(0.0),
                policy: row.policy.max(0.0),
            });
        }
        Ok(LeaderConfig {
            reassign_secs: self.reassign_secs,
            lifespan_secs: self.lifespan_secs.max(1),
            period_secs: self.period_secs.max(1),
            empty_governor: self.empty_governor.clamp(0.0, 1.0),
            empty_admiral: self.empty_admiral.clamp(0.0, 1.0),
            recruit_cost: crate::state::Resources::from_parts(
                self.recruit_metal.max(0.0),
                self.recruit_crystal.max(0.0),
                self.recruit_deuterium.max(0.0),
            ),
            traits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses() {
        let cfg = LeaderConfig::default();
        assert!(cfg.trait_by_id("none").is_some());
        assert!(cfg.trait_by_id("logistics").is_some());
        assert!(cfg.production("logistics") > 1.0);
        assert!(cfg.production("logistics") < 1.1);
    }

    #[test]
    fn actor_round_trips() {
        assert_eq!(Actor::parse("player"), Some(Actor::Player));
        assert_eq!(Actor::parse("leader:4"), Some(Actor::Leader(4)));
        assert_eq!(Actor::parse("4"), Some(Actor::Leader(4)));
        assert_eq!(Actor::Leader(4).as_key(), "leader:4");
    }

    #[test]
    fn roles_grant_expected_permissions() {
        assert!(RoleKind::Governor.grants(Permission::QueueBuild));
        assert!(!RoleKind::Admiral.grants(Permission::QueueBuild));
        assert!(RoleKind::Admiral.grants(Permission::AcceptBattle));
        assert!(RoleKind::Minister.grants(Permission::ProposeLaw));
    }
}
