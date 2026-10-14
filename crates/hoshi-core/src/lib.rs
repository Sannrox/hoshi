//! hoshi-core: game rules, state, timers and offline progression.

mod achievements;
mod activity;
mod advisor;
mod balance;
mod buildings;
mod chains;
mod classification;
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
mod governance;
mod history;
mod intent;
mod laws;
mod leaders;
mod market;
mod moons;
mod ontology;
mod orbits;
mod outposts;
mod planets;
mod planning;
mod prestige;
mod production;
mod research;
mod save;
mod ships;
mod simulate;
mod state;
mod storage;
mod story;
mod tactics;
mod tutorial;
mod victory;
mod waste;
mod wonders;

pub use achievements::{
    AchievementCondition, AchievementConfig, AchievementError, AchievementSpec, CareerMeters,
};
pub use activity::{
    ActivityApply, ActivityChannel, ActivityChannelConfig, ActivityEffectsConfig, ActivityPulse,
    ActivityTracker,
};
pub use advisor::{
    Advice, advise, advisor_endpoint_allowed, polish_advice, polish_all, polish_all_named,
};
pub use balance::{
    BalanceConfig, BalanceError, BuildingCatalog, BuildingSpec, EnergyProducerSpec, PacingTargets,
    ProducerSpec, RaidConfig, StorageSpec,
};
pub use buildings::{BuildError, BuildJob, BuildingKind, Buildings};
pub use chains::{ChainConfig, GoodKind, GoodSpec, Recipe, component_kind};
pub use classification::{
    COMPARTMENT_FLEET, COMPARTMENT_INTEL, COMPARTMENT_PLANET, COMPARTMENT_POLICY, Classification,
    ClassificationBoard, ClassificationError, ClassifiedKind, Clearance, LeakIncident, Marking,
    SharingAgreement, join_compartments, parse_compartment, split_compartments,
};
pub use clock::{Clock, FakeClock, SystemClock};
pub use combat::{
    CombatFrame, CombatLog, CombatModifiers, CombatReport, CombatWinner, GroundReport,
    MAX_COMBAT_LOGS, RoundAction, next_u64, plunder_from, rapid_fire_table, resolve_combat,
    resolve_combat_with, resolve_ground, stats_from_catalog,
};
pub use commentary::{
    CommentaryCatalog, CommentaryError, model_connect_addr, probe_model_endpoint, rephrase_lines,
    rephrase_lines_allow, rephrase_lines_named, rephrase_local,
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
pub use governance::{
    AuditEntry, GovernanceConfig, GovernanceLedger, InvasionApproval, ModelTier, ModelUse,
    PolicyBlock, check_move, dest_of, recorded_response, route_model, spend_of, take_budget,
};
pub use history::{
    EmpireSnapshot, HISTORY_EVENT_CAP, HistoryDraft, HistoryEvent, HistoryKind, HistoryLog,
    HistoryQuery,
};
pub use intent::{Intent, IntentError, Proposal, apply, propose};
pub use laws::{
    CLOCK as LAW_CLOCK, Jurisdiction, JurisdictionKind, Law, LawAction, LawActionKind, LawApproval,
    LawConfig, LawEffectKind, LawError, LawMetric, LawMetrics, LawSample, LawState, LawStatus,
    LawTemplate, PLAYER as LAW_PLAYER, emission_throttle,
};
pub use leaders::{
    Actor, Assignment, AssignmentTarget, Delegation, Leader, LeaderConfig, LeaderError,
    PLAYER as LEADER_PLAYER, Permission, RoleKind, TraitSpec,
};
pub use market::{
    ContractKind, ContractStatus, MarketBoard, MarketConfig, MarketContract, MarketError,
    MarketState, OrderSide, RegionMarket, RestingOrder, Shipment, TradeQuote, TraderId,
};
pub use moons::{Moon, MoonConfig, MoonError, MoonFacility, PhalanxContact};
pub use ontology::{
    ACTIONS as ONTOLOGY_ACTIONS, BalanceLineage, CHISEI_API_VERSION, OBJECT_TYPES,
    ONTOLOGY_PACKAGE, ONTOLOGY_SCHEMA_VERSION, OntologyError, RELATIONS, bundled_ontology,
    transform_balance, validate_ontology,
};
pub use orbits::{
    Hohmann, OrbitConfig, fuel_mass_for_dv, heliocentric_radius_m, hohmann_transfer, mean_motion,
    rocket_delta_v,
};
pub use outposts::{
    HomeworldSpec, Outpost, OutpostConfig, RarePlanetBonus, generate_homeworld, generate_outposts,
    scaled_defence, scaled_garrison,
};
pub use planets::{Colony, PlanetType, PlanetTypeBonus, PlanetTypeCatalog, planet_profile};
pub use planning::{
    Customer, LaunchWindow, MissionPlan, MissionSchedule, Payload, PlanError, PlanRequest,
    PlannedMission, Route, ScheduleAlert, ScheduleAlertKind, TransferEstimate, TransferKind,
    Vehicle,
};
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
    DaySnapshot, HeavyIndustryReport, PrestigeSimRow, SimReport, VictorySimRow,
    heavy_industry_check, simulate_greedy, simulate_victory_paths, tactics_no_combo_dominates,
};
pub use state::{GameEvent, GameState, LogisticsGlance, ResourceKind, Resources};
#[cfg(feature = "chisei")]
pub use storage::ChiseiStorage;
#[cfg(feature = "mikura")]
pub use storage::MikuraStorage;
pub use storage::{BackendKind, SqliteStorage, Storage, open_storage};
pub use story::{PendingStory, StoryCatalog, StoryChoice, StoryError, StoryEvent};
pub use tactics::{
    Admiral, AdmiralTrait, FleetTactics, Formation, PendingTactical, Stance, TacticalPlan,
    TacticsConfig, TacticsError, Terrain, auto_action, resolve_tactical,
};
pub use tutorial::{
    HintScreen, HintSpec, HintTrigger, TutorialBoard, TutorialCatalog, TutorialError,
};
pub use victory::{
    PathProgress, VictoryConfig, VictoryError, VictoryPath, VictoryProgress, VictoryRecord,
};
pub use waste::{WasteConfig, WasteReport, report as waste_report};
pub use wonders::{WonderError, WonderJob, WonderModifier, WonderSpec, WonderStage};
