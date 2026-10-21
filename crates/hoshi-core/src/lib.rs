//! hoshi-core: game rules, state, timers and offline progression.

mod achievements;
mod activity;
mod advisor;
mod automation;
mod balance;
mod blueprint;
mod buildings;
mod chains;
mod classification;
mod clock;
mod combat;
mod commentary;
mod daily;
mod defences;
mod detail;
mod diplomacy;
mod espionage;
mod expedition;
mod finance;
mod fleets;
mod frontier;
mod galaxy;
mod governance;
mod history;
mod incident;
mod intel;
mod intent;
mod kpi;
mod ladder;
mod laws;
mod leaders;
mod maintenance;
mod market;
mod moons;
mod ontology;
mod orbits;
mod outposts;
mod planets;
mod planning;
mod prestige;
mod production;
mod reactor;
mod research;
mod rivals;
mod save;
mod scenario;
mod season;
mod self_play;
mod shard;
mod ships;
mod simulate;
mod species;
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
pub use automation::{
    AutomationCatalog, AutomationError, AutomationRule, AutomationTier, RuleAction, RuleActionKind,
    RuleCondition, RuleConditionKind, RulePreview,
};
pub use balance::{
    BalanceConfig, BalanceError, BuildingCatalog, BuildingSpec, EnergyProducerSpec, PacingTargets,
    ProducerSpec, RaidConfig, StorageSpec,
};
pub use blueprint::{
    APPLY_ORDER, Blueprint, BlueprintApply, BlueprintCatalog, BlueprintError, BlueprintMismatch,
    BlueprintPreview, BlueprintStep, blueprint_dir,
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
pub use detail::{
    AffordEta, DeltaLine, Detail, DetailSubject, MaterialLine, RequirementLine, ShipTravel,
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
pub use finance::{
    Budget, CostCenter, Department, FinanceConfig, FinanceError, FinanceReport, FinanceState,
    Ledger, PendingKind, PendingSpend, Treasury,
};
pub use fleets::{
    Coord, FleetError, FlightConfig, Mission, MissionPhase, TradeRoute, distance, flight_secs,
    fuel_cost,
};
pub use frontier::{
    FrontierBoard, FrontierConfig, FrontierError, FrontierGlance, FrontierSignal, FrontierWorld,
    generate_frontier,
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
pub use incident::{
    Incident, IncidentConfig, IncidentError, IncidentKind, IncidentReview, IncidentSnapshot,
    IncidentState, IncidentStatus, Risk, Severity,
};
pub use intel::{
    IntelFact, IntelValue, Observation, SPEED_OF_LIGHT_MPS, aged_confidence, apply_amount_error,
    apply_count_error, group_facts, intel_confidence, light_delay_secs, light_range_au,
    noisy_fleet, noisy_resources, observations_from, sensor_error, signed_unit,
};
pub use intent::{Intent, IntentError, Proposal, apply, propose};
pub use kpi::{
    KpiConfig, KpiError, KpiId, KpiSnapshot, KpiState, KpiTarget, Measurement, PeriodicReport,
    TargetOwner,
};
pub use ladder::{
    LadderBoard, LadderCompletion, LadderConfig, LadderError, LadderGoalProgress, LadderProgress,
    LadderTier,
};
pub use laws::{
    CLOCK as LAW_CLOCK, Jurisdiction, JurisdictionKind, Law, LawAction, LawActionKind, LawApproval,
    LawConfig, LawEffectKind, LawError, LawMetric, LawMetrics, LawSample, LawState, LawStatus,
    LawTemplate, PLAYER as LAW_PLAYER, emission_throttle,
};
pub use leaders::{
    Actor, Assignment, AssignmentTarget, Delegation, Leader, LeaderConfig, LeaderError,
    PLAYER as LEADER_PLAYER, Permission, RoleKind, TraitSpec,
};
pub use maintenance::{
    Asset, AssetKind, FailurePrediction, MaintenanceConfig, MaintenanceError, MaintenancePlan,
    MaintenanceSnapshot, MaintenanceState, PlanKind, TelemetryReading, WorkKind, WorkOrder,
    fail_p_per_hour, predict_failure,
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
pub use rivals::{
    RivalBoard, RivalCatalog, RivalEmpire, RivalError, RivalIntel, RivalPersonality, RivalSettings,
};
pub use save::{
    CareerGlance, SCHEMA_VERSION, SaveError, SaveGlance, SaveLock, backup_save, default_save_dir,
    default_save_path, inspect_career, inspect_save, load_or_new, load_or_new_with_seed,
    load_state, lock_save, lock_save_migrations, open_save, resolve_save_path, save_is_locked,
    save_lock_path, save_schema, save_state,
};
pub use scenario::{Baseline, Outcome, PlannedAction, Scenario, ScenarioError};
pub use season::{
    SEASON_SYSTEMS, SeasonCatalog, SeasonError, SeasonRuleset, SeasonRun, SeasonWindow,
    season_slot_name,
};
pub use self_play::{
    SelfPlayCell, SelfPlayConfig, SelfPlayReport, path_bot_relpaths, run_self_play,
};
pub use shard::{
    ShardCommand, ShardConfig, ShardError, ShardRaid, ShardRaidResult, ShardSession,
    SignedBridgeEvent,
};
pub use ships::{
    Fleet, SHIP_KINDS, ShipCatalog, ShipCatalogError, ShipError, ShipJob, ShipKind, ShipSpec,
    ShipStats,
};
pub use simulate::{
    DaySnapshot, HeavyIndustryReport, LadderSimRow, PrestigeSimRow, SimReport, VictorySimRow,
    greedy_curve, heavy_industry_check, simulate_greedy, simulate_species_victory_paths,
    simulate_victory_paths, simulate_victory_paths_for_seed, simulate_victory_paths_with_rivals,
    tactics_no_combo_dominates,
};
pub use species::{Flagship, SpeciesCatalog, SpeciesError, SpeciesRun, SpeciesSpec};
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

pub use reactor::{ReactorConfig, ReactorShutdown};
