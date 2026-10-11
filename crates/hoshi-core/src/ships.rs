//! Ship types and the per-planet shipyard queue.

use crate::state::Resources;
use serde::Deserialize;
use std::fmt;

const DEFAULT_SHIPS: &str = include_str!("../data/ships.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShipKind {
    LightFighter,
    HeavyFighter,
    SmallCargo,
}

impl ShipKind {
    pub const ALL: [ShipKind; 3] = [
        ShipKind::LightFighter,
        ShipKind::HeavyFighter,
        ShipKind::SmallCargo,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            ShipKind::LightFighter => "light_fighter",
            ShipKind::HeavyFighter => "heavy_fighter",
            ShipKind::SmallCargo => "small_cargo",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "light_fighter" => ShipKind::LightFighter,
            "heavy_fighter" => ShipKind::HeavyFighter,
            "small_cargo" => ShipKind::SmallCargo,
            _ => return None,
        })
    }
}

impl fmt::Display for ShipKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ShipKind::LightFighter => "light fighter",
            ShipKind::HeavyFighter => "heavy fighter",
            ShipKind::SmallCargo => "small cargo",
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

#[derive(Debug, Clone, PartialEq)]
pub struct ShipCatalog {
    pub light_fighter: ShipSpec,
    pub heavy_fighter: ShipSpec,
    pub small_cargo: ShipSpec,
}

impl ShipCatalog {
    pub fn parse(toml: &str) -> Result<Self, ShipCatalogError> {
        let file: File =
            toml::from_str(toml).map_err(|e| ShipCatalogError::Parse(e.to_string()))?;
        let mut light = None;
        let mut heavy = None;
        let mut cargo = None;
        for raw in file.ship {
            let kind = ShipKind::from_key(&raw.id)
                .ok_or_else(|| ShipCatalogError::UnknownId(raw.id.clone()))?;
            let spec = ShipSpec {
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
            };
            match kind {
                ShipKind::LightFighter => {
                    if light.is_some() {
                        return Err(ShipCatalogError::DuplicateId(raw.id));
                    }
                    light = Some(spec);
                }
                ShipKind::HeavyFighter => {
                    if heavy.is_some() {
                        return Err(ShipCatalogError::DuplicateId(raw.id));
                    }
                    heavy = Some(spec);
                }
                ShipKind::SmallCargo => {
                    if cargo.is_some() {
                        return Err(ShipCatalogError::DuplicateId(raw.id));
                    }
                    cargo = Some(spec);
                }
            }
        }
        Ok(Self {
            light_fighter: light.ok_or(ShipCatalogError::MissingId("light_fighter"))?,
            heavy_fighter: heavy.ok_or(ShipCatalogError::MissingId("heavy_fighter"))?,
            small_cargo: cargo.ok_or(ShipCatalogError::MissingId("small_cargo"))?,
        })
    }

    pub fn get(&self, kind: ShipKind) -> &ShipSpec {
        match kind {
            ShipKind::LightFighter => &self.light_fighter,
            ShipKind::HeavyFighter => &self.heavy_fighter,
            ShipKind::SmallCargo => &self.small_cargo,
        }
    }
}

impl Default for ShipCatalog {
    fn default() -> Self {
        Self::parse(DEFAULT_SHIPS).expect("bundled ships.toml must be valid")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fleet {
    pub light_fighter: u32,
    pub heavy_fighter: u32,
    pub small_cargo: u32,
}

impl Fleet {
    pub fn get(self, kind: ShipKind) -> u32 {
        match kind {
            ShipKind::LightFighter => self.light_fighter,
            ShipKind::HeavyFighter => self.heavy_fighter,
            ShipKind::SmallCargo => self.small_cargo,
        }
    }

    pub fn add(&mut self, kind: ShipKind, n: u32) {
        match kind {
            ShipKind::LightFighter => self.light_fighter = self.light_fighter.saturating_add(n),
            ShipKind::HeavyFighter => self.heavy_fighter = self.heavy_fighter.saturating_add(n),
            ShipKind::SmallCargo => self.small_cargo = self.small_cargo.saturating_add(n),
        }
    }
}

/// One shipyard batch. Only the head has a timer; ships in that batch complete one by one.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipJob {
    pub ship: ShipKind,
    pub remaining: u32,
    pub unit_paid: Resources,
    pub started_at: Option<u64>,
    pub completes_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShipError {
    NeedShipyard { required: u32, actual: u32 },
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
}

impl fmt::Display for ShipCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShipCatalogError::Parse(e) => write!(f, "ships.toml: {e}"),
            ShipCatalogError::UnknownId(id) => write!(f, "unknown ship id {id}"),
            ShipCatalogError::DuplicateId(id) => write!(f, "duplicate ship id {id}"),
            ShipCatalogError::MissingId(id) => write!(f, "ships.toml missing {id}"),
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
    metal: f64,
    crystal: f64,
    deuterium: f64,
    base_seconds: f64,
    attack: f64,
    shield: f64,
    hull: f64,
    cargo: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_has_three_named_types() {
        let catalog = ShipCatalog::default();
        assert_eq!(catalog.light_fighter.kind, ShipKind::LightFighter);
        assert_eq!(catalog.heavy_fighter.kind, ShipKind::HeavyFighter);
        assert_eq!(catalog.small_cargo.kind, ShipKind::SmallCargo);
        assert!(catalog.light_fighter.attack < catalog.heavy_fighter.attack);
        assert!(catalog.small_cargo.cargo > catalog.light_fighter.cargo);
        assert!(catalog.light_fighter.metal < catalog.heavy_fighter.metal);
    }
}
