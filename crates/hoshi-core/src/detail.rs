//! Read-only selection details for the TUI side panel. No game maths in the UI.

use crate::buildings::BuildingKind;
use crate::defences::DefenceKind;
use crate::fleets::Coord;
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, ResourceKind, Resources};

/// What the side panel is describing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailSubject {
    Building {
        kind: BuildingKind,
        planet: Coord,
    },
    Tech(String),
    Ship(ShipKind),
    Defence(DefenceKind),
    Wonder(String),
    Law(String),
    /// A spot trade of `amount` whole units of `pay`.
    Trade {
        pay: ResourceKind,
        recv: ResourceKind,
        amount: u64,
    },
    Production {
        planet: Coord,
        id: String,
    },
    Faction(String),
    Coord(Coord),
}

/// Time until the stockpile can pay `cost` at current net income.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffordEta {
    Now,
    In(u64),
    Never,
}

/// One construction stockpile versus the next payment.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialLine {
    pub id: &'static str,
    pub stock: f64,
    pub cost: f64,
    pub missing: f64,
}

/// Current versus next-level quantity.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaLine {
    pub label: String,
    pub from: String,
    pub to: String,
}

/// A prerequisite with whether it is already met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementLine {
    pub text: String,
    pub met: bool,
}

/// Ship performance plus optional travel to a map target.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipTravel {
    pub speed: f64,
    pub fuel_per_unit: f64,
    pub cargo: f64,
    pub outbound_secs: Option<u64>,
    pub fuel_cost: Option<f64>,
}

/// Structured panel body. The TUI only formats these fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub name: String,
    pub description: String,
    pub level: u32,
    pub costs: Vec<MaterialLine>,
    pub energy_draw: Option<f64>,
    pub heat: Option<f64>,
    pub duration_secs: Option<u64>,
    pub afford: Option<AffordEta>,
    pub deltas: Vec<DeltaLine>,
    pub requirements: Vec<RequirementLine>,
    pub unlocks: Vec<String>,
    pub exclusive_warning: Option<String>,
    pub ship: Option<ShipTravel>,
}

/// Plant line for the side panel: mode, output, fuel, start-up and cooling cap.
fn fusion_summary(status: &crate::FusionStatus) -> String {
    let state = if status.plant.mode == crate::FusionMode::Off {
        "off".to_string()
    } else if status.plant.burning() {
        format!("{:.0}/{:.0} energy", status.output, status.rated)
    } else {
        format!(
            "start-up {:.0}% drawing {:.0}",
            status.startup_progress * 100.0,
            status.startup_draw
        )
    };
    let mut fuel = format!("deuterium {:.2}/h", status.deuterium_per_hour);
    if status.helium3_per_hour > 0.0 {
        fuel.push_str(&format!(", helium-3 {:.2}/h", status.helium3_per_hour));
    }
    format!(
        "; fusion {} {state}; {fuel}; cooling cap {:.0}",
        status.plant.mode.as_key(),
        status.cooling_cap
    )
}

impl GameState {
    /// Pure glance for a list selection. `travel_dest` is the map cursor for ships.
    pub fn describe(&self, subject: DetailSubject, travel_dest: Option<Coord>) -> Option<Detail> {
        match subject {
            DetailSubject::Building { kind, planet } => self.describe_building(kind, planet),
            DetailSubject::Tech(id) => self.describe_tech(&id),
            DetailSubject::Ship(kind) => Some(self.describe_ship(kind, travel_dest)),
            DetailSubject::Defence(kind) => Some(self.describe_defence(kind)),
            DetailSubject::Wonder(id) => self.describe_wonder(&id),
            DetailSubject::Law(id) => self.describe_law(&id),
            DetailSubject::Trade { pay, recv, amount } => {
                Some(self.describe_trade(pay, recv, amount as f64))
            }
            DetailSubject::Production { planet, id } => self.describe_production(planet, &id),
            DetailSubject::Faction(id) => self.describe_faction(&id),
            DetailSubject::Coord(coord) => Some(self.describe_coord(coord)),
        }
    }

