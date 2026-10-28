//! Rule-based next-move suggestions from player-visible state. Never acts.

use crate::buildings::BuildingKind;
use crate::defences::{DefenceKind, DefenceRole};
use crate::ships::ShipKind;
use crate::state::{GameState, ResourceKind};
use crate::victory::VictoryPath;

/// One suggestion. `id` is stable for tests; `text` is what the player reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    pub id: &'static str,
    pub text: String,
}

/// Ranked suggestions from public stockpiles, energy, queues and raid timing.
/// A `focus` from `config.toml` puts that victory path's next step first.
pub fn advise(state: &GameState, focus: Option<VictoryPath>) -> Vec<Advice> {
    let mut out = Vec::new();
    if let Some(path) = focus {
        out.extend(path_step(state, path));
    }
    push_storage_wall(state, &mut out);
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
    polish_all_named(texts, endpoint, allow_lan, "")
}

/// Same as [`polish_all`], sending `model` in the JSON body when non-empty.
pub fn polish_all_named(
    texts: &[String],
    endpoint: &str,
    allow_lan: bool,
    model: &str,
) -> Vec<String> {
    if texts.is_empty() {
        return Vec::new();
    }
    if endpoint.is_empty() || !endpoint_allowed(endpoint, allow_lan) {
        return texts.to_vec();
    }
    #[cfg(feature = "advisor-model")]
    {
        let mut slots: Vec<Option<String>> = texts.iter().cloned().map(Some).collect();
        crate::rephrase_lines_named(&mut slots, endpoint, allow_lan, model);
        return slots
            .into_iter()
            .zip(texts.iter())
            .map(|(slot, orig)| slot.unwrap_or_else(|| orig.clone()))
            .collect();
    }
    #[cfg(not(feature = "advisor-model"))]
    {
        let _ = (endpoint, allow_lan, model);
        texts.to_vec()
    }
}

/// True when the URL is loopback, or a private IPv4 host with `allow_lan`.
pub fn advisor_endpoint_allowed(endpoint: &str, allow_lan: bool) -> bool {
    crate::commentary::model_connect_addr(endpoint, allow_lan).is_some()
}

fn endpoint_allowed(endpoint: &str, allow_lan: bool) -> bool {
    advisor_endpoint_allowed(endpoint, allow_lan)
}

fn push_energy(state: &GameState, out: &mut Vec<Advice>) {
    for (coord, buildings, kind) in std::iter::once((state.home, state.buildings, state.home_kind))
        .chain(
            state
                .colonies
                .iter()
                .map(|c| (c.coord, c.buildings, c.kind)),
        )
    {
        let produced = state.site_energy_produced(coord, buildings, kind);
        let consumed = state.energy_consumed_at(coord);
        if produced + 1e-12 >= consumed {
            continue;
        }
        let cap = state.max_solar_level(coord).unwrap_or(0);
        if buildings.solar_plant >= cap {
            out.push(Advice { id: "energy", text: format!("energy is short at {coord} ({produced:.0}/{consumed:.0}); solar is at this planet's cap of {cap}; add fission or fusion for more power") });
            continue;
        }
        out.push(Advice {
            id: "energy",
            text: format!(
                "energy is short at {coord} ({produced:.0}/{consumed:.0}); upgrade the solar plant or idle a mine"
            ),
        });
    }
}

/// Defence wanted before a raid lands this soon.
const RAID_WARNING_SECS: u64 = 7_200;
/// Units the advice aims for; past this the home counts as guarded.
const RAID_GUARD: u32 = 5;

