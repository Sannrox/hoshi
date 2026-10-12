//! hoshi-core: game rules, state, timers and offline progression.

mod achievements;
mod activity;
mod advisor;
mod balance;
mod buildings;
mod chains;
mod clock;
mod combat;
mod commentary;
mod daily;
mod defences;
mod diplomacy;
mod espionage;
mod expedition;
mod fleets;
mod galaxy;
mod laws;
mod market;
mod moons;
mod outposts;
mod planets;
mod prestige;
mod production;
mod research;
mod save;
mod ships;
mod simulate;
mod state;
mod story;
mod tutorial;
mod waste;
mod wonders;

pub use achievements::{
    AchievementCondition, AchievementConfig, AchievementError, AchievementSpec, CareerMeters,
};
pub use activity::{
    ActivityApply, ActivityChannel, ActivityChannelConfig, ActivityEffectsConfig, ActivityPulse,
    ActivityTracker,
};
pub use advisor::{Advice, advise, polish_advice, polish_all};
pub use balance::{
    BalanceConfig, BalanceError, BuildingCatalog, BuildingSpec, EnergyProducerSpec, PacingTargets,
    ProducerSpec, RaidConfig, StorageSpec,
};
pub use buildings::{BuildError, BuildJob, BuildingKind, Buildings};
pub use chains::{ChainConfig, GoodKind, GoodSpec, Recipe, component_kind};
pub use clock::{Clock, FakeClock, SystemClock};
pub use combat::{
    CombatFrame, CombatLog, CombatReport, CombatWinner, GroundReport, MAX_COMBAT_LOGS, next_u64,
    plunder_from, rapid_fire_table, resolve_combat, resolve_ground,
};
pub use commentary::{
    CommentaryCatalog, CommentaryError, rephrase_lines, rephrase_lines_allow, rephrase_local,
};
pub use daily::{
    DailyAction, DailyBoard, DailyConfig, DailyError, DailyKind, DailySlot, DailySpec,
};
pub use defences::{
    DEFENCE_KINDS, DefenceConfig, DefenceGrid, DefenceKind, DefenceRole, DefenceSpec, PlanetBattle,
    apply_rebuild, intercept_missiles, resolve_planet_battle,
};
pub use diplomacy::{
    DiplomacyConfig, DiplomacyError, DiplomacyState, FactionSpec, FactionTier, StandingChange,
};
pub use espionage::{
    EspionageConfig, EspionageTier, SpyOutcome, SpyReport, detection_chance, resolve_spy, reveals,
    spy_score, spy_tier,
};
pub use expedition::{
    ExpeditionConfig, ExpeditionKind, ExpeditionResult, ExpeditionWeights, fleet_points,
    roll_expedition, scaled_weights,
};
pub use fleets::{
    Coord, FleetError, FlightConfig, Mission, MissionPhase, TradeRoute, distance, flight_secs,
    fuel_cost,
};
pub use galaxy::{
    GALAXY_GENERATOR_VERSION, GalaxyPreview, encode_galaxy_seed, parse_galaxy_seed, preview_galaxy,
    supports_galaxy_generator,
};
pub use laws::{
    CLOCK as LAW_CLOCK, Jurisdiction, JurisdictionKind, Law, LawAction, LawActionKind, LawApproval,
    LawConfig, LawEffectKind, LawError, LawMetric, LawMetrics, LawSample, LawState, LawStatus,
    LawTemplate, PLAYER as LAW_PLAYER, emission_throttle,
};
pub use market::{MarketConfig, MarketError, MarketState, TradeQuote};
pub use moons::{Moon, MoonConfig, MoonError, MoonFacility, PhalanxContact};
pub use outposts::{
    Outpost, OutpostConfig, RarePlanetBonus, generate_outposts, scaled_defence, scaled_garrison,
};
pub use planets::{Colony, PlanetType, PlanetTypeBonus, PlanetTypeCatalog, planet_profile};
pub use prestige::{
    PRESTIGE_KEPT, PRESTIGE_LOST, Prestige, PrestigeBonusSpec, PrestigeConfig, PrestigeError,
    PrestigePreview, RunStats,
};
pub use production::{LimitingFactor, ProductionReport, RecipeFlow};
pub use research::{
    BuildingPrereq, ModifierKind, PrereqGap, ResearchError, ResearchJob, TechBranch, TechModifier,
    TechNodeState, TechPrereq, TechSpec, TechTree, TechTreeError,
};
pub use save::{
    CareerGlance, SCHEMA_VERSION, SaveError, SaveGlance, SaveLock, default_save_dir,
    default_save_path, inspect_career, inspect_save, load_or_new, load_or_new_with_seed,
    load_state, lock_save, open_save, resolve_save_path, save_is_locked, save_lock_path,
    save_state,
};
pub use ships::{
    Fleet, SHIP_KINDS, ShipCatalog, ShipCatalogError, ShipError, ShipJob, ShipKind, ShipSpec,
    ShipStats,
};
pub use simulate::{
    DaySnapshot, HeavyIndustryReport, PrestigeSimRow, SimReport, heavy_industry_check,
    simulate_greedy,
};
pub use state::{GameEvent, GameState, ResourceKind, Resources};
pub use story::{PendingStory, StoryCatalog, StoryChoice, StoryError, StoryEvent};
pub use tutorial::{
    HintScreen, HintSpec, HintTrigger, TutorialBoard, TutorialCatalog, TutorialError,
};
pub use waste::{WasteConfig, WasteReport, report as waste_report};
pub use wonders::{WonderError, WonderJob, WonderModifier, WonderSpec, WonderStage};