    fn describe_building(&self, kind: BuildingKind, planet: Coord) -> Option<Detail> {
        let buildings = *self.buildings_at(planet)?;
        let queue = self.queue_at(planet).unwrap_or(&[]);
        let level = crate::state::projected_on(&buildings, queue, kind, queue.len());
        let spec = self.config.buildings.get(kind);
        let cost = self.build_cost_on(planet, kind);
        let duration = self.build_time_secs_on(planet, kind);
        let mut future_buildings = buildings;
        for building in BuildingKind::ALL {
            future_buildings.set(
                building,
                crate::state::projected_on(&buildings, queue, building, queue.len()),
            );
        }
        let mut projected = self.clone();
        if planet == projected.home {
            projected.buildings = future_buildings;
            projected.build_queue.clear();
        } else if let Some(colony) = projected
            .colonies
            .iter_mut()
            .find(|colony| colony.coord == planet)
        {
            colony.buildings = future_buildings;
            colony.build_queue.clear();
        }
        let mut next_state = projected.clone();
        apply_building_at(&mut next_state, planet, kind);
        let projected_buildings = *projected.buildings_at(planet).unwrap_or(&buildings);
        let next_buildings = *next_state.buildings_at(planet).unwrap_or(&buildings);
        let energy_now = self.config.energy_draw(&projected_buildings);
        let energy_next = next_state.config.energy_draw(&next_buildings);
        let mut deltas = Vec::new();
        for kind_res in ResourceKind::ALL {
            deltas.push(DeltaLine {
                label: format!("{} /h", kind_res.as_key()),
                from: format!("{:.1}", projected.production_at(planet, kind_res)),
                to: format!("{:.1}", next_state.production_at(planet, kind_res)),
            });
        }
        deltas.push(DeltaLine {
            label: "energy output".into(),
            from: format!("{:.1}", projected.energy_produced_at(planet)?),
            to: format!("{:.1}", next_state.energy_produced_at(planet)?),
        });
        deltas.push(DeltaLine {
            label: "energy draw".into(),
            from: format!("{energy_now:.0}"),
            to: format!("{energy_next:.0}"),
        });
        if let (Some(waste_now), Some(waste_next)) =
            (projected.waste_at(planet), next_state.waste_at(planet))
        {
            deltas.push(DeltaLine {
                label: "heat load".into(),
                from: format!("{:.0}", waste_now.heat_load),
                to: format!("{:.0}", waste_next.heat_load),
            });
        }
        let mut requirements: Vec<RequirementLine> = spec
            .prerequisites
            .iter()
            .map(|&(building, required)| {
                let actual = projected_buildings.get(building);
                RequirementLine {
                    text: format!("{building} {required} (have {actual})"),
                    met: actual >= required,
                }
            })
            .chain(
                spec.tech_prerequisites
                    .iter()
                    .map(|prerequisite| RequirementLine {
                        text: format!(
                            "{} {} (have {})",
                            prerequisite.id,
                            prerequisite.level,
                            self.tech_level(&prerequisite.id)
                        ),
                        met: self.tech_level(&prerequisite.id) >= prerequisite.level,
                    }),
            )
            .collect();
        let mut description = format!("next level {}", level.saturating_add(1));
        if kind == BuildingKind::FusionPlant
            && let Some(status) = self.fusion_status_at(planet)
        {
            description.push_str(&fusion_summary(&status));
        }
        if kind == BuildingKind::SolarPlant {
            let cap = self.max_solar_level(planet)?;
            requirements.push(RequirementLine {
                text: format!("solar cap {cap}; add fission or fusion for more power"),
                met: level < cap,
            });
        }
        Some(Detail {
            name: kind.to_string(),
            description,
            level,
            costs: material_lines_at(self, planet, cost),
            energy_draw: Some(energy_next - energy_now),
            heat: None,
            duration_secs: Some(duration),
            afford: Some(afford_eta_at(self, planet, cost)),
            deltas,
            requirements,
            unlocks: unlocks_from_building(self, kind),
            exclusive_warning: None,
            ship: None,
        })
    }

