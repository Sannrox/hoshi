//! Ship types and the per-planet shipyard queue.

use crate::defences::DefenceKind;
use crate::research::TechPrereq;
use crate::state::Resources;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_SHIPS: &str = include_str!("../data/ships.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShipKind {
    SmallCargo,
    LargeCargo,
    LightFighter,
    HeavyFighter,
    Cruiser,
    Battleship,
    Recycler,
    EspionageProbe,
    Explorer,
    ColonyShip,
    TroopTransport,
}

pub const SHIP_KINDS: usize = 11;

impl ShipKind {
    pub const ALL: [ShipKind; SHIP_KINDS] = [
        ShipKind::SmallCargo,
        ShipKind::LargeCargo,
        ShipKind::LightFighter,
        ShipKind::HeavyFighter,
        ShipKind::Cruiser,
        ShipKind::Battleship,
        ShipKind::Recycler,
        ShipKind::EspionageProbe,
        ShipKind::Explorer,
        ShipKind::ColonyShip,
        ShipKind::TroopTransport,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            ShipKind::SmallCargo => "small_cargo",
            ShipKind::LargeCargo => "large_cargo",
            ShipKind::LightFighter => "light_fighter",
            ShipKind::HeavyFighter => "heavy_fighter",
            ShipKind::Cruiser => "cruiser",
            ShipKind::Battleship => "battleship",
            ShipKind::Recycler => "recycler",
            ShipKind::EspionageProbe => "espionage_probe",
            ShipKind::Explorer => "explorer",
            ShipKind::ColonyShip => "colony_ship",
            ShipKind::TroopTransport => "troop_transport",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "small_cargo" => ShipKind::SmallCargo,
            "large_cargo" => ShipKind::LargeCargo,
            "light_fighter" => ShipKind::LightFighter,
            "heavy_fighter" => ShipKind::HeavyFighter,
            "cruiser" => ShipKind::Cruiser,
            "battleship" => ShipKind::Battleship,
            "recycler" => ShipKind::Recycler,
            "espionage_probe" => ShipKind::EspionageProbe,
            "explorer" => ShipKind::Explorer,
            "colony_ship" => ShipKind::ColonyShip,
            "troop_transport" => ShipKind::TroopTransport,
            _ => return None,
        })
    }

    pub fn index(self) -> usize {
        match self {
            ShipKind::SmallCargo => 0,
            ShipKind::LargeCargo => 1,
            ShipKind::LightFighter => 2,
            ShipKind::HeavyFighter => 3,
            ShipKind::Cruiser => 4,
            ShipKind::Battleship => 5,
            ShipKind::Recycler => 6,
            ShipKind::EspionageProbe => 7,
            ShipKind::Explorer => 8,
            ShipKind::ColonyShip => 9,
            ShipKind::TroopTransport => 10,
        }
    }
}

impl fmt::Display for ShipKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ShipKind::SmallCargo => "small cargo",
            ShipKind::LargeCargo => "large cargo",
            ShipKind::LightFighter => "light fighter",
            ShipKind::HeavyFighter => "heavy fighter",
            ShipKind::Cruiser => "cruiser",
            ShipKind::Battleship => "battleship",
            ShipKind::Recycler => "recycler",
            ShipKind::EspionageProbe => "espionage probe",
            ShipKind::Explorer => "explorer",
            ShipKind::ColonyShip => "colony ship",
            ShipKind::TroopTransport => "troop transport",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShipSpec {
    pub kind: ShipKind,
    pub name: String,
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
    pub base_seconds: f64,
    pub attack: f64,
    pub shield: f64,
    pub hull: f64,
    pub cargo: f64,
    pub speed: f64,
    pub fuel: f64,
    pub shipyard: u32,
    pub drive: String,
    pub prereq_techs: Vec<TechPrereq>,
    /// Extra shots against a target kind. `value` 5 means four-in-five chance to fire again.
    pub rapid_fire: Vec<(ShipKind, u32)>,
}

impl ShipSpec {
    pub fn unit_cost(&self) -> Resources {
        Resources {
            metal: self.metal,
            crystal: self.crystal,
            deuterium: self.deuterium,
        }
    }

    pub fn duration_secs(&self) -> u64 {
        self.base_seconds.ceil().max(1.0) as u64
    }
}

