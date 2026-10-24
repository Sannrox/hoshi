//! Galaxy victory: conquest, wonder, or diplomacy. Checked in core, including catch-up.

use crate::fleets::Coord;
use crate::state::GameState;
use std::fmt;

/// How this galaxy ended. Exactly one path wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VictoryPath {
    Conquest,
    Wonder,
    Diplomacy,
}

impl VictoryPath {
    pub fn as_key(self) -> &'static str {
        match self {
            VictoryPath::Conquest => "conquest",
            VictoryPath::Wonder => "wonder",
            VictoryPath::Diplomacy => "diplomacy",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "conquest" => Some(Self::Conquest),
            "wonder" => Some(Self::Wonder),
            "diplomacy" => Some(Self::Diplomacy),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VictoryPath::Conquest => "conquest",
            VictoryPath::Wonder => "wonder",
            VictoryPath::Diplomacy => "diplomacy",
        }
    }
}

/// Recorded once per galaxy. Prestige reset starts a new galaxy.
#[derive(Debug, Clone, PartialEq)]
pub struct VictoryRecord {
    pub path: VictoryPath,
    pub at: u64,
    pub prestige: u32,
    pub elapsed_secs: u64,
    pub kept_playing: bool,
}

/// Live progress for the TUI goals panel.
#[derive(Debug, Clone, PartialEq)]
pub struct VictoryProgress {
    pub conquest: PathProgress,
    pub wonder: PathProgress,
    pub diplomacy: PathProgress,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathProgress {
    pub ready: bool,
    pub next_step: String,
    pub ratio: f64,
}

/// Balance knobs for victory and the pirate homeworld.
#[derive(Debug, Clone, PartialEq)]
pub struct VictoryConfig {
    pub enabled: bool,
    pub wonder_id: String,
    pub allied_min: f64,
    pub hold_secs: u64,
    pub base_prestige: u32,
    pub conquest_bonus: u32,
    pub wonder_bonus: u32,
    pub diplomacy_bonus: u32,
    pub speed_bonus_max: u32,
    pub speed_target_secs: u64,
    pub casual_min_secs: u64,
    pub casual_max_secs: u64,
    pub homeworld_min_system: u16,
    pub homeworld_defence_mult: f64,
    pub homeworld_garrison_mult: f64,
}

impl VictoryConfig {
    pub fn inactive() -> Self {
        Self {
            enabled: false,
            ..Self::default_active()
        }
    }

    pub fn default_active() -> Self {
        Self {
            enabled: true,
            wonder_id: "dyson_ring".into(),
            allied_min: 40.0,
            hold_secs: 86_400,
            base_prestige: 20,
            conquest_bonus: 8,
            wonder_bonus: 6,
            diplomacy_bonus: 10,
            speed_bonus_max: 8,
            speed_target_secs: 1_209_600,
            casual_min_secs: 1_209_600,
            casual_max_secs: 2_419_200,
            homeworld_min_system: 40,
            homeworld_defence_mult: 3.0,
            homeworld_garrison_mult: 3.0,
        }
    }

    pub fn award(&self, path: VictoryPath, elapsed_secs: u64) -> u32 {
        let path_bonus = match path {
            VictoryPath::Conquest => self.conquest_bonus,
            VictoryPath::Wonder => self.wonder_bonus,
            VictoryPath::Diplomacy => self.diplomacy_bonus,
        };
        let speed = if self.speed_target_secs == 0 || elapsed_secs >= self.speed_target_secs {
            0
        } else {
            let frac = 1.0 - (elapsed_secs as f64 / self.speed_target_secs as f64);
            ((frac * f64::from(self.speed_bonus_max)).round() as u32).min(self.speed_bonus_max)
        };
        self.base_prestige
            .saturating_add(path_bonus)
            .saturating_add(speed)
    }
}

impl Default for VictoryConfig {
    fn default() -> Self {
        Self::default_active()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VictoryError {
    Disabled,
    AlreadyWon,
}

impl fmt::Display for VictoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VictoryError::Disabled => write!(f, "victory is disabled"),
            VictoryError::AlreadyWon => write!(f, "this galaxy already ended"),
        }
    }
}

impl std::error::Error for VictoryError {}