    fn describe_tech(&self, id: &str) -> Option<Detail> {
        let spec = self.config.techs.get(id)?;
        let level = self.tech_level(id);
        let cost = spec.cost(level);
        let duration = spec.duration_secs(level);
        let requirements = spec
            .prereq_buildings
            .iter()
            .map(|pre| {
                let actual = self.buildings.get(pre.building);
                RequirementLine {
                    text: format!("{} {} (have {actual})", pre.building, pre.level),
                    met: actual >= pre.level,
                }
            })
            .chain(spec.prereq_techs.iter().map(|pre| {
                let actual = self.tech_level(&pre.id);
                RequirementLine {
                    text: format!("{} {} (have {actual})", pre.id, pre.level),
                    met: actual >= pre.level,
                }
            }))
            .collect();
        Some(Detail {
            name: spec.name.clone(),
            description: format!("{} tier {}", spec.branch.as_key(), spec.tier),
            level,
            costs: material_lines(self, cost),
            energy_draw: None,
            heat: None,
            duration_secs: Some(duration),
            afford: Some(afford_eta(self, cost)),
            deltas: spec
                .modifiers
                .iter()
                .map(|m| DeltaLine {
                    label: format!("{:?}", m.kind).to_ascii_lowercase(),
                    from: format!("{:.2}", m.per_level * f64::from(level)),
                    to: format!("{:.2}", m.per_level * f64::from(level.saturating_add(1))),
                })
                .collect(),
            requirements,
            unlocks: unlocks_from_tech(self, id),
            exclusive_warning: exclusive_warning(self, spec),
            ship: None,
        })
    }

    fn describe_ship(&self, kind: ShipKind, travel_dest: Option<Coord>) -> Detail {
        let spec = self.config.ships.get(kind);
        let stats = self.ship_stats(kind);
        let cost = spec.unit_cost();
        let mut fleet = Fleet::default();
        fleet.add(kind, 1);
        let travel =
            travel_dest.and_then(|dest| self.flight_from(self.home, dest, fleet, 100).ok());
        let requirements: Vec<RequirementLine> = self
            .missing_ship_prereqs(kind)
            .into_iter()
            .map(|gap| RequirementLine {
                text: gap.to_string(),
                met: false,
            })
            .collect();
        Detail {
            name: spec.name.clone(),
            description: format!("drive {}", spec.drive),
            level: 0,
            costs: material_lines(self, cost),
            energy_draw: None,
            heat: None,
            duration_secs: Some(spec.duration_secs()),
            afford: Some(afford_eta(self, cost)),
            deltas: vec![
                DeltaLine {
                    label: "attack".into(),
                    from: "0".into(),
                    to: format!("{:.0}", stats.attack),
                },
                DeltaLine {
                    label: "hull".into(),
                    from: "0".into(),
                    to: format!("{:.0}", stats.hull),
                },
            ],
            requirements,
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: Some(ShipTravel {
                speed: stats.speed,
                fuel_per_unit: stats.fuel,
                cargo: stats.cargo,
                outbound_secs: travel.map(|(s, _)| s),
                fuel_cost: travel.map(|(_, f)| f),
            }),
        }
    }

    fn describe_defence(&self, kind: DefenceKind) -> Detail {
        let spec = self.config.defences.get(kind);
        let cost = spec.unit_cost();
        Detail {
            name: spec.name.clone(),
            description: "orbital or ground defence".into(),
            level: self.defences.get(kind),
            costs: material_lines(self, cost),
            energy_draw: None,
            heat: None,
            duration_secs: Some(spec.duration_secs()),
            afford: Some(afford_eta(self, cost)),
            deltas: Vec::new(),
            requirements: Vec::new(),
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: None,
        }
    }

