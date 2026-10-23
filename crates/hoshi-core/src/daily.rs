//! Data-driven daily goals. Picks are deterministic from galaxy seed and in-game day.

use crate::buildings::BuildingKind;
use crate::combat::next_u64;
use crate::state::{GameState, Resources};
use serde::Deserialize;
use std::fmt;

const DEFAULT_MISSIONS: &str = include_str!("../data/missions.toml");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DailyError {
    Parse(String),
    DuplicateId(String),
    EmptyId,
    EmptyName(String),
    ZeroWeight(String),
    ZeroTarget(String),
    EmptyPool,
}

impl fmt::Display for DailyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DailyError::Parse(msg) => write!(f, "daily: {msg}"),
            DailyError::DuplicateId(id) => write!(f, "daily: duplicate id {id}"),
            DailyError::EmptyId => write!(f, "daily: empty id"),
            DailyError::EmptyName(id) => write!(f, "daily: {id} has no name"),
            DailyError::ZeroWeight(id) => write!(f, "daily: {id} weight must be positive"),
            DailyError::ZeroTarget(id) => write!(f, "daily: {id} target must be greater than zero"),
            DailyError::EmptyPool => write!(f, "daily: catalog is empty"),
        }
    }
}

impl std::error::Error for DailyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DailyKind {
    UpgradeMine,
    FinishResearch,
    WinRaid,
    SendTrade,
    SendExpedition,
}