/// Combat and flight numbers after research modifiers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShipStats {
    pub attack: f64,
    pub shield: f64,
    pub hull: f64,
    pub cargo: f64,
    pub speed: f64,
    pub fuel: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShipCatalog {
    specs: BTreeMap<ShipKind, ShipSpec>,
}

impl ShipCatalog {
    pub fn parse(toml: &str) -> Result<Self, ShipCatalogError> {
        let file: File =
            toml::from_str(toml).map_err(|e| ShipCatalogError::Parse(e.to_string()))?;
        let mut specs = BTreeMap::new();
        for raw in file.ship {
            let kind = ShipKind::from_key(&raw.id)
                .ok_or_else(|| ShipCatalogError::UnknownId(raw.id.clone()))?;
            if specs.contains_key(&kind) {
                return Err(ShipCatalogError::DuplicateId(raw.id));
            }
            if raw.drive.is_empty() {
                return Err(ShipCatalogError::MissingDrive(raw.id));
            }
            let mut rapid_fire = Vec::new();
            for entry in raw.rapid_fire {
                let target = ShipKind::from_key(&entry.target).ok_or_else(|| {
                    ShipCatalogError::UnknownRapidFireTarget {
                        ship: raw.id.clone(),
                        target: entry.target.clone(),
                    }
                })?;
                rapid_fire.push((target, entry.value));
            }
            specs.insert(
                kind,
                ShipSpec {
                    kind,
                    name: raw.name,
                    metal: raw.metal,
                    crystal: raw.crystal,
                    deuterium: raw.deuterium,
                    base_seconds: raw.base_seconds,
                    attack: raw.attack,
                    shield: raw.shield,
                    hull: raw.hull,
                    cargo: raw.cargo,
                    speed: raw.speed,
                    fuel: raw.fuel,
                    shipyard: raw.shipyard,
                    drive: raw.drive,
                    prereq_techs: raw
                        .prereq_techs
                        .into_iter()
                        .map(|p| TechPrereq {
                            id: p.id,
                            level: p.level,
                        })
                        .collect(),
                    rapid_fire,
                },
            );
        }
        for kind in ShipKind::ALL {
            if !specs.contains_key(&kind) {
                return Err(ShipCatalogError::MissingId(kind.as_key()));
            }
        }
        Ok(Self { specs })
    }

    pub fn get(&self, kind: ShipKind) -> &ShipSpec {
        self.specs
            .get(&kind)
            .expect("catalog always contains every ShipKind")
    }

    pub fn iter(&self) -> impl Iterator<Item = &ShipSpec> {
        ShipKind::ALL.iter().map(|&kind| self.get(kind))
    }
}

impl Default for ShipCatalog {
    fn default() -> Self {
        Self::parse(DEFAULT_SHIPS).expect("bundled ships.toml must be valid")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fleet {
    counts: [u32; SHIP_KINDS],
}

impl Fleet {
    pub fn get(self, kind: ShipKind) -> u32 {
        self.counts[kind.index()]
    }

    pub fn add(&mut self, kind: ShipKind, n: u32) {
        let i = kind.index();
        self.counts[i] = self.counts[i].saturating_add(n);
    }

    pub fn saturating_sub(&mut self, kind: ShipKind, n: u32) {
        let i = kind.index();
        self.counts[i] = self.counts[i].saturating_sub(n);
    }

    pub fn is_empty(self) -> bool {
        self.total() == 0
    }

    pub fn total(self) -> u32 {
        self.counts.iter().copied().fold(0, u32::saturating_add)
    }
}

/// One shipyard batch. Only the head has a timer; ships in that batch complete one by one.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipJob {
    pub ship: Option<ShipKind>,
    pub defence: Option<DefenceKind>,
    pub remaining: u32,
    pub unit_paid: Resources,
    pub started_at: Option<u64>,
    pub completes_at: Option<u64>,
}

impl ShipJob {
    pub fn label(&self) -> String {
        if let Some(kind) = self.defence {
            kind.to_string()
        } else if let Some(kind) = self.ship {
            kind.to_string()
        } else {
            "yard".into()
        }
    }