    fn describe_wonder(&self, id: &str) -> Option<Detail> {
        let spec = self.config.wonders.iter().find(|s| s.id == id)?;
        let done = self.completed_wonders.iter().any(|done| done == id);
        let job = self.wonder.as_ref().filter(|job| job.id == id);
        let stage_idx = job.map(|job| job.stage).unwrap_or(0);
        let stage = spec.stages.get(stage_idx).or_else(|| spec.stages.first())?;
        let requirements = spec
            .prereq_techs
            .iter()
            .map(|pre| {
                let actual = self.tech_level(&pre.id);
                RequirementLine {
                    text: format!("{} {} (have {actual})", pre.id, pre.level),
                    met: actual >= pre.level,
                }
            })
            .collect();
        let (costs, duration, afford) = if done {
            (Vec::new(), None, None)
        } else {
            (
                material_lines(self, stage.cost),
                Some(stage.duration_secs),
                Some(afford_eta(self, stage.cost)),
            )
        };
        Some(Detail {
            name: spec.name.clone(),
            description: if done {
                "complete".into()
            } else {
                format!("stage {} / {}", stage_idx + 1, spec.stages.len())
            },
            level: if done {
                spec.stages.len() as u32
            } else {
                stage_idx as u32
            },
            costs,
            energy_draw: None,
            heat: None,
            duration_secs: duration,
            afford,
            deltas: Vec::new(),
            requirements,
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: None,
        })
    }

    fn describe_law(&self, id: &str) -> Option<Detail> {
        let spec = self.config.laws.get(id)?;
        let effects: Vec<String> = spec
            .effects
            .iter()
            .map(|e| format!("{:?}", e.kind))
            .collect();
        Some(Detail {
            name: spec.title.clone(),
            description: effects.join(", "),
            level: 0,
            costs: Vec::new(),
            energy_draw: None,
            heat: None,
            duration_secs: Some(spec.phase_in_secs),
            afford: None,
            deltas: Vec::new(),
            requirements: Vec::new(),
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: None,
        })
    }

    fn describe_trade(&self, pay: ResourceKind, recv: ResourceKind, amount: f64) -> Detail {
        let quote = self.quote_market(pay, recv, amount).ok();
        let paid = quote.as_ref().map_or(0.0, |q| q.pay_amount);
        let got = quote.as_ref().map_or(0.0, |q| q.recv_amount);
        let stock = |kind: ResourceKind| self.resources.get(kind);
        let mut deltas = vec![
            DeltaLine {
                label: format!("{} stock", pay.label()),
                from: format!("{:.0}", stock(pay)),
                to: format!("{:.0}", (stock(pay) - paid).max(0.0)),
            },
            DeltaLine {
                label: format!("{} stock", recv.label()),
                from: format!("{:.0}", stock(recv)),
                to: format!(
                    "{:.0}",
                    (stock(recv) + got)
                        .min(self.storage_cap(recv))
                        .max(stock(recv))
                ),
            },
        ];
        if let Some(q) = &quote {
            deltas.push(DeltaLine {
                label: "pay -> recv".into(),
                from: format!("{:.0} {}", q.pay_amount, pay.label()),
                to: format!("{:.0} {}", q.recv_amount, recv.label()),
            });
        }
        let requirements = quote
            .as_ref()
            .and_then(|q| self.trade_blocker(q))
            .map(|blocker| RequirementLine {
                text: blocker
                    .to_string()
                    .trim_start_matches("market: ")
                    .to_string(),
                met: false,
            })
            .into_iter()
            .collect();
        Detail {
            name: format!("{} -> {}", pay.as_key(), recv.as_key()),
            description: "spot trade".into(),
            level: 0,
            costs: Vec::new(),
            energy_draw: None,
            heat: None,
            duration_secs: None,
            afford: None,
            deltas,
            requirements,
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: None,
        }
    }

    fn describe_production(&self, planet: Coord, id: &str) -> Option<Detail> {
        let step = self
            .production_report()
            .steps
            .into_iter()
            .find(|s| s.planet == planet && s.id == id)?;
        let mut deltas: Vec<DeltaLine> = step
            .outputs_per_hour
            .iter()
            .map(|(good, rate)| DeltaLine {
                label: format!("{good} /h"),
                from: format!("{rate:.1}"),
                to: format!("{rate:.1}"),
            })
            .collect();
        for (good, rate) in &step.inputs_per_hour {
            deltas.push(DeltaLine {
                label: format!("{good} in /h"),
                from: format!("{rate:.1}"),
                to: format!("{rate:.1}"),
            });
        }
        Some(Detail {
            name: step.id,
            description: format!("{} at {planet}", step.limited_by.as_key()),
            level: 0,
            costs: Vec::new(),
            energy_draw: None,
            heat: None,
            duration_secs: None,
            afford: None,
            deltas,
            requirements: Vec::new(),
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: None,
        })
    }