impl GameState {
    pub fn seed_pirate_home(&mut self, now: u64) {
        if let Some(existing) = self.outposts.iter().find(|o| o.homeworld) {
            self.pirate_home = existing.coord;
            return;
        }
        let min_sys = self.config.victory.homeworld_min_system;
        let idx = self
            .outposts
            .iter()
            .enumerate()
            .filter(|(_, o)| o.coord.system >= min_sys)
            .max_by_key(|(_, o)| (o.coord.system, o.coord.position))
            .map(|(i, _)| i);
        if let Some(i) = idx {
            self.pirate_home = self.outposts[i].coord;
            if !self.config.victory.enabled {
                return;
            }
            self.outposts[i].homeworld = true;
            let def_mult = self.config.victory.homeworld_defence_mult.max(1.0);
            let gar_mult = self.config.victory.homeworld_garrison_mult.max(1.0);
            let n = ((self.outposts[i].defences.get(crate::ShipKind::LightFighter) as f64)
                * def_mult)
                .ceil()
                .max(1.0) as u32;
            let mut defences = crate::Fleet::default();
            defences.add(crate::ShipKind::LightFighter, n);
            self.outposts[i].defences = defences;
            self.outposts[i].garrison = ((self.outposts[i].garrison as f64) * gar_mult)
                .ceil()
                .max(1.0) as u32;
            return;
        }
        if !self.config.victory.enabled {
            self.pirate_home = Coord {
                galaxy: 1,
                system: 80,
                position: 16,
            };
            return;
        }
        let mut occupied: Vec<Coord> = vec![self.home];
        occupied.extend(self.colonies.iter().map(|c| c.coord));
        let fortress = crate::generate_homeworld(
            self.galaxy_seed,
            now,
            &self.config.outposts,
            &occupied,
            self.generator_version,
            crate::outposts::HomeworldSpec {
                min_system: self.config.victory.homeworld_min_system,
                defence_mult: self.config.victory.homeworld_defence_mult,
                garrison_mult: self.config.victory.homeworld_garrison_mult,
            },
        );
        self.pirate_home = fortress.coord;
        self.outposts.push(fortress);
    }

    pub fn pirate_homeworld(&self) -> Option<Coord> {
        if !self.config.victory.enabled {
            return None;
        }
        Some(self.pirate_home)
    }

    pub fn victory_progress(&self) -> VictoryProgress {
        let cfg = &self.config.victory;
        if !cfg.enabled {
            let off = PathProgress {
                ready: false,
                next_step: "victory is off".into(),
                ratio: 0.0,
            };
            return VictoryProgress {
                conquest: off.clone(),
                wonder: off.clone(),
                diplomacy: off,
            };
        }
        let home = self.pirate_homeworld();
        let conquered = home.is_some_and(|coord| self.colonies.iter().any(|c| c.coord == coord));
        let conquest_next = if conquered {
            "pirate homeworld captured".into()
        } else if let Some(coord) = home {
            format!("invade pirate homeworld {coord}")
        } else {
            "scout the pirate homeworld".into()
        };
        let wonder_done = self.completed_wonders.iter().any(|id| id == &cfg.wonder_id);
        let wonder = self.display_name(&cfg.wonder_id);
        let wonder_next = if wonder_done {
            format!("{wonder} complete")
        } else if self.wonder.as_ref().is_some_and(|j| j.id == cfg.wonder_id) {
            format!("finish {wonder}")
        } else {
            format!("start {wonder}")
        };
        let allied = self.all_factions_allied();
        let diplomacy_next = if allied {
            if let Some(since) = self.allied_since {
                let ready_at = since.saturating_add(cfg.hold_secs);
                if self.last_tick_unix >= ready_at {
                    "alliance hold complete".into()
                } else {
                    format!(
                        "hold alliance {}s",
                        ready_at.saturating_sub(self.last_tick_unix)
                    )
                }
            } else {
                "alliance hold starting".into()
            }
        } else {
            "ally with every faction".into()
        };
        let factions = self.config.diplomacy.iter().count().max(1) as f64;
        let allied_n = self
            .config
            .diplomacy
            .iter()
            .filter(|f| self.diplomacy.standing(&f.id) + 1e-9 >= cfg.allied_min)
            .count() as f64;
        VictoryProgress {
            conquest: PathProgress {
                ready: conquered,
                next_step: conquest_next,
                ratio: if conquered { 1.0 } else { 0.0 },
            },
            wonder: PathProgress {
                ready: wonder_done,
                next_step: wonder_next,
                ratio: if wonder_done { 1.0 } else { 0.0 },
            },
            diplomacy: PathProgress {
                ready: self.diplomacy_held(self.last_tick_unix),
                next_step: diplomacy_next,
                ratio: (allied_n / factions).clamp(0.0, 1.0),
            },
        }
    }

    pub(crate) fn next_victory_at(&self) -> Option<u64> {
        if !self.config.victory.enabled || self.victory.is_some() {
            return None;
        }
        let hold = self
            .allied_since
            .map(|since| since.saturating_add(self.config.victory.hold_secs))?;
        if hold > self.last_tick_unix {
            Some(hold)
        } else {
            None
        }
    }