    pub fn kind_key(&self) -> &'static str {
        if let Some(kind) = self.defence {
            kind.as_key()
        } else if let Some(kind) = self.ship {
            kind.as_key()
        } else {
            "yard"
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShipError {
    NeedShipyard { required: u32, actual: u32 },
    MissingPrerequisites { gaps: Vec<crate::PrereqGap> },
    QueueFull { cap: usize },
    ZeroCount,
    InsufficientResources { missing: Resources, cost: Resources },
    EmptySlot { index: usize, len: usize },
}

impl fmt::Display for ShipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShipError::NeedShipyard { required, actual } => {
                write!(f, "requires shipyard level {required} (have {actual})")
            }
            ShipError::MissingPrerequisites { gaps } => {
                write!(f, "missing ship prerequisites: ")?;
                for (i, gap) in gaps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{gap}")?;
                }
                Ok(())
            }
            ShipError::QueueFull { cap } => write!(f, "ship queue is full ({cap}/{cap})"),
            ShipError::ZeroCount => write!(f, "ship batch count must be at least 1"),
            ShipError::InsufficientResources { missing, cost } => write!(
                f,
                "not enough resources (missing metal {:.0}, crystal {:.0}, deuterium {:.0}; cost metal {:.0}, crystal {:.0}, deuterium {:.0})",
                missing.metal,
                missing.crystal,
                missing.deuterium,
                cost.metal,
                cost.crystal,
                cost.deuterium
            ),
            ShipError::EmptySlot { index, len } => {
                write!(f, "no ship job at index {index} (queue length {len})")
            }
        }
    }
}

impl std::error::Error for ShipError {}

#[derive(Debug, Clone, PartialEq)]
pub enum ShipCatalogError {
    Parse(String),
    UnknownId(String),
    DuplicateId(String),
    MissingId(&'static str),
    MissingDrive(String),
    UnknownRapidFireTarget { ship: String, target: String },
}

impl fmt::Display for ShipCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShipCatalogError::Parse(e) => write!(f, "ships.toml: {e}"),
            ShipCatalogError::UnknownId(id) => write!(f, "unknown ship id {id}"),
            ShipCatalogError::DuplicateId(id) => write!(f, "duplicate ship id {id}"),
            ShipCatalogError::MissingId(id) => write!(f, "ships.toml missing {id}"),
            ShipCatalogError::MissingDrive(id) => write!(f, "ships.toml {id} missing drive"),
            ShipCatalogError::UnknownRapidFireTarget { ship, target } => {
                write!(f, "ships.toml {ship} rapid_fire unknown target {target}")
            }
        }
    }
}

impl std::error::Error for ShipCatalogError {}

#[derive(Deserialize)]
struct File {
    ship: Vec<RawShip>,
}

#[derive(Deserialize)]
struct RawShip {
    id: String,
    name: String,
    #[serde(alias = "frames")]
    metal: f64,
    #[serde(alias = "electronics")]
    crystal: f64,
    #[serde(alias = "propellant")]
    deuterium: f64,
    base_seconds: f64,
    attack: f64,
    shield: f64,
    hull: f64,
    cargo: f64,
    speed: f64,
    fuel: f64,
    shipyard: u32,
    drive: String,
    #[serde(default)]
    prereq_techs: Vec<RawPrereq>,
    #[serde(default)]
    rapid_fire: Vec<RawRapidFire>,
}

#[derive(Deserialize)]
struct RawPrereq {
    id: String,
    level: u32,
}

#[derive(Deserialize)]
struct RawRapidFire {
    target: String,
    value: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_has_eleven_named_types() {
        let catalog = ShipCatalog::default();
        for kind in ShipKind::ALL {
            assert_eq!(catalog.get(kind).kind, kind);
            assert!(!catalog.get(kind).drive.is_empty());
            assert!(catalog.get(kind).shipyard >= 1);
        }
        assert!(
            catalog.get(ShipKind::LightFighter).attack < catalog.get(ShipKind::HeavyFighter).attack
        );
        assert!(catalog.get(ShipKind::SmallCargo).cargo < catalog.get(ShipKind::LargeCargo).cargo);
        assert!(catalog.get(ShipKind::Cruiser).attack < catalog.get(ShipKind::Battleship).attack);
        assert!(
            catalog.get(ShipKind::EspionageProbe).speed > catalog.get(ShipKind::LightFighter).speed
        );
        assert!(
            catalog
                .get(ShipKind::Cruiser)
                .rapid_fire
                .iter()
                .any(|&(kind, value)| kind == ShipKind::LightFighter && value == 6)
        );
    }
}