    fn describe_faction(&self, id: &str) -> Option<Detail> {
        let spec = self.config.diplomacy.get(id)?;
        let standing = self.diplomacy.standing(id);
        let tier = spec
            .tier(standing)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "-".into());
        Some(Detail {
            name: spec.name.clone(),
            description: format!("standing {standing:.0} ({tier})"),
            level: 0,
            costs: Vec::new(),
            energy_draw: None,
            heat: None,
            duration_secs: None,
            afford: None,
            deltas: vec![DeltaLine {
                label: "tribute".into(),
                from: format!("{:.0}", spec.tribute_metal),
                to: format!("{:+.0}", spec.tribute_delta),
            }],
            requirements: Vec::new(),
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: None,
        })
    }

    fn describe_coord(&self, coord: Coord) -> Detail {
        let kind = self
            .planet_kind(coord)
            .map(|k| k.as_key().to_string())
            .unwrap_or_else(|| "empty".into());
        let size = self.planet_size(coord).unwrap_or(0);
        Detail {
            name: coord.to_string(),
            description: format!("{kind} size {size}"),
            level: 0,
            costs: Vec::new(),
            energy_draw: None,
            heat: None,
            duration_secs: None,
            afford: None,
            deltas: Vec::new(),
            requirements: Vec::new(),
            unlocks: Vec::new(),
            exclusive_warning: None,
            ship: None,
        }
    }
}

fn apply_building_at(state: &mut GameState, planet: Coord, kind: BuildingKind) {
    if planet == state.home {
        state.buildings.increment(kind);
    } else if let Some(colony) = state
        .colonies
        .iter_mut()
        .find(|colony| colony.coord == planet)
    {
        colony.buildings.increment(kind);
    }
}

fn stockpile_at(state: &GameState, planet: Coord) -> Resources {
    state
        .resources_at(planet)
        .copied()
        .unwrap_or(state.resources)
}

fn material_lines(state: &GameState, cost: Resources) -> Vec<MaterialLine> {
    material_from_stock(state.resources, cost)
}

fn material_lines_at(state: &GameState, planet: Coord, cost: Resources) -> Vec<MaterialLine> {
    material_from_stock(stockpile_at(state, planet), cost)
}

fn material_from_stock(stockpile: Resources, cost: Resources) -> Vec<MaterialLine> {
    ResourceKind::ALL
        .iter()
        .copied()
        .filter_map(|kind| {
            let need = cost.get(kind);
            let quoted = ResourceKind::QUOTED.contains(&kind);
            if need <= 1e-12 && !quoted {
                return None;
            }
            let stock = stockpile.get(kind);
            Some(MaterialLine {
                id: kind.as_key(),
                stock,
                cost: need,
                missing: (need - stock).max(0.0),
            })
        })
        .collect()
}

fn afford_eta(state: &GameState, cost: Resources) -> AffordEta {
    afford_from(
        state.resources,
        |kind| state.production_per_hour(kind),
        cost,
    )
}

fn afford_eta_at(state: &GameState, planet: Coord, cost: Resources) -> AffordEta {
    afford_from(
        stockpile_at(state, planet),
        |kind| state.production_at(planet, kind),
        cost,
    )
}

fn afford_from(
    stockpile: Resources,
    rate_of: impl Fn(ResourceKind) -> f64,
    cost: Resources,
) -> AffordEta {
    if stockpile.can_afford(cost) {
        return AffordEta::Now;
    }
    let mut wait = 0.0_f64;
    for kind in ResourceKind::ALL {
        let need = cost.get(kind);
        let got = stockpile.get(kind);
        if got + 1e-9 >= need {
            continue;
        }
        let rate = rate_of(kind);
        if rate <= 1e-12 {
            return AffordEta::Never;
        }
        wait = wait.max((need - got) / rate * 3600.0);
    }
    AffordEta::In(wait.ceil().max(1.0) as u64)
}

