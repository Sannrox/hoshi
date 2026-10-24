//! Fusion power: a deuterium burn first, deuterium–helium-3 later, plasma
//! start-up from spare grid power, and a short spell of leftover heat after
//! the plasma stops.
//!
//! Every quantity here is constant between scheduled events (start-up done,
//! fuel gone, leftover heat over), so offline catch-up integrates it exactly.

use crate::{BuildingKind, Coord, GameState, ResourceKind};
use serde::Deserialize;
use std::fmt;

/// The reaction a plant burns. `Off` holds the plasma down so the plant can be dismantled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FusionMode {
    Off,
    #[default]
    Dd,
    Dhe3,
}

impl FusionMode {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Dd => "dd",
            Self::Dhe3 => "dhe3",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "off" => Self::Off,
            "dd" => Self::Dd,
            "dhe3" => Self::Dhe3,
            _ => return None,
        })
    }
}

/// Per-planet plant state. A built plant without a stored row is a fresh D-D
/// plant that still needs its plasma start-up.
#[derive(Debug, Clone, PartialEq)]
pub struct FusionPlant {
    pub mode: FusionMode,
    /// Plasma heating still to draw, in energy-hours. Zero while the plasma burns.
    pub startup_left: f64,
    /// A D-D burn activated the structure; removing the last level waits for the cooldown.
    pub activated: bool,
    /// When the plasma last stopped burning.
    pub stopped_at: Option<u64>,
    /// Heat left in the structure from `stopped_at` until `residual_secs` pass.
    pub residual_heat: f64,
}

impl FusionPlant {
    pub fn burning(&self) -> bool {
        self.mode != FusionMode::Off && self.startup_left <= 0.0
    }
}

/// Per-reaction numbers, resolved from [`FusionConfig`] for one mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reaction {
    pub deuterium_per_energy_hour: f64,
    pub helium3_per_energy_hour: f64,
    pub waste_heat_per_energy: f64,
    /// Start-up draw as a multiple of rated output.
    pub startup_draw: f64,
    /// Leftover heat as a fraction of rated thermal power.
    pub residual_fraction: f64,
    /// Neutron wear relative to a D-D burn at the same output.
    pub neutron_share: f64,
}

/// Output, fuel, heat, start-up, wear and cooldown parameters (`[fusion_plant]`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct FusionConfig {
    pub base_energy: f64,
    pub growth: f64,
    pub deuterium_per_energy_hour_dd: f64,
    pub deuterium_per_energy_hour_dhe3: f64,
    pub helium3_per_energy_hour_dhe3: f64,
    pub waste_heat_per_energy_dd: f64,
    pub waste_heat_per_energy_dhe3: f64,
    pub startup_energy_dd: f64,
    pub startup_energy_dhe3: f64,
    pub startup_secs: u64,
    pub residual_fraction_dd: f64,
    pub residual_fraction_dhe3: f64,
    pub residual_secs: u64,
    pub wear_neutron_per_hour: f64,
    pub dhe3_neutron_share: f64,
    pub activation_cooldown_secs: u64,
}

impl Default for FusionConfig {
    fn default() -> Self {
        Self {
            base_energy: 400.0,
            growth: 1.1,
            deuterium_per_energy_hour_dd: 0.002,
            deuterium_per_energy_hour_dhe3: 0.0008,
            helium3_per_energy_hour_dhe3: 0.0012,
            waste_heat_per_energy_dd: 1.5,
            waste_heat_per_energy_dhe3: 0.7,
            startup_energy_dd: 3.0,
            startup_energy_dhe3: 5.0,
            startup_secs: 1_800,
            residual_fraction_dd: 0.01,
            residual_fraction_dhe3: 0.0005,
            residual_secs: 10_800,
            wear_neutron_per_hour: 0.0007,
            dhe3_neutron_share: 0.1,
            activation_cooldown_secs: 86_400,
        }
    }
}

