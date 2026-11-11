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
    /// A unit as `planet`'s shipyard would build it.
    Ship {
        kind: ShipKind,
        planet: Coord,
    },
    Defence {
        kind: DefenceKind,
        planet: Coord,
    },
    Wonder(String),
    Law(String),
    /// A spot trade of `amount` whole units of `pay` from `planet`.
    Trade {
        pay: ResourceKind,
        recv: ResourceKind,
        amount: u64,
        planet: Coord,
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
    /// Deuterium one ship burns per `fuel_distance` at full speed.
    pub fuel_per_unit: f64,
    pub fuel_distance: f64,
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
    /// At the level cap: nothing further to build or research.
    pub complete: bool,
    pub ship: Option<ShipTravel>,
}

impl GameState {
    /// Pure glance for a list selection. `travel_dest` is the map cursor for ships.
    pub fn describe(&self, subject: DetailSubject, travel_dest: Option<Coord>) -> Option<Detail> {
        match subject {
            DetailSubject::Building { kind, planet } => self.describe_building(kind, planet),
            DetailSubject::Tech(id) => self.describe_tech(&id),
            DetailSubject::Ship { kind, planet } => {
                Some(self.describe_ship(kind, planet, travel_dest))
            }
            DetailSubject::Defence { kind, planet } => Some(self.describe_defence(kind, planet)),
            DetailSubject::Wonder(id) => self.describe_wonder(&id),
            DetailSubject::Law(id) => self.describe_law(&id),
            DetailSubject::Trade {
                pay,
                recv,
                amount,
                planet,
            } => Some(self.describe_trade(pay, recv, amount as f64, planet)),
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
                label: format!("{} /h", kind_res.label()),
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
            let heat_sink_moves = (waste_now.heat_sink - waste_next.heat_sink).abs() > f64::EPSILON;
            if kind == BuildingKind::Radiator || heat_sink_moves {
                deltas.push(DeltaLine {
                    label: "heat sink".into(),
                    from: format!("{:.0}", waste_now.heat_sink),
                    to: format!("{:.0}", waste_next.heat_sink),
                });
            }
            let pollution_load_moves =
                (waste_now.pollution_load - waste_next.pollution_load).abs() > f64::EPSILON;
            if pollution_load_moves {
                deltas.push(DeltaLine {
                    label: "pollution load".into(),
                    from: format!("{:.0}", waste_now.pollution_load),
                    to: format!("{:.0}", waste_next.pollution_load),
                });
            }
            let pollution_sink_moves =
                (waste_now.pollution_sink - waste_next.pollution_sink).abs() > f64::EPSILON;
            if kind == BuildingKind::Scrubber || pollution_sink_moves {
                deltas.push(DeltaLine {
                    label: "pollution sink".into(),
                    from: format!("{:.0}", waste_now.pollution_sink),
                    to: format!("{:.0}", waste_next.pollution_sink),
                });
            }
        }
        if kind == BuildingKind::FissionReactor {
            let spent = self
                .goods_at(planet)
                .get("spent_fuel")
                .copied()
                .unwrap_or(0.0);
            let pool = &self.config.fission_reactor;
            deltas.push(DeltaLine {
                label: "spent fuel pool".into(),
                from: format!(
                    "{spent:.0}/{:.0}",
                    pool.pool_capacity(projected_buildings.fission_reactor)
                ),
                to: format!(
                    "{spent:.0}/{:.0}",
                    pool.pool_capacity(next_buildings.fission_reactor)
                ),
            });
        }
        let mut requirements: Vec<RequirementLine> = spec
            .prerequisites
            .iter()
            .map(|&(building, required)| {
                let actual = self.available_building_level_at(planet, building);
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
                            self.display_name(&prerequisite.id),
                            prerequisite.level,
                            self.tech_level(&prerequisite.id)
                        ),
                        met: self.tech_level(&prerequisite.id) >= prerequisite.level,
                    }),
            )
            .collect();
        if kind == BuildingKind::SolarPlant {
            let cap = self.max_solar_level(planet)?;
            requirements.push(RequirementLine {
                text: format!("solar cap {cap}; add fission or fusion for more power"),
                met: level < cap,
            });
        }
        if kind == BuildingKind::RoboticsFactory {
            deltas.insert(
                0,
                DeltaLine {
                    label: "construction ×".into(),
                    from: format!(
                        "{:.1}",
                        construction_speed(
                            projected_buildings.robotics_factory,
                            self.config.robotics_speed_per_level,
                        )
                    ),
                    to: format!(
                        "{:.1}",
                        construction_speed(
                            next_buildings.robotics_factory,
                            self.config.robotics_speed_per_level,
                        )
                    ),
                },
            );
        }
        Some(Detail {
            name: kind.to_string(),
            description: spec.purpose.clone(),
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
            complete: false,
            ship: None,
        })
    }

    fn describe_tech(&self, id: &str) -> Option<Detail> {
        let spec = self.config.techs.get(id)?;
        let level = self.tech_level(id);
        let complete = spec.at_max(level);
        let cost = spec.cost(level);
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
                    text: format!(
                        "{} {} (have {actual})",
                        self.display_name(&pre.id),
                        pre.level
                    ),
                    met: actual >= pre.level,
                }
            }))
            .collect();
        // At the cap there is no next level: no cost, time or delta, so each
        // effect reads its current value on both sides.
        let next = if complete {
            level
        } else {
            level.saturating_add(1)
        };
        Some(Detail {
            name: spec.name.clone(),
            description: spec.purpose.clone(),
            level,
            costs: if complete {
                Vec::new()
            } else {
                material_lines(self, cost)
            },
            energy_draw: None,
            heat: None,
            duration_secs: (!complete).then(|| spec.duration_secs(level)),
            afford: (!complete).then(|| afford_eta(self, cost)),
            deltas: spec
                .modifiers
                .iter()
                .map(|m| DeltaLine {
                    label: m.kind.label().to_string(),
                    from: format!("{:.2}", m.per_level * f64::from(level)),
                    to: format!("{:.2}", m.per_level * f64::from(next)),
                })
                .collect(),
            requirements,
            unlocks: unlocks_from_tech(self, id),
            exclusive_warning: exclusive_warning(self, spec),
            complete,
            ship: None,
        })
    }

    fn describe_ship(&self, kind: ShipKind, planet: Coord, travel_dest: Option<Coord>) -> Detail {
        let spec = self.config.ships.get(kind);
        let stats = self.ship_stats(kind);
        let cost = spec.unit_cost();
        let mut fleet = Fleet::default();
        fleet.add(kind, 1);
        let travel =
            travel_dest.and_then(|dest| self.flight_from(self.home, dest, fleet, 100).ok());
        // Every prerequisite, met and unmet, like buildings and research.
        let requirements: Vec<RequirementLine> =
            std::iter::once(self.shipyard_line(planet, spec.shipyard))
                .chain(spec.prereq_techs.iter().map(|pre| {
                    let actual = self.tech_level(&pre.id);
                    RequirementLine {
                        text: format!(
                            "{} {} (have {actual})",
                            self.display_name(&pre.id),
                            pre.level
                        ),
                        met: actual >= pre.level,
                    }
                }))
                .collect();
        Detail {
            name: spec.name.clone(),
            description: spec.purpose.clone(),
            level: 0,
            costs: material_lines(self, cost),
            energy_draw: None,
            heat: None,
            duration_secs: Some(spec.duration_secs()),
            afford: Some(afford_eta_at(self, planet, cost)),
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
            complete: false,
            ship: Some(ShipTravel {
                speed: stats.speed,
                fuel_per_unit: stats.fuel,
                fuel_distance: self.config.flight.fuel_distance,
                cargo: stats.cargo,
                outbound_secs: travel.map(|(s, _)| s),
                fuel_cost: travel.map(|(_, f)| f),
            }),
        }
    }

    fn describe_defence(&self, kind: DefenceKind, planet: Coord) -> Detail {
        let spec = self.config.defences.get(kind);
        let cost = spec.unit_cost();
        Detail {
            name: spec.name.clone(),
            description: spec.purpose.clone(),
            level: self.defences_at(planet).unwrap_or_default().get(kind),
            costs: material_lines(self, cost),
            energy_draw: None,
            heat: None,
            duration_secs: Some(spec.duration_secs()),
            afford: Some(afford_eta_at(self, planet, cost)),
            deltas: Vec::new(),
            requirements: vec![self.shipyard_line(planet, spec.shipyard)],
            unlocks: Vec::new(),
            exclusive_warning: None,
            complete: false,
            ship: None,
        }
    }

    /// The shipyard level a ship or defence needs, against the level
    /// `planet`'s queue gate uses (0 while that shipyard is being dismantled).
    fn shipyard_line(&self, planet: Coord, required: u32) -> RequirementLine {
        let actual = self.available_building_level_at(planet, BuildingKind::Shipyard);
        RequirementLine {
            text: format!("{} {required} (have {actual})", BuildingKind::Shipyard),
            met: actual >= required,
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
                    text: format!(
                        "{} {} (have {actual})",
                        self.display_name(&pre.id),
                        pre.level
                    ),
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
            description: spec.purpose.clone(),
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
            complete: false,
            ship: None,
        })
    }

    fn describe_law(&self, id: &str) -> Option<Detail> {
        let spec = self.config.laws.get(id)?;
        let description = format!(
            "At {} {:.2}: {}",
            spec.param_name.replace('_', " "),
            spec.default,
            spec.effect_text(spec.default).join("; ")
        );
        Some(Detail {
            name: spec.title.clone(),
            description,
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
            complete: false,
            ship: None,
        })
    }

    /// Player-facing name for an id from the data files: a tech, wonder or
    /// ladder tier name, a building name, a resource's label (`helium_3` →
    /// `helium-3`), otherwise the id in words
    /// (`iron_ore` → `iron ore`). Ids never reach the screen as snake_case.
    pub fn display_name(&self, id: &str) -> String {
        if let Some(tech) = self.config.techs.get(id) {
            return tech.name.clone();
        }
        if let Some(wonder) = self.config.wonders.iter().find(|w| w.id == id) {
            return wonder.name.clone();
        }
        if let Some(tier) = self.config.ladder.tiers.iter().find(|t| t.id == id) {
            return tier.name.clone();
        }
        if let Some(kind) = BuildingKind::from_key(id) {
            return kind.to_string();
        }
        ResourceKind::from_key(id)
            .map_or_else(|| id.replace('_', " "), |kind| kind.label().to_string())
    }

    /// Player-facing form of a history object: prefixed references such as
    /// `tier:water_propellant`, `incident:1` or `building:1:1:1:metal_mine`
    /// read as `Water propellant`, `incident #1`, `metal mine at 1:1:1`.
    /// Coordinates and plain words pass through [`Self::display_name`].
    pub fn object_label(&self, object: &str) -> String {
        if object.split(':').all(|part| part.parse::<u32>().is_ok()) {
            return object.to_string();
        }
        let Some((prefix, rest)) = object.split_once(':') else {
            return self.display_name(object);
        };
        match prefix {
            // Ids can repeat across tables (a tier and a tech are both
            // `logistics`), so a typed prefix looks in its own table first.
            "tier" => self.ladder_tier_name(rest),
            "wonder" => self
                .config
                .wonders
                .iter()
                .find(|w| w.id == rest)
                .map_or_else(|| self.display_name(rest), |w| w.name.clone()),
            "research" | "tech" => self.display_name(rest),
            "building" | "defence" => match rest.rsplit_once(':') {
                Some((coord, id)) => format!("{} at {coord}", self.display_name(id)),
                None => self.display_name(rest),
            },
            _ if rest.parse::<u64>().is_ok() => format!("{} #{rest}", prefix.replace('_', " ")),
            _ => format!("{} {}", prefix.replace('_', " "), self.display_name(rest)),
        }
    }

    /// A ladder tier's name, looked up in the tier table alone.
    pub fn ladder_tier_name(&self, id: &str) -> String {
        self.config
            .ladder
            .tiers
            .iter()
            .find(|t| t.id == id)
            .map_or_else(|| self.display_name(id), |t| t.name.clone())
    }

    /// A prerequisite gap with the tech's display name.
    pub fn prereq_text(&self, gap: &crate::PrereqGap) -> String {
        match gap {
            crate::PrereqGap::Tech {
                id,
                required,
                actual,
            } => format!("{} level {required} (have {actual})", self.display_name(id)),
            other => other.to_string(),
        }
    }

    fn describe_trade(
        &self,
        pay: ResourceKind,
        recv: ResourceKind,
        amount: f64,
        planet: Coord,
    ) -> Detail {
        let quote = self.quote_market(pay, recv, amount).ok();
        let paid = quote.as_ref().map_or(0.0, |q| q.pay_amount);
        let got = quote.as_ref().map_or(0.0, |q| q.recv_amount);
        let stock = |kind: ResourceKind| {
            self.resources_at(planet)
                .map(|resources| resources.get(kind))
                .unwrap_or(0.0)
        };
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
                        .min(self.storage_cap_at(planet, recv))
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
            .and_then(|q| self.trade_blocker_at(planet, q))
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
            complete: false,
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
                label: format!("{} /h", self.display_name(good)),
                from: format!("{rate:.1}"),
                to: format!("{rate:.1}"),
            })
            .collect();
        for (good, rate) in &step.inputs_per_hour {
            deltas.push(DeltaLine {
                label: format!("{} in /h", self.display_name(good)),
                from: format!("{rate:.1}"),
                to: format!("{rate:.1}"),
            });
        }
        let purpose = step
            .outputs_per_hour
            .keys()
            .find_map(|id| {
                self.config
                    .chains
                    .goods
                    .iter()
                    .find(|good| good.id == *id)
                    .map(|good| good.purpose.clone())
            })
            .filter(|purpose| !purpose.is_empty())
            .unwrap_or_else(|| match step.limited_by.as_key() {
                "none" => format!("full rate at {planet}"),
                limit => format!("limited by {} at {planet}", limit.replace('_', " ")),
            });
        Some(Detail {
            name: self.display_name(&step.id),
            description: purpose,
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
            complete: false,
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
            complete: false,
            ship: None,
        })
    }

    /// What the star map knows about a slot. Size is only shown for owned
    /// planets; outposts and frontier worlds show the type the map rows show.
    fn describe_coord(&self, coord: Coord) -> Detail {
        let description = if let (Some(kind), Some(size)) =
            (self.planet_kind(coord), self.planet_size(coord))
        {
            format!("{} size {size}", kind.as_key())
        } else if let Some(outpost) = self.outpost(coord) {
            format!("pirate outpost · {}", outpost.kind.as_key())
        } else if let Some(world) = self.frontier.visible_worlds().find(|w| w.coord == coord) {
            format!("frontier world · {}", world.kind.as_key())
        } else if let Some(claim) = self.visible_rival_colony_at(coord) {
            format!("rival {} colony", claim.rival_id)
        } else {
            let kind = self.slot_type(coord);
            let type_name = self.display_name(kind.as_key());
            match self.deposits_known_at(coord) {
                Some(deposits) => {
                    let parts: Vec<String> = deposits
                        .iter()
                        .map(|(id, richness)| format!("{} {richness:.2}", self.display_name(id)))
                        .collect();
                    if parts.is_empty() {
                        format!("{type_name} · deposits none")
                    } else {
                        format!("{type_name} · {}", parts.join(", "))
                    }
                }
                None => format!("{type_name} · deposits unknown"),
            }
        };
        Detail {
            name: coord.to_string(),
            description,
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
            complete: false,
            ship: None,
        }
    }
}

