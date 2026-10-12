//! Prestige currency, purchased bonuses and per-run statistics.
//! Meta progress lives here so a reset can wipe the run without wiping the empire's history.

use std::collections::BTreeMap;
use std::fmt;

/// One purchasable bonus. Counts stack; effects add.
#[derive(Debug, Clone, PartialEq)]
pub struct PrestigeBonusSpec {
    pub id: String,
    pub name: String,
    pub cost: u32,
    pub starting_metal: f64,
    pub starting_crystal: f64,
    pub starting_deuterium: f64,
    pub production: f64,
    pub research: f64,
    pub queue_slots: u32,
}

/// Reset threshold and the shop. Values come from balance config.
#[derive(Debug, Clone, PartialEq)]
pub struct PrestigeConfig {
    pub min_score: u32,
    pub colony_weight: u32,
    pub wonder_weight: u32,
    pub points_per_currency: u32,
    pub bonuses: Vec<PrestigeBonusSpec>,
}

impl Default for PrestigeConfig {
    fn default() -> Self {
        Self {
            min_score: 80,
            colony_weight: 8,
            wonder_weight: 40,
            points_per_currency: 20,
            bonuses: vec![
                PrestigeBonusSpec {
                    id: "starter_cache".into(),
                    name: "Starter cache".into(),
                    cost: 1,
                    starting_metal: 250.0,
                    starting_crystal: 100.0,
                    starting_deuterium: 50.0,
                    production: 0.0,
                    research: 0.0,
                    queue_slots: 0,
                },
                PrestigeBonusSpec {
                    id: "mine_focus".into(),
                    name: "Mine focus".into(),
                    cost: 2,
                    starting_metal: 0.0,
                    starting_crystal: 0.0,
                    starting_deuterium: 0.0,
                    production: 0.08,
                    research: 0.0,
                    queue_slots: 0,
                },
                PrestigeBonusSpec {
                    id: "lab_focus".into(),
                    name: "Lab focus".into(),
                    cost: 2,
                    starting_metal: 0.0,
                    starting_crystal: 0.0,
                    starting_deuterium: 0.0,
                    production: 0.0,
                    research: 0.08,
                    queue_slots: 0,
                },
                PrestigeBonusSpec {
                    id: "extra_queue".into(),
                    name: "Extra queue".into(),
                    cost: 3,
                    starting_metal: 0.0,
                    starting_crystal: 0.0,
                    starting_deuterium: 0.0,
                    production: 0.0,
                    research: 0.0,
                    queue_slots: 1,
                },
            ],
        }
    }
}

impl PrestigeConfig {
    pub fn bonus(&self, id: &str) -> Option<&PrestigeBonusSpec> {
        self.bonuses.iter().find(|row| row.id == id)
    }

    pub fn run_score(
        &self,
        building_levels: u32,
        research_levels: u32,
        colonies: u32,
        wonders: u32,
    ) -> u32 {
        building_levels
            .saturating_add(research_levels)
            .saturating_add(colonies.saturating_mul(self.colony_weight))
            .saturating_add(wonders.saturating_mul(self.wonder_weight))
    }

    pub fn can_reset(&self, wonders: u32, score: u32) -> bool {
        wonders > 0 || score >= self.min_score
    }
}

/// Closed or in-progress run totals. `ended_at` is set when the run resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStats {
    pub index: u32,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub score: u32,
    pub prestige_earned: u32,
    pub wonders: u32,
    pub colonies: u32,
    pub building_levels: u32,
    pub research_levels: u32,
}

impl RunStats {
    pub fn new(index: u32, started_at: u64) -> Self {
        Self {
            index,
            started_at,
            ended_at: None,
            score: 0,
            prestige_earned: 0,
            wonders: 0,
            colonies: 0,
            building_levels: 0,
            research_levels: 0,
        }
    }
}

/// Meta progress that survives a reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prestige {
    pub currency: u32,
    pub lifetime: u32,
    pub owned: BTreeMap<String, u32>,
    pub runs: Vec<RunStats>,
    pub current: RunStats,
}

impl Default for Prestige {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Prestige {
    pub fn new(now_unix: u64) -> Self {
        Self {
            currency: 0,
            lifetime: 0,
            owned: BTreeMap::new(),
            runs: Vec::new(),
            current: RunStats::new(1, now_unix),
        }
    }

    pub fn owned_count(&self, id: &str) -> u32 {
        self.owned.get(id).copied().unwrap_or(0)
    }

    fn fold(&self, config: &PrestigeConfig, f: impl Fn(&PrestigeBonusSpec) -> f64) -> f64 {
        self.owned
            .iter()
            .map(|(id, count)| {
                config
                    .bonus(id)
                    .map(|spec| f(spec) * f64::from(*count))
                    .unwrap_or(0.0)
            })
            .sum()
    }

    pub fn production_bonus(&self, config: &PrestigeConfig) -> f64 {
        self.fold(config, |spec| spec.production)
    }

    pub fn research_bonus(&self, config: &PrestigeConfig) -> f64 {
        self.fold(config, |spec| spec.research)
    }

    pub fn queue_slots(&self, config: &PrestigeConfig) -> u32 {
        self.owned
            .iter()
            .map(|(id, count)| {
                config
                    .bonus(id)
                    .map(|spec| spec.queue_slots.saturating_mul(*count))
                    .unwrap_or(0)
            })
            .sum()
    }

    pub fn starting_bonus(&self, config: &PrestigeConfig) -> (f64, f64, f64) {
        let metal = self.fold(config, |spec| spec.starting_metal);
        let crystal = self.fold(config, |spec| spec.starting_crystal);
        let deuterium = self.fold(config, |spec| spec.starting_deuterium);
        (metal, crystal, deuterium)
    }
}

/// What a reset keeps and drops, plus the currency it would award.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrestigePreview {
    pub eligible: bool,
    pub score: u32,
    pub earned: u32,
    pub currency_after: u32,
    pub kept: &'static [&'static str],
    pub lost: &'static [&'static str],
    pub run: RunStats,
}

pub const PRESTIGE_KEPT: &[&str] = &[
    "prestige currency",
    "purchased bonuses",
    "run history",
    "achievements",
];
pub const PRESTIGE_LOST: &[&str] = &[
    "resources",
    "buildings",
    "research",
    "fleets",
    "colonies",
    "wonders",
    "missions",
    "spy reports",
    "planet defences",
];

/// Why a prestige action was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrestigeError {
    NotEligible { score: u32, min_score: u32 },
    UnknownBonus { id: String },
    CannotAfford { cost: u32, have: u32 },
}

impl fmt::Display for PrestigeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrestigeError::NotEligible { score, min_score } => {
                write!(
                    f,
                    "prestige needs a completed wonder or score {min_score} (have {score})"
                )
            }
            PrestigeError::UnknownBonus { id } => write!(f, "unknown prestige bonus {id}"),
            PrestigeError::CannotAfford { cost, have } => {
                write!(f, "prestige bonus costs {cost} (have {have})")
            }
        }
    }
}

impl std::error::Error for PrestigeError {}