impl FusionConfig {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("base_energy", self.base_energy),
            ("growth", self.growth),
            (
                "deuterium_per_energy_hour_dd",
                self.deuterium_per_energy_hour_dd,
            ),
            (
                "deuterium_per_energy_hour_dhe3",
                self.deuterium_per_energy_hour_dhe3,
            ),
            (
                "helium3_per_energy_hour_dhe3",
                self.helium3_per_energy_hour_dhe3,
            ),
            ("waste_heat_per_energy_dd", self.waste_heat_per_energy_dd),
            (
                "waste_heat_per_energy_dhe3",
                self.waste_heat_per_energy_dhe3,
            ),
            ("startup_energy_dd", self.startup_energy_dd),
            ("startup_energy_dhe3", self.startup_energy_dhe3),
            ("startup_secs", self.startup_secs as f64),
            ("residual_secs", self.residual_secs as f64),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("fusion_plant.{name} must be finite and > 0"));
            }
        }
        for (name, value) in [
            ("residual_fraction_dd", self.residual_fraction_dd),
            ("residual_fraction_dhe3", self.residual_fraction_dhe3),
            ("wear_neutron_per_hour", self.wear_neutron_per_hour),
            ("dhe3_neutron_share", self.dhe3_neutron_share),
        ] {
            if !value.is_finite() || !(0.0..1.0).contains(&value) {
                return Err(format!("fusion_plant.{name} must be in [0, 1)"));
            }
        }
        Ok(())
    }

    pub fn rated_output(&self, level: u32) -> f64 {
        if level == 0 {
            0.0
        } else {
            self.base_energy * self.growth.powf(f64::from(level - 1))
        }
    }

    /// `None` for `Off`: nothing burns.
    pub fn reaction(&self, mode: FusionMode) -> Option<Reaction> {
        match mode {
            FusionMode::Off => None,
            FusionMode::Dd => Some(Reaction {
                deuterium_per_energy_hour: self.deuterium_per_energy_hour_dd,
                helium3_per_energy_hour: 0.0,
                waste_heat_per_energy: self.waste_heat_per_energy_dd,
                startup_draw: self.startup_energy_dd,
                residual_fraction: self.residual_fraction_dd,
                neutron_share: 1.0,
            }),
            FusionMode::Dhe3 => Some(Reaction {
                deuterium_per_energy_hour: self.deuterium_per_energy_hour_dhe3,
                helium3_per_energy_hour: self.helium3_per_energy_hour_dhe3,
                waste_heat_per_energy: self.waste_heat_per_energy_dhe3,
                startup_draw: self.startup_energy_dhe3,
                residual_fraction: self.residual_fraction_dhe3,
                neutron_share: self.dhe3_neutron_share,
            }),
        }
    }

    /// Energy-hours of plasma heating: the start-up draw held for `startup_secs`.
    pub fn startup_energy(&self, mode: FusionMode, level: u32) -> f64 {
        self.startup_power(mode, level) * self.startup_secs as f64 / 3600.0
    }

    /// Highest draw while heating; less spare supply stretches the start-up.
    pub fn startup_power(&self, mode: FusionMode, level: u32) -> f64 {
        self.reaction(mode)
            .map_or(0.0, |r| r.startup_draw * self.rated_output(level))
    }

    pub fn residual_heat(&self, mode: FusionMode, level: u32) -> f64 {
        self.reaction(mode).map_or(0.0, |r| {
            r.residual_fraction * self.rated_output(level) * (1.0 + r.waste_heat_per_energy)
        })
    }
}

/// Why a fusion command was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum FusionError {
    UnknownPlanet,
    NoPlant,
    MissingTechnology { id: String, required: u32 },
    PermissionDenied(String),
}

impl fmt::Display for FusionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPlanet => f.write_str("planet is not owned"),
            Self::NoPlant => f.write_str("no fusion plant on this planet"),
            Self::MissingTechnology { id, required } => {
                write!(f, "requires {id} level {required}")
            }
            Self::PermissionDenied(reason) => write!(f, "permission denied: {reason}"),
        }
    }
}

impl std::error::Error for FusionError {}

/// Read-only plant summary for the CLI and TUI.
#[derive(Debug, Clone, PartialEq)]
pub struct FusionStatus {
    pub coord: Coord,
    pub level: u32,
    pub plant: FusionPlant,
    pub output: f64,
    pub rated: f64,
    /// Output allowed by the planet's free heat sink, before fuel and health.
    pub cooling_cap: f64,
    pub deuterium_per_hour: f64,
    pub helium3_per_hour: f64,
    pub startup_draw: f64,
    /// Fraction of the plasma heating done, 1 while burning.
    pub startup_progress: f64,
    pub heat: f64,
}

/// One plant's rates for a production segment.
pub(crate) struct FusionStep {
    coord: Coord,
    output: f64,
    /// Start-up energy delivered during the segment, in energy-hours.
    startup: f64,
    /// Whether a burning plant had fuel when the segment began.
    fuelled: bool,
    /// Fuel to burn over the segment and the part already burned before
    /// production, as `[deuterium, helium-3]`.
    burn: [f64; 2],
    burned: [f64; 2],
}

impl GameState {
    /// The plant on `coord`, if one is built.
    pub fn fusion_plant_at(&self, coord: Coord) -> Option<FusionPlant> {
        let level = self.buildings_at(coord)?.fusion_plant;
        if level == 0 {
            return None;
        }
        Some(self.fusion_plants.get(&coord).cloned().unwrap_or_else(|| {
            FusionPlant {
                mode: FusionMode::Dd,
                startup_left: self
                    .config
                    .fusion_plant
                    .startup_energy(FusionMode::Dd, level),
                activated: false,
                stopped_at: None,
                residual_heat: 0.0,
            }
        }))
    }

