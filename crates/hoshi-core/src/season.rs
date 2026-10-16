//! Seasonal galaxies: local date windows and data-driven rule overlays.

use crate::balance::BalanceConfig;
use crate::state::GameState;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_SEASONS: &str = include_str!("../data/seasons.toml");

/// Systems a rule set may enable or disable. Unknown keys fail load.
pub const SEASON_SYSTEMS: &[&str] = &["raid", "market", "activity", "victory", "moons"];

/// One playable seasonal galaxy window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonWindow {
    pub index: u64,
    pub start: u64,
    pub end: u64,
}

/// Recorded on a seasonal save so load can re-apply the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonRun {
    pub ruleset_id: String,
    pub window_start: u64,
    pub window_end: u64,
    pub seed: u64,
    pub prestige: bool,
}

/// One shipped or loaded rule set.
#[derive(Debug, Clone, PartialEq)]
pub struct SeasonRuleset {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub prestige: bool,
    pub systems: BTreeMap<String, bool>,
    pub reward_mult: Option<f64>,
    pub raid_first_delay_secs: Option<u64>,
    pub raid_interval_secs: Option<u64>,
    pub raid_strength_factor: Option<f64>,
}

impl SeasonRuleset {
    /// Overlay this rule set onto a base balance file. Unknown systems already failed at parse.
    pub fn apply(&self, cfg: &mut BalanceConfig) {
        for (system, on) in &self.systems {
            match system.as_str() {
                "raid" => cfg.raid.enabled = *on,
                "market" => cfg.market.enabled = *on,
                "activity" => cfg.activity.enabled = *on,
                "victory" => cfg.victory.enabled = *on,
                "moons" => cfg.moons.enabled = *on,
                _ => {}
            }
        }
        if let Some(mult) = self.reward_mult {
            cfg.expedition.reward_mult = mult.max(0.0);
        }
        if let Some(secs) = self.raid_first_delay_secs {
            cfg.raid.first_delay_secs = secs.max(1);
        }
        if let Some(secs) = self.raid_interval_secs {
            cfg.raid.interval_secs = secs.max(1);
        }
        if let Some(factor) = self.raid_strength_factor {
            cfg.raid.strength_factor = factor.max(0.0);
        }
    }
}

/// Catalog of rule sets plus the shared calendar.
#[derive(Debug, Clone, PartialEq)]
pub struct SeasonCatalog {
    pub epoch_unix: u64,
    pub duration_secs: u64,
    rulesets: Vec<SeasonRuleset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeasonError {
    Parse(String),
    Invalid(String),
    UnknownRuleset(String),
}

impl fmt::Display for SeasonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeasonError::Parse(msg) | SeasonError::Invalid(msg) => write!(f, "{msg}"),
            SeasonError::UnknownRuleset(id) => write!(f, "unknown season {id}"),
        }
    }
}

impl std::error::Error for SeasonError {}

impl SeasonCatalog {
    pub fn parse(toml: &str) -> Result<Self, SeasonError> {
        let file: File = toml::from_str(toml).map_err(|e| SeasonError::Parse(e.to_string()))?;
        file.into_catalog()
    }