/// Before a raid: the cheapest defence the shipyard can build now (as many
/// as the stock pays for), or else the next building on the way to the
/// shipyard. Never a unit whose shipyard or research is still out of reach.
fn push_raid(state: &GameState, out: &mut Vec<Advice>) {
    let Some(at) = state.upcoming_raid_at() else {
        return;
    };
    let wait = at.saturating_sub(state.last_tick_unix);
    if wait > RAID_WARNING_SECS {
        return;
    }
    let batteries: u32 = DefenceKind::ALL
        .into_iter()
        .filter(|&kind| state.config.defences.get(kind).role == DefenceRole::Battery)
        .map(|kind| state.defences.get(kind))
        .sum();
    if batteries >= RAID_GUARD || state.fleet.get(ShipKind::LightFighter) >= RAID_GUARD {
        return;
    }
    let shipyard = state.buildings.shipyard;
    let cheapest = DefenceKind::ALL
        .into_iter()
        // Batteries fire on raiders; garrisons only fight landings and domes
        // only absorb, so neither holds a raid on its own.
        .filter(|&kind| {
            let spec = state.config.defences.get(kind);
            spec.role == DefenceRole::Battery && spec.shipyard <= shipyard.max(1)
        })
        .min_by(|&a, &b| {
            let cost = |kind| state.config.defences.get(kind).unit_cost().total();
            cost(a).total_cmp(&cost(b))
        });
    if shipyard > 0
        && let Some(kind) = cheapest
    {
        let unit = state.config.defences.get(kind).unit_cost();
        let afford = ResourceKind::ALL
            .into_iter()
            .filter(|&r| unit.get(r) > 0.0)
            .map(|r| (state.resources.get(r) / unit.get(r)).floor())
            .fold(f64::from(RAID_GUARD), f64::min)
            .max(0.0) as u32;
        let name = state.config.defences.get(kind).name.to_lowercase();
        out.push(Advice {
            id: "raid",
            text: if afford > 0 {
                format!(
                    "a raid in {wait}s; queue {afford} {name} in the shipyard ({} each)",
                    unit.brief()
                )
            } else {
                format!(
                    "a raid in {wait}s; a {name} costs {}, more than the stock holds",
                    unit.brief()
                )
            },
        });
        return;
    }
    let step = shipyard_step(state);
    let market = if state.config.market.enabled {
        ", and trade iron for deuterium on the market if it is short"
    } else {
        ""
    };
    out.push(Advice {
        id: "raid",
        text: format!("a raid in {wait}s; no defence can be built yet: {step}{market}"),
    });
}

/// No shipyard yet: the next building on its prerequisite path.
fn shipyard_step(state: &GameState) -> String {
    let gate = state
        .config
        .buildings
        .get(BuildingKind::Shipyard)
        .prerequisites
        .iter()
        .find(|(building, level)| state.buildings.get(*building) < *level)
        .map(|(building, _)| *building)
        .unwrap_or(BuildingKind::Shipyard);
    if gate == BuildingKind::Shipyard {
        "build the shipyard".to_string()
    } else {
        format!("build the {gate} on the way to the shipyard")
    }
}

/// The next step on one victory path, or `None` when the path is off, done,
/// or only waiting on a timer. The id is the path key.
fn path_step(state: &GameState, path: VictoryPath) -> Option<Advice> {
    let cfg = &state.config.victory;
    if !cfg.enabled || state.victory.is_some() {
        return None;
    }
    let text = match path {
        VictoryPath::Conquest => conquest_step(state)?,
        VictoryPath::Wonder => wonder_step(state)?,
        VictoryPath::Diplomacy => {
            let (name, standing) = state
                .config
                .diplomacy
                .iter()
                .map(|f| (&f.name, state.diplomacy.standing(&f.id)))
                .filter(|(_, standing)| standing + 1e-9 < cfg.allied_min)
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            format!(
                "raise standing with {name} ({standing:.0} of {:.0} to ally)",
                cfg.allied_min
            )
        }
    };
    Some(Advice {
        id: path.as_key(),
        text: format!("{}: {text}", path.label()),
    })
}