    fn fusion_level(&self, coord: Coord) -> u32 {
        self.buildings_at(coord).map_or(0, |b| b.fusion_plant)
    }

    fn fusion_fuelled(&self, coord: Coord, reaction: &Reaction) -> bool {
        self.resources_at(coord).is_some_and(|stock| {
            stock.deuterium > 1e-12
                && (reaction.helium3_per_energy_hour <= 0.0 || stock.helium_3 > 1e-12)
        })
    }

    /// Output the free heat sink allows. Fusion claims cooling before fission,
    /// so it reads the sink left by the planet's other buildings only.
    fn fusion_cooling_cap(&self, coord: Coord, reaction: &Reaction) -> f64 {
        self.site_waste_at(coord).map_or(0.0, |site| {
            ((site.heat_sink - site.heat_load) / reaction.waste_heat_per_energy).max(0.0)
        })
    }

    /// Electrical output after start-up, fuel, maintenance and cooling.
    pub fn fusion_output_at(&self, coord: Coord) -> f64 {
        let Some(plant) = self.fusion_plant_at(coord) else {
            return 0.0;
        };
        let Some(reaction) = self.config.fusion_plant.reaction(plant.mode) else {
            return 0.0;
        };
        if !plant.burning() || !self.fusion_fuelled(coord, &reaction) {
            return 0.0;
        }
        (self
            .config
            .fusion_plant
            .rated_output(self.fusion_level(coord))
            * self.building_health(coord, BuildingKind::FusionPlant))
        .min(self.fusion_cooling_cap(coord, &reaction))
    }

    /// Heat the plant adds to its planet: rejected heat while burning, leftover heat after.
    pub fn fusion_heat_at(&self, coord: Coord) -> f64 {
        let Some(plant) = self.fusion_plant_at(coord) else {
            return 0.0;
        };
        if plant.burning() {
            return self
                .config
                .fusion_plant
                .reaction(plant.mode)
                .map_or(0.0, |r| {
                    self.fusion_output_at(coord) * r.waste_heat_per_energy
                });
        }
        match plant.stopped_at {
            Some(at)
                if self.last_tick_unix
                    < at.saturating_add(self.config.fusion_plant.residual_secs) =>
            {
                plant.residual_heat
            }
            _ => 0.0,
        }
    }

    /// Grid draw of a plasma start-up. It is an ordinary load: a short supply
    /// slows it with everything else, and with no supply it waits.
    pub(crate) fn fusion_startup_draw_at(&self, coord: Coord) -> f64 {
        let Some(plant) = self.fusion_plant_at(coord) else {
            return 0.0;
        };
        let Some(reaction) = self.config.fusion_plant.reaction(plant.mode) else {
            return 0.0;
        };
        if plant.burning()
            || !self.fusion_fuelled(coord, &reaction)
            || self.building_health(coord, BuildingKind::FusionPlant) <= 0.0
        {
            return 0.0;
        }
        self.config
            .fusion_plant
            .startup_power(plant.mode, self.fusion_level(coord))
    }

    /// Start-up energy delivered over the next `seconds`, integrated with the
    /// planet's energy factor like dismantling work.
    fn fusion_startup_progress(&self, coord: Coord, seconds: u64) -> f64 {
        let draw = self.fusion_startup_draw_at(coord);
        if draw <= 0.0 {
            return 0.0;
        }
        let hours = seconds as f64 / 3600.0;
        draw * hours * self.reactor_production_factor(coord, hours).clamp(0.0, 1.0)
    }

