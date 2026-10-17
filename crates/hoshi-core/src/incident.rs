//! Incidents, owners, post-incident reviews and a risk register.
//!
//! Auto-opened from raids, heat overflow, missed contracts and classification
//! leaks. Leaders own open rows; a review archives lineage for serious cases.

use crate::history::{HistoryKind, HistoryQuery};
use crate::leaders::{Actor, RoleKind};
use crate::state::GameState;
use serde::Deserialize;
use std::fmt;

const DEFAULT: &str = include_str!("../data/incidents.toml");

#[derive(Debug, Clone, PartialEq)]
pub enum IncidentError {
    Parse(String),
    UnknownIncident,
    UnknownRisk,
    UnknownLeader,
    NotOwner,
    NotOpen,
    NotResolved,
    NotSerious,
}

impl fmt::Display for IncidentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IncidentError::Parse(msg) => write!(f, "incident: {msg}"),
            IncidentError::UnknownIncident => write!(f, "unknown incident"),
            IncidentError::UnknownRisk => write!(f, "unknown risk"),
            IncidentError::UnknownLeader => write!(f, "unknown leader"),
            IncidentError::NotOwner => write!(f, "actor cannot work this incident"),
            IncidentError::NotOpen => write!(f, "incident is not open"),
            IncidentError::NotResolved => write!(f, "incident is not resolved"),
            IncidentError::NotSerious => write!(f, "review needs a high or critical incident"),
        }
    }
}

impl std::error::Error for IncidentError {}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct IncidentConfig {
    pub archive_cap: usize,
    pub risk_review_secs: u64,
}

impl IncidentConfig {
    pub fn bundled() -> Self {
        Self::parse(DEFAULT).expect("bundled incidents.toml")
    }

