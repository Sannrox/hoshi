//! Planet layout blueprints: capture, preview, apply, and text import/export.

use crate::buildings::BuildingKind;
use crate::fleets::Coord;
use crate::planets::PlanetType;
use crate::state::GameState;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Capture of target building levels. Production-chain setup is the building mix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blueprint {
    pub name: String,
    /// World class the layout was captured on. Missing on imported text that omits it.
    pub planet_type: Option<PlanetType>,
    pub buildings: Vec<(BuildingKind, u32)>,
}

/// Why a step does not fit the target world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintMismatch {
    pub building: BuildingKind,
    pub reason: String,
}

/// One upgrade that would be queued on apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintStep {
    pub building: BuildingKind,
    pub from_level: u32,
}

/// Dry-run of applying a blueprint to one owned planet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintPreview {
    pub planet: Coord,
    pub steps: Vec<BlueprintStep>,
    pub mismatches: Vec<BlueprintMismatch>,
    /// Set when the captured world class differs from the target.
    pub type_mismatch: Option<String>,
}

/// Result after queueing as many matching steps as resources and the cap allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintApply {
    pub queued: Vec<BlueprintStep>,
    pub mismatches: Vec<BlueprintMismatch>,
    pub skipped: Vec<String>,
    pub type_mismatch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlueprintError {
    UnknownPlanet,
    EmptyName,
    InvalidName(String),
    UnknownBuilding(String),
    Parse(String),
    Io(String),
    Missing(String),
}

impl fmt::Display for BlueprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlueprintError::UnknownPlanet => write!(f, "unknown planet"),
            BlueprintError::EmptyName => write!(f, "blueprint name is empty"),
            BlueprintError::InvalidName(name) => write!(f, "invalid blueprint name {name}"),
            BlueprintError::UnknownBuilding(id) => write!(f, "unknown building {id}"),
            BlueprintError::Parse(err) => write!(f, "{err}"),
            BlueprintError::Io(err) => write!(f, "{err}"),
            BlueprintError::Missing(name) => write!(f, "no blueprint named {name}"),
        }
    }
}

/// Power first, then extraction, buffers, industry. Shipyard stays after robotics.
pub const APPLY_ORDER: [BuildingKind; 13] = [
    BuildingKind::SolarPlant,
    BuildingKind::Radiator,
    BuildingKind::Scrubber,
    BuildingKind::MetalMine,
    BuildingKind::CrystalMine,
    BuildingKind::DeuteriumSynthesizer,
    BuildingKind::MetalStorage,
    BuildingKind::CrystalStorage,
    BuildingKind::DeuteriumStorage,
    BuildingKind::RoboticsFactory,
    BuildingKind::ResearchLab,
    BuildingKind::FissionReactor,
    BuildingKind::Shipyard,
];

impl GameState {
    pub fn capture_blueprint(
        &self,
        planet: Coord,
        name: &str,
    ) -> Result<Blueprint, BlueprintError> {
        let name = sanitize_name(name)?;
        let buildings = self
            .buildings_at(planet)
            .ok_or(BlueprintError::UnknownPlanet)?;
        let mut levels = Vec::new();
        for kind in BuildingKind::ALL {
            let level = buildings.get(kind);
            if level > 0 {
                levels.push((kind, level));
            }
        }
        let planet_type = self
            .planet_kind(planet)
            .ok_or(BlueprintError::UnknownPlanet)?;
        Ok(Blueprint {
            name,
            planet_type: Some(planet_type),
            buildings: levels,
        })
    }

    pub fn preview_blueprint(
        &self,
        planet: Coord,
        blueprint: &Blueprint,
    ) -> Result<BlueprintPreview, BlueprintError> {
        let buildings = *self
            .buildings_at(planet)
            .ok_or(BlueprintError::UnknownPlanet)?;
        let queue = self.queue_at(planet).unwrap_or(&[]);
        let kind = self
            .planet_kind(planet)
            .ok_or(BlueprintError::UnknownPlanet)?;
        let type_mismatch = blueprint.planet_type.and_then(|source| {
            (source != kind).then(|| {
                format!(
                    "captured for {} world, target is {}",
                    source.as_key(),
                    kind.as_key()
                )
            })
        });
        let mut steps = Vec::new();
        let mut mismatches = Vec::new();
        for &building in &APPLY_ORDER {
            let Some(want) = blueprint.level(building) else {
                continue;
            };
            if let Some(reason) = mismatch_reason(self, planet, kind, building) {
                mismatches.push(BlueprintMismatch { building, reason });
                continue;
            }
            let have = crate::state::projected_on(&buildings, queue, building, queue.len());
            let mut from = have;
            while from < want {
                steps.push(BlueprintStep {
                    building,
                    from_level: from,
                });
                from += 1;
            }
        }
        Ok(BlueprintPreview {
            planet,
            steps,
            mismatches,
            type_mismatch,
        })
    }