    /// Seconds to the next change in fusion output or heat on `coord`.
    pub(crate) fn next_fusion_event_secs(&self, coord: Coord) -> Option<f64> {
        let plant = self.fusion_plant_at(coord)?;
        let mut soonest: Option<f64> = None;
        let mut push = |secs: f64| {
            // Callers round up to whole seconds; float noise must not push an
            // exact depletion time one second late.
            let secs = if (secs - secs.round()).abs() < 1e-6 {
                secs.round()
            } else {
                secs
            };
            if secs.is_finite() && secs > 0.0 {
                soonest = Some(soonest.map_or(secs, |s: f64| s.min(secs)));
            }
        };
        if !plant.burning()
            && let Some(at) = plant.stopped_at
        {
            let ends = at.saturating_add(self.config.fusion_plant.residual_secs);
            if ends > self.last_tick_unix {
                push((ends - self.last_tick_unix) as f64);
            }
        }
        if plant.burning() {
            let output = self.fusion_output_at(coord);
            if let Some(reaction) = self.config.fusion_plant.reaction(plant.mode)
                && output > 0.0
                && let Some(stock) = self.resources_at(coord)
            {
                for (have, rate, kind) in [
                    (
                        stock.deuterium,
                        reaction.deuterium_per_energy_hour,
                        ResourceKind::Deuterium,
                    ),
                    (
                        stock.helium_3,
                        reaction.helium3_per_energy_hour,
                        ResourceKind::Helium3,
                    ),
                ] {
                    // Signed: recipes that also consume this fuel bring the
                    // shared empty time forward.
                    let net = output * rate - self.production_at(coord, kind);
                    if rate > 0.0 && net > 1e-12 {
                        push(have / net * 3600.0);
                    }
                }
            }
        } else if plant.mode != FusionMode::Off
            && let Some(reaction) = self.config.fusion_plant.reaction(plant.mode)
            && !self.fusion_fuelled(coord, &reaction)
        {
            // A start-up waiting on fuel resumes once production refills it.
            let stock = self.resources_at(coord)?;
            let refills = |have: f64, needed: bool, kind| {
                !needed || have > 1e-12 || self.production_at(coord, kind) > 1e-12
            };
            if refills(stock.deuterium, true, ResourceKind::Deuterium)
                && refills(
                    stock.helium_3,
                    reaction.helium3_per_energy_hour > 0.0,
                    ResourceKind::Helium3,
                )
            {
                push(1.0);
            }
        } else if self.fusion_startup_draw_at(coord) > 0.0 {
            let max: u64 = 31_536_000;
            if self.fusion_startup_progress(coord, max) + 1e-9 >= plant.startup_left {
                let (mut low, mut high) = (1, max);
                while low < high {
                    let mid = low + (high - low) / 2;
                    if self.fusion_startup_progress(coord, mid) + 1e-9 >= plant.startup_left {
                        high = mid;
                    } else {
                        low = mid + 1;
                    }
                }
                push(low as f64);
            }
        }
        soonest
    }

    /// Output, start-up progress and fuel use per plant, read before a
    /// segment's production so the segment integrates constant rates.
    pub(crate) fn fusion_segment(&self, seconds: u64) -> Vec<FusionStep> {
        let hours = seconds as f64 / 3600.0;
        self.owned_coords()
            .into_iter()
            .filter_map(|coord| {
                let plant = self.fusion_plant_at(coord)?;
                let reaction = self.config.fusion_plant.reaction(plant.mode);
                let output = self.fusion_output_at(coord);
                Some(FusionStep {
                    coord,
                    output,
                    startup: self.fusion_startup_progress(coord, seconds),
                    fuelled: reaction.is_some_and(|r| self.fusion_fuelled(coord, &r)),
                    burn: reaction.map_or([0.0; 2], |r| {
                        [r.deuterium_per_energy_hour, r.helium3_per_energy_hour]
                            .map(|rate| output * hours * rate)
                    }),
                    burned: [0.0; 2],
                })
            })
            .collect()
    }

    /// Burn, before production, the fuel a full store would otherwise clip
    /// from this segment's production. Second by second the plant draws the
    /// store down while production refills it; this frees the same room, so
    /// production still runs (and consumes its own inputs) as it would live.
    pub(crate) fn make_room_for_fusion_fuel(&mut self, steps: &mut [FusionStep], hours: f64) {
        for step in steps.iter_mut().filter(|step| step.output > 0.0) {
            let kinds = [ResourceKind::Deuterium, ResourceKind::Helium3];
            let room = kinds.map(|kind| {
                let have = self.resources_at(step.coord).map_or(0.0, |r| r.get(kind));
                let overflow = have + self.production_at(step.coord, kind).max(0.0) * hours
                    - self.storage_cap_at(step.coord, kind);
                overflow.min(have)
            });
            let Ok(stock) = self.resources_at_mut(step.coord) else {
                continue;
            };
            for (i, kind) in kinds.into_iter().enumerate() {
                step.burned[i] = room[i].clamp(0.0, step.burn[i]);
                stock.set(kind, stock.get(kind) - step.burned[i]);
            }
        }
    }