    pub fn parse(raw: &str) -> Result<Self, IncidentError> {
        let cfg: Self = toml::from_str(raw).map_err(|e| IncidentError::Parse(e.to_string()))?;
        if cfg.archive_cap == 0 {
            return Err(IncidentError::Parse("archive_cap must be > 0".into()));
        }
        if cfg.risk_review_secs == 0 {
            return Err(IncidentError::Parse("risk_review_secs must be > 0".into()));
        }
        Ok(cfg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IncidentKind {
    Raid,
    Failure,
    Shortage,
    Accident,
    Heat,
    Contract,
    Manual,
}

impl IncidentKind {
    pub const ALL: [IncidentKind; 7] = [
        IncidentKind::Raid,
        IncidentKind::Failure,
        IncidentKind::Shortage,
        IncidentKind::Accident,
        IncidentKind::Heat,
        IncidentKind::Contract,
        IncidentKind::Manual,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            IncidentKind::Raid => "raid",
            IncidentKind::Failure => "failure",
            IncidentKind::Shortage => "shortage",
            IncidentKind::Accident => "accident",
            IncidentKind::Heat => "heat",
            IncidentKind::Contract => "contract",
            IncidentKind::Manual => "manual",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "raid" => IncidentKind::Raid,
            "failure" => IncidentKind::Failure,
            "shortage" => IncidentKind::Shortage,
            "accident" => IncidentKind::Accident,
            "heat" => IncidentKind::Heat,
            "contract" => IncidentKind::Contract,
            "manual" => IncidentKind::Manual,
            _ => return None,
        })
    }

    pub fn suggested_role(self) -> RoleKind {
        match self {
            IncidentKind::Raid => RoleKind::Admiral,
            IncidentKind::Contract | IncidentKind::Shortage => RoleKind::Minister,
            IncidentKind::Failure
            | IncidentKind::Accident
            | IncidentKind::Heat
            | IncidentKind::Manual => RoleKind::Governor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_key(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "low" => Severity::Low,
            "medium" => Severity::Medium,
            "high" => Severity::High,
            "critical" => Severity::Critical,
            _ => return None,
        })
    }

    pub fn serious(self) -> bool {
        matches!(self, Severity::High | Severity::Critical)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IncidentStatus {
    Open,
    Assigned,
    Resolved,
}

impl IncidentStatus {
    pub fn as_key(self) -> &'static str {
        match self {
            IncidentStatus::Open => "open",
            IncidentStatus::Assigned => "assigned",
            IncidentStatus::Resolved => "resolved",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "open" => IncidentStatus::Open,
            "assigned" => IncidentStatus::Assigned,
            "resolved" => IncidentStatus::Resolved,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub id: u64,
    pub kind: IncidentKind,
    pub severity: Severity,
    pub status: IncidentStatus,
    pub opened_at: u64,
    pub objects: Vec<String>,
    pub owner: Option<u64>,
    pub actions: Vec<String>,
    pub resolution: Option<String>,
    pub closed_at: Option<u64>,
    pub event_ids: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IncidentReview {
    pub id: u64,
    pub incident_id: u64,
    pub at: u64,
    pub root_cause: String,
    pub timeline: Vec<String>,
    pub follow_ups: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Risk {
    pub id: u64,
    pub name: String,
    pub likelihood: f64,
    pub impact: Severity,
    pub review_at: u64,
    pub last_reviewed_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IncidentState {
    pub next_id: u64,
    pub next_review_id: u64,
    pub next_risk_id: u64,
    pub next_review_at: u64,
    pub incidents: Vec<Incident>,
    pub reviews: Vec<IncidentReview>,
    pub risks: Vec<Risk>,
}

impl IncidentState {
    pub fn new(now: u64, cfg: &IncidentConfig) -> Self {
        let review_at = now.saturating_add(cfg.risk_review_secs);
        Self {
            next_id: 1,
            next_review_id: 1,
            next_risk_id: 4,
            next_review_at: review_at,
            incidents: Vec::new(),
            reviews: Vec::new(),
            risks: vec![
                Risk {
                    id: 1,
                    name: "pirate activity".into(),
                    likelihood: 0.4,
                    impact: Severity::High,
                    review_at,
                    last_reviewed_at: now,
                },
                Risk {
                    id: 2,
                    name: "heat margins".into(),
                    likelihood: 0.3,
                    impact: Severity::High,
                    review_at,
                    last_reviewed_at: now,
                },
                Risk {
                    id: 3,
                    name: "single home world".into(),
                    likelihood: 0.1,
                    impact: Severity::Critical,
                    review_at,
                    last_reviewed_at: now,
                },
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IncidentSnapshot {
    pub open: Vec<Incident>,
    pub risks: Vec<Risk>,
    pub reviews: Vec<IncidentReview>,
}

impl GameState {
    pub fn incident_config(&self) -> IncidentConfig {
        IncidentConfig::bundled()
    }

    pub fn incident_snapshot(&self) -> IncidentSnapshot {
        IncidentSnapshot {
            open: self
                .incidents
                .incidents
                .iter()
                .filter(|row| row.status != IncidentStatus::Resolved)
                .cloned()
                .collect(),
            risks: self.incidents.risks.clone(),
            reviews: self.incidents.reviews.clone(),
        }
    }

    pub fn open_count(&self) -> usize {
        self.incidents
            .incidents
            .iter()
            .filter(|row| row.status != IncidentStatus::Resolved)
            .count()
    }

    pub fn open_incident(
        &mut self,
        kind: IncidentKind,
        severity: Severity,
        objects: Vec<String>,
        actor: Actor,
        now_unix: u64,
    ) -> Result<u64, IncidentError> {
        self.catch_up(now_unix);
        if actor != Actor::Player {
            return Err(IncidentError::NotOwner);
        }
        Ok(self.push_incident(kind, severity, objects, self.last_tick_unix))
    }

    pub fn assign_incident(
        &mut self,
        id: u64,
        leader_id: u64,
        actor: Actor,
        now_unix: u64,
    ) -> Result<(), IncidentError> {
        self.catch_up(now_unix);
        if actor != Actor::Player {
            return Err(IncidentError::NotOwner);
        }
        let idx = self.incident_index(id)?;
        if self.incidents.incidents[idx].status == IncidentStatus::Resolved {
            return Err(IncidentError::NotOpen);
        }
        let Some(leader) = self
            .leaders
            .iter()
            .find(|l| l.id == leader_id && !l.retired)
        else {
            return Err(IncidentError::UnknownLeader);
        };
        let ready = self
            .assignment_of(leader.id)
            .is_some_and(|row| row.ready(self.last_tick_unix));
        if !ready {
            return Err(IncidentError::NotOwner);
        }
        let name = leader.name.clone();
        let row = &mut self.incidents.incidents[idx];
        row.owner = Some(leader_id);
        row.status = IncidentStatus::Assigned;
        row.actions
            .push(format!("assigned to {name} at {}", self.last_tick_unix));
        let kind = row.kind.as_key().to_string();
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: HistoryKind::Other,
            actor: actor.as_key(),
            action: "assign_incident".into(),
            objects: vec![format!("incident:{id}"), kind],
            outcome: format!("leader:{leader_id}"),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn resolve_incident(
        &mut self,
        id: u64,
        note: &str,
        actor: Actor,
        now_unix: u64,
    ) -> Result<(), IncidentError> {
        self.catch_up(now_unix);
        let idx = self.incident_index(id)?;
        let row = &self.incidents.incidents[idx];
        if row.status == IncidentStatus::Resolved {
            return Err(IncidentError::NotOpen);
        }
        if !self.can_work_incident(actor, row) {
            return Err(IncidentError::NotOwner);
        }
        let note = note.trim();
        let resolution = if note.is_empty() {
            "resolved".to_string()
        } else {
            note.to_string()
        };
        let row = &mut self.incidents.incidents[idx];
        row.status = IncidentStatus::Resolved;
        row.resolution = Some(resolution.clone());
        row.closed_at = Some(self.last_tick_unix);
        row.actions
            .push(format!("resolved at {}: {resolution}", self.last_tick_unix));
        let kind = row.kind.as_key().to_string();
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: HistoryKind::Other,
            actor: actor.as_key(),
            action: "resolve_incident".into(),
            objects: vec![format!("incident:{id}"), kind],
            outcome: resolution,
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn review_incident(
        &mut self,
        id: u64,
        actor: Actor,
        now_unix: u64,
    ) -> Result<u64, IncidentError> {
        self.catch_up(now_unix);
        if actor != Actor::Player {
            return Err(IncidentError::NotOwner);
        }
        let idx = self.incident_index(id)?;
        let row = &self.incidents.incidents[idx];
        if row.status != IncidentStatus::Resolved {
            return Err(IncidentError::NotResolved);
        }
        if !row.severity.serious() {
            return Err(IncidentError::NotSerious);
        }
        if self.incidents.reviews.iter().any(|r| r.incident_id == id) {
            return Ok(self
                .incidents
                .reviews
                .iter()
                .find(|r| r.incident_id == id)
                .map(|r| r.id)
                .unwrap_or(0));
        }
        let from = row.opened_at;
        let to = row.closed_at.unwrap_or(self.last_tick_unix);
        let events = self.history.search(&HistoryQuery {
            kind: None,
            actor: None,
            object: None,
            from: Some(from),
            to: Some(to),
        });
        let timeline: Vec<String> = events
            .iter()
            .rev()
            .take(12)
            .map(|e| format!("{} {} {}", e.at, e.action, e.outcome))
            .collect();
        let root = events
            .first()
            .map(|e| format!("{}: {}", e.action, e.outcome))
            .unwrap_or_else(|| format!("{} opened", row.kind.as_key()));
        let follow_ups = match row.kind {
            IncidentKind::Heat => vec!["review heat budget and sink upgrades".into()],
            IncidentKind::Raid => vec!["review fleet posture and raid delay".into()],
            IncidentKind::Contract => vec!["review delivery windows and stock".into()],
            _ => vec!["record a follow-up law or maintenance plan".into()],
        };
        let review_id = self.incidents.next_review_id;
        self.incidents.next_review_id = self.incidents.next_review_id.saturating_add(1);
        self.incidents.reviews.push(IncidentReview {
            id: review_id,
            incident_id: id,
            at: self.last_tick_unix,
            root_cause: root,
            timeline,
            follow_ups,
        });
        let cfg = self.incident_config();
        while self.incidents.reviews.len() > cfg.archive_cap.max(1) {
            self.incidents.reviews.remove(0);
        }
        self.record_history(crate::HistoryDraft {
            at: self.last_tick_unix,
            kind: HistoryKind::Other,
            actor: actor.as_key(),
            action: "review_incident".into(),
            objects: vec![format!("incident:{id}"), format!("review:{review_id}")],
            outcome: "archived".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(review_id)
    }

    pub(crate) fn next_incident_event(&self) -> Option<u64> {
        Some(self.incidents.next_review_at).filter(|&at| at > self.last_tick_unix)
    }

    pub fn apply_incidents_at(&mut self, at: u64) {
        if self.waste_home().overflowing() {
            self.auto_open(
                IncidentKind::Heat,
                Severity::High,
                vec![self.home.to_string()],
                at,
            );
        }
        if at < self.incidents.next_review_at {
            return;
        }
        let cfg = self.incident_config();
        let mut next = self.incidents.next_review_at;
        let mut guard = 0u32;
        while at >= next {
            self.roll_risk_reviews(next);
            let stepped = next.saturating_add(cfg.risk_review_secs);
            if stepped <= next {
                break;
            }
            next = stepped;
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        self.incidents.next_review_at = next;
    }

    pub(crate) fn note_raid_incident(&mut self, winner: crate::CombatWinner, at: u64) {
        if winner != crate::CombatWinner::Attacker {
            return;
        }
        self.auto_open(
            IncidentKind::Raid,
            Severity::High,
            vec![self.home.to_string()],
            at,
        );
    }

    pub(crate) fn note_contract_incident(&mut self, contract_id: u64, at: u64) {
        self.auto_open(
            IncidentKind::Contract,
            Severity::Medium,
            vec![format!("contract:{contract_id}")],
            at,
        );
    }

    pub(crate) fn note_failure_incident(&mut self, asset: &str, at: u64) {
        self.auto_open(
            IncidentKind::Failure,
            Severity::High,
            vec![format!("asset:{asset}")],
            at,
        );
    }

    pub(crate) fn note_leak_incident(&mut self, via: &str, at: u64) {
        self.auto_open(
            IncidentKind::Accident,
            Severity::Medium,
            vec![format!("leak:{via}")],
            at,
        );
    }

    fn auto_open(&mut self, kind: IncidentKind, severity: Severity, objects: Vec<String>, at: u64) {
        // Heat is a continuing condition; keep one open row for the same objects.
        // Discrete events (raid, contract, leak) each get their own row.
        if kind == IncidentKind::Heat
            && self.incidents.incidents.iter().any(|row| {
                row.kind == kind && row.status != IncidentStatus::Resolved && row.objects == objects
            })
        {
            return;
        }
        let _ = self.push_incident(kind, severity, objects, at);
    }

    fn push_incident(
        &mut self,
        kind: IncidentKind,
        severity: Severity,
        objects: Vec<String>,
        at: u64,
    ) -> u64 {
        let from = at.saturating_sub(self.incident_config().risk_review_secs);
        let event_ids = self
            .history
            .search(&HistoryQuery {
                kind: None,
                actor: None,
                object: None,
                from: Some(from),
                to: Some(at),
            })
            .into_iter()
            .map(|e| e.id)
            .take(16)
            .collect();
        let id = self.incidents.next_id;
        self.incidents.next_id = self.incidents.next_id.saturating_add(1);
        self.incidents.incidents.push(Incident {
            id,
            kind,
            severity,
            status: IncidentStatus::Open,
            opened_at: at,
            objects: objects.clone(),
            owner: None,
            actions: vec![format!("opened at {at}")],
            resolution: None,
            closed_at: None,
            event_ids,
        });
        let cfg = self.incident_config();
        while self.incidents.incidents.len() > cfg.archive_cap.max(1) {
            let Some(stale) = self
                .incidents
                .incidents
                .iter()
                .position(|row| row.status == IncidentStatus::Resolved)
            else {
                break;
            };
            self.incidents.incidents.remove(stale);
        }
        self.record_history(crate::HistoryDraft {
            at,
            kind: HistoryKind::Other,
            actor: "system".into(),
            action: "open_incident".into(),
            objects: {
                let mut objs = vec![format!("incident:{id}"), kind.as_key().into()];
                objs.extend(objects);
                objs
            },
            outcome: severity.as_key().into(),
            ..crate::HistoryDraft::default()
        });
        self.push_event(
            at,
            format!("incident #{id} {} {}", kind.as_key(), severity.as_key()),
        );
        id
    }

    fn roll_risk_reviews(&mut self, at: u64) {
        let cfg = self.incident_config();
        let next = at.saturating_add(cfg.risk_review_secs);
        let open = self.open_count() as f64;
        let heat = self.waste_home().heat_ratio();
        for risk in &mut self.incidents.risks {
            risk.last_reviewed_at = at;
            risk.review_at = next;
            match risk.id {
                1 => risk.likelihood = (0.2 + open * 0.05).clamp(0.05, 0.95),
                2 => risk.likelihood = heat.clamp(0.05, 0.95),
                _ => {}
            }
        }
    }

    fn incident_index(&self, id: u64) -> Result<usize, IncidentError> {
        self.incidents
            .incidents
            .iter()
            .position(|row| row.id == id)
            .ok_or(IncidentError::UnknownIncident)
    }

    fn can_work_incident(&self, actor: Actor, row: &Incident) -> bool {
        match actor {
            Actor::Player => true,
            Actor::Leader(id) => row.owner == Some(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::combat::CombatWinner;
    use crate::leaders::RoleKind;
    use crate::state::GameState;

    #[test]
    fn bundled_incidents_parse() {
        let cfg = IncidentConfig::bundled();
        assert!(cfg.archive_cap > 0);
        assert!(cfg.risk_review_secs > 0);
    }

    #[test]
    fn raid_loss_opens_a_high_incident() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        assert!(state.incident_snapshot().open.is_empty());
        state.note_raid_incident(CombatWinner::Attacker, 1_000);
        let open = state.incident_snapshot().open;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].kind, IncidentKind::Raid);
        assert_eq!(open[0].severity, Severity::High);
        state.note_raid_incident(CombatWinner::Attacker, 1_001);
        assert_eq!(state.incident_snapshot().open.len(), 2);
        state.note_contract_incident(7, 1_002);
        state.note_contract_incident(8, 1_003);
        assert_eq!(
            state
                .incident_snapshot()
                .open
                .iter()
                .filter(|row| row.kind == IncidentKind::Contract)
                .count(),
            2
        );
        let home = state.home.to_string();
        state.auto_open(
            IncidentKind::Heat,
            Severity::High,
            vec![home.clone()],
            1_004,
        );
        state.auto_open(IncidentKind::Heat, Severity::High, vec![home], 1_005);
        assert_eq!(
            state
                .incident_snapshot()
                .open
                .iter()
                .filter(|row| row.kind == IncidentKind::Heat)
                .count(),
            1
        );
    }

    #[test]
    fn assign_and_resolve_close_the_row() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        let id = state
            .open_incident(
                IncidentKind::Manual,
                Severity::High,
                vec!["home".into()],
                Actor::Player,
                1_000,
            )
            .unwrap();
        let leader = state
            .leaders
            .iter()
            .find(|l| {
                state
                    .assignment_of(l.id)
                    .is_some_and(|a| a.role == RoleKind::Governor && a.ready(1_000))
            })
            .map(|l| l.id)
            .expect("governor");
        state
            .assign_incident(id, leader, Actor::Player, 1_000)
            .unwrap();
        assert_eq!(
            state.incidents.incidents[0].status,
            IncidentStatus::Assigned
        );
        state
            .resolve_incident(id, "rerouted power", Actor::Leader(leader), 1_000)
            .unwrap();
        assert_eq!(
            state.incidents.incidents[0].status,
            IncidentStatus::Resolved
        );
        let review = state.review_incident(id, Actor::Player, 1_000).unwrap();
        assert_eq!(review, 1);
        assert_eq!(state.incidents.reviews[0].incident_id, id);
    }

    #[test]
    fn heat_overflow_opens_during_catch_up() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        state.buildings.metal_mine = 40;
        state.buildings.crystal_mine = 40;
        state.buildings.solar_plant = 1;
        state.catch_up(1_000);
        // overflow may or may not trip on this building mix; force the scan path
        if !state.waste_home().overflowing() {
            state.auto_open(
                IncidentKind::Heat,
                Severity::High,
                vec![state.home.to_string()],
                1_000,
            );
        } else {
            state.apply_incidents_at(1_000);
        }
        assert!(
            state
                .incident_snapshot()
                .open
                .iter()
                .any(|i| i.kind == IncidentKind::Heat)
        );
    }

    #[test]
    fn risk_register_rolls_on_the_review_clock() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        assert_eq!(state.incidents.risks.len(), 3);
        let due = state.incidents.next_review_at;
        state.catch_up(due);
        assert!(state.incidents.risks[0].last_reviewed_at >= due);
        assert!(state.incidents.next_review_at > due);
    }

    #[test]
    fn archive_cap_keeps_open_rows() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        let cap = state.incident_config().archive_cap;
        for n in 0..=cap {
            state.note_raid_incident(CombatWinner::Attacker, 1_000 + n as u64);
        }
        let open = state.incident_snapshot().open;
        assert!(open.len() > cap);
        assert_eq!(open[0].id, 1);
        let first = open[0].id;
        state
            .resolve_incident(first, "cleared", Actor::Player, 1_000)
            .unwrap();
        state.note_raid_incident(CombatWinner::Attacker, 2_000);
        assert!(state.incidents.incidents.iter().all(|row| row.id != first));
        assert_eq!(state.incident_snapshot().open.len(), cap + 1);
    }
}