    pub fn apply_blueprint(
        &mut self,
        planet: Coord,
        blueprint: &Blueprint,
        now_unix: u64,
    ) -> Result<BlueprintApply, BlueprintError> {
        let preview = self.preview_blueprint(planet, blueprint)?;
        let mut queued = Vec::new();
        let mut skipped = Vec::new();
        for step in &preview.steps {
            match self.enqueue_build_on(planet, step.building, now_unix) {
                Ok(()) => queued.push(step.clone()),
                Err(err) => {
                    skipped.push(format!("{}: {err}", step.building));
                    break;
                }
            }
        }
        Ok(BlueprintApply {
            queued,
            mismatches: preview.mismatches,
            skipped,
            type_mismatch: preview.type_mismatch,
        })
    }
}

fn mismatch_reason(
    _state: &GameState,
    planet: Coord,
    kind: PlanetType,
    building: BuildingKind,
) -> Option<String> {
    match building {
        BuildingKind::SolarPlant if planet.position >= 9 => Some(format!(
            "solar plant far from the star at slot {}",
            planet.position
        )),
        BuildingKind::SolarPlant if kind == PlanetType::Ice => {
            Some(format!("poor solar on {}", kind.as_key()))
        }
        _ => None,
    }
}

impl Blueprint {
    pub fn to_toml(&self) -> String {
        let mut out = format!("name = \"{}\"\n", self.name.replace('"', ""));
        if let Some(kind) = self.planet_type {
            out.push_str(&format!("planet_type = \"{}\"\n", kind.as_key()));
        }
        for kind in APPLY_ORDER {
            if let Some(level) = self.level(kind) {
                out.push_str(&format!("{} = {level}\n", kind.as_key()));
            }
        }
        out
    }

    pub fn from_toml(text: &str) -> Result<Self, BlueprintError> {
        let mut name = String::new();
        let mut planet_type = None;
        let mut buildings = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(BlueprintError::Parse(format!("invalid line {line}")));
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if key == "name" {
                name = sanitize_name(value)?;
                continue;
            }
            if key == "planet_type" {
                planet_type = Some(PlanetType::from_key(value).ok_or_else(|| {
                    BlueprintError::Parse(format!("unknown planet type {value}"))
                })?);
                continue;
            }
            let Some(kind) = BuildingKind::from_key(key) else {
                return Err(BlueprintError::UnknownBuilding(key.into()));
            };
            let level: u32 = value
                .parse()
                .map_err(|_| BlueprintError::Parse(format!("invalid level for {key}")))?;
            if level > 0 {
                buildings.push((kind, level));
            }
        }
        if name.is_empty() {
            return Err(BlueprintError::EmptyName);
        }
        Ok(Self {
            name,
            planet_type,
            buildings,
        })
    }

    pub fn level(&self, kind: BuildingKind) -> Option<u32> {
        self.buildings
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, level)| *level)
    }
}

/// On-disk catalog beside a save. One `{name}.toml` per blueprint.
pub struct BlueprintCatalog {
    dir: PathBuf,
}

