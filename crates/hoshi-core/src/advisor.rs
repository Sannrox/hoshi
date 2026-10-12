//! Rule-based next-move suggestions from player-visible state. Never acts.

use crate::buildings::BuildingKind;
use crate::ships::ShipKind;
use crate::state::{GameState, ResourceKind};

/// One suggestion. `id` is stable for tests; `text` is what the player reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    pub id: &'static str,
    pub text: String,
}

/// Ranked suggestions from public stockpiles, energy, queues and raid timing.
pub fn advise(state: &GameState) -> Vec<Advice> {
    let mut out = Vec::new();
    push_energy(state, &mut out);
    push_raid(state, &mut out);
    push_storage(state, &mut out);
    push_idle_queues(state, &mut out);
    push_chain_limit(state, &mut out);
    if out.is_empty() {
        out.push(Advice {
            id: "clear",
            text: "flows look healthy; pick a long-term goal".into(),
        });
    }
    out
}

/// Optional local rephrase. Compiled out unless `advisor-model` is enabled;
/// always falls back to the rule text and never talks to a non-loopback host
/// unless `allow_lan` is set and the host is a private IPv4 address.
pub fn polish_advice(text: &str, endpoint: &str, allow_lan: bool) -> String {
    polish_all(&[text.to_string()], endpoint, allow_lan)
        .into_iter()
        .next()
        .unwrap_or_else(|| text.to_string())
}

/// One request for every suggestion so the overlay stays inside the model budget.
pub fn polish_all(texts: &[String], endpoint: &str, allow_lan: bool) -> Vec<String> {
    if texts.is_empty() {
        return Vec::new();
    }
    if endpoint.is_empty() || !endpoint_allowed(endpoint, allow_lan) {
        return texts.to_vec();
    }
    #[cfg(feature = "advisor-model")]
    {
        let mut slots: Vec<Option<String>> = texts.iter().cloned().map(Some).collect();
        crate::rephrase_lines_allow(&mut slots, endpoint, allow_lan);
        return slots
            .into_iter()
            .zip(texts.iter())
            .map(|(slot, orig)| slot.unwrap_or_else(|| orig.clone()))
            .collect();
    }
    #[cfg(not(feature = "advisor-model"))]
    {
        let _ = (endpoint, allow_lan);
        texts.to_vec()
    }
}

fn endpoint_allowed(endpoint: &str, allow_lan: bool) -> bool {
    crate::commentary::model_connect_addr(endpoint, allow_lan).is_some()
}

fn push_energy(state: &GameState, out: &mut Vec<Advice>) {
    if state.energy_factor() + 1e-12 >= 1.0 {
        return;
    }
    out.push(Advice {
        id: "energy",
        text: format!(
            "energy is short ({:.0}/{:.0}); upgrade the solar plant or idle a mine",
            state.energy_produced(),
            state.energy_consumed()
        ),
    });
}

fn push_raid(state: &GameState, out: &mut Vec<Advice>) {
    let Some(at) = state.next_raid_at else {
        return;
    };
    let wait = at.saturating_sub(state.last_tick_unix);
    if wait > 7_200 {
        return;
    }
    let fighters = state.fleet.get(ShipKind::LightFighter);
    if fighters >= 5 {
        return;
    }
    out.push(Advice {
        id: "raid",
        text: format!("a raid in {wait}s; build 5 light fighters (you have {fighters})"),
    });
}

fn push_storage(state: &GameState, out: &mut Vec<Advice>) {
    for kind in ResourceKind::ALL {
        let cap = state.storage_cap(kind);
        if cap <= 0.0 {
            continue;
        }
        let have = state.resources.get(kind);
        if have + 1e-9 < cap * 0.9 {
            continue;
        }
        out.push(Advice {
            id: "storage",
            text: format!(
                "{} storage is nearly full ({:.0}/{:.0}); upgrade it or trade",
                kind.label(),
                have,
                cap
            ),
        });
        return;
    }
}