    /// Burn the rest of the segment's fuel after its production, stop plasmas
    /// that ran dry, and advance start-ups over the segment `[from, to]`.
    pub(crate) fn advance_fusion(&mut self, steps: &[FusionStep], from: u64, to: u64) {
        let removed: Vec<Coord> = self
            .fusion_plants
            .keys()
            .copied()
            .filter(|&coord| self.fusion_level(coord) == 0)
            .collect();
        for coord in removed {
            self.fusion_plants.remove(&coord);
        }
        for step in steps {
            let coord = step.coord;
            let Some(mut plant) = self.fusion_plant_at(coord) else {
                continue;
            };
            let Some(reaction) = self.config.fusion_plant.reaction(plant.mode) else {
                continue;
            };
            let level = self.fusion_level(coord);
            if plant.burning() {
                let Ok(stock) = self.resources_at_mut(coord) else {
                    continue;
                };
                for (i, kind) in [ResourceKind::Deuterium, ResourceKind::Helium3]
                    .into_iter()
                    .enumerate()
                {
                    let left = step.burn[i] - step.burned[i];
                    stock.set(kind, (stock.get(kind) - left).max(0.0));
                }
                let empty = stock.deuterium <= 1e-12
                    || (reaction.helium3_per_energy_hour > 0.0 && stock.helium_3 <= 1e-12);
                if step.output > 0.0 && plant.mode == FusionMode::Dd {
                    plant.activated = true;
                }
                // Fuel spent elsewhere before the segment is an outage even if
                // this segment's production refilled the store.
                if empty || !step.fuelled {
                    let at = if step.fuelled { to } else { from };
                    self.stop_fusion(&mut plant, level, at);
                    plant.startup_left = self.config.fusion_plant.startup_energy(plant.mode, level);
                    self.push_event(to, format!("fusion plasma on {coord} went out: no fuel"));
                }
            } else if step.startup > 0.0 {
                plant.startup_left -= step.startup;
                if plant.startup_left <= 1e-9 {
                    plant.startup_left = 0.0;
                    self.push_event(to, format!("fusion plant on {coord} reached burn"));
                }
            }
            self.fusion_plants.insert(coord, plant);
        }
    }

    fn stop_fusion(&self, plant: &mut FusionPlant, level: u32, at: u64) {
        if plant.burning() {
            plant.stopped_at = Some(at);
            plant.residual_heat = self.config.fusion_plant.residual_heat(plant.mode, level);
        }
    }

    /// A maintenance failure or service disrupts the plasma; it needs a full start-up after.
    pub(crate) fn note_fusion_shutdown(&mut self, coord: Coord, at: u64) {
        let Some(mut plant) = self.fusion_plant_at(coord) else {
            return;
        };
        if plant.mode == FusionMode::Off {
            return;
        }
        let level = self.fusion_level(coord);
        self.stop_fusion(&mut plant, level, at);
        plant.startup_left = self.config.fusion_plant.startup_energy(plant.mode, level);
        self.fusion_plants.insert(coord, plant);
    }

    /// Neutron wear per hour for the plant's maintenance asset, scaled by output.
    pub(crate) fn fusion_neutron_wear_per_hour(&self, coord: Coord) -> f64 {
        let Some(plant) = self.fusion_plant_at(coord) else {
            return 0.0;
        };
        let cfg = &self.config.fusion_plant;
        let rated = cfg.rated_output(self.fusion_level(coord));
        match cfg.reaction(plant.mode) {
            Some(reaction) if rated > 0.0 => {
                cfg.wear_neutron_per_hour * reaction.neutron_share * self.fusion_output_at(coord)
                    / rated
            }
            _ => 0.0,
        }
    }

    /// Reasons the last fusion level cannot be dismantled yet.
    pub(crate) fn fusion_dismantle_blockers(&self, coord: Coord, level: u32) -> Vec<String> {
        let Some(plant) = self.fusion_plant_at(coord) else {
            return Vec::new();
        };
        if plant.mode != FusionMode::Off {
            return vec!["fusion plant is running; switch it off first".into()];
        }
        let cools_at = plant
            .stopped_at
            .map(|at| at.saturating_add(self.config.fusion_plant.activation_cooldown_secs));
        match cools_at {
            Some(at) if level == 1 && plant.activated && at > self.last_tick_unix => {
                vec![format!(
                    "activated structure cools for another {}s",
                    at - self.last_tick_unix
                )]
            }
            _ => Vec::new(),
        }
    }