fn exclusive_warning(state: &GameState, spec: &crate::TechSpec) -> Option<String> {
    let group = spec.exclusive_group.as_deref()?;
    let others: Vec<String> = state
        .config
        .techs
        .iter()
        .filter(|other| other.exclusive_group.as_deref() == Some(group) && other.id != spec.id)
        .map(|other| other.name.clone())
        .collect();
    if others.is_empty() {
        return None;
    }
    Some(format!("locks {}: {}", group, others.join(", ")))
}

fn unlocks_from_tech(state: &GameState, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for spec in state.config.techs.iter() {
        if spec.prereq_techs.iter().any(|pre| pre.id == id) {
            out.push(spec.name.clone());
        }
    }
    for kind in ShipKind::ALL {
        if state
            .config
            .ships
            .get(kind)
            .prereq_techs
            .iter()
            .any(|pre| pre.id == id)
        {
            out.push(kind.to_string());
        }
    }
    out
}

fn unlocks_from_building(state: &GameState, kind: BuildingKind) -> Vec<String> {
    let mut out = Vec::new();
    for spec in state.config.techs.iter() {
        if spec.prereq_buildings.iter().any(|pre| pre.building == kind) {
            out.push(spec.name.clone());
        }
    }
    for other in BuildingKind::ALL {
        if self_prereq(state, other, kind) {
            out.push(other.to_string());
        }
    }
    out
}

