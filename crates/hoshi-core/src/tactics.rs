//! Formations, stances, terrain and admirals on top of the fleet resolver.

use crate::combat::{CombatModifiers, CombatReport, RoundAction, resolve_combat_with};
use crate::defences::DefenceKind;
use crate::ships::{Fleet, SHIP_KINDS, ShipKind, ShipStats};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_TACTICS: &str = include_str!("../data/tactics.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Formation {
    Neutral,
    Wedge,
    Wall,
    Screen,
}

impl Formation {
    pub const ALL: [Formation; 4] = [
        Formation::Neutral,
        Formation::Wedge,
        Formation::Wall,
        Formation::Screen,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            Formation::Neutral => "neutral",
            Formation::Wedge => "wedge",
            Formation::Wall => "wall",
            Formation::Screen => "screen",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "neutral" => Formation::Neutral,
            "wedge" => Formation::Wedge,
            "wall" => Formation::Wall,
            "screen" => Formation::Screen,
            _ => return None,
        })
    }
}

impl fmt::Display for Formation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stance {
    Neutral,
    Aggressive,
    Defensive,
    HitAndRun,
}

impl Stance {
    pub const ALL: [Stance; 4] = [
        Stance::Neutral,
        Stance::Aggressive,
        Stance::Defensive,
        Stance::HitAndRun,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            Stance::Neutral => "neutral",
            Stance::Aggressive => "aggressive",
            Stance::Defensive => "defensive",
            Stance::HitAndRun => "hit_and_run",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "neutral" => Stance::Neutral,
            "aggressive" => Stance::Aggressive,
            "defensive" => Stance::Defensive,
            "hit_and_run" => Stance::HitAndRun,
            _ => return None,
        })
    }
}

impl fmt::Display for Stance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Terrain {
    Open,
    Nebula,
    Asteroid,
    GravityWell,
}