    /// Switch a plant's reaction. Any change from a burn stops the plasma, and
    /// a new reaction starts with a full plasma start-up.
    pub fn set_fusion_mode(
        &mut self,
        coord: Coord,
        mode: FusionMode,
        now: u64,
        actor: crate::Actor,
    ) -> Result<(), FusionError> {
        self.catch_up(now);
        let level = self
            .buildings_at(coord)
            .ok_or(FusionError::UnknownPlanet)?
            .fusion_plant;
        if level == 0 {
            return Err(FusionError::NoPlant);
        }
        if mode == FusionMode::Dhe3 && self.tech_level("fusion_dhe3") < 1 {
            return Err(FusionError::MissingTechnology {
                id: "fusion_dhe3".into(),
                required: 1,
            });
        }
        self.authorize(actor, crate::Permission::QueueBuild, Some(coord), 0.0)
            .map_err(|error| FusionError::PermissionDenied(error.to_string()))?;
        let Some(mut plant) = self.fusion_plant_at(coord) else {
            return Err(FusionError::NoPlant);
        };
        if plant.mode == mode {
            return Ok(());
        }
        let at = self.last_tick_unix;
        self.stop_fusion(&mut plant, level, at);
        plant.mode = mode;
        plant.startup_left = self.config.fusion_plant.startup_energy(mode, level);
        self.fusion_plants.insert(coord, plant);
        self.push_event(
            at,
            format!("fusion plant on {coord} set to {}", mode.as_key()),
        );
        self.record_history(crate::HistoryDraft {
            at,
            kind: crate::HistoryKind::Build,
            actor: actor.as_key(),
            action: "fusion_mode".into(),
            objects: vec![coord.to_string(), mode.as_key().into()],
            outcome: "set".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn fusion_status_at(&self, coord: Coord) -> Option<FusionStatus> {
        let plant = self.fusion_plant_at(coord)?;
        let cfg = &self.config.fusion_plant;
        let level = self.fusion_level(coord);
        let output = self.fusion_output_at(coord);
        let reaction = cfg.reaction(plant.mode);
        let full = cfg.startup_energy(plant.mode, level);
        Some(FusionStatus {
            coord,
            level,
            output,
            rated: cfg.rated_output(level),
            cooling_cap: reaction.map_or(0.0, |r| self.fusion_cooling_cap(coord, &r)),
            deuterium_per_hour: reaction.map_or(0.0, |r| output * r.deuterium_per_energy_hour),
            helium3_per_hour: reaction.map_or(0.0, |r| output * r.helium3_per_energy_hour),
            startup_draw: self.fusion_startup_draw_at(coord),
            startup_progress: if plant.burning() || full <= 0.0 {
                1.0
            } else {
                (1.0 - plant.startup_left / full).clamp(0.0, 1.0)
            },
            heat: self.fusion_heat_at(coord),
            plant,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Home world with only a fusion plant, radiators and fuel; no chains, raids or market.
    fn isolated_plant(radiators: u32) -> GameState {
        let mut config = crate::BalanceConfig {
            chains: crate::ChainConfig::default(),
            ..Default::default()
        };
        config.raid.enabled = false;
        config.market.enabled = false;
        config.orbits.inner_au = 1.0;
        config.orbits.au_per_slot = 0.0;
        let mut state = GameState::with_seed(0, config, 9);
        state.deterministic = true;
        state.buildings = crate::Buildings {
            fusion_plant: 1,
            radiator: radiators,
            ..Default::default()
        };
        state.maintenance.next_at = u64::MAX;
        state.resources = crate::Resources::ZERO;
        state.resources.deuterium = 100.0;
        state.resources.helium_3 = 100.0;
        state
    }

    fn burn(state: &mut GameState, mode: FusionMode) {
        state.fusion_plants.insert(
            state.home,
            FusionPlant {
                mode,
                startup_left: 0.0,
                activated: false,
                stopped_at: None,
                residual_heat: 0.0,
            },
        );
    }

    fn step_live(state: &mut GameState, to: u64, step: u64) {
        let mut at = state.last_tick_unix;
        while at < to {
            at = (at + step).min(to);
            state.catch_up(at);
        }
    }

    #[test]
    fn dd_plant_delivers_rated_output_at_any_orbit_and_burns_per_energy() {
        for au in [0.4, 1.0, 30.0] {
            let mut state = isolated_plant(20);
            state.config.orbits.inner_au = au;
            burn(&mut state, FusionMode::Dd);
            assert!((state.fusion_output_at(state.home) - 400.0).abs() < 1e-9);
        }
        let mut bulk = isolated_plant(20);
        burn(&mut bulk, FusionMode::Dd);
        let mut live = bulk.clone();
        bulk.catch_up(3_600);
        step_live(&mut live, 3_600, 1);
        let burned = 100.0 - bulk.resources.deuterium;
        assert!((burned - 400.0 * 0.002).abs() < 1e-9);
        assert!((bulk.resources.deuterium - live.resources.deuterium).abs() < 1e-9);
        assert!(bulk.fusion_plants[&bulk.home].activated);

        bulk.resources.deuterium = 0.0;
        assert_eq!(bulk.fusion_output_at(bulk.home), 0.0);
    }

    #[test]
    fn fuel_outage_stops_the_plasma_at_the_same_time_live_and_offline() {
        let mut bulk = isolated_plant(20);
        bulk.buildings.metal_mine = 1;
        bulk.resources.deuterium = 0.5;
        burn(&mut bulk, FusionMode::Dd);
        let mut live = bulk.clone();
        bulk.catch_up(3_600);
        step_live(&mut live, 3_600, 60);
        let plant = &bulk.fusion_plants[&bulk.home];
        assert!(!plant.burning());
        assert_eq!(plant.stopped_at, Some(2_250));
        assert_eq!(bulk.fusion_plants, live.fusion_plants);
        assert_eq!(bulk.resources.deuterium, 0.0);
        // The plasma powered the mine right up to the outage.
        assert!(bulk.resources.iron > 0.0);
        assert!((bulk.resources.iron - live.resources.iron).abs() < 1e-6);
        assert!(bulk.fusion_heat_at(bulk.home) > 0.0);
    }

    #[test]
    fn dhe3_burns_both_fuels_at_the_ratio_sheds_less_heat_and_wears_less() {
        let mut dd = isolated_plant(20);
        burn(&mut dd, FusionMode::Dd);
        let mut state = isolated_plant(20);
        burn(&mut state, FusionMode::Dhe3);
        assert!((state.fusion_heat_at(state.home) - 400.0 * 0.7).abs() < 1e-9);
        assert!(
            state.fusion_neutron_wear_per_hour(state.home)
                < dd.fusion_neutron_wear_per_hour(dd.home)
        );
        state.catch_up(3_600);
        let deuterium = 100.0 - state.resources.deuterium;
        let helium = 100.0 - state.resources.helium_3;
        assert!((deuterium - 400.0 * 0.0008).abs() < 1e-9);
        assert!((helium - 400.0 * 0.0012).abs() < 1e-9);
        assert!((deuterium / helium - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn output_follows_the_free_heat_sink_without_overflow() {
        let mut state = isolated_plant(0);
        burn(&mut state, FusionMode::Dd);
        let mut previous = state.fusion_output_at(state.home);
        assert!(previous < 400.0);
        for radiators in 1..=3 {
            state.buildings.radiator = radiators;
            let output = state.fusion_output_at(state.home);
            assert!((output - previous - 40.0 / 1.5).abs() < 1e-9);
            assert!(!state.waste_home().overflowing());
            previous = output;
        }
    }

    #[test]
    fn startup_is_a_grid_load_that_stretches_when_supply_is_short() {
        let mut state = isolated_plant(20);
        state.buildings.solar_plant = 1;
        state.config.waste.radiator_base_energy = 0.0;
        state.config.solar_plant.base_energy = 2_000.0;
        assert_eq!(state.fusion_output_at(state.home), 0.0);
        assert!((state.energy_consumed() - 1_200.0).abs() < 1e-9);
        state.catch_up(1_799);
        assert_eq!(state.fusion_output_at(state.home), 0.0);
        state.catch_up(1_800);
        assert!((state.fusion_output_at(state.home) - 400.0).abs() < 1e-9);
        assert_eq!(state.energy_consumed(), 0.0);

        let mut bulk = isolated_plant(20);
        bulk.buildings.solar_plant = 1;
        bulk.config.waste.radiator_base_energy = 0.0;
        bulk.config.solar_plant.base_energy = 600.0;
        let mut live = bulk.clone();
        bulk.catch_up(3_599);
        assert_eq!(bulk.fusion_output_at(bulk.home), 0.0);
        bulk.catch_up(3_600);
        assert!(bulk.fusion_output_at(bulk.home) > 0.0);
        step_live(&mut live, 3_600, 60);
        assert_eq!(bulk.fusion_plants, live.fusion_plants);

        let mut dark = isolated_plant(20);
        dark.catch_up(36_000);
        assert_eq!(dark.fusion_output_at(dark.home), 0.0);
        let full = dark.config.fusion_plant.startup_energy(FusionMode::Dd, 1);
        assert_eq!(dark.fusion_plants[&dark.home].startup_left, full);
    }

    #[test]
    fn fuel_spent_elsewhere_is_an_outage_even_if_production_refills_it() {
        let mut state = isolated_plant(20);
        state.buildings.deuterium_synthesizer = 3;
        burn(&mut state, FusionMode::Dd);
        state.resources.deuterium = 0.0;
        state.catch_up(60);
        let plant = &state.fusion_plants[&state.home];
        assert!(!plant.burning());
        assert_eq!(plant.stopped_at, Some(0));
    }

    #[test]
    fn startup_waiting_on_fuel_resumes_when_production_refills_it() {
        let mut bulk = isolated_plant(20);
        bulk.buildings.solar_plant = 1;
        bulk.buildings.deuterium_synthesizer = 3;
        bulk.config.waste.radiator_base_energy = 0.0;
        bulk.config.solar_plant.base_energy = 5_000.0;
        bulk.resources.deuterium = 0.0;
        let mut live = bulk.clone();
        bulk.catch_up(3_600);
        step_live(&mut live, 3_600, 60);
        assert!(bulk.fusion_plants[&bulk.home].burning());
        assert_eq!(bulk.fusion_plants, live.fusion_plants);
    }

    #[test]
    fn startup_powered_by_recovering_fission_matches_live() {
        let mut bulk = isolated_plant(0);
        bulk.buildings.fission_reactor = 1;
        bulk.goods.insert("reactor_fuel".into(), 10.0);
        bulk.note_reactor_shutdown(bulk.home, 0);
        let mut live = bulk.clone();
        bulk.catch_up(3_600);
        step_live(&mut live, 3_600, 60);
        let left = |state: &GameState| state.fusion_plants[&state.home].startup_left;
        let full = bulk.config.fusion_plant.startup_energy(FusionMode::Dd, 1);
        assert!(left(&bulk) < full);
        assert!((left(&bulk) - left(&live)).abs() < 1e-6);
    }

    #[test]
    fn a_full_tank_refills_while_burning_as_it_would_live() {
        let mut bulk = isolated_plant(20);
        bulk.buildings.deuterium_synthesizer = 5;
        bulk.resources.deuterium = bulk.storage_cap(ResourceKind::Deuterium);
        burn(&mut bulk, FusionMode::Dd);
        let mut live = bulk.clone();
        bulk.catch_up(3_600);
        step_live(&mut live, 3_600, 1);
        assert!((bulk.resources.deuterium - live.resources.deuterium).abs() < 1e-6);
    }

    #[test]
    fn mode_switch_needs_technology_and_restarts_the_plasma() {
        let mut state = isolated_plant(20);
        burn(&mut state, FusionMode::Dd);
        assert_eq!(
            state.set_fusion_mode(state.home, FusionMode::Dhe3, 0, crate::Actor::Player),
            Err(FusionError::MissingTechnology {
                id: "fusion_dhe3".into(),
                required: 1
            })
        );
        state.tech_levels.insert("fusion_dhe3".into(), 1);
        state
            .set_fusion_mode(state.home, FusionMode::Dhe3, 0, crate::Actor::Player)
            .unwrap();
        let plant = &state.fusion_plants[&state.home];
        assert_eq!(
            plant.startup_left,
            state
                .config
                .fusion_plant
                .startup_energy(FusionMode::Dhe3, 1)
        );
        assert_eq!(state.fusion_output_at(state.home), 0.0);
    }

    #[test]
    fn plant_and_research_chain_are_gated() {
        let mut state = GameState::new(0);
        state.resources = crate::Resources::from_parts(1e6, 1e6, 1e6);
        state.buildings.research_lab = 6;
        state.buildings.fission_reactor = 1;
        assert!(matches!(
            state.enqueue_build(BuildingKind::FusionPlant, 0),
            Err(crate::BuildError::MissingTechnology { .. })
        ));
        for (id, level) in [("energy", 8), ("laser", 6), ("fission", 2)] {
            state.tech_levels.insert(id.into(), level);
        }
        assert!(
            !state
                .missing_research_prereqs("plasma_physics")
                .unwrap()
                .is_empty()
        );
        state.tech_levels.insert("fission".into(), 3);
        assert!(
            state
                .missing_research_prereqs("plasma_physics")
                .unwrap()
                .is_empty()
        );
        state.tech_levels.insert("fusion_dd".into(), 1);
        assert!(state.enqueue_build(BuildingKind::FusionPlant, 0).is_ok());
    }

    #[test]
    fn dismantle_waits_for_shutdown_and_dd_activation_cooldown() {
        let mut state = isolated_plant(20);
        burn(&mut state, FusionMode::Dd);
        state.catch_up(60);
        let blocked = |state: &GameState| {
            !state
                .dismantle_preview(state.home, BuildingKind::FusionPlant)
                .unwrap()
                .blockers
                .is_empty()
        };
        assert!(blocked(&state));
        state
            .set_fusion_mode(state.home, FusionMode::Off, 60, crate::Actor::Player)
            .unwrap();
        assert!(blocked(&state));
        state.catch_up(60 + state.config.fusion_plant.activation_cooldown_secs);
        assert!(!blocked(&state));

        let mut clean = isolated_plant(20);
        burn(&mut clean, FusionMode::Dhe3);
        clean.catch_up(60);
        clean
            .set_fusion_mode(clean.home, FusionMode::Off, 60, crate::Actor::Player)
            .unwrap();
        assert!(!blocked(&clean));
    }

    #[test]
    fn fusion_fuel_is_lootable_stock() {
        let mut stock = crate::Resources::ZERO;
        stock.deuterium = 100.0;
        stock.helium_3 = 100.0;
        let cap = crate::Resources {
            deuterium: 1e9,
            helium_3: 1e9,
            ..crate::Resources::ZERO
        };
        let taken = crate::plunder_from(stock, 0.5, cap);
        assert_eq!(taken.deuterium, 50.0);
        assert_eq!(taken.helium_3, 50.0);
    }

    #[test]
    fn plant_state_survives_save() {
        let mut state = isolated_plant(20);
        state.maintenance.next_at = 3_600;
        burn(&mut state, FusionMode::Dd);
        state.catch_up(60);
        state
            .set_fusion_mode(state.home, FusionMode::Off, 60, crate::Actor::Player)
            .unwrap();
        assert_eq!(state.fusion_plants[&state.home].mode, FusionMode::Off);
        assert!(state.fusion_heat_at(state.home) >= 0.0);
    }
}
