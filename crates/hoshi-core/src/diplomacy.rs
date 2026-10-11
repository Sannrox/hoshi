//! NPC factions and standing. Specs live in `data/factions.toml`.

use crate::research::ModifierKind;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::fmt;

const DEFAULT_FACTIONS: &str = include_str!("../data/factions.toml");

#[derive(Debug, Clone, PartialEq)]
pub enum DiplomacyError {
    Parse(String),
    DuplicateId(String),
    EmptyTiers(String),
    DuplicateTier { faction: String, tier: String },
    Disabled,
    UnknownFaction(String),
    NoTribute(String),
    NoStandingChange(String),
    Insufficient { missing: f64 },
}

impl fmt::Display for DiplomacyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiplomacyError::Parse(msg) => write!(f, "diplomacy: {msg}"),
            DiplomacyError::DuplicateId(id) => write!(f, "diplomacy: duplicate faction {id}"),
            DiplomacyError::EmptyTiers(id) => write!(f, "diplomacy: {id} has no tiers"),
            DiplomacyError::DuplicateTier { faction, tier } => {
                write!(f, "diplomacy: {faction} duplicate tier {tier}")
            }
            DiplomacyError::Disabled => write!(f, "diplomacy is off"),
            DiplomacyError::UnknownFaction(id) => write!(f, "unknown faction {id}"),
            DiplomacyError::NoTribute(id) => write!(f, "{id} does not take tribute"),
            DiplomacyError::NoStandingChange(id) => {
                write!(f, "{id} standing cannot move")
            }
            DiplomacyError::Insufficient { missing } => {
                write!(f, "diplomacy: missing {missing:.0} metal")
            }
        }
    }
}

impl std::error::Error for DiplomacyError {}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DiplomacyModifier {
    pub kind: ModifierKind,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FactionTier {
    pub id: String,
    pub name: String,
    pub min: f64,
    pub market_fee: f64,
    pub raid_delay_mult: f64,
    pub schedule_raid: bool,
    pub modifiers: Vec<DiplomacyModifier>,
}

impl FactionTier {
    pub fn modifier(&self, kind: ModifierKind) -> f64 {
        self.modifiers
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| m.value)
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FactionSpec {
    pub id: String,
    pub name: String,
    pub start: f64,
    pub trade_delta: f64,
    pub raid_win_delta: f64,
    pub raid_loss_delta: f64,
    pub conquest_delta: f64,
    pub tribute_metal: f64,
    pub tribute_delta: f64,
    pub tiers: Vec<FactionTier>,
}

impl FactionSpec {
    pub fn tier(&self, standing: f64) -> Option<&FactionTier> {
        self.tiers
            .iter()
            .filter(|t| standing + 1e-9 >= t.min)
            .max_by(|a, b| a.min.total_cmp(&b.min))
    }