impl Terrain {
    pub fn as_key(self) -> &'static str {
        match self {
            Terrain::Open => "open",
            Terrain::Nebula => "nebula",
            Terrain::Asteroid => "asteroid",
            Terrain::GravityWell => "gravity_well",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "open" => Terrain::Open,
            "nebula" => Terrain::Nebula,
            "asteroid" => Terrain::Asteroid,
            "gravity_well" => Terrain::GravityWell,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipRole {
    None,
    Fighter,
    Bomber,
    Capital,
}

impl ShipRole {
    fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "none" => ShipRole::None,
            "fighter" => ShipRole::Fighter,
            "bomber" => ShipRole::Bomber,
            "capital" => ShipRole::Capital,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdmiralTrait {
    pub id: String,
    pub name: String,
    pub attack: f64,
    pub shield: f64,
    pub xp: u32,
    pub portrait: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Admiral {
    pub id: u64,
    pub name: String,
    pub trait_id: String,
    pub xp: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FleetTactics {
    pub formation: Formation,
    pub stance: Stance,
    pub retreat_below: f64,
}

impl Default for FleetTactics {
    fn default() -> Self {
        Self {
            formation: Formation::Neutral,
            stance: Stance::Neutral,
            retreat_below: 0.4,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TacticsConfig {
    pub min_rounds: u8,
    pub max_rounds: u8,
    pub hit_and_run_after: u8,
    pub default_retreat_below: f64,
    pub xp_per_battle: u32,
    pub xp_on_win: u32,
    pub xp_trait_cap: u32,
    pub spy_tactics_tier: String,
    formations: BTreeMap<Formation, StatMod>,
    stances: BTreeMap<Stance, StanceMod>,
    terrain: BTreeMap<&'static str, TerrainMod>,
    ship_roles: [ShipRole; SHIP_KINDS],
    defence_roles: BTreeMap<DefenceKind, ShipRole>,
    rps: RpsTable,
    pub traits: Vec<AdmiralTrait>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StatMod {
    attack: f64,
    shield: f64,
    hull: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StanceMod {
    attack: f64,
    shield: f64,
    hull: f64,
    accuracy: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TerrainMod {
    accuracy: f64,
    small_incoming: f64,
    retreat_ok: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RpsTable {
    fighter_vs_bomber: f64,
    fighter_vs_capital: f64,
    bomber_vs_capital: f64,
    bomber_vs_fighter: f64,
    capital_vs_fighter: f64,
    capital_vs_bomber: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TacticsError {
    Parse(String),
    Unknown(String),
}

impl fmt::Display for TacticsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TacticsError::Parse(msg) | TacticsError::Unknown(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for TacticsError {}

impl TacticsConfig {
    pub fn parse(toml: &str) -> Result<Self, TacticsError> {
        let file: File = toml::from_str(toml).map_err(|e| TacticsError::Parse(e.to_string()))?;
        file.into_config()
    }

    pub fn modifiers(
        &self,
        formation: Formation,
        stance: Stance,
        terrain: Terrain,
        admiral: Option<&Admiral>,
        enable_rps: bool,
    ) -> CombatModifiers {
        let form = self.formations.get(&formation).copied().unwrap_or(StatMod {
            attack: 1.0,
            shield: 1.0,
            hull: 1.0,
        });
        let st = self.stances.get(&stance).copied().unwrap_or(StanceMod {
            attack: 1.0,
            shield: 1.0,
            hull: 1.0,
            accuracy: 1.0,
        });
        let terr = self
            .terrain
            .get(terrain.as_key())
            .copied()
            .unwrap_or(TerrainMod {
                accuracy: 1.0,
                small_incoming: 1.0,
                retreat_ok: 1.0,
            });
        let mut attack = form.attack * st.attack;
        let mut shield = form.shield * st.shield;
        let hull = form.hull * st.hull;
        if let Some(adm) = admiral
            && let Some(tr) = self.trait_by_id(&adm.trait_id)
        {
            attack *= tr.attack;
            shield *= tr.shield;
        }
        let mut rps = [[1.0; SHIP_KINDS]; SHIP_KINDS];
        let mut small = [false; SHIP_KINDS];
        if enable_rps {
            for shooter in ShipKind::ALL {
                for target in ShipKind::ALL {
                    rps[shooter.index()][target.index()] = self.rps_mult(
                        self.ship_roles[shooter.index()],
                        self.ship_roles[target.index()],
                    );
                }
                small[shooter.index()] =
                    matches!(self.ship_roles[shooter.index()], ShipRole::Fighter);
            }
        }
        CombatModifiers {
            attack,
            shield,
            hull,
            accuracy: st.accuracy * terr.accuracy,
            small_incoming: terr.small_incoming,
            rps,
            small,
        }
    }

    pub fn trait_by_id(&self, id: &str) -> Option<&AdmiralTrait> {
        self.traits.iter().find(|t| t.id == id)
    }

    pub fn next_trait(&self, xp: u32, current: &str) -> String {
        let mut best = current.to_string();
        let mut best_xp = self.trait_by_id(current).map(|t| t.xp).unwrap_or(0);
        for tr in &self.traits {
            if tr.xp > best_xp && xp >= tr.xp && tr.xp <= self.xp_trait_cap {
                best = tr.id.clone();
                best_xp = tr.xp;
            }
        }
        best
    }

    pub fn retreat_allowed(&self, terrain: Terrain, rng: &mut u64) -> bool {
        let terr = self
            .terrain
            .get(terrain.as_key())
            .copied()
            .unwrap_or(TerrainMod {
                accuracy: 1.0,
                small_incoming: 1.0,
                retreat_ok: 1.0,
            });
        if terr.retreat_ok >= 1.0 - 1e-12 {
            return true;
        }
        let u = (crate::next_u64(rng) as f64) / (u64::MAX as f64);
        u < terr.retreat_ok.clamp(0.0, 1.0)
    }

    fn rps_mult(&self, shooter: ShipRole, target: ShipRole) -> f64 {
        match (shooter, target) {
            (ShipRole::Fighter, ShipRole::Bomber) => self.rps.fighter_vs_bomber,
            (ShipRole::Fighter, ShipRole::Capital) => self.rps.fighter_vs_capital,
            (ShipRole::Bomber, ShipRole::Capital) => self.rps.bomber_vs_capital,
            (ShipRole::Bomber, ShipRole::Fighter) => self.rps.bomber_vs_fighter,
            (ShipRole::Capital, ShipRole::Fighter) => self.rps.capital_vs_fighter,
            (ShipRole::Capital, ShipRole::Bomber) => self.rps.capital_vs_bomber,
            _ => 1.0,
        }
    }
}

impl Default for TacticsConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_TACTICS).expect("bundled tactics.toml must be valid")
    }
}

/// In-progress fight waiting for a round decision. Not stored; save auto-finishes it.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTactical {
    pub seed: u64,
    pub attacker: Fleet,
    pub defender: Fleet,
    pub reserves: Fleet,
    pub atk_plan: TacticalPlan,
    pub def_plan: TacticalPlan,
    pub actions: Vec<RoundAction>,
    pub next_at: u64,
    pub queued: Option<RoundAction>,
}

/// Player-facing plan for one side of a fight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TacticalPlan {
    pub formation: Formation,
    pub stance: Stance,
    pub terrain: Terrain,
}

impl Default for TacticalPlan {
    fn default() -> Self {
        Self {
            formation: Formation::Neutral,
            stance: Stance::Neutral,
            terrain: Terrain::Open,
        }
    }
}

pub fn auto_action(
    before: Fleet,
    now: Fleet,
    reserves: Fleet,
    tactics: FleetTactics,
    stance: Stance,
    round: u8,
    cfg: &TacticsConfig,
) -> RoundAction {
    if stance == Stance::HitAndRun && round >= cfg.hit_and_run_after {
        return RoundAction::Retreat;
    }
    let start = before.total() as f64;
    let live = now.total() as f64;
    if start > 0.0 && live / start <= tactics.retreat_below {
        return RoundAction::Retreat;
    }
    if !reserves.is_empty() && start > 0.0 && live / start <= 0.7 {
        return RoundAction::CommitReserves;
    }
    RoundAction::Hold
}

#[allow(clippy::too_many_arguments)]
pub fn resolve_tactical(
    attacker: Fleet,
    defender: Fleet,
    attacker_stats: &[ShipStats; SHIP_KINDS],
    defender_stats: &[ShipStats; SHIP_KINDS],
    rapid_fire: &[[u32; SHIP_KINDS]; SHIP_KINDS],
    rng: &mut u64,
    cfg: &TacticsConfig,
    atk_plan: TacticalPlan,
    def_plan: TacticalPlan,
    def_tactics: FleetTactics,
    atk_admiral: Option<&Admiral>,
    def_admiral: Option<&Admiral>,
    actions: &[RoundAction],
    reserves: Fleet,
) -> CombatReport {
    let enable_rps = atk_plan.formation != Formation::Neutral
        || def_plan.formation != Formation::Neutral
        || atk_plan.stance != Stance::Neutral
        || def_plan.stance != Stance::Neutral;
    let atk_mod = cfg.modifiers(
        atk_plan.formation,
        atk_plan.stance,
        atk_plan.terrain,
        atk_admiral,
        enable_rps,
    );
    let def_mod = cfg.modifiers(
        def_plan.formation,
        def_plan.stance,
        def_plan.terrain,
        def_admiral,
        enable_rps,
    );
    let cap = cfg.max_rounds.max(cfg.min_rounds);
    let mut used = actions.to_vec();
    if used.is_empty() {
        used = vec![RoundAction::Hold; cap as usize];
        if def_plan.stance == Stance::HitAndRun
            && let Some(slot) = used.get_mut(cfg.hit_and_run_after.saturating_sub(1) as usize)
        {
            *slot = RoundAction::Retreat;
        }
        let _ = def_tactics;
    }
    let play = if actions.is_empty() {
        cap
    } else {
        (used.len() as u8).clamp(1, cap)
    };
    resolve_combat_with(
        attacker,
        defender,
        attacker_stats,
        defender_stats,
        play,
        rapid_fire,
        rng,
        &atk_mod,
        &def_mod,
        &used,
        reserves,
    )
}

#[derive(Deserialize)]
struct File {
    min_rounds: u8,
    max_rounds: u8,
    hit_and_run_after: u8,
    default_retreat_below: f64,
    xp_per_battle: u32,
    xp_on_win: u32,
    xp_trait_cap: u32,
    spy_tactics_tier: String,
    rps: FileRps,
    formation: BTreeMap<String, FileStat>,
    stance: BTreeMap<String, FileStance>,
    terrain: BTreeMap<String, FileTerrain>,
    role: BTreeMap<String, String>,
    #[serde(rename = "trait", default)]
    traits: Vec<FileTrait>,
}

#[derive(Deserialize)]
struct FileRps {
    fighter_vs_bomber: f64,
    fighter_vs_capital: f64,
    bomber_vs_capital: f64,
    bomber_vs_fighter: f64,
    capital_vs_fighter: f64,
    capital_vs_bomber: f64,
}

#[derive(Deserialize)]
struct FileStat {
    attack: f64,
    shield: f64,
    hull: f64,
}

#[derive(Deserialize)]
struct FileStance {
    attack: f64,
    shield: f64,
    hull: f64,
    accuracy: f64,
}

#[derive(Deserialize)]
struct FileTerrain {
    accuracy: f64,
    small_incoming: f64,
    retreat_ok: f64,
}

#[derive(Deserialize)]
struct FileTrait {
    id: String,
    name: String,
    attack: f64,
    shield: f64,
    xp: u32,
    portrait: String,
}

impl File {
    fn into_config(self) -> Result<TacticsConfig, TacticsError> {
        let mut formations = BTreeMap::new();
        for (key, row) in self.formation {
            let id = Formation::from_key(&key)
                .ok_or_else(|| TacticsError::Unknown(format!("formation {key}")))?;
            formations.insert(
                id,
                StatMod {
                    attack: row.attack,
                    shield: row.shield,
                    hull: row.hull,
                },
            );
        }
        let mut stances = BTreeMap::new();
        for (key, row) in self.stance {
            let id = Stance::from_key(&key)
                .ok_or_else(|| TacticsError::Unknown(format!("stance {key}")))?;
            stances.insert(
                id,
                StanceMod {
                    attack: row.attack,
                    shield: row.shield,
                    hull: row.hull,
                    accuracy: row.accuracy,
                },
            );
        }
        let mut terrain = BTreeMap::new();
        for (key, row) in self.terrain {
            if Terrain::from_key(&key).is_none() {
                return Err(TacticsError::Unknown(format!("terrain {key}")));
            }
            let leaked: &'static str = match key.as_str() {
                "open" => "open",
                "nebula" => "nebula",
                "asteroid" => "asteroid",
                "gravity_well" => "gravity_well",
                _ => return Err(TacticsError::Unknown(format!("terrain {key}"))),
            };
            terrain.insert(
                leaked,
                TerrainMod {
                    accuracy: row.accuracy,
                    small_incoming: row.small_incoming,
                    retreat_ok: row.retreat_ok,
                },
            );
        }
        let mut ship_roles = [ShipRole::None; SHIP_KINDS];
        let mut defence_roles = BTreeMap::new();
        for (key, role) in self.role {
            let parsed = ShipRole::from_key(&role)
                .ok_or_else(|| TacticsError::Unknown(format!("role {role}")))?;
            if let Some(kind) = ShipKind::from_key(&key) {
                ship_roles[kind.index()] = parsed;
            } else if let Some(kind) = DefenceKind::from_key(&key) {
                defence_roles.insert(kind, parsed);
            } else {
                return Err(TacticsError::Unknown(format!("role target {key}")));
            }
        }
        if self.max_rounds < self.min_rounds || self.min_rounds == 0 {
            return Err(TacticsError::Unknown("tactics round bounds".into()));
        }
        Ok(TacticsConfig {
            min_rounds: self.min_rounds,
            max_rounds: self.max_rounds,
            hit_and_run_after: self.hit_and_run_after.max(1),
            default_retreat_below: self.default_retreat_below.clamp(0.0, 1.0),
            xp_per_battle: self.xp_per_battle,
            xp_on_win: self.xp_on_win,
            xp_trait_cap: self.xp_trait_cap,
            spy_tactics_tier: self.spy_tactics_tier,
            formations,
            stances,
            terrain,
            ship_roles,
            defence_roles,
            rps: RpsTable {
                fighter_vs_bomber: self.rps.fighter_vs_bomber,
                fighter_vs_capital: self.rps.fighter_vs_capital,
                bomber_vs_capital: self.rps.bomber_vs_capital,
                bomber_vs_fighter: self.rps.bomber_vs_fighter,
                capital_vs_fighter: self.rps.capital_vs_fighter,
                capital_vs_bomber: self.rps.capital_vs_bomber,
            },
            traits: self
                .traits
                .into_iter()
                .map(|t| AdmiralTrait {
                    id: t.id,
                    name: t.name,
                    attack: t.attack,
                    shield: t.shield,
                    xp: t.xp,
                    portrait: t.portrait,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShipCatalog;
    use crate::combat::stats_from_catalog;

    fn lf(n: u32) -> Fleet {
        let mut f = Fleet::default();
        f.add(ShipKind::LightFighter, n);
        f
    }

    #[test]
    fn bundled_tactics_parse() {
        let cfg = TacticsConfig::default();
        assert!(cfg.max_rounds >= 3);
        assert!(cfg.trait_by_id("flanker").is_some());
        assert_eq!(cfg.defence_roles.len(), 8);
    }

    #[test]
    fn neutral_plan_matches_plain_combat() {
        let cfg = TacticsConfig::default();
        let stats = stats_from_catalog(&ShipCatalog::default());
        let mut a = 9;
        let mut b = 9;
        let plain = crate::resolve_combat(
            lf(4),
            lf(4),
            &stats,
            &stats,
            cfg.max_rounds,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut a,
        );
        let tactical = resolve_tactical(
            lf(4),
            lf(4),
            &stats,
            &stats,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut b,
            &cfg,
            TacticalPlan::default(),
            TacticalPlan::default(),
            FleetTactics::default(),
            None,
            None,
            &[],
            Fleet::default(),
        );
        assert_eq!(plain, tactical);
    }

    #[test]
    fn wedge_changes_a_non_neutral_fight() {
        let cfg = TacticsConfig::default();
        let stats = stats_from_catalog(&ShipCatalog::default());
        let mut mixed = Fleet::default();
        mixed.add(ShipKind::LightFighter, 10);
        mixed.add(ShipKind::Cruiser, 4);
        mixed.add(ShipKind::Battleship, 2);
        let mut a = 2;
        let mut b = 2;
        let plan = TacticalPlan {
            formation: Formation::Wedge,
            ..TacticalPlan::default()
        };
        let wedge_mod = cfg.modifiers(Formation::Wedge, Stance::Neutral, Terrain::Open, None, true);
        assert!(wedge_mod.attack > 1.0);
        let neutral = resolve_tactical(
            mixed,
            mixed,
            &stats,
            &stats,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut a,
            &cfg,
            TacticalPlan::default(),
            TacticalPlan::default(),
            FleetTactics::default(),
            None,
            None,
            &[],
            Fleet::default(),
        );
        let wedge = resolve_tactical(
            mixed,
            mixed,
            &stats,
            &stats,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut b,
            &cfg,
            plan,
            TacticalPlan::default(),
            FleetTactics::default(),
            None,
            None,
            &[],
            Fleet::default(),
        );
        assert_ne!(neutral.attacker_after, wedge.attacker_after);
    }

    #[test]
    fn auto_retreats_below_the_threshold() {
        let cfg = TacticsConfig::default();
        let mut before = lf(10);
        let now = lf(3);
        let tactics = FleetTactics {
            retreat_below: 0.4,
            ..FleetTactics::default()
        };
        assert_eq!(
            auto_action(
                before,
                now,
                Fleet::default(),
                tactics,
                Stance::Neutral,
                1,
                &cfg
            ),
            RoundAction::Retreat
        );
        before = lf(10);
        assert_eq!(
            auto_action(
                before,
                lf(9),
                Fleet::default(),
                tactics,
                Stance::Neutral,
                1,
                &cfg
            ),
            RoundAction::Hold
        );
    }

    #[test]
    fn scripted_retreat_is_deterministic() {
        let cfg = TacticsConfig::default();
        let stats = stats_from_catalog(&ShipCatalog::default());
        let actions = [RoundAction::Hold, RoundAction::Retreat];
        let mut a = 3;
        let mut b = 3;
        let left = resolve_tactical(
            lf(8),
            lf(8),
            &stats,
            &stats,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut a,
            &cfg,
            TacticalPlan::default(),
            TacticalPlan::default(),
            FleetTactics::default(),
            None,
            None,
            &actions,
            Fleet::default(),
        );
        let right = resolve_tactical(
            lf(8),
            lf(8),
            &stats,
            &stats,
            &[[0; SHIP_KINDS]; SHIP_KINDS],
            &mut b,
            &cfg,
            TacticalPlan::default(),
            TacticalPlan::default(),
            FleetTactics::default(),
            None,
            None,
            &actions,
            Fleet::default(),
        );
        assert_eq!(left, right);
        assert_eq!(left.winner, crate::CombatWinner::Draw);
        assert_eq!(left.rounds, 2);
    }
}
