//! hoshi-core: game rules, state, timers and offline progression.

mod activity;
mod balance;
mod buildings;
mod chains;
mod clock;
mod combat;
mod defences;
mod diplomacy;
mod espionage;
mod expedition;
mod fleets;
mod galaxy;
mod market;
mod outposts;
mod planets;
mod prestige;
mod research;
mod save;
mod ships;
mod simulate;
mod state;
mod story;
mod wonders;

pub use activity::{
    ActivityApply, ActivityChannel, ActivityChannelConfig, ActivityEffectsConfig, ActivityPulse,
    ActivityTracker,
};
pub use balance::{
    BalanceConfig, BalanceError, BuildingCatalog, BuildingSpec, EnergyProducerSpec, PacingTargets,
    ProducerSpec, RaidConfig, StorageSpec,
};
pub use buildings::{BuildError, BuildJob, BuildingKind, Buildings};
pub use chains::{ChainConfig, GoodKind, GoodSpec, Recipe, component_kind};
pub use clock::{Clock, FakeClock, SystemClock};
pub use combat::{
    CombatReport, CombatWinner, GroundReport, next_u64, plunder_from, rapid_fire_table,
    resolve_combat, resolve_ground,
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
    GALAXY_GENERATOR_VERSION, encode_galaxy_seed, parse_galaxy_seed, supports_galaxy_generator,
};
pub use market::{MarketConfig, MarketError, MarketState, TradeQuote};
pub use outposts::{
    Outpost, OutpostConfig, RarePlanetBonus, generate_outposts, scaled_defence, scaled_garrison,
};
pub use planets::{Colony, PlanetType, PlanetTypeBonus, PlanetTypeCatalog, planet_profile};
pub use prestige::{
    PRESTIGE_KEPT, PRESTIGE_LOST, Prestige, PrestigeBonusSpec, PrestigeConfig, PrestigeError,
    PrestigePreview, RunStats,
};
pub use research::{
    BuildingPrereq, ModifierKind, PrereqGap, ResearchError, ResearchJob, TechModifier, TechPrereq,
    TechSpec, TechTree, TechTreeError,
};
pub use save::{
    SCHEMA_VERSION, SaveError, SaveGlance, SaveLock, default_save_dir, default_save_path,
    inspect_save, load_or_new, load_or_new_with_seed, load_state, lock_save, open_save,
    resolve_save_path, save_is_locked, save_lock_path, save_state,
};
pub use ships::{
    Fleet, SHIP_KINDS, ShipCatalog, ShipCatalogError, ShipError, ShipJob, ShipKind, ShipSpec,
    ShipStats,
};
pub use simulate::{DaySnapshot, PrestigeSimRow, SimReport, simulate_greedy};
pub use state::{GameEvent, GameState, ResourceKind, Resources};
pub use story::{PendingStory, StoryCatalog, StoryChoice, StoryError, StoryEvent};
pub use wonders::{WonderError, WonderJob, WonderModifier, WonderSpec, WonderStage};