impl BlueprintCatalog {
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn list(&self) -> Result<Vec<String>, BlueprintError> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.dir).map_err(|e| BlueprintError::Io(e.to_string()))? {
            let entry = entry.map_err(|e| BlueprintError::Io(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn load(&self, name: &str) -> Result<Blueprint, BlueprintError> {
        let name = sanitize_name(name)?;
        let path = self.path_for(&name);
        let text = fs::read_to_string(&path).map_err(|_| BlueprintError::Missing(name.clone()))?;
        Blueprint::from_toml(&text)
    }

    pub fn save(&self, blueprint: &Blueprint) -> Result<PathBuf, BlueprintError> {
        fs::create_dir_all(&self.dir).map_err(|e| BlueprintError::Io(e.to_string()))?;
        let path = self.path_for(&blueprint.name);
        fs::write(&path, blueprint.to_toml()).map_err(|e| BlueprintError::Io(e.to_string()))?;
        Ok(path)
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.toml"))
    }
}

pub fn blueprint_dir(save: &Path) -> PathBuf {
    save.parent().unwrap_or(save).join("blueprints")
}

fn sanitize_name(name: &str) -> Result<String, BlueprintError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(BlueprintError::EmptyName);
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(BlueprintError::InvalidName(name.into()));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::buildings::Buildings;
    use crate::defences::DefenceGrid;
    use crate::planets::Colony;
    use crate::ships::Fleet;
    use crate::state::Resources;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn quiet() -> BalanceConfig {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg
    }

    fn unique_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = env::temp_dir().join(format!("hoshi-blueprint-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn capture_round_trips_as_text() {
        let mut state = GameState::with_config(0, quiet());
        state.buildings.metal_mine = 4;
        state.buildings.solar_plant = 3;
        let bp = state.capture_blueprint(state.home, "home-core").unwrap();
        let text = bp.to_toml();
        let parsed = Blueprint::from_toml(&text).unwrap();
        assert_eq!(parsed.name, "home-core");
        assert_eq!(parsed.planet_type, Some(PlanetType::Rocky));
        assert_eq!(parsed.level(BuildingKind::MetalMine), Some(4));
        assert_eq!(parsed.level(BuildingKind::SolarPlant), Some(3));
        assert!(text.contains("planet_type = \"rocky\""), "{text}");
    }

    #[test]
    fn apply_queues_only_missing_levels_in_power_then_extract_order() {
        let mut state = GameState::with_config(0, quiet());
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 50_000.0;
        state.buildings.solar_plant = 1;
        let bp = Blueprint {
            name: "starter".into(),
            planet_type: Some(PlanetType::Rocky),
            buildings: vec![(BuildingKind::SolarPlant, 2), (BuildingKind::MetalMine, 2)],
        };
        let preview = state.preview_blueprint(state.home, &bp).unwrap();
        assert_eq!(preview.steps[0].building, BuildingKind::SolarPlant);
        assert_eq!(preview.steps[0].from_level, 1);
        assert_eq!(preview.steps[1].building, BuildingKind::MetalMine);
        let applied = state.apply_blueprint(state.home, &bp, 0).unwrap();
        assert_eq!(applied.queued.len(), preview.steps.len());
        assert!(applied.mismatches.is_empty());
        let queued: Vec<_> = state.build_queue.iter().map(|j| j.building).collect();
        assert_eq!(queued.first(), Some(&BuildingKind::SolarPlant));
        assert!(queued.contains(&BuildingKind::MetalMine), "{queued:?}");
    }

    #[test]
    fn blueprint_restores_target_after_pending_dismantle() {
        let mut state = GameState::with_config(0, quiet());
        state.buildings.metal_mine = 3;
        state.resources = crate::Resources::from_parts(50_000.0, 50_000.0, 50_000.0);
        state
            .enqueue_dismantle_as(state.home, BuildingKind::MetalMine, 0, crate::Actor::Player)
            .unwrap();
        let blueprint = Blueprint {
            name: "restore".into(),
            planet_type: None,
            buildings: vec![(BuildingKind::MetalMine, 3)],
        };
        let preview = state.preview_blueprint(state.home, &blueprint).unwrap();
        assert_eq!(preview.steps.len(), 1);
        assert_eq!(preview.steps[0].from_level, 2);
        state.apply_blueprint(state.home, &blueprint, 0).unwrap();
        assert_eq!(state.build_queue.len(), 2);
    }

    #[test]
    fn mismatches_are_listed_and_those_buildings_are_not_queued() {
        let mut cfg = quiet();
        cfg.starting_resources.iron = 50_000.0;
        cfg.starting_resources.silicon = 50_000.0;
        let mut state = GameState::with_config(0, cfg);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 12,
        };
        state.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Ice,
            size: 80,
            resources: Resources::from_parts(50_000.0, 50_000.0, 0.0),
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        let bp = Blueprint {
            name: "mines".into(),
            planet_type: Some(PlanetType::Rocky),
            buildings: vec![
                (BuildingKind::MetalMine, 1),
                (BuildingKind::SolarPlant, 1),
                (BuildingKind::DeuteriumSynthesizer, 1),
            ],
        };
        let preview = state.preview_blueprint(dest, &bp).unwrap();
        assert_eq!(
            preview.type_mismatch.as_deref(),
            Some("captured for rocky world, target is ice")
        );
        assert!(
            preview
                .steps
                .iter()
                .any(|s| s.building == BuildingKind::MetalMine),
            "{:?}",
            preview.mismatches
        );
        assert!(
            preview
                .mismatches
                .iter()
                .any(|row| row.building == BuildingKind::SolarPlant),
            "{:?}",
            preview.mismatches
        );
        assert!(
            preview
                .steps
                .iter()
                .any(|s| s.building == BuildingKind::DeuteriumSynthesizer)
        );
        let applied = state.apply_blueprint(dest, &bp, 0).unwrap();
        assert!(!applied.mismatches.is_empty());
        assert!(
            applied
                .queued
                .iter()
                .all(|s| s.building != BuildingKind::SolarPlant)
        );
    }

    #[test]
    fn catalog_saves_and_loads_beside_the_player_dir() {
        let dir = unique_dir();
        let catalog = BlueprintCatalog::open(&dir);
        let bp = Blueprint {
            name: "alpha".into(),
            planet_type: Some(PlanetType::Rocky),
            buildings: vec![(BuildingKind::RoboticsFactory, 1)],
        };
        catalog.save(&bp).unwrap();
        assert_eq!(catalog.list().unwrap(), vec!["alpha".to_string()]);
        let loaded = catalog.load("alpha").unwrap();
        assert_eq!(loaded, bp);
        let _ = fs::remove_dir_all(&dir);
    }
}