fn push_idle_queues(state: &GameState, out: &mut Vec<Advice>) {
    if state.build_queue.is_empty() {
        out.push(Advice {
            id: "build",
            text: format!(
                "the build queue is idle; start a {} upgrade",
                BuildingKind::MetalMine
            ),
        });
    }
    if state.research.is_none() {
        out.push(Advice {
            id: "research",
            text: "the lab is idle; queue a technology".into(),
        });
    }
}

fn push_chain_limit(state: &GameState, out: &mut Vec<Advice>) {
    let report = state.production_report();
    if matches!(report.limited_by, crate::LimitingFactor::None) {
        return;
    }
    out.push(Advice {
        id: "limit",
        text: format!(
            "production is limited by {}: {}",
            report.limited_by.as_key(),
            report.limited_by.suggestion()
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildings::Buildings;
    use crate::{BalanceConfig, GameState};

    fn close_state() -> GameState {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        GameState::with_config(0, cfg)
    }

    #[test]
    fn advise_does_not_mutate_state() {
        let state = close_state();
        let before = state.clone();
        let _ = advise(&state);
        assert_eq!(state, before);
    }

    #[test]
    fn energy_shortfall_suggests_solar() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.starting_buildings.solar_plant = 0;
        let state = GameState::with_config(0, cfg);
        let ids: Vec<_> = advise(&state).iter().map(|a| a.id).collect();
        assert!(ids.contains(&"energy"), "{ids:?}");
        assert!(
            advise(&state)
                .iter()
                .any(|a| a.text.contains("solar") || a.text.contains("energy"))
        );
    }

    #[test]
    fn full_storage_suggests_upgrade_or_trade() {
        let mut state = close_state();
        state.resources.metal = state.storage_cap(ResourceKind::Metal);
        let hit = advise(&state).into_iter().find(|a| a.id == "storage");
        assert!(hit.is_some(), "{hit:?}");
        assert!(hit.unwrap().text.contains("metal"));
    }

    #[test]
    fn raid_warning_uses_visible_fleet_and_timer() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = true;
        let mut state = GameState::with_config(0, cfg);
        state.next_raid_at = Some(1_800);
        state.last_tick_unix = 0;
        let mut fleet = crate::Fleet::default();
        fleet.add(ShipKind::LightFighter, 1);
        state.fleet = fleet;
        let hit = advise(&state).into_iter().find(|a| a.id == "raid").unwrap();
        assert!(hit.text.contains("1800"));
        assert!(hit.text.contains("light fighter"));
    }

    #[test]
    fn polish_refuses_remote_hosts() {
        let line = "upgrade storage";
        assert_eq!(
            polish_advice(line, "https://example.invalid/v1", false),
            line
        );
        assert_eq!(polish_advice(line, "http://8.8.8.8/v1", false), line);
        assert_eq!(
            polish_advice(line, "http://8.8.8.8/v1", true),
            line,
            "public IPs stay refused even with allow_lan"
        );
        assert!(endpoint_allowed("http://127.0.0.1:8080/v1", false));
        assert!(endpoint_allowed("http://192.168.1.10:8080/v1", true));
        assert!(!endpoint_allowed("http://192.168.1.10:8080/v1", false));
    }

    #[test]
    fn idle_queues_are_visible_without_hidden_intel() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.starting_buildings = Buildings {
            metal_mine: 1,
            solar_plant: 8,
            ..Buildings::default()
        };
        let mut state = GameState::with_config(0, cfg);
        state.build_queue.clear();
        state.research = None;
        let advice = advise(&state);
        assert!(advice.iter().any(|a| a.id == "build"));
        assert!(advice.iter().any(|a| a.id == "research"));
        let blob = advice
            .iter()
            .map(|a| a.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!blob.contains("outpost"), "{blob}");
        assert!(!blob.contains("spy"), "{blob}");
    }
}
