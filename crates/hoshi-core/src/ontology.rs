//! Versioned hoshi ontology package. Compatible with sekai-ontology schema 1.

use crate::history::{HistoryEvent, HistoryKind};
use std::fmt;

/// Importable package id recorded in provenance.
pub const ONTOLOGY_PACKAGE: &str = "hoshi-ontology/v1";
/// Pinned Chisei product/API version (`sekai-chisei` 1.3.x).
pub const CHISEI_API_VERSION: &str = "1.3.0";
/// Portable ontology document version accepted by `sekai import`.
pub const ONTOLOGY_SCHEMA_VERSION: u32 = 1;

pub const OBJECT_TYPES: [&str; 16] = [
    "Player",
    "Planet",
    "Fleet",
    "Faction",
    "Outpost",
    "Marking",
    "Compartment",
    "Clearance",
    "SharingAgreement",
    "Region",
    "Warehouse",
    "Good",
    "Order",
    "Contract",
    "Trader",
    "Shipment",
];
pub const RELATIONS: [&str; 6] = [
    "owns",
    "stationed-at",
    "allied-with",
    "placed-in",
    "binds",
    "fulfils",
];
pub const ACTIONS: [&str; 9] = [
    "build",
    "research",
    "launch",
    "colonize",
    "invade",
    "place_order",
    "accept_contract",
    "fulfil_contract",
    "ship_goods",
];

const ONTOLOGY_JSON: &str = include_str!("../data/ontology/hoshi-v1.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OntologyError {
    Schema,
    MissingClass(&'static str),
    MissingRelation(&'static str),
}

impl fmt::Display for OntologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OntologyError::Schema => write!(f, "ontology schema_version must be 1"),
            OntologyError::MissingClass(name) => write!(f, "ontology missing class {name}"),
            OntologyError::MissingRelation(name) => write!(f, "ontology missing relation {name}"),
        }
    }
}

impl std::error::Error for OntologyError {}

/// Bundled `schema_version: 1` document.
pub fn bundled_ontology() -> &'static str {
    ONTOLOGY_JSON
}

/// Reject an ontology that is not the hoshi v1 object model.
pub fn validate_ontology(json: &str) -> Result<(), OntologyError> {
    if !has_schema_one(json) {
        return Err(OntologyError::Schema);
    }
    let classes = names_in_section(json, "\"classes\"");
    let relations = names_in_section(json, "\"relations\"");
    for need in OBJECT_TYPES {
        if !classes.iter().any(|c| c == need) {
            return Err(OntologyError::MissingClass(need));
        }
    }
    for need in RELATIONS {
        if !relations.iter().any(|c| c == need) {
            return Err(OntologyError::MissingRelation(need));
        }
    }
    Ok(())
}

fn has_schema_one(json: &str) -> bool {
    json.contains("\"schema_version\": 1") || json.contains("\"schema_version\":1")
}

fn names_in_section(json: &str, key: &str) -> Vec<String> {
    let Some(idx) = json.find(key) else {
        return Vec::new();
    };
    let rest = &json[idx + key.len()..];
    let Some(start) = rest.find('[') else {
        return Vec::new();
    };
    let mut depth = 0i32;
    let mut end = start;
    for (i, c) in rest[start..].char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &rest[start..=end];
    let mut names = Vec::new();
    let mut search = body;
    while let Some(at) = search.find("\"name\"") {
        search = &search[at + 6..];
        let Some(colon) = search.find(':') else {
            break;
        };
        search = &search[colon + 1..];
        let Some(q) = search.find('"') else {
            break;
        };
        search = &search[q + 1..];
        let Some(q2) = search.find('"') else {
            break;
        };
        names.push(search[..q2].to_string());
        search = &search[q2 + 1..];
    }
    names
}

/// One aggregated balance row with the history ids that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct BalanceLineage {
    pub kind: HistoryKind,
    pub action: String,
    pub count: u32,
    pub mass: f64,
    pub energy: f64,
    pub event_ids: Vec<u64>,
}

/// Fold history into per-kind totals. `event_ids` are the lineage.
pub fn transform_balance(events: &[HistoryEvent]) -> Vec<BalanceLineage> {
    let mut rows: Vec<BalanceLineage> = Vec::new();
    for event in events {
        if let Some(row) = rows
            .iter_mut()
            .find(|row| row.kind == event.kind && row.action == event.action)
        {
            row.count = row.count.saturating_add(1);
            row.mass += event.mass;
            row.energy += event.energy;
            row.event_ids.push(event.id);
        } else {
            rows.push(BalanceLineage {
                kind: event.kind,
                action: event.action.clone(),
                count: 1,
                mass: event.mass,
                energy: event.energy,
                event_ids: vec![event.id],
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildings::BuildingKind;
    use crate::state::GameState;

    #[test]
    fn bundled_package_is_hoshi_v1() {
        validate_ontology(bundled_ontology()).unwrap();
        let json = bundled_ontology();
        assert!(json.contains("chisei-api:1.3.0"));
        assert!(json.contains(ONTOLOGY_PACKAGE));
        for action in ACTIONS {
            let _ = action;
        }
    }

    #[test]
    fn reject_wrong_schema() {
        assert_eq!(
            validate_ontology("{\"schema_version\": 2, \"classes\": [], \"relations\": []}"),
            Err(OntologyError::Schema)
        );
    }

    #[test]
    fn balance_transform_keeps_event_ids() {
        let mut state = GameState::with_config(1_000, crate::BalanceConfig::default());
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 50_000.0;
        state.enqueue_build(BuildingKind::MetalMine, 1_000).unwrap();
        let rows = transform_balance(&state.history.events);
        let build = rows
            .iter()
            .find(|row| row.kind == HistoryKind::Build)
            .expect("build row");
        assert_eq!(build.count, 1);
        assert_eq!(build.event_ids.len(), 1);
        assert_eq!(build.event_ids[0], state.history.events[0].id);
    }
}