/// The nearest pirate outpost the docked warships can reach, or the
/// shipyard step that arms the home. Names a target, not a battle plan:
/// whether to attack or invade depends on intel the player reads first.
fn conquest_step(state: &GameState) -> Option<String> {
    let home = state.pirate_homeworld()?;
    if state.colonies.iter().any(|c| c.coord == home) {
        return None;
    }
    let mut warships = crate::ships::Fleet::default();
    for kind in ShipKind::ALL {
        if state.config.ships.get(kind).attack > 0.0 {
            warships.add(kind, state.fleet.get(kind));
        }
    }
    if warships.is_empty() {
        return Some(if state.buildings.shipyard == 0 {
            format!("{} to arm the home", shipyard_step(state))
        } else {
            "queue warships in the shipyard to arm the home".into()
        });
    }
    let (secs, fuel, coord) = state
        .outposts
        .iter()
        .filter_map(|o| {
            let (secs, fuel) = state.flight_time_and_fuel(warships, o.coord, 100).ok()?;
            Some((secs, fuel, o.coord))
        })
        .min_by_key(|&(secs, _, coord)| (secs, coord))?;
    let stock = state.resources.deuterium;
    Some(if fuel > stock + 1e-9 {
        format!(
            "the docked warships need {fuel:.0} deuterium to reach the pirate outpost at {coord} ({stock:.0} in stock); stock more or send fewer ships"
        )
    } else {
        format!(
            "the nearest pirate outpost is {coord} ({secs}s flight); spy on it before you attack or invade"
        )
    })
}

/// The next unpaid stage of the victory wonder, or what blocks paying it.
fn wonder_step(state: &GameState) -> Option<String> {
    let id = &state.config.victory.wonder_id;
    if state.completed_wonders.iter().any(|done| done == id) {
        return None;
    }
    let spec = state.config.wonders.iter().find(|spec| &spec.id == id)?;
    let name = state.display_name(id);
    let (index, paid) = match &state.wonder {
        Some(job) if &job.id == id => {
            if job.completes_at.is_some() {
                return None;
            }
            (job.stage, job.contributed)
        }
        Some(job) => {
            let other = state.display_name(&job.id);
            return Some(format!("finish {other} to free the slot for {name}"));
        }
        None => {
            if let Some(tech) = spec
                .prereq_techs
                .iter()
                .find(|tech| state.tech_level(&tech.id) < tech.level)
            {
                return Some(format!(
                    "research {} to level {} to start {name}",
                    tech.id, tech.level
                ));
            }
            (0, crate::Resources::ZERO)
        }
    };
    let stage = spec.stage(index)?;
    if state.wonder_tier_ok(stage).is_err()
        && let Some(tier) = stage.require_tier.as_deref()
    {
        let tier = state
            .config
            .ladder
            .get(tier)
            .map_or_else(|| tier.to_string(), |tier| tier.name.clone());
        return Some(format!(
            "reach the {tier} goal to pay stage {} of {name}",
            index + 1
        ));
    }
    let owed = stage.cost.saturating_sub(paid);
    let verb = if state.wonder.is_none() {
        "start"
    } else {
        "pay"
    };
    Some(format!(
        "{verb} {name} stage {} of {} ({} owed)",
        index + 1,
        spec.stages.len(),
        owed.brief()
    ))
}