    pub fn maybe_declare_victory(&mut self, at: u64) {
        if !self.config.victory.enabled || self.victory.is_some() {
            return;
        }
        if self.all_factions_allied() && self.allied_since.is_none() {
            self.allied_since = Some(at);
        }
        if !self.all_factions_allied() {
            self.allied_since = None;
        }
        let path = if self.conquest_complete() {
            VictoryPath::Conquest
        } else if self.wonder_complete() {
            VictoryPath::Wonder
        } else if self.diplomacy_held(at) {
            VictoryPath::Diplomacy
        } else {
            return;
        };
        let started = self.prestige.current.started_at.min(at);
        let elapsed = at.saturating_sub(started);
        let prestige = if self.season.as_ref().is_some_and(|run| !run.prestige) {
            0
        } else {
            self.config.victory.award(path, elapsed)
        };
        self.prestige.currency = self.prestige.currency.saturating_add(prestige);
        self.prestige.lifetime = self.prestige.lifetime.saturating_add(prestige);
        self.victory = Some(VictoryRecord {
            path,
            at,
            prestige,
            elapsed_secs: elapsed,
            kept_playing: false,
        });
        self.push_event(
            at,
            format!(
                "victory: {} in {elapsed}s (+{prestige} prestige)",
                path.label()
            ),
        );
        self.evaluate_achievements(at);
    }

    pub fn keep_playing_after_victory(&mut self) -> Result<(), VictoryError> {
        if !self.config.victory.enabled {
            return Err(VictoryError::Disabled);
        }
        let Some(record) = self.victory.as_mut() else {
            return Err(VictoryError::AlreadyWon);
        };
        record.kept_playing = true;
        Ok(())
    }

    fn conquest_complete(&self) -> bool {
        let Some(home) = self.pirate_homeworld() else {
            return false;
        };
        self.colonies.iter().any(|c| c.coord == home)
            && !self.outposts.iter().any(|o| o.coord == home)
    }

    fn wonder_complete(&self) -> bool {
        self.completed_wonders
            .iter()
            .any(|id| id == &self.config.victory.wonder_id)
    }

    fn all_factions_allied(&self) -> bool {
        let min = self.config.victory.allied_min;
        let mut any = false;
        for faction in self.config.diplomacy.iter() {
            any = true;
            if self.diplomacy.standing(&faction.id) + 1e-9 < min {
                return false;
            }
        }
        any
    }

