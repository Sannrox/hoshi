//! Classification markings, role clearances, sharing agreements and leaks.
//!
//! Missing markings are Open. Hidden data stays in core; callers ask through
//! the current actor's clearance.

use crate::fleets::Coord;
use crate::leaders::{Actor, RoleKind};
use crate::state::GameState;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const COMPARTMENT_INTEL: &str = "intel";
pub const COMPARTMENT_FLEET: &str = "fleet";
pub const COMPARTMENT_POLICY: &str = "policy";
pub const COMPARTMENT_PLANET: &str = "planet";

const ALLIED_TIERS: [&str; 3] = ["favored", "honored", "truce"];
const DISLOYAL: &str = "disloyal";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Classification {
    #[default]
    Open,
    Internal,
    Restricted,
    Secret,
}

impl Classification {
    pub fn as_key(self) -> &'static str {
        match self {
            Classification::Open => "open",
            Classification::Internal => "internal",
            Classification::Restricted => "restricted",
            Classification::Secret => "secret",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "open" => Classification::Open,
            "internal" => Classification::Internal,
            "restricted" => Classification::Restricted,
            "secret" => Classification::Secret,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClassifiedKind {
    Report,
    Law,
    Plan,
    Fact,
}

impl ClassifiedKind {
    pub fn as_key(self) -> &'static str {
        match self {
            ClassifiedKind::Report => "report",
            ClassifiedKind::Law => "law",
            ClassifiedKind::Plan => "plan",
            ClassifiedKind::Fact => "fact",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "report" => ClassifiedKind::Report,
            "law" => ClassifiedKind::Law,
            "plan" => ClassifiedKind::Plan,
            "fact" => ClassifiedKind::Fact,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClassifiedRef {
    pub kind: ClassifiedKind,
    pub id: String,
}

impl ClassifiedRef {
    pub fn new(kind: ClassifiedKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Marking {
    pub classification: Classification,
    pub compartments: BTreeSet<String>,
}

impl Marking {
    pub fn open() -> Self {
        Self::default()
    }

    pub fn new(
        classification: Classification,
        compartments: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            classification,
            compartments: compartments.into_iter().filter(|c| !c.is_empty()).collect(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.classification == Classification::Open && self.compartments.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Clearance {
    pub classification: Classification,
    pub compartments: BTreeSet<String>,
    pub unrestricted: bool,
}

impl Clearance {
    pub fn player() -> Self {
        Self {
            classification: Classification::Secret,
            compartments: BTreeSet::new(),
            unrestricted: true,
        }
    }

    pub fn role(role: RoleKind) -> Self {
        match role {
            RoleKind::Admiral => Self {
                classification: Classification::Restricted,
                compartments: BTreeSet::from([COMPARTMENT_FLEET.into()]),
                unrestricted: false,
            },
            RoleKind::Governor => Self {
                classification: Classification::Internal,
                compartments: BTreeSet::from([COMPARTMENT_PLANET.into()]),
                unrestricted: false,
            },
            RoleKind::Minister => Self {
                classification: Classification::Internal,
                compartments: BTreeSet::from([COMPARTMENT_POLICY.into()]),
                unrestricted: false,
            },
        }
    }

    pub fn merge(&self, extra: &Clearance) -> Clearance {
        if self.unrestricted || extra.unrestricted {
            return Clearance::player();
        }
        let classification = self.classification.max(extra.classification);
        let mut compartments = self.compartments.clone();
        compartments.extend(extra.compartments.iter().cloned());
        Clearance {
            classification,
            compartments,
            unrestricted: false,
        }
    }

    pub fn can_see(&self, marking: &Marking) -> bool {
        if self.unrestricted || marking.classification == Classification::Open {
            return true;
        }
        self.classification >= marking.classification
            && marking
                .compartments
                .iter()
                .all(|c| self.compartments.contains(c))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharingAgreement {
    pub faction: String,
    pub classification: Classification,
    pub compartments: BTreeSet<String>,
}

impl SharingAgreement {
    pub fn covers(&self, marking: &Marking) -> bool {
        if marking.classification == Classification::Open {
            return true;
        }
        self.classification >= marking.classification
            && marking
                .compartments
                .iter()
                .all(|c| self.compartments.contains(c))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeakIncident {
    pub id: u64,
    pub at: u64,
    pub source: String,
    pub kind: ClassifiedKind,
    pub object_id: String,
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassificationBoard {
    pub objects: BTreeMap<ClassifiedRef, Marking>,
    pub agreements: BTreeMap<String, SharingAgreement>,
    pub incidents: Vec<LeakIncident>,
    pub grants: BTreeMap<u64, Clearance>,
    pub next_incident_id: u64,
    pub next_disloyal_at: u64,
}

impl ClassificationBoard {
    pub fn marking(&self, kind: ClassifiedKind, id: &str) -> Marking {
        self.objects
            .get(&ClassifiedRef::new(kind, id))
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_marking(&mut self, kind: ClassifiedKind, id: &str, marking: Marking) {
        let key = ClassifiedRef::new(kind, id);
        if marking.is_open() {
            self.objects.remove(&key);
        } else {
            self.objects.insert(key, marking);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassificationError {
    UnknownKind(String),
    UnknownClass(String),
    UnknownObject { kind: ClassifiedKind, id: String },
    UnknownFaction(String),
    NotAllied(String),
    UnknownLeader(u64),
    InsufficientClearance,
    InvalidCompartment(String),
}

impl fmt::Display for ClassificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClassificationError::UnknownKind(id) => write!(f, "unknown classified kind {id}"),
            ClassificationError::UnknownClass(id) => write!(f, "unknown classification {id}"),
            ClassificationError::UnknownObject { kind, id } => {
                write!(f, "unknown {} {id}", kind.as_key())
            }
            ClassificationError::UnknownFaction(id) => write!(f, "unknown faction {id}"),
            ClassificationError::NotAllied(id) => {
                write!(f, "{id} is not allied enough to share markings")
            }
            ClassificationError::UnknownLeader(id) => write!(f, "unknown leader #{id}"),
            ClassificationError::InsufficientClearance => {
                write!(f, "clearance is too low for that marking")
            }
            ClassificationError::InvalidCompartment(id) => {
                write!(f, "invalid compartment {id}")
            }
        }
    }
}

impl std::error::Error for ClassificationError {}

pub fn join_compartments(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(",")
}

pub fn split_compartments(raw: &str) -> BTreeSet<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Compartment ids are lowercase tokens so comma-joined save rows stay round-trip safe.
pub fn parse_compartment(id: &str) -> Result<String, ClassificationError> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 32
        || !id.starts_with(|c: char| c.is_ascii_lowercase())
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(ClassificationError::InvalidCompartment(id.to_string()));
    }
    Ok(id.to_string())
}

fn collect_compartments(
    raw: impl IntoIterator<Item = String>,
) -> Result<BTreeSet<String>, ClassificationError> {
    let mut out = BTreeSet::new();
    for id in raw {
        if id.is_empty() {
            continue;
        }
        out.insert(parse_compartment(&id)?);
    }
    Ok(out)
}

impl GameState {
    pub fn marking_of(&self, kind: ClassifiedKind, id: &str) -> Marking {
        self.classification.marking(kind, id)
    }

    pub fn clearance_for(&self, actor: Actor) -> Clearance {
        match actor {
            Actor::Player => Clearance::player(),
            Actor::Leader(id) => {
                let Some(leader) = self.leader(id) else {
                    return Clearance::default();
                };
                if leader.retired {
                    return Clearance::default();
                }
                let Some(assignment) = self.assignment_of(id) else {
                    return self
                        .classification
                        .grants
                        .get(&id)
                        .cloned()
                        .unwrap_or_default();
                };
                if !assignment.ready(self.last_tick_unix) {
                    return Clearance::default();
                }
                let mut base = Clearance::role(assignment.role);
                if let Some(extra) = self.classification.grants.get(&id) {
                    base = base.merge(extra);
                }
                base
            }
        }
    }

    pub fn can_see(&self, actor: Actor, kind: ClassifiedKind, id: &str) -> bool {
        self.clearance_for(actor)
            .can_see(&self.marking_of(kind, id))
    }

    pub fn visible_reports(&self, actor: Actor) -> Vec<&crate::SpyReport> {
        self.spy_reports
            .iter()
            .filter(|row| row.delivered(self.last_tick_unix))
            .filter(|row| self.can_see(actor, ClassifiedKind::Report, &row.id.to_string()))
            .collect()
    }

    pub fn visible_laws(&self, actor: Actor) -> Vec<&crate::Law> {
        self.laws
            .laws
            .iter()
            .filter(|row| self.can_see(actor, ClassifiedKind::Law, &row.id.to_string()))
            .collect()
    }

    pub fn visible_plans(&self, actor: Actor) -> Vec<&crate::PlannedMission> {
        self.planned
            .iter()
            .filter(|row| self.can_see(actor, ClassifiedKind::Plan, &row.id.to_string()))
            .collect()
    }

    pub fn visible_incidents(&self, actor: Actor) -> Vec<&LeakIncident> {
        self.classification
            .incidents
            .iter()
            .filter(|row| row.at <= self.last_tick_unix)
            .filter(|row| self.can_see(actor, row.kind, &row.object_id))
            .collect()
    }

    /// Log lines whose timestamp has already arrived on the injected clock.
    pub fn delivered_events(&self) -> impl Iterator<Item = &crate::GameEvent> {
        self.events
            .iter()
            .filter(|row| row.at <= self.last_tick_unix)
    }

    pub fn visible_events(&self, actor: Actor) -> Vec<&crate::GameEvent> {
        self.delivered_events()
            .filter(|row| self.event_visible(actor, &row.text))
            .collect()
    }

    pub fn latest_report_for(&self, actor: Actor, coord: Coord) -> Option<&crate::SpyReport> {
        self.visible_reports(actor)
            .into_iter()
            .filter(|row| row.target == coord)
            .max_by_key(|row| (row.created_at, row.id))
    }

    pub fn faction_can_see(&self, faction: &str, kind: ClassifiedKind, id: &str) -> bool {
        let marking = self.marking_of(kind, id);
        if marking.is_open() {
            return true;
        }
        if self
            .classification
            .incidents
            .iter()
            .any(|row| row.via == faction && row.kind == kind && row.object_id == id)
        {
            return true;
        }
        self.classification
            .agreements
            .get(faction)
            .is_some_and(|row| row.covers(&marking))
    }

    pub fn classify(
        &mut self,
        actor: Actor,
        kind: ClassifiedKind,
        id: &str,
        classification: Classification,
        compartments: impl IntoIterator<Item = String>,
    ) -> Result<(), ClassificationError> {
        if !self.object_exists(kind, id) {
            return Err(ClassificationError::UnknownObject {
                kind,
                id: id.to_string(),
            });
        }
        let clearance = self.clearance_for(actor);
        if !clearance.can_see(&self.marking_of(kind, id)) {
            return Err(ClassificationError::InsufficientClearance);
        }
        let marking = Marking::new(classification, collect_compartments(compartments)?);
        if classification != Classification::Open && !clearance.can_see(&marking) {
            return Err(ClassificationError::InsufficientClearance);
        }
        self.classification.set_marking(kind, id, marking);
        Ok(())
    }

    pub fn share(
        &mut self,
        faction: &str,
        classification: Classification,
        compartments: impl IntoIterator<Item = String>,
    ) -> Result<(), ClassificationError> {
        if self.config.diplomacy.get(faction).is_none() {
            return Err(ClassificationError::UnknownFaction(faction.into()));
        }
        if classification == Classification::Open {
            self.classification.agreements.remove(faction);
            return Ok(());
        }
        if !self.faction_is_allied(faction) {
            return Err(ClassificationError::NotAllied(faction.into()));
        }
        self.classification.agreements.insert(
            faction.to_string(),
            SharingAgreement {
                faction: faction.to_string(),
                classification,
                compartments: collect_compartments(compartments)?,
            },
        );
        Ok(())
    }

    pub fn grant_clearance(
        &mut self,
        leader_id: u64,
        classification: Classification,
        compartments: impl IntoIterator<Item = String>,
    ) -> Result<(), ClassificationError> {
        if self.leader(leader_id).is_none() {
            return Err(ClassificationError::UnknownLeader(leader_id));
        }
        if classification == Classification::Open {
            self.classification.grants.remove(&leader_id);
            return Ok(());
        }
        self.classification.grants.insert(
            leader_id,
            Clearance {
                classification,
                compartments: collect_compartments(compartments)?,
                unrestricted: false,
            },
        );
        Ok(())
    }

    pub(crate) fn maybe_leak_spy(&mut self, at: u64, detected: bool) {
        if !detected {
            return;
        }
        let Some(target) = self.first_sensitive() else {
            return;
        };
        self.record_leak(at, "spy", target, "pirates");
    }

    pub(crate) fn tick_classification_leaks(&mut self, now: u64) {
        let disloyal: Vec<u64> = self
            .leaders
            .iter()
            .filter(|row| !row.retired && row.trait_id == DISLOYAL)
            .map(|row| row.id)
            .collect();
        if disloyal.is_empty() {
            self.classification.next_disloyal_at = 0;
            return;
        }
        if self.classification.next_disloyal_at == 0 {
            self.classification.next_disloyal_at =
                now.saturating_add(self.config.leaders.period_secs.max(1));
            return;
        }
        if now < self.classification.next_disloyal_at {
            return;
        }
        self.classification.next_disloyal_at =
            now.saturating_add(self.config.leaders.period_secs.max(1));
        if let Some(target) = self.first_sensitive() {
            let via = format!("leader:{}", disloyal[0]);
            self.record_leak(now, "disloyal", target, &via);
        }
    }

    pub(crate) fn schedule_disloyal_if_needed(&mut self, trait_id: &str, now: u64) {
        if trait_id != DISLOYAL {
            return;
        }
        if self.classification.next_disloyal_at == 0 {
            self.classification.next_disloyal_at =
                now.saturating_add(self.config.leaders.period_secs.max(1));
        }
    }

    fn faction_is_allied(&self, faction: &str) -> bool {
        let standing = self.diplomacy.standing(faction);
        let Some(spec) = self.config.diplomacy.get(faction) else {
            return false;
        };
        spec.tier(standing)
            .is_some_and(|tier| ALLIED_TIERS.contains(&tier.id.as_str()))
    }

    fn event_visible(&self, actor: Actor, text: &str) -> bool {
        if matches!(actor, Actor::Player) {
            return true;
        }
        for key in self.classification.objects.keys() {
            if self.can_see(actor, key.kind, &key.id) {
                continue;
            }
            let needle = format!("#{}", key.id);
            let hidden = match key.kind {
                ClassifiedKind::Law => text.contains("law") && text.contains(&needle),
                ClassifiedKind::Plan => {
                    (text.contains("mission") || text.contains("plan")) && text.contains(&needle)
                }
                ClassifiedKind::Report => self
                    .spy_reports
                    .iter()
                    .find(|row| row.id.to_string() == key.id)
                    .is_some_and(|row| {
                        text.contains(&row.target.to_string())
                            && (text.contains("spy") || text.contains("probe"))
                    }),
                ClassifiedKind::Fact => text.contains(&key.id),
            };
            if hidden {
                return false;
            }
        }
        if text.contains("classification leak")
            && self
                .classification
                .incidents
                .iter()
                .any(|row| !self.can_see(actor, row.kind, &row.object_id))
        {
            return false;
        }
        true
    }

    fn object_exists(&self, kind: ClassifiedKind, id: &str) -> bool {
        match kind {
            ClassifiedKind::Report => self.spy_reports.iter().any(|row| row.id.to_string() == id),
            ClassifiedKind::Law => self.laws.laws.iter().any(|row| row.id.to_string() == id),
            ClassifiedKind::Plan => self.planned.iter().any(|row| row.id.to_string() == id),
            ClassifiedKind::Fact => !id.is_empty(),
        }
    }

    fn first_sensitive(&self) -> Option<ClassifiedRef> {
        self.classification
            .objects
            .iter()
            .find(|(_, marking)| marking.classification >= Classification::Restricted)
            .map(|(key, _)| key.clone())
    }

    fn record_leak(&mut self, at: u64, source: &str, target: ClassifiedRef, via: &str) {
        if self
            .classification
            .incidents
            .iter()
            .any(|row| row.kind == target.kind && row.object_id == target.id && row.via == via)
        {
            return;
        }
        let id = self.classification.next_incident_id;
        self.classification.next_incident_id = id.saturating_add(1);
        self.classification.incidents.push(LeakIncident {
            id,
            at,
            source: source.into(),
            kind: target.kind,
            object_id: target.id,
            via: via.into(),
        });
        self.push_event(at, format!("classification leak via {via}"));
        self.note_leak_incident(via, at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::espionage::SpyReport;
    use crate::fleets::Coord;
    use crate::laws::{Jurisdiction, Law, LawStatus};
    use crate::planning::{Customer, LaunchWindow, Payload, PlannedMission, Route, TransferKind};
    use crate::state::Resources;

    fn report(id: u64) -> SpyReport {
        SpyReport {
            id,
            target: Coord::HOME,
            created_at: 10,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 1.0,
            tier: "fleet".into(),
            resources: None,
            fleet: None,
            defences: None,
            garrison: None,
            kind: None,
            size: None,
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 10,
            arrives_at: 0,
            confidence: 1.0,
            deceived: false,
        }
    }

    fn law(id: u64) -> Law {
        Law {
            id,
            template_id: "tax_rate".into(),
            param: 1.0,
            previous_param: None,
            owner: crate::LAW_PLAYER.into(),
            jurisdiction: Jurisdiction::Empire,
            proposed_at: 10,
            effective_at: 10,
            phase_in_secs: 0,
            status: LawStatus::Proposed,
            repeal_effective_at: None,
            enacted_at: None,
            repealed_at: None,
            approvals: Vec::new(),
            baseline_output: 0.0,
            baseline_pollution: 0.0,
            baseline_research: 0.0,
            last_sample_at: 0,
            revenue_accrued: 0.0,
            samples: Vec::new(),
        }
    }

    fn plan(id: u64) -> PlannedMission {
        PlannedMission {
            id,
            vehicle_id: 1,
            route: Route {
                origin: Coord::HOME,
                dest: Coord {
                    galaxy: 1,
                    system: 1,
                    position: 8,
                },
            },
            transfer: TransferKind::Hohmann,
            customer: Customer::Colony,
            window: LaunchWindow {
                opens_at: 10,
                closes_at: 100,
            },
            launched_at: 10,
            arrives_at: 400,
            delta_v: 1.0,
            fuel_mass: 1.0,
            payload: Payload {
                cargo: Resources::ZERO,
            },
        }
    }

    #[test]
    fn uncleared_admiral_cannot_see_secret_intel_through_any_path() {
        let mut state = GameState::new(10);
        state.spy_reports.push(report(0));
        state.laws.laws.push(law(0));
        state.planned.push(plan(0));
        state.push_event(10, "proposed law tax_rate #0".into());
        state.push_event(10, "metal mine queued".into());
        state
            .classify(
                Actor::Player,
                ClassifiedKind::Report,
                "0",
                Classification::Secret,
                [COMPARTMENT_INTEL.into()],
            )
            .unwrap();
        state
            .classify(
                Actor::Player,
                ClassifiedKind::Law,
                "0",
                Classification::Restricted,
                [COMPARTMENT_POLICY.into()],
            )
            .unwrap();
        state
            .classify(
                Actor::Player,
                ClassifiedKind::Plan,
                "0",
                Classification::Restricted,
                [COMPARTMENT_FLEET.into()],
            )
            .unwrap();

        let admiral = Actor::Leader(1);
        assert!(state.can_see(Actor::Player, ClassifiedKind::Report, "0"));
        assert!(!state.can_see(admiral, ClassifiedKind::Report, "0"));
        assert!(state.visible_reports(admiral).is_empty());
        assert!(state.latest_report_for(admiral, Coord::HOME).is_none());
        assert!(state.visible_laws(admiral).is_empty());
        assert_eq!(state.visible_plans(admiral).len(), 1);
        assert_eq!(state.visible_reports(Actor::Player).len(), 1);
        assert_eq!(state.visible_laws(Actor::Player).len(), 1);
        let events: Vec<&str> = state
            .visible_events(admiral)
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        assert!(!events.iter().any(|text| text.contains("law tax_rate #0")));
        assert!(events.iter().any(|text| text.contains("metal mine queued")));
    }

    #[test]
    fn compartment_ids_reject_commas() {
        let mut state = GameState::new(10);
        state.spy_reports.push(report(5));
        let err = state
            .classify(
                Actor::Player,
                ClassifiedKind::Report,
                "5",
                Classification::Secret,
                ["intel,ops".into()],
            )
            .unwrap_err();
        assert_eq!(
            err,
            ClassificationError::InvalidCompartment("intel,ops".into())
        );
        assert_eq!(parse_compartment("intel").unwrap(), "intel");
    }

    #[test]
    fn missing_marking_is_open() {
        let mut state = GameState::new(10);
        state.spy_reports.push(report(3));
        assert_eq!(
            state.marking_of(ClassifiedKind::Report, "3").classification,
            Classification::Open
        );
        assert!(state.can_see(Actor::Leader(1), ClassifiedKind::Report, "3"));
    }

    #[test]
    fn sharing_needs_allied_standing_then_covers_need_to_know() {
        let mut state = GameState::new(10);
        state.spy_reports.push(report(1));
        state
            .classify(
                Actor::Player,
                ClassifiedKind::Report,
                "1",
                Classification::Restricted,
                [COMPARTMENT_INTEL.into()],
            )
            .unwrap();
        assert_eq!(
            state.share(
                "traders",
                Classification::Restricted,
                [COMPARTMENT_INTEL.into()]
            ),
            Err(ClassificationError::NotAllied("traders".into()))
        );
        state.diplomacy.set_standing("traders", 25.0);
        state
            .share(
                "traders",
                Classification::Restricted,
                [COMPARTMENT_INTEL.into()],
            )
            .unwrap();
        assert!(state.faction_can_see("traders", ClassifiedKind::Report, "1"));
        assert!(!state.faction_can_see("pirates", ClassifiedKind::Report, "1"));
    }

    #[test]
    fn detected_spy_and_disloyal_leader_open_incidents() {
        let mut state = GameState::new(10);
        state.spy_reports.push(report(4));
        state
            .classify(
                Actor::Player,
                ClassifiedKind::Report,
                "4",
                Classification::Secret,
                [COMPARTMENT_INTEL.into()],
            )
            .unwrap();
        state.maybe_leak_spy(20, true);
        assert_eq!(state.classification.incidents.len(), 1);
        assert_eq!(state.classification.incidents[0].via, "pirates");
        assert!(state.faction_can_see("pirates", ClassifiedKind::Report, "4"));

        state.resources.iron = 5_000.0;
        state.resources.silicon = 5_000.0;
        let id = state.recruit_leader("mole", DISLOYAL, 10).unwrap();
        state.classification.next_disloyal_at = 10;
        state.tick_classification_leaks(10);
        assert!(
            state
                .classification
                .incidents
                .iter()
                .any(|row| row.source == "disloyal" && row.via == format!("leader:{id}"))
        );
        state.last_tick_unix = 20;
        let admiral = Actor::Leader(1);
        assert!(state.visible_incidents(admiral).is_empty());
        assert_eq!(state.visible_incidents(Actor::Player).len(), 2);
    }

    #[test]
    fn retired_or_unready_leaders_lose_role_clearance() {
        let mut state = GameState::new(10);
        state.planned.push(plan(0));
        state
            .classify(
                Actor::Player,
                ClassifiedKind::Plan,
                "0",
                Classification::Restricted,
                [COMPARTMENT_FLEET.into()],
            )
            .unwrap();
        let admiral = Actor::Leader(1);
        assert!(state.can_see(admiral, ClassifiedKind::Plan, "0"));
        state.assignments[0].ready_at = 10_000;
        assert!(!state.can_see(admiral, ClassifiedKind::Plan, "0"));
        state.assignments[0].ready_at = 10;
        state.leaders[0].retired = true;
        assert!(!state.can_see(admiral, ClassifiedKind::Plan, "0"));
    }

    #[test]
    fn extra_grant_lets_admiral_read_secret_intel() {
        let mut state = GameState::new(10);
        state.spy_reports.push(report(8));
        state
            .classify(
                Actor::Player,
                ClassifiedKind::Report,
                "8",
                Classification::Secret,
                [COMPARTMENT_INTEL.into()],
            )
            .unwrap();
        assert!(!state.can_see(Actor::Leader(1), ClassifiedKind::Report, "8"));
        state
            .grant_clearance(1, Classification::Secret, [COMPARTMENT_INTEL.into()])
            .unwrap();
        assert!(state.can_see(Actor::Leader(1), ClassifiedKind::Report, "8"));
    }

    #[test]
    fn governor_cannot_classify_above_clearance() {
        let mut state = GameState::new(10);
        state.spy_reports.push(report(2));
        let err = state
            .classify(
                Actor::Leader(2),
                ClassifiedKind::Report,
                "2",
                Classification::Secret,
                [COMPARTMENT_INTEL.into()],
            )
            .unwrap_err();
        assert_eq!(err, ClassificationError::InsufficientClearance);
    }
}