impl DailyKind {
    pub fn as_key(self) -> &'static str {
        match self {
            DailyKind::UpgradeMine => "upgrade_mine",
            DailyKind::FinishResearch => "finish_research",
            DailyKind::WinRaid => "win_raid",
            DailyKind::SendTrade => "send_trade",
            DailyKind::SendExpedition => "send_expedition",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DailyAction {
    MineUpgrade,
    ResearchComplete,
    RaidWon,
    Trade,
    ExpeditionLaunch,
}

impl DailyAction {
    fn matches(self, kind: DailyKind) -> bool {
        matches!(
            (self, kind),
            (DailyAction::MineUpgrade, DailyKind::UpgradeMine)
                | (DailyAction::ResearchComplete, DailyKind::FinishResearch)
                | (DailyAction::RaidWon, DailyKind::WinRaid)
                | (DailyAction::Trade, DailyKind::SendTrade)
                | (DailyAction::ExpeditionLaunch, DailyKind::SendExpedition)
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailySpec {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: DailyKind,
    pub weight: u32,
    pub target: u32,
    pub reward: Resources,
    pub min_lab: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailySlot {
    pub spec_id: String,
    pub progress: u32,
    pub target: u32,
    pub rewarded: bool,
}

impl DailySlot {
    pub fn done(&self) -> bool {
        self.progress >= self.target
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DailyBoard {
    pub day_index: Option<u64>,
    pub streak: u32,
    pub slots: Vec<DailySlot>,
}

impl DailyBoard {
    pub fn all_complete(&self) -> bool {
        !self.slots.is_empty() && self.slots.iter().all(DailySlot::done)
    }

    pub fn spec<'a>(&self, config: &'a DailyConfig, slot: &DailySlot) -> Option<&'a DailySpec> {
        config.get(&slot.spec_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailyConfig {
    pub day_secs: u64,
    pub reset_offset_secs: u64,
    pub slots: u32,
    pub streak_cap: u32,
    pub streak_reward: Resources,
    pub catalog: Vec<DailySpec>,
}

impl DailyConfig {
    pub fn empty() -> Self {
        Self {
            day_secs: 86_400,
            reset_offset_secs: 0,
            slots: 3,
            streak_cap: 7,
            streak_reward: Resources::ZERO,
            catalog: Vec::new(),
        }
    }

    pub fn parse(toml: &str) -> Result<Vec<DailySpec>, DailyError> {
        File::parse(toml)?.into_specs()
    }

    pub fn get(&self, id: &str) -> Option<&DailySpec> {
        self.catalog.iter().find(|s| s.id == id)
    }

    pub fn day_index(&self, now: u64) -> u64 {
        let secs = self.day_secs.max(1);
        now.saturating_sub(self.reset_offset_secs) / secs
    }

    pub fn resets_in(&self, now: u64) -> u64 {
        let secs = self.day_secs.max(1);
        let elapsed = now.saturating_sub(self.reset_offset_secs) % secs;
        secs - elapsed
    }

    pub fn pick_slots(
        &self,
        galaxy_seed: u64,
        day: u64,
        eligible: impl Fn(&DailySpec) -> bool,
    ) -> Vec<DailySlot> {
        let mut pool: Vec<&DailySpec> = self.catalog.iter().filter(|spec| eligible(spec)).collect();
        if pool.is_empty() {
            pool = self.catalog.iter().collect();
        }
        let want = self.slots as usize;
        let mut seed = galaxy_seed.wrapping_add(day.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut out = Vec::new();
        while out.len() < want && !pool.is_empty() {
            let total: u64 = pool.iter().map(|s| u64::from(s.weight)).sum();
            if total == 0 {
                break;
            }
            let mut roll = next_u64(&mut seed) % total;
            let mut idx = 0;
            for (i, spec) in pool.iter().enumerate() {
                let w = u64::from(spec.weight);
                if roll < w {
                    idx = i;
                    break;
                }
                roll -= w;
            }
            let spec = pool.remove(idx);
            out.push(DailySlot {
                spec_id: spec.id.clone(),
                progress: 0,
                target: spec.target,
                rewarded: false,
            });
        }
        out
    }
}

impl Default for DailyConfig {
    fn default() -> Self {
        let catalog = Self::parse(DEFAULT_MISSIONS).expect("bundled missions.toml must be valid");
        Self {
            day_secs: 86_400,
            reset_offset_secs: 0,
            slots: 3,
            streak_cap: 7,
            streak_reward: Resources::from_parts(80.0, 40.0, 0.0),
            catalog,
        }
    }
}

#[derive(Deserialize)]
struct File {
    mission: Vec<FileMission>,
}

impl File {
    fn parse(toml: &str) -> Result<Self, DailyError> {
        toml::from_str(toml).map_err(|e| DailyError::Parse(e.to_string()))
    }

    fn into_specs(self) -> Result<Vec<DailySpec>, DailyError> {
        if self.mission.is_empty() {
            return Err(DailyError::EmptyPool);
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for row in self.mission {
            if row.id.is_empty() {
                return Err(DailyError::EmptyId);
            }
            if !seen.insert(row.id.clone()) {
                return Err(DailyError::DuplicateId(row.id));
            }
            if row.name.is_empty() {
                return Err(DailyError::EmptyName(row.id));
            }
            if row.weight == 0 {
                return Err(DailyError::ZeroWeight(row.id));
            }
            if row.target == 0 {
                return Err(DailyError::ZeroTarget(row.id));
            }
            out.push(DailySpec {
                id: row.id,
                name: row.name,
                description: row.description,
                kind: row.kind,
                weight: row.weight,
                target: row.target,
                reward: Resources::from_parts(row.iron, row.silicon, row.deuterium),
                min_lab: row.min_lab,
            });
        }
        Ok(out)
    }
}

#[derive(Deserialize)]
struct FileMission {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    kind: DailyKind,
    weight: u32,
    #[serde(default = "one")]
    target: u32,
    #[serde(default, alias = "metal")]
    iron: f64,
    #[serde(default, alias = "crystal")]
    silicon: f64,
    #[serde(default)]
    deuterium: f64,
    #[serde(default)]
    min_lab: u32,
}

fn one() -> u32 {
    1
}

pub fn mine_kind(kind: BuildingKind) -> bool {
    matches!(
        kind,
        BuildingKind::MetalMine | BuildingKind::CrystalMine | BuildingKind::DeuteriumSynthesizer
    )
}

impl GameState {
    pub fn daily_resets_in(&self) -> u64 {
        self.config.daily.resets_in(self.last_tick_unix)
    }

    pub fn sync_daily(&mut self, now: u64) {
        if self.config.daily.catalog.is_empty() {
            return;
        }
        let day = self.config.daily.day_index(now);
        if self.daily.day_index == Some(day) && !self.daily.slots.is_empty() {
            return;
        }
        if let Some(prev) = self.daily.day_index
            && (day > prev.saturating_add(1)
                || (day == prev.saturating_add(1) && !self.daily.all_complete()))
        {
            self.daily.streak = 0;
        }
        self.daily.day_index = Some(day);
        let lab = self.buildings.research_lab;
        // A raid can only be held with a shipyard, defences or a fleet; an
        // expedition needs ships or a shipyard to build them before reset.
        let can_fight =
            self.buildings.shipyard > 0 || !self.defences.is_empty() || self.fleet.total() > 0;
        let raid = self.config.raid.enabled && can_fight;
        let ships = self.buildings.shipyard > 0 || self.fleet.total() > 0;
        let market = self.config.market.enabled;
        self.daily.slots = self.config.daily.pick_slots(self.galaxy_seed, day, |spec| {
            if spec.min_lab > lab {
                return false;
            }
            match spec.kind {
                DailyKind::WinRaid => raid,
                DailyKind::SendTrade => market,
                DailyKind::SendExpedition => ships,
                _ => true,
            }
        });
    }

    pub fn note_daily(&mut self, action: DailyAction, now: u64) {
        self.sync_daily(now);
        let mut grant = Resources::ZERO;
        let mut streak_grant = Resources::ZERO;
        let mut completed_id = None;
        let mut finished_set = false;
        for slot in &mut self.daily.slots {
            if slot.done() {
                continue;
            }
            let Some(spec) = self.config.daily.get(&slot.spec_id) else {
                continue;
            };
            if !action.matches(spec.kind) {
                continue;
            }
            slot.progress = slot.progress.saturating_add(1).min(slot.target);
            if slot.done() && !slot.rewarded {
                slot.rewarded = true;
                grant.iron += spec.reward.iron;
                grant.silicon += spec.reward.silicon;
                grant.deuterium += spec.reward.deuterium;
                completed_id = Some(spec.name.clone());
            }
        }
        if self.daily.all_complete()
            && self
                .daily
                .slots
                .iter()
                .any(|s| s.rewarded && s.progress == s.target)
        {
            let already = self.daily.streak;
            let cap = self.config.daily.streak_cap.max(1);
            if completed_id.is_some() {
                let next = already.saturating_add(1).min(cap);
                if next > already {
                    self.daily.streak = next;
                    streak_grant = self.config.daily.streak_reward;
                    finished_set = true;
                }
            }
        }
        if grant != Resources::ZERO {
            self.grant_capped(grant);
        }
        if streak_grant != Resources::ZERO {
            let n = f64::from(self.daily.streak);
            self.grant_capped(Resources::from_parts(
                streak_grant.iron * n,
                streak_grant.silicon * n,
                streak_grant.deuterium * n,
            ));
        }
        if let Some(name) = completed_id {
            self.push_event(now, format!("daily: {name} complete"));
        }
        if finished_set {
            self.push_event(now, format!("daily: streak {}", self.daily.streak));
        }
    }

    fn grant_capped(&mut self, add: Resources) {
        for kind in crate::state::ResourceKind::ALL {
            let cap = self.storage_cap(kind);
            let current = self.resources.get(kind);
            self.resources
                .set(kind, (current + add.get(kind)).min(cap).max(current));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::clock::{Clock, FakeClock};
    use crate::state::GameState;

    #[test]
    fn bundled_catalog_has_five_validated_rows() {
        let cfg = DailyConfig::default();
        assert_eq!(cfg.catalog.len(), 5);
        assert_eq!(cfg.slots, 3);
    }

    #[test]
    fn same_seed_and_day_repeat() {
        let cfg = DailyConfig::default();
        let a = cfg.pick_slots(7, 10, |_| true);
        let b = cfg.pick_slots(7, 10, |_| true);
        assert_eq!(a, b);
        let c = cfg.pick_slots(8, 10, |_| true);
        assert_ne!(a, c);
        assert_eq!(a.len(), 3);
        let ids: std::collections::BTreeSet<_> = a.iter().map(|s| s.spec_id.as_str()).collect();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn catch_up_counts_a_finished_mine() {
        let mut cfg = BalanceConfig::default();
        cfg.daily.day_secs = 86_400;
        cfg.starting_resources.iron = 5_000.0;
        cfg.starting_resources.silicon = 5_000.0;
        cfg.starting_resources.deuterium = 5_000.0;
        let mut state = GameState::with_seed(0, cfg, 1);
        state.sync_daily(0);
        let had = state
            .daily
            .slots
            .iter()
            .any(|s| s.spec_id == "upgrade_mine");
        if !had {
            state.daily.slots.insert(
                0,
                DailySlot {
                    spec_id: "upgrade_mine".into(),
                    progress: 0,
                    target: 1,
                    rewarded: false,
                },
            );
        }
        state.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        let clock = FakeClock::new(10_000);
        state.catch_up(clock.now_unix());
        let slot = state
            .daily
            .slots
            .iter()
            .find(|s| s.spec_id == "upgrade_mine")
            .unwrap();
        assert!(slot.done(), "{slot:?}");
        assert!(
            state
                .events
                .iter()
                .any(|e| e.text.contains("daily: Upgrade a mine")),
            "{:?}",
            state.events
        );
    }

    #[test]
    fn day_zero_offers_only_reachable_goals() {
        let mut state = GameState::new(0);
        state.daily = Default::default();
        state.sync_daily(0);
        let ids: Vec<&str> = state
            .daily
            .slots
            .iter()
            .map(|s| s.spec_id.as_str())
            .collect();
        assert!(!ids.contains(&"win_raid"), "{ids:?}");
        assert!(!ids.contains(&"send_expedition"), "{ids:?}");
        assert!(!ids.contains(&"finish_research"), "{ids:?}");
        assert!(!ids.is_empty());
    }

    #[test]
    fn missed_day_breaks_the_streak() {
        let mut cfg = BalanceConfig::default();
        cfg.daily.day_secs = 100;
        cfg.daily.streak_cap = 5;
        let mut state = GameState::with_seed(0, cfg, 1);
        state.sync_daily(0);
        state.daily.streak = 3;
        for slot in &mut state.daily.slots {
            slot.progress = slot.target;
            slot.rewarded = true;
        }
        state.sync_daily(250);
        assert_eq!(state.daily.streak, 0);
        assert_eq!(state.daily.day_index, Some(2));
    }

    #[test]
    fn parse_rejects_a_duplicate_id() {
        let err = DailyConfig::parse(
            r#"
[[mission]]
id = "a"
name = "A"
kind = "upgrade_mine"
weight = 1
[[mission]]
id = "a"
name = "B"
kind = "win_raid"
weight = 1
"#,
        )
        .unwrap_err();
        assert!(matches!(err, DailyError::DuplicateId(id) if id == "a"));
    }
}