    pub fn get(&self, id: &str) -> Option<&SeasonRuleset> {
        self.rulesets.iter().find(|r| r.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SeasonRuleset> {
        self.rulesets.iter()
    }

    pub fn len(&self) -> usize {
        self.rulesets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rulesets.is_empty()
    }

    /// Inclusive-start, exclusive-end window containing `now` (injected unix, UTC).
    pub fn window(&self, now: u64) -> SeasonWindow {
        window_at(now, self.epoch_unix, self.duration_secs)
    }

    /// Rule set for the UTC window containing `now`. None before epoch or with an empty catalog.
    pub fn current(&self, now: u64) -> Option<&SeasonRuleset> {
        if self.is_empty() || now < self.epoch_unix {
            return None;
        }
        let window = self.window(now);
        let n = self.rulesets.len();
        self.rulesets.get((window.index as usize) % n)
    }

    pub fn seed_for(&self, ruleset_id: &str, window: &SeasonWindow) -> Result<u64, SeasonError> {
        if self.get(ruleset_id).is_none() {
            return Err(SeasonError::UnknownRuleset(ruleset_id.into()));
        }
        Ok(season_seed(ruleset_id, window.index))
    }

    pub fn overlay(
        &self,
        ruleset_id: &str,
        mut cfg: BalanceConfig,
    ) -> Result<(BalanceConfig, &SeasonRuleset), SeasonError> {
        let ruleset = self
            .get(ruleset_id)
            .ok_or_else(|| SeasonError::UnknownRuleset(ruleset_id.into()))?;
        ruleset.apply(&mut cfg);
        Ok((cfg, ruleset))
    }
}

impl Default for SeasonCatalog {
    fn default() -> Self {
        Self::parse(DEFAULT_SEASONS).expect("bundled seasons.toml must be valid")
    }
}

fn window_at(now: u64, epoch: u64, duration: u64) -> SeasonWindow {
    let duration = duration.max(1);
    let elapsed = now.saturating_sub(epoch);
    let index = elapsed / duration;
    let start = epoch.saturating_add(index.saturating_mul(duration));
    SeasonWindow {
        index,
        start,
        end: start.saturating_add(duration),
    }
}

fn season_seed(id: &str, index: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325;
    for b in id.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h ^= index;
    h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h
}

/// Slot stem for a seasonal save, kept apart from the main `save.sqlite`.
pub fn season_slot_name(ruleset_id: &str, window_start: u64) -> String {
    format!("season-{ruleset_id}-{window_start}")
}

#[derive(Deserialize)]
struct File {
    version: u32,
    epoch_unix: u64,
    duration_secs: u64,
    #[serde(rename = "season")]
    seasons: Vec<FileSeason>,
}

#[derive(Deserialize)]
struct FileSeason {
    id: String,
    name: String,
    #[serde(default)]
    blurb: String,
    #[serde(default)]
    prestige: bool,
    #[serde(default)]
    systems: BTreeMap<String, bool>,
    #[serde(default)]
    expedition: FileExpeditionOverlay,
    #[serde(default)]
    raid: FileRaidOverlay,
}

#[derive(Deserialize, Default)]
struct FileExpeditionOverlay {
    reward_mult: Option<f64>,
}

#[derive(Deserialize, Default)]
struct FileRaidOverlay {
    first_delay_secs: Option<u64>,
    interval_secs: Option<u64>,
    strength_factor: Option<f64>,
}

impl File {
    fn into_catalog(self) -> Result<SeasonCatalog, SeasonError> {
        if self.version != 1 {
            return Err(SeasonError::Invalid(format!(
                "unsupported seasons version {} (want 1)",
                self.version
            )));
        }
        if self.duration_secs == 0 {
            return Err(SeasonError::Invalid("duration_secs must be > 0".into()));
        }
        if self.seasons.is_empty() {
            return Err(SeasonError::Invalid(
                "at least one season is required".into(),
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut rulesets = Vec::with_capacity(self.seasons.len());
        for row in self.seasons {
            if row.id.is_empty() {
                return Err(SeasonError::Invalid("season id is empty".into()));
            }
            if !row
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(SeasonError::Invalid(format!(
                    "invalid season id {}",
                    row.id
                )));
            }
            if row.name.is_empty() {
                return Err(SeasonError::Invalid(format!(
                    "season {} has no name",
                    row.id
                )));
            }
            if !ids.insert(row.id.clone()) {
                return Err(SeasonError::Invalid(format!("duplicate season {}", row.id)));
            }
            for key in row.systems.keys() {
                if !SEASON_SYSTEMS.contains(&key.as_str()) {
                    return Err(SeasonError::Invalid(format!(
                        "unknown system {key} on season {}",
                        row.id
                    )));
                }
            }
            if row
                .expedition
                .reward_mult
                .is_some_and(|m| !m.is_finite() || m < 0.0)
            {
                return Err(SeasonError::Invalid(format!(
                    "season {} reward_mult must be finite and >= 0",
                    row.id
                )));
            }
            if row
                .raid
                .strength_factor
                .is_some_and(|f| !f.is_finite() || f < 0.0)
            {
                return Err(SeasonError::Invalid(format!(
                    "season {} raid strength_factor must be finite and >= 0",
                    row.id
                )));
            }
            rulesets.push(SeasonRuleset {
                id: row.id,
                name: row.name,
                blurb: row.blurb,
                prestige: row.prestige,
                systems: row.systems,
                reward_mult: row.expedition.reward_mult,
                raid_first_delay_secs: row.raid.first_delay_secs,
                raid_interval_secs: row.raid.interval_secs,
                raid_strength_factor: row.raid.strength_factor,
            });
        }
        Ok(SeasonCatalog {
            epoch_unix: self.epoch_unix,
            duration_secs: self.duration_secs,
            rulesets,
        })
    }
}

impl GameState {
    /// Start a seasonal galaxy. Prestige stays empty unless the rule set opts in.
    pub fn new_season(
        now_unix: u64,
        catalog: &SeasonCatalog,
        ruleset_id: &str,
        base: BalanceConfig,
    ) -> Result<Self, SeasonError> {
        let window = catalog.window(now_unix);
        let seed = catalog.seed_for(ruleset_id, &window)?;
        let (config, ruleset) = catalog.overlay(ruleset_id, base)?;
        let mut state = Self::new_live_galaxy(now_unix, config, seed);
        if !ruleset.prestige {
            state.prestige = crate::Prestige::new(now_unix);
        }
        state.season = Some(SeasonRun {
            ruleset_id: ruleset.id.clone(),
            window_start: window.start,
            window_end: window.end,
            seed,
            prestige: ruleset.prestige,
        });
        Ok(state)
    }

    pub fn apply_loaded_season(&mut self, catalog: &SeasonCatalog) -> Result<(), SeasonError> {
        let Some(run) = self.season.as_ref() else {
            return Ok(());
        };
        let (config, _) = catalog.overlay(&run.ruleset_id, self.config.clone())?;
        self.config = config;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_has_three_rule_sets() {
        let cat = SeasonCatalog::default();
        assert!(cat.len() >= 3);
        assert!(cat.get("double_haul").is_some());
        assert!(cat.get("closed_market").is_some());
        assert!(cat.get("pirate_weather").is_some());
    }

    #[test]
    fn window_repeats_for_a_clock() {
        let cat = SeasonCatalog::default();
        let a = cat.window(1_704_067_200 + 100);
        let b = cat.window(1_704_067_200 + 100);
        assert_eq!(a, b);
        assert_eq!(a.start, cat.epoch_unix);
        assert_eq!(a.end.saturating_sub(a.start), cat.duration_secs);
        let later = cat.window(a.end);
        assert_eq!(later.index, a.index + 1);
        assert_eq!(later.start, a.end);
    }

    #[test]
    fn seed_is_stable_and_differs_by_ruleset() {
        let cat = SeasonCatalog::default();
        let window = cat.window(1_800_000_000);
        let haul = cat.seed_for("double_haul", &window).unwrap();
        let market = cat.seed_for("closed_market", &window).unwrap();
        assert_eq!(haul, cat.seed_for("double_haul", &window).unwrap());
        assert_ne!(haul, market);
        let next = SeasonWindow {
            index: window.index + 1,
            start: window.end,
            end: window.end + cat.duration_secs,
        };
        assert_ne!(haul, cat.seed_for("double_haul", &next).unwrap());
    }

    #[test]
    fn overlays_toggle_systems_and_scale_expedition() {
        let cat = SeasonCatalog::default();
        let mut base = BalanceConfig::default();
        base.market.enabled = true;
        let closed = cat.get("closed_market").unwrap();
        closed.apply(&mut base);
        assert!(!base.market.enabled);

        let mut haul = BalanceConfig::default();
        cat.get("double_haul").unwrap().apply(&mut haul);
        assert!((haul.expedition.reward_mult - 2.0).abs() < 1e-9);

        let mut pirate = BalanceConfig::default();
        pirate.raid.enabled = false;
        cat.get("pirate_weather").unwrap().apply(&mut pirate);
        assert!(pirate.raid.enabled);
        assert_eq!(pirate.raid.interval_secs, 3600);
    }

    #[test]
    fn unknown_system_is_rejected() {
        let err = SeasonCatalog::parse(
            r#"
version = 1
epoch_unix = 0
duration_secs = 10
[[season]]
id = "bad"
name = "Bad"
[season.systems]
narnia = true
"#,
        )
        .unwrap_err();
        assert!(matches!(err, SeasonError::Invalid(msg) if msg.contains("narnia")));
    }

    #[test]
    fn new_season_isolates_prestige() {
        let cat = SeasonCatalog::default();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        let state = GameState::new_season(1_800_000_000, &cat, "double_haul", cfg).unwrap();
        let run = state.season.as_ref().expect("season");
        assert_eq!(run.ruleset_id, "double_haul");
        assert!(!run.prestige);
        assert_eq!(state.prestige.currency, 0);
        assert_eq!(state.galaxy_seed, run.seed);
        assert!((state.config.expedition.reward_mult - 2.0).abs() < 1e-9);
    }

    #[test]
    fn current_rotates_with_the_utc_window() {
        let cat = SeasonCatalog::default();
        assert!(cat.current(cat.epoch_unix.saturating_sub(1)).is_none());
        let first = cat.current(cat.epoch_unix).expect("first window");
        assert_eq!(first.id, "double_haul");
        let second = cat
            .current(cat.epoch_unix + cat.duration_secs)
            .expect("second window");
        assert_eq!(second.id, "closed_market");
    }

    #[test]
    fn slot_name_stays_apart_from_main() {
        assert_eq!(
            season_slot_name("double_haul", 1704067200),
            "season-double_haul-1704067200"
        );
    }

    #[test]
    fn isolated_season_victory_awards_no_prestige_and_unlocks() {
        let cat = SeasonCatalog::default();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg.victory.enabled = true;
        cfg.victory.wonder_id = "dyson_ring".into();
        let mut state = GameState::new_season(1_800_000_000, &cat, "double_haul", cfg).unwrap();
        let before = state.prestige.currency;
        state.completed_wonders.push("dyson_ring".into());
        state.maybe_declare_victory(state.last_tick_unix);
        let rec = state.victory.as_ref().expect("victory");
        assert_eq!(rec.prestige, 0);
        assert_eq!(state.prestige.currency, before);
        assert!(state.achievements.contains_key("season_double_haul"));
    }
}