    fn diplomacy_held(&self, at: u64) -> bool {
        if !self.all_factions_allied() {
            return false;
        }
        match self.allied_since {
            Some(since) => at >= since.saturating_add(self.config.victory.hold_secs),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;
    use crate::Colony;
    use crate::planets::PlanetType;
    use crate::ships::Fleet;

    fn quiet(now: u64) -> GameState {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg.victory.enabled = true;
        cfg.victory.hold_secs = 10;
        cfg.victory.allied_min = 5.0;
        cfg.victory.wonder_id = "dyson_ring".into();
        GameState::with_config(now, cfg)
    }

    #[test]
    fn wonder_path_fires_once_and_awards_prestige() {
        let mut state = quiet(1_000);
        assert!(state.victory.is_none());
        let before = state.prestige.currency;
        state.completed_wonders.push("dyson_ring".into());
        state.maybe_declare_victory(1_000);
        let first = state.victory.clone().expect("victory");
        assert_eq!(first.path, VictoryPath::Wonder);
        assert!(state.prestige.currency > before);
        state.maybe_declare_victory(2_000);
        assert_eq!(state.victory.as_ref().map(|v| v.at), Some(1_000));
    }

    #[test]
    fn conquest_needs_the_homeworld_colony() {
        let mut state = quiet(1_000);
        let home = state.pirate_homeworld().expect("homeworld");
        state.maybe_declare_victory(1_000);
        assert!(state.victory.is_none());
        state.outposts.retain(|o| o.coord != home);
        state.colonies.push(Colony {
            coord: home,
            kind: PlanetType::Rocky,
            size: 8,
            resources: crate::Resources::ZERO,
            buildings: Default::default(),
            fleet: Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        state.maybe_declare_victory(1_050);
        assert_eq!(
            state.victory.as_ref().map(|v| v.path),
            Some(VictoryPath::Conquest)
        );
    }

    #[test]
    fn diplomacy_waits_for_the_hold() {
        let mut state = quiet(1_000);
        for faction in state
            .config
            .diplomacy
            .iter()
            .map(|f| f.id.clone())
            .collect::<Vec<_>>()
        {
            state.diplomacy.set_standing(&faction, 10.0);
        }
        state.maybe_declare_victory(1_000);
        assert!(state.victory.is_none());
        assert_eq!(state.allied_since, Some(1_000));
        state.maybe_declare_victory(1_009);
        assert!(state.victory.is_none());
        state.maybe_declare_victory(1_010);
        assert_eq!(
            state.victory.as_ref().map(|v| v.path),
            Some(VictoryPath::Diplomacy)
        );
    }

    #[test]
    fn homeworld_repeats_for_a_seed() {
        let a = quiet(1_000);
        let b = quiet(1_000);
        assert_eq!(a.pirate_homeworld(), b.pirate_homeworld());
        let coord = a.pirate_homeworld().unwrap();
        assert!(coord.system >= a.config.victory.homeworld_min_system);
        assert_ne!(coord, crate::Coord::HOME);
        assert!(a.outposts.iter().any(|o| o.homeworld && o.coord == coord));
    }

    #[test]
    fn promoting_a_mature_outpost_scales_existing_defences() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg.victory.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        let min_sys = state.config.victory.homeworld_min_system;
        let idx = state
            .outposts
            .iter()
            .enumerate()
            .filter(|(_, o)| o.coord.system >= min_sys)
            .max_by_key(|(_, o)| (o.coord.system, o.coord.position))
            .map(|(i, _)| i)
            .unwrap_or_else(|| {
                state.outposts.push(crate::Outpost {
                    coord: crate::Coord {
                        galaxy: 1,
                        system: min_sys,
                        position: 16,
                    },
                    kind: PlanetType::Rocky,
                    size: 8,
                    defences: Fleet::default(),
                    garrison: 1,
                    rare: None,
                    last_rebuild_at: 1_000,
                    homeworld: false,
                });
                state.outposts.len() - 1
            });
        let mut fleet = Fleet::default();
        fleet.add(crate::ShipKind::LightFighter, 50);
        state.outposts[idx].defences = fleet;
        state.outposts[idx].garrison = 100;
        state.config.victory.enabled = true;
        state.seed_pirate_home(1_000);
        let outpost = &state.outposts[idx];
        assert!(outpost.homeworld);
        assert_eq!(outpost.garrison, 300);
        assert_eq!(outpost.defences.get(crate::ShipKind::LightFighter), 150);
    }

    #[test]
    fn disabled_victory_does_not_mark_a_fortress() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg.victory.enabled = false;
        let mut state = GameState::with_config(1_000, cfg);
        assert!(state.outposts.iter().all(|o| !o.homeworld));
        state.completed_wonders.push("dyson_ring".into());
        let progress = state.victory_progress();
        assert!(!progress.conquest.ready);
        assert!(!progress.wonder.ready);
        assert!(!progress.diplomacy.ready);
        assert_eq!(progress.wonder.next_step, "victory is off");
        let mut on = BalanceConfig::default();
        on.raid.enabled = false;
        on.market.enabled = false;
        on.victory.enabled = true;
        let scaled = GameState::with_config(1_000, on);
        let home = scaled.pirate_homeworld().unwrap();
        let outpost = scaled.outposts.iter().find(|o| o.coord == home).unwrap();
        assert!(outpost.homeworld);
        assert!(outpost.garrison >= 2);
    }

    #[test]
    fn keep_playing_dismisses_the_prompt() {
        let mut state = quiet(1_000);
        state.completed_wonders.push("dyson_ring".into());
        state.maybe_declare_victory(1_000);
        state.keep_playing_after_victory().unwrap();
        assert!(state.victory.as_ref().unwrap().kept_playing);
    }

    #[test]
    fn diplomacy_hold_gap_matches_step_by_step() {
        let factions = {
            let probe = quiet(1_000);
            probe
                .config
                .diplomacy
                .iter()
                .map(|f| f.id.clone())
                .collect::<Vec<_>>()
        };
        let mut gap = quiet(1_000);
        let mut step = quiet(1_000);
        for id in &factions {
            gap.diplomacy.set_standing(id, 10.0);
            step.diplomacy.set_standing(id, 10.0);
        }
        gap.maybe_declare_victory(1_000);
        step.maybe_declare_victory(1_000);
        gap.catch_up(1_010);
        for t in 1_001..=1_010 {
            step.catch_up(t);
        }
        assert_eq!(
            gap.victory.as_ref().map(|v| v.path),
            Some(VictoryPath::Diplomacy)
        );
        assert_eq!(
            gap.victory.as_ref().map(|v| (v.path, v.at)),
            step.victory.as_ref().map(|v| (v.path, v.at))
        );
        assert_eq!(gap.prestige.currency, step.prestige.currency);
    }

    #[test]
    fn tribute_starts_the_alliance_hold() {
        let mut state = quiet(1_000);
        state.resources.iron = 10_000.0;
        let ids: Vec<String> = state
            .config
            .diplomacy
            .iter()
            .map(|f| f.id.clone())
            .collect();
        for id in &ids {
            if id != "pirates" {
                state.diplomacy.set_standing(id, 10.0);
            }
        }
        state.pay_tribute("pirates", 1_000).unwrap();
        assert!(state.diplomacy.standing("pirates") + 1e-9 >= state.config.victory.allied_min);
        assert_eq!(state.allied_since, Some(1_000));
    }
}