fn push_storage_wall(state: &GameState, out: &mut Vec<Advice>) {
    for coord in std::iter::once(state.home).chain(state.colonies.iter().map(|colony| colony.coord))
    {
        let Some(buildings) = state.buildings_at(coord) else {
            continue;
        };
        let mut candidates = vec![
            BuildingKind::SolarPlant,
            BuildingKind::MetalMine,
            BuildingKind::CrystalMine,
            BuildingKind::DeuteriumSynthesizer,
        ];
        for job in state.queue_at(coord).unwrap_or(&[]) {
            if !candidates.contains(&job.building) {
                candidates.push(job.building);
            }
        }
        for building in candidates {
            if building == BuildingKind::SolarPlant
                && (state.energy_balance_at(coord).unwrap_or(0.0) >= 0.0
                    || buildings.solar_plant >= state.max_solar_level(coord).unwrap_or(0))
            {
                continue;
            }
            if building != BuildingKind::SolarPlant && buildings.get(building) == 0 {
                continue;
            }
            let cost = state.build_cost_on(coord, building);
            let queue = state.queue_at(coord).unwrap_or(&[]);
            let next = crate::state::projected_on(buildings, queue, building, queue.len())
                .saturating_add(1);
            for resource in ResourceKind::ALL {
                let needed = cost.get(resource);
                let cap = state.storage_cap_at(coord, resource);
                if needed > cap + 1e-9 {
                    out.push(Advice { id: "storage_wall", text: format!("{building} level {} at {coord} needs {needed:.0} {}, but storage holds {cap:.0}; build {} storage first", next, resource.label(), resource.label()) });
                    break;
                }
            }
        }
    }
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
    if state.research.is_none() && state.buildings.research_lab > 0 {
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
    fn next_level_storage_wall_and_solar_cap_explain_the_real_blocker() {
        let mut state = GameState::new(0);
        assert!(
            !advise(&state, None)
                .iter()
                .any(|advice| advice.id == "storage_wall")
        );
        state.buildings.metal_mine = 14;
        assert!(
            state
                .build_cost_on(state.home, BuildingKind::MetalMine)
                .iron
                > state.storage_cap(ResourceKind::Iron)
        );
        let advice = advise(&state, None);
        assert!(advice.iter().any(
            |advice| advice.id == "storage_wall" && advice.text.contains("iron storage first")
        ));
        state.buildings.solar_plant = state.max_solar_level(state.home).unwrap();
        state.buildings.metal_mine = 30;
        assert!(
            advise(&state, None).iter().any(
                |advice| advice.id == "energy" && advice.text.contains("add fission or fusion")
            )
        );
    }

    #[test]
    fn advise_does_not_mutate_state() {
        let state = close_state();
        let before = state.clone();
        let _ = advise(&state, None);
        assert_eq!(state, before);
    }

    #[test]
    fn energy_shortfall_suggests_solar() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.starting_buildings.solar_plant = 0;
        let state = GameState::with_config(0, cfg);
        let ids: Vec<_> = advise(&state, None).iter().map(|a| a.id).collect();
        assert!(ids.contains(&"energy"), "{ids:?}");
        assert!(
            advise(&state, None)
                .iter()
                .any(|a| a.text.contains("solar") || a.text.contains("energy"))
        );
    }

    #[test]
    fn full_storage_suggests_upgrade_or_trade() {
        let mut state = close_state();
        state.resources.iron = state.storage_cap(ResourceKind::Iron);
        let hit = advise(&state, None).into_iter().find(|a| a.id == "storage");
        assert!(hit.is_some(), "{hit:?}");
        assert!(hit.unwrap().text.contains("iron"));
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
        let hit = advise(&state, None)
            .into_iter()
            .find(|a| a.id == "raid")
            .unwrap();
        assert!(hit.text.contains("1800"));
        // No shipyard: point at the next building toward it, not at ships.
        assert!(hit.text.contains("robotics factory"), "{}", hit.text);
        assert!(!hit.text.contains("light fighter"), "{}", hit.text);
        state.buildings.robotics_factory = 1;
        let hit = advise(&state, None)
            .into_iter()
            .find(|a| a.id == "raid")
            .unwrap();
        assert!(
            hit.text
                .contains("no defence can be built yet: build the shipyard"),
            "{}",
            hit.text
        );
        // Domes do not hold a raid, so they do not silence the warning.
        state.defences.set(DefenceKind::ShieldDome, 9);
        assert!(advise(&state, None).iter().any(|a| a.id == "raid"));

        // With a shipyard: the cheapest defence it can build, as many as paid for.
        state.buildings.robotics_factory = 1;
        state.buildings.shipyard = 1;
        state.resources = crate::Resources::from_parts(500.0, 500.0, 0.0);
        let hit = advise(&state, None)
            .into_iter()
            .find(|a| a.id == "raid")
            .unwrap();
        let cannon = state
            .config
            .defences
            .get(DefenceKind::LightCannon)
            .unit_cost();
        let afford = (500.0 / cannon.iron).floor().min(5.0) as u32;
        assert!(
            hit.text.contains(&format!("queue {afford} light cannon")),
            "{}",
            hit.text
        );
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
        let advice = advise(&state, None);
        assert!(advice.iter().any(|a| a.id == "build"));
        // No lab, so an idle lab is not advice.
        assert!(!advice.iter().any(|a| a.id == "research"));
        state.buildings.research_lab = 1;
        assert!(advise(&state, None).iter().any(|a| a.id == "research"));
        let blob = advice
            .iter()
            .map(|a| a.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!blob.contains("outpost"), "{blob}");
        assert!(!blob.contains("spy"), "{blob}");
    }

    fn first_id(state: &GameState, focus: VictoryPath) -> &'static str {
        advise(state, Some(focus))[0].id
    }

    #[test]
    fn no_focus_adds_no_path_step() {
        let state = GameState::new(0);
        let ids: Vec<_> = advise(&state, None).iter().map(|a| a.id).collect();
        for path in [
            VictoryPath::Conquest,
            VictoryPath::Wonder,
            VictoryPath::Diplomacy,
        ] {
            assert!(!ids.contains(&path.as_key()), "{ids:?}");
        }
    }

    #[test]
    fn conquest_focus_arms_the_home_then_names_the_nearest_outpost() {
        let mut state = close_state();
        state.fleet = crate::ships::Fleet::default();
        state.buildings.metal_mine = 30;
        let advice = advise(&state, Some(VictoryPath::Conquest));
        assert_eq!(advice[0].id, "conquest");
        assert!(advice[0].text.contains("shipyard"), "{}", advice[0].text);
        assert!(advice.iter().any(|a| a.id == "energy"), "energy stays");
        state.fleet.add(ShipKind::LightFighter, 5);
        state.resources.deuterium = 1e9;
        let text = |state: &GameState| advise(state, Some(VictoryPath::Conquest))[0].text.clone();
        let near = text(&state);
        assert!(near.contains("nearest pirate outpost"), "{near}");
        // The same text whether or not the outpost's defences are gone.
        for outpost in &mut state.outposts {
            outpost.defences = crate::ships::Fleet::default();
        }
        assert_eq!(text(&state), near, "no hidden intel");
        state.resources.deuterium = 0.0;
        assert!(
            text(&state).contains("send fewer ships"),
            "{}",
            text(&state)
        );
    }

    #[test]
    fn wonder_focus_names_the_gate_before_the_next_stage() {
        let mut state = close_state();
        assert_eq!(first_id(&state, VictoryPath::Wonder), "wonder");
        let text = &advise(&state, Some(VictoryPath::Wonder))[0].text;
        assert!(text.contains("research energy to level 8"), "{text}");
        state.tech_levels.insert("energy".into(), 8);
        let text = &advise(&state, Some(VictoryPath::Wonder))[0].text;
        assert!(text.contains("Wonder supply chain goal"), "{text}");
        state.config.wonders[0].stages[0].require_tier = None;
        let text = &advise(&state, Some(VictoryPath::Wonder))[0].text;
        assert!(text.contains("start Dyson ring stage 1 of 2"), "{text}");
        state.completed_wonders.push("dyson_ring".into());
        assert_ne!(first_id(&state, VictoryPath::Wonder), "wonder");
    }

    #[test]
    fn diplomacy_focus_names_the_furthest_faction() {
        let mut state = close_state();
        let ids: Vec<_> = state
            .config
            .diplomacy
            .iter()
            .map(|f| f.id.clone())
            .collect();
        assert!(ids.len() >= 2);
        for id in &ids {
            state.diplomacy.set_standing(id, 50.0);
        }
        state.diplomacy.set_standing(&ids[1], -30.0);
        let hit = &advise(&state, Some(VictoryPath::Diplomacy))[0];
        assert_eq!(hit.id, "diplomacy");
        let name = &state.config.diplomacy.iter().nth(1).unwrap().name;
        assert!(hit.text.contains(name.as_str()), "{}", hit.text);
    }
}