    pub fn next_tier(&self, standing: f64) -> Option<&FactionTier> {
        self.tiers
            .iter()
            .filter(|t| t.min > standing + 1e-9)
            .min_by(|a, b| a.min.total_cmp(&b.min))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiplomacyConfig {
    pub enabled: bool,
    pub tick_secs: u64,
    pub decay: f64,
    pub history_len: usize,
    pub min_standing: f64,
    pub max_standing: f64,
    factions: Vec<FactionSpec>,
}

impl DiplomacyConfig {
    pub fn parse(toml: &str) -> Result<Self, DiplomacyError> {
        let file: File = toml::from_str(toml).map_err(|e| DiplomacyError::Parse(e.to_string()))?;
        file.into_config()
    }

    pub fn inactive() -> Self {
        Self {
            enabled: false,
            tick_secs: 3600,
            decay: 0.05,
            history_len: 8,
            min_standing: -100.0,
            max_standing: 100.0,
            factions: Vec::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&FactionSpec> {
        self.factions.iter().find(|f| f.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &FactionSpec> {
        self.factions.iter()
    }

    pub fn len(&self) -> usize {
        self.factions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.factions.is_empty()
    }
}

impl Default for DiplomacyConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_FACTIONS).expect("bundled factions.toml must be valid")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StandingChange {
    pub at: u64,
    pub faction: String,
    pub delta: f64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiplomacyState {
    pub standing: BTreeMap<String, f64>,
    pub log: Vec<StandingChange>,
}

impl DiplomacyState {
    pub fn from_config(cfg: &DiplomacyConfig) -> Self {
        let standing = cfg.iter().map(|f| (f.id.clone(), f.start)).collect();
        Self {
            standing,
            log: Vec::new(),
        }
    }

    pub fn standing(&self, id: &str) -> f64 {
        self.standing.get(id).copied().unwrap_or(0.0)
    }

    pub fn set_standing(&mut self, id: &str, value: f64) {
        self.standing.insert(id.to_string(), value);
    }
}

#[derive(Deserialize)]
struct File {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_tick")]
    tick_secs: u64,
    #[serde(default = "default_decay")]
    decay: f64,
    #[serde(default = "default_hist")]
    history_len: u32,
    #[serde(default = "default_min")]
    min_standing: f64,
    #[serde(default = "default_max")]
    max_standing: f64,
    #[serde(default)]
    faction: Vec<FileFaction>,
}

#[derive(Deserialize)]
struct FileFaction {
    id: String,
    name: String,
    #[serde(default)]
    start: f64,
    #[serde(default)]
    trade_delta: f64,
    #[serde(default)]
    raid_win_delta: f64,
    #[serde(default)]
    raid_loss_delta: f64,
    #[serde(default)]
    conquest_delta: f64,
    #[serde(default)]
    tribute_metal: f64,
    #[serde(default)]
    tribute_delta: f64,
    #[serde(default)]
    tier: Vec<FileTier>,
}

#[derive(Deserialize)]
struct FileTier {
    id: String,
    name: String,
    min: f64,
    #[serde(default)]
    market_fee: f64,
    #[serde(default = "default_raid_mult")]
    raid_delay_mult: f64,
    #[serde(default)]
    schedule_raid: bool,
    #[serde(default)]
    modifiers: Vec<DiplomacyModifier>,
}

fn default_true() -> bool {
    true
}
fn default_tick() -> u64 {
    3600
}
fn default_decay() -> f64 {
    0.05
}
fn default_hist() -> u32 {
    8
}
fn default_min() -> f64 {
    -100.0
}
fn default_max() -> f64 {
    100.0
}
fn default_raid_mult() -> f64 {
    1.0
}

impl File {
    fn into_config(self) -> Result<DiplomacyConfig, DiplomacyError> {
        if self.enabled {
            if self.tick_secs < 1 {
                return Err(DiplomacyError::Parse("tick_secs must be >= 1".into()));
            }
            if self.history_len < 1 {
                return Err(DiplomacyError::Parse("history_len must be >= 1".into()));
            }
            if !self.decay.is_finite() || self.decay < 0.0 || self.decay > 1.0 {
                return Err(DiplomacyError::Parse("decay must be in [0, 1]".into()));
            }
            if !self.min_standing.is_finite() || !self.max_standing.is_finite() {
                return Err(DiplomacyError::Parse(
                    "standing bounds must be finite".into(),
                ));
            }
            if self.max_standing < self.min_standing {
                return Err(DiplomacyError::Parse(
                    "max_standing must be >= min_standing".into(),
                ));
            }
            if self.faction.is_empty() {
                return Err(DiplomacyError::Parse(
                    "enabled diplomacy needs factions".into(),
                ));
            }
        }
        let mut seen = HashSet::new();
        let mut factions = Vec::new();
        for raw in self.faction {
            if !seen.insert(raw.id.clone()) {
                return Err(DiplomacyError::DuplicateId(raw.id));
            }
            if raw.tier.is_empty() {
                return Err(DiplomacyError::EmptyTiers(raw.id));
            }
            let mut tier_ids = HashSet::new();
            let mut tiers = Vec::new();
            for t in raw.tier {
                if !tier_ids.insert(t.id.clone()) {
                    return Err(DiplomacyError::DuplicateTier {
                        faction: raw.id,
                        tier: t.id,
                    });
                }
                if !t.min.is_finite() {
                    return Err(DiplomacyError::Parse(format!(
                        "{} tier {} min must be finite",
                        raw.id, t.id
                    )));
                }
                if !t.market_fee.is_finite() {
                    return Err(DiplomacyError::Parse(format!(
                        "{} tier {} market_fee must be finite",
                        raw.id, t.id
                    )));
                }
                if !t.raid_delay_mult.is_finite() || t.raid_delay_mult < 0.0 {
                    return Err(DiplomacyError::Parse(format!(
                        "{} tier {} raid_delay_mult must be >= 0",
                        raw.id, t.id
                    )));
                }
                for m in &t.modifiers {
                    if !m.value.is_finite() {
                        return Err(DiplomacyError::Parse(format!(
                            "{} tier {} modifier must be finite",
                            raw.id, t.id
                        )));
                    }
                }
                tiers.push(FactionTier {
                    id: t.id,
                    name: t.name,
                    min: t.min,
                    market_fee: t.market_fee,
                    raid_delay_mult: t.raid_delay_mult,
                    schedule_raid: t.schedule_raid,
                    modifiers: t.modifiers,
                });
            }
            factions.push(FactionSpec {
                id: raw.id,
                name: raw.name,
                start: raw.start,
                trade_delta: raw.trade_delta,
                raid_win_delta: raw.raid_win_delta,
                raid_loss_delta: raw.raid_loss_delta,
                conquest_delta: raw.conquest_delta,
                tribute_metal: raw.tribute_metal,
                tribute_delta: raw.tribute_delta,
                tiers,
            });
        }
        Ok(DiplomacyConfig {
            enabled: self.enabled,
            tick_secs: self.tick_secs,
            decay: self.decay,
            history_len: self.history_len.max(1) as usize,
            min_standing: self.min_standing,
            max_standing: self.max_standing,
            factions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_has_three_factions_and_tiers() {
        let cfg = DiplomacyConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.len(), 3);
        for faction in cfg.iter() {
            assert!(!faction.tiers.is_empty(), "{}", faction.id);
            assert!(faction.tier(0.0).is_some(), "{}", faction.id);
        }
        let honored = cfg.get("traders").unwrap().tier(50.0).unwrap();
        assert_eq!(honored.id, "honored");
        assert!(honored.modifier(ModifierKind::MetalOutput) > 0.0);
        let pact = cfg.get("pirates").unwrap().tier(60.0).unwrap();
        assert_eq!(pact.raid_delay_mult, 0.0);
        let woke = cfg.get("ancients").unwrap().tier(-50.0).unwrap();
        assert!(woke.schedule_raid);
    }

    #[test]
    fn next_tier_is_the_next_threshold_up() {
        let cfg = DiplomacyConfig::default();
        let traders = cfg.get("traders").unwrap();
        let next = traders.next_tier(0.0).unwrap();
        assert_eq!(next.id, "favored");
        assert!(traders.next_tier(50.0).is_none());
    }
}