fn construction_speed(robotics: u32, per_level: f64) -> f64 {
    1.0 + f64::from(robotics) * per_level
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
    Some(format!(
        "locks {}: {}",
        group.replace('_', " "),
        others.join(", ")
    ))
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

    #[test]
    fn law_detail_previews_its_effect_at_the_default_parameter() {
        let state = GameState::new(0);
        let research = state.describe(DetailSubject::Law("research_priority".into()), None);
        let research = research.unwrap();
        assert_eq!(research.description, "At focus 0.50: +5% research speed");
        let cap = state.config.laws.get("emission_cap").unwrap();
        assert!(cap.effect_text(0.8)[0].starts_with("emissions held to 80%"));
    }

    #[test]
    fn star_map_detail_names_outposts_and_empty_slots() {
        let state = GameState::new_live_galaxy(10, quiet(), 7);
        let outpost = state.outposts[0].clone();
        let detail = state
            .describe(DetailSubject::Coord(outpost.coord), None)
            .unwrap();
        assert_eq!(
            detail.description,
            format!("pirate outpost · {}", outpost.kind.as_key())
        );
        let empty = Coord {
            galaxy: 9,
            system: 499,
            position: 15,
        };
        let detail = state.describe(DetailSubject::Coord(empty), None).unwrap();
        let kind = state.slot_type(empty);
        assert_eq!(
            detail.description,
            format!("{} · deposits unknown", state.display_name(kind.as_key()))
        );
        let home = state
            .describe(DetailSubject::Coord(state.home), None)
            .unwrap();
        assert!(home.description.contains(" size "), "{}", home.description);
    }

    #[test]
    fn star_map_detail_lists_deposits_after_a_survey() {
        let mut state = GameState::new(0);
        state.config.survey = crate::SurveyConfig::default();
        state.resources.deuterium = 10_000.0;
        state.fleet.add(crate::ShipKind::EspionageProbe, 1);
        let dest = (2u16..=16)
            .flat_map(|system| {
                (2u16..=16).map(move |position| Coord {
                    galaxy: state.home.galaxy,
                    system,
                    position,
                })
            })
            .find(|coord| !state.occupied(*coord) && state.site_at(*coord).is_none())
            .expect("empty unowned slot");
        let before = state.describe(DetailSubject::Coord(dest), None).unwrap();
        assert!(
            before.description.contains("deposits unknown"),
            "{}",
            before.description
        );
        state
            .launch_spy(
                state.home,
                {
                    let mut ships = crate::Fleet::default();
                    ships.add(crate::ShipKind::EspionageProbe, 1);
                    ships
                },
                dest,
                100,
                0,
            )
            .unwrap();
        let arrive = state.missions[0].phase_ends_at;
        state.catch_up(arrive);
        let after = state.describe(DetailSubject::Coord(dest), None).unwrap();
        assert!(
            !after.description.contains("deposits unknown"),
            "{}",
            after.description
        );
        assert!(after.description.contains(" · "), "{}", after.description);
    }

    #[test]
    fn star_map_detail_names_a_visible_rival_colony() {
        let mut state = GameState::new_live_galaxy(10, quiet(), 7);
        let dest = Coord {
            galaxy: state.home.galaxy,
            system: state.home.system,
            position: 4,
        };
        state.rivals.colonies.push(crate::RivalColony {
            rival_id: "rival-0-trader".into(),
            coord: dest,
            founded_at: 1,
            visible_at: 10,
        });
        state.last_tick_unix = 9;
        let hidden = state.describe(DetailSubject::Coord(dest), None).unwrap();
        let kind = state.slot_type(dest);
        assert!(
            hidden
                .description
                .starts_with(&state.display_name(kind.as_key())),
            "{}",
            hidden.description
        );
        assert!(
            !hidden.description.contains("rival"),
            "{}",
            hidden.description
        );
        state.last_tick_unix = 10;
        let seen = state.describe(DetailSubject::Coord(dest), None).unwrap();
        assert_eq!(seen.description, "rival rival-0-trader colony");
    }

    #[test]
    fn object_labels_read_as_names_not_ids() {
        let state = GameState::new(0);
        let tier = &state.config.ladder.tiers[0];
        assert_eq!(state.object_label(&format!("tier:{}", tier.id)), tier.name);
        assert_eq!(state.object_label("incident:1"), "incident #1");
        assert_eq!(state.object_label("1:1:1"), "1:1:1");
        assert_eq!(
            state.object_label("building:1:1:1:metal_mine"),
            "iron mine at 1:1:1"
        );
        assert_eq!(state.object_label("rare_earths"), "rare earths");
        // The tier and the tech share the id `logistics`.
        if let Some(tier) = state
            .config
            .ladder
            .tiers
            .iter()
            .find(|t| t.id == "logistics")
        {
            assert_eq!(state.object_label("tier:logistics"), tier.name);
        }
        let wonder = &state.config.wonders[0];
        assert_eq!(state.display_name(&wonder.id), wonder.name);
    }

    fn quiet() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg
    }

    #[test]
    fn shipyard_detail_stays_blocked_while_robotics_is_only_queued() {
        let mut cfg = quiet();
        cfg.starting_resources = Resources::from_parts(1e6, 1e6, 1e6);
        let mut state = GameState::with_config(0, cfg);
        state
            .enqueue_build(BuildingKind::RoboticsFactory, 0)
            .unwrap();
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::Shipyard,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert!(
            detail.requirements.iter().any(|row| {
                !row.met && row.text == format!("{} 1 (have 0)", BuildingKind::RoboticsFactory)
            }),
            "{:?}",
            detail.requirements
        );
        let done = state.build_queue[0].completes_at.unwrap();
        state.catch_up(done);
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::Shipyard,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert!(
            detail.requirements.iter().any(|row| {
                row.met && row.text == format!("{} 1 (have 1)", BuildingKind::RoboticsFactory)
            }),
            "{:?}",
            detail.requirements
        );
    }

    /// #297: a ship's inspector listed only its unmet research, and never
    /// the shipyard the row was waiting for.
    #[test]
    fn ship_and_defence_list_every_prerequisite_met_or_not() {
        let mut state = GameState::new(0);
        let ship = state
            .describe(
                DetailSubject::Ship {
                    kind: ShipKind::SmallCargo,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        let spec = state.config.ships.get(ShipKind::SmallCargo).clone();
        assert_eq!(
            ship.requirements.len(),
            1 + spec.prereq_techs.len(),
            "{:?}",
            ship.requirements
        );
        assert!(
            ship.requirements
                .iter()
                .any(|r| !r.met && r.text == format!("shipyard {} (have 0)", spec.shipyard))
        );
        assert!(ship.requirements.iter().skip(1).all(|r| !r.met));

        state.buildings.shipyard = spec.shipyard;
        let ship = state
            .describe(
                DetailSubject::Ship {
                    kind: ShipKind::SmallCargo,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert!(ship.requirements[0].met, "{:?}", ship.requirements);
        assert_eq!(ship.requirements.len(), 1 + spec.prereq_techs.len());

        let cannon = state
            .describe(
                DetailSubject::Defence {
                    kind: crate::DefenceKind::LightCannon,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert_eq!(cannon.requirements.len(), 1);
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
    fn single_level_tech_reads_complete_once_researched() {
        let mut state = GameState::with_config(0, quiet());
        let fresh = state
            .describe(DetailSubject::Tech("deep_core".into()), None)
            .unwrap();
        assert!(!fresh.complete);
        assert_eq!(
            fresh.description,
            state.config.techs.get("deep_core").unwrap().purpose
        );
        assert_eq!(
            (fresh.deltas[0].from.as_str(), fresh.deltas[0].to.as_str()),
            ("0.00", "0.08")
        );
        state.tech_levels.insert("deep_core".into(), 1);
        let done = state
            .describe(DetailSubject::Tech("deep_core".into()), None)
            .unwrap();
        assert!(done.complete);
        assert!(done.costs.is_empty());
        assert_eq!((done.duration_secs, done.afford), (None, None));
        assert_eq!(
            (done.deltas[0].from.as_str(), done.deltas[0].to.as_str()),
            ("0.08", "0.08")
        );
        let energy = state
            .describe(DetailSubject::Tech("energy".into()), None)
            .unwrap();
        assert!(!energy.complete);
        assert_ne!(energy.deltas[0].from, energy.deltas[0].to);
    }

    #[test]
    fn complete_flag_matches_research_refusal_for_every_capped_tech() {
        let base = GameState::with_config(0, quiet());
        let capped: Vec<_> = base
            .config
            .techs
            .iter()
            .filter_map(|spec| spec.max_level.map(|max| (spec.clone(), max)))
            .collect();
        assert!(!capped.is_empty());
        for (spec, max) in capped {
            for level in [max - 1, max] {
                let mut state = base.clone();
                for _ in 0..state.config.techs.ancient_prestige_runs {
                    state.prestige.runs.push(crate::RunStats::new(1, 0));
                }
                for pre in &spec.prereq_buildings {
                    state.buildings.set(pre.building, pre.level);
                }
                for pre in &spec.prereq_techs {
                    state.tech_levels.insert(pre.id.clone(), pre.level);
                }
                state.tech_levels.insert(spec.id.clone(), level);
                state.resources = Resources::from_parts(1e12, 1e12, 1e12);
                let detail = state
                    .describe(DetailSubject::Tech(spec.id.clone()), None)
                    .unwrap();
                let refused = state.start_research(&spec.id, 0);
                assert_eq!(
                    detail.complete,
                    matches!(refused, Err(crate::ResearchError::Complete { .. })),
                    "{} at {level}: {refused:?}",
                    spec.id
                );
                assert_eq!(detail.complete, level == max, "{}", spec.id);
                if let Err(crate::ResearchError::Complete { .. }) = &refused {
                    assert_eq!(
                        refused.unwrap_err().to_string(),
                        format!("{} is already complete", spec.name)
                    );
                }
            }
        }
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
            .describe(
                DetailSubject::Ship {
                    kind: ShipKind::SmallCargo,
                    planet: state.home,
                },
                Some(dest),
            )
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
        assert_eq!(detail.name, "iron mine");
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
        assert_eq!(
            detail.description,
            state
                .config
                .buildings
                .get(BuildingKind::FusionPlant)
                .purpose
        );
        let brief = state
            .fusion_status_at(state.home)
            .expect("fusion plant")
            .brief();
        assert!(
            brief.contains("dd start-up") && brief.contains("cooling cap"),
            "{brief}"
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
            ship_queue: Vec::new(),
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
    fn trade_detail_uses_the_selected_planet_stock() {
        let mut state = GameState::new(0);
        state.config.market.enabled = true;
        state.config.market.cargo_free = 10_000.0;
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        state.colonies.push(crate::Colony {
            coord: dest,
            kind: crate::PlanetType::Rocky,
            size: 120,
            resources: Resources::from_parts(400.0, 0.0, 0.0),
            buildings: crate::Buildings::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            ship_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.resources.iron = 9_000.0;
        let detail = state
            .describe(
                DetailSubject::Trade {
                    pay: ResourceKind::Iron,
                    recv: ResourceKind::Silicon,
                    amount: 100,
                    planet: dest,
                },
                None,
            )
            .unwrap();
        let iron = detail
            .deltas
            .iter()
            .find(|row| row.label == "iron stock")
            .expect("iron stock");
        assert_eq!(iron.from, "400");
        assert_eq!(iron.to, "300");
        assert!(
            !detail.requirements.iter().any(|row| !row.met),
            "{:?}",
            detail.requirements
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
        let spec = state
            .config
            .wonders
            .iter()
            .find(|wonder| wonder.id == "dyson_ring")
            .unwrap();
        assert_eq!(active.description, spec.purpose);
        assert_eq!(active.level, 1);
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
        assert_eq!(done.description, spec.purpose);
        assert_eq!(done.level, spec.stages.len() as u32);
        assert!(
            done.costs.iter().all(|row| row.cost == 0.0),
            "{:?}",
            done.costs
        );
        assert_eq!(done.afford, None);
    }

    /// #426: the 1 -> 2 reactor preview showed +6 output against +8 draw.
    #[test]
    fn reactor_upgrade_preview_adds_net_energy() {
        let mut state = GameState::with_config(0, quiet());
        state.buildings.fission_reactor = 1;
        state.buildings.radiator = 10;
        state.goods.insert("reactor_fuel".into(), 10.0);
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::FissionReactor,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        let output = detail
            .deltas
            .iter()
            .find(|row| row.label == "energy output")
            .expect("energy output");
        let output_gain = output.to.parse::<f64>().unwrap() - output.from.parse::<f64>().unwrap();
        let draw_gain = detail.energy_draw.expect("energy draw");
        assert!((draw_gain - 8.0 * 0.3).abs() < 1e-9, "draw {draw_gain}");
        assert!(output_gain - draw_gain > 0.0, "output {output_gain}");
    }

    #[test]
    fn reactor_detail_shows_pool_fill_against_both_capacities() {
        let mut state = GameState::with_config(0, quiet());
        state.buildings.fission_reactor = 2;
        state.goods.insert("spent_fuel".into(), 42.0);
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::FissionReactor,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        let pool = detail
            .deltas
            .iter()
            .find(|row| row.label == "spent fuel pool")
            .expect("pool line");
        assert_eq!((pool.from.as_str(), pool.to.as_str()), ("42/60", "42/90"));
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
        assert!(
            detail
                .deltas
                .iter()
                .all(|row| row.label != "construction ×"),
            "{:?}",
            detail.deltas
        );
    }

    fn construction_delta(state: &GameState) -> DeltaLine {
        state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::RoboticsFactory,
                    planet: state.home,
                },
                None,
            )
            .unwrap()
            .deltas
            .into_iter()
            .find(|row| row.label == "construction ×")
            .expect("construction ×")
    }

    #[test]
    fn robotics_detail_shows_construction_speed_now_and_next() {
        let state = GameState::with_config(0, quiet());
        assert_eq!(state.config.robotics_speed_per_level, 1.0);
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::RoboticsFactory,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert_eq!(
            detail.description,
            state
                .config
                .buildings
                .get(BuildingKind::RoboticsFactory)
                .purpose
        );
        let row = construction_delta(&state);
        assert_eq!((row.from.as_str(), row.to.as_str()), ("1.0", "2.0"));

        let mut upgraded = state;
        upgraded.buildings.robotics_factory = 1;
        let row = construction_delta(&upgraded);
        assert_eq!((row.from.as_str(), row.to.as_str()), ("2.0", "3.0"));
    }

    #[test]
    fn explorer_detail_names_the_expedition_find_bonus() {
        let state = GameState::with_config(0, quiet());
        assert_eq!(state.config.expedition.explorer_mult, 2.0);
        let explorer = state
            .describe(
                DetailSubject::Ship {
                    kind: ShipKind::Explorer,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert_eq!(
            explorer.description,
            state.config.ships.get(ShipKind::Explorer).purpose
        );
        let cargo = state
            .describe(
                DetailSubject::Ship {
                    kind: ShipKind::SmallCargo,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert_eq!(
            cargo.description,
            state.config.ships.get(ShipKind::SmallCargo).purpose
        );
        assert!(!cargo.description.contains("expedition"));
    }

    #[test]
    fn bundled_catalog_objects_have_a_purpose() {
        let cfg = BalanceConfig::default();
        for kind in BuildingKind::ALL {
            assert!(!cfg.buildings.get(kind).purpose.trim().is_empty(), "{kind}");
        }
        for kind in ShipKind::ALL {
            assert!(!cfg.ships.get(kind).purpose.trim().is_empty(), "{kind}");
        }
        for kind in crate::DefenceKind::ALL {
            assert!(!cfg.defences.get(kind).purpose.trim().is_empty(), "{kind}");
        }
        for spec in cfg.techs.iter() {
            assert!(!spec.purpose.trim().is_empty(), "{}", spec.id);
        }
        for spec in &cfg.wonders {
            assert!(!spec.purpose.trim().is_empty(), "{}", spec.id);
        }
        for spec in &cfg.chains.goods {
            assert!(!spec.purpose.trim().is_empty(), "{}", spec.id);
        }
        for species in crate::SpeciesCatalog::default().iter() {
            for spec in &species.techs {
                assert!(
                    !spec.purpose.trim().is_empty(),
                    "{} {}",
                    species.id,
                    spec.id
                );
            }
        }
    }

    fn waste_delta(state: &GameState, kind: BuildingKind, label: &str) -> DeltaLine {
        state
            .describe(
                DetailSubject::Building {
                    kind,
                    planet: state.home,
                },
                None,
            )
            .unwrap()
            .deltas
            .into_iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| panic!("{kind} {label}"))
    }

    #[test]
    fn scrubber_detail_names_purpose_and_pollution_sink() {
        let state = GameState::with_config(0, quiet());
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::Scrubber,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert_eq!(
            detail.description,
            state.config.buildings.get(BuildingKind::Scrubber).purpose
        );
        let row = waste_delta(&state, BuildingKind::Scrubber, "pollution sink");
        assert_ne!(row.from, row.to, "{row:?}");
    }

    #[test]
    fn radiator_detail_names_purpose_and_heat_sink() {
        let state = GameState::with_config(0, quiet());
        let detail = state
            .describe(
                DetailSubject::Building {
                    kind: BuildingKind::Radiator,
                    planet: state.home,
                },
                None,
            )
            .unwrap();
        assert_eq!(
            detail.description,
            state.config.buildings.get(BuildingKind::Radiator).purpose
        );
        let row = waste_delta(&state, BuildingKind::Radiator, "heat sink");
        assert_ne!(row.from, row.to, "{row:?}");
    }
}