fn self_prereq(state: &GameState, building: BuildingKind, want: BuildingKind) -> bool {
    state
        .config
        .buildings
        .get(building)
        .prerequisites
        .iter()
        .any(|&(req, _)| req == want)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;

    fn quiet() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg
    }

    #[test]
    fn time_until_affordable_is_never_when_income_is_zero() {
        let mut cfg = quiet();
        cfg.starting_buildings.metal_mine = 0;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_buildings.deuterium_synthesizer = 0;
        cfg.starting_buildings.solar_plant = 0;
        cfg.starting_resources.iron = 0.0;
        cfg.starting_resources.silicon = 0.0;
        cfg.starting_resources.deuterium = 0.0;
        let state = GameState::with_config(0, cfg);
        let detail = state
            .describe(DetailSubject::Tech("energy".into()), None)
            .unwrap();
        assert_eq!(detail.afford, Some(AffordEta::Never));
    }

    #[test]
    fn exclusive_branch_warning_lists_the_locked_siblings() {
        let state = GameState::with_config(0, quiet());
        let detail = state
            .describe(DetailSubject::Tech("orbital_harvest".into()), None)
            .unwrap();
        let warning = detail.exclusive_warning.expect("exclusive group");
        assert!(warning.contains("locks"), "{warning}");
        assert!(warning.contains("Deep-core mining"), "{warning}");
        assert!(!warning.contains("Surface"), "{warning}");
        assert!(!warning.contains("trade_focus"), "{warning}");
    }

    #[test]
    fn ship_travel_matches_dispatch_flight_maths() {
        let state = GameState::with_config(0, quiet());
        let dest = Coord {
            galaxy: 1,
            system: 2,
            position: 4,
        };
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let expected = state.flight_from(state.home, dest, ships, 100).unwrap();
        let detail = state
            .describe(DetailSubject::Ship(ShipKind::SmallCargo), Some(dest))
            .unwrap();
        let travel = detail.ship.expect("ship block");
        assert_eq!(travel.outbound_secs, Some(expected.0));
        assert!((travel.fuel_cost.unwrap() - expected.1).abs() < 1e-9);
    }

    #[test]
    fn building_detail_lists_stock_vs_cost() {
        let state = GameState::with_config(0, quiet());
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::MetalMine,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert_eq!(detail.name, "metal mine");
        assert_eq!(detail.costs.len(), 3);
        assert!(
            detail
                .costs
                .iter()
                .any(|row| row.id == "iron" && row.cost > 0.0)
        );
        assert!(matches!(
            detail.afford,
            Some(AffordEta::Now | AffordEta::In(_))
        ));
    }

    #[test]
    fn fusion_detail_shows_mode_startup_and_cooling_cap() {
        let mut state = GameState::with_config(0, quiet());
        state.buildings.fusion_plant = 1;
        state.resources.deuterium = 100.0;
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::FusionPlant,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert!(
            detail.description.contains("fusion dd start-up 0%")
                && detail.description.contains("cooling cap"),
            "{}",
            detail.description
        );
    }

    #[test]
    fn building_detail_uses_the_selected_planet_level_and_stock() {
        let mut state = GameState::with_config(0, quiet());
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        state.colonies.push(crate::Colony {
            coord: dest,
            kind: crate::PlanetType::Rocky,
            size: 120,
            resources: Resources::from_parts(50.0, 0.0, 0.0),
            buildings: crate::Buildings {
                metal_mine: 4,
                ..crate::Buildings::default()
            },
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        let home = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::MetalMine,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        let colony = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::MetalMine,
                    planet: dest,
                },
                None,
            )
            .unwrap();
        assert_ne!(home.level, colony.level);
        assert_eq!(colony.level, 4);
        let metal = colony
            .costs
            .iter()
            .find(|row| row.id == "iron")
            .expect("iron");
        assert!((metal.stock - 50.0).abs() < 1e-9, "{metal:?}");
        assert_eq!(
            colony.afford,
            Some(afford_eta_at(
                &state,
                dest,
                state.build_cost_on(dest, BuildingKind::MetalMine)
            ))
        );
    }

    #[test]
    fn wonder_detail_uses_the_active_stage_and_hides_cost_when_done() {
        let mut state = GameState::with_config(0, quiet());
        state.wonder = Some(crate::WonderJob {
            id: "dyson_ring".into(),
            stage: 1,
            contributed: Resources::ZERO,
            completes_at: None,
        });
        let active = state
            .describe(DetailSubject::Wonder("dyson_ring".into()), None)
            .unwrap();
        assert!(
            active.description.contains("stage 2"),
            "{}",
            active.description
        );
        let metal = active
            .costs
            .iter()
            .find(|row| row.id == "iron")
            .expect("iron");
        assert!((metal.cost - 40000.0).abs() < 1e-6, "{metal:?}");
        state.wonder = None;
        state.completed_wonders.push("dyson_ring".into());
        let done = state
            .describe(DetailSubject::Wonder("dyson_ring".into()), None)
            .unwrap();
        assert_eq!(done.description, "complete");
        assert!(
            done.costs.iter().all(|row| row.cost == 0.0),
            "{:?}",
            done.costs
        );
        assert_eq!(done.afford, None);
    }

    #[test]
    fn building_deltas_match_the_queued_next_level() {
        let mut state = GameState::with_config(0, quiet());
        state.buildings.metal_mine = 1;
        state.build_queue.push(crate::BuildJob {
            kind: crate::buildings::BuildJobKind::Build,
            work_left: 0.0,
            building: BuildingKind::MetalMine,
            from_level: 1,
            paid: Resources::ZERO,
            credits: 0.0,
            started_at: None,
            completes_at: None,
        });
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::MetalMine,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert_eq!(detail.level, 2);
        let mut after_queue = state.clone();
        after_queue.buildings.metal_mine = 2;
        after_queue.build_queue.clear();
        let mut after_next = after_queue.clone();
        after_next.buildings.increment(BuildingKind::MetalMine);
        let want = format!(
            "{:.1}",
            after_queue.production_at(state.home, ResourceKind::Iron)
        );
        let got = detail
            .deltas
            .iter()
            .find(|row| row.label == "iron /h")
            .expect("iron /h");
        assert_eq!(got.from, want);
        let want_to = format!(
            "{:.1}",
            after_next.production_at(state.home, ResourceKind::Iron)
        );
        assert_eq!(got.to, want_to);
    }
}
