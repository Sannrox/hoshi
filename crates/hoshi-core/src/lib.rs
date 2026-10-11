//! hoshi-core: game rules, state, timers and offline progression.

mod activity;
mod balance;
mod buildings;
mod clock;
mod combat;
mod expedition;
mod fleets;
mod planets;
mod research;
mod save;
mod ships;
mod state;

pub use activity::{
    ActivityApply, ActivityChannel, ActivityChannelConfig, ActivityEffectsConfig, ActivityPulse,
    ActivityTracker,
};
pub use balance::{
    BalanceConfig, BuildingCatalog, BuildingSpec, EnergyProducerSpec, ProducerSpec, RaidConfig,
    StorageSpec,
};
pub use buildings::{BuildError, BuildJob, BuildingKind, Buildings};
pub use clock::{Clock, FakeClock, SystemClock};
pub use combat::{
    CombatReport, CombatWinner, next_u64, plunder_from, rapid_fire_table, resolve_combat,
};
pub use expedition::{
    ExpeditionConfig, ExpeditionKind, ExpeditionResult, ExpeditionWeights, fleet_points,
    roll_expedition, scaled_weights,
};
pub use fleets::{
    Coord, FleetError, FlightConfig, Mission, MissionPhase, TradeRoute, distance, flight_secs,
    fuel_cost,
};
pub use planets::{Colony, PlanetType, PlanetTypeBonus, PlanetTypeCatalog, planet_profile};
pub use research::{
    BuildingPrereq, ModifierKind, PrereqGap, ResearchError, ResearchJob, TechModifier, TechPrereq,
    TechSpec, TechTree, TechTreeError,
};
pub use save::{
    SCHEMA_VERSION, SaveError, default_save_path, load_or_new, load_state, open_save,
    resolve_save_path, save_state,
};
pub use ships::{
    Fleet, SHIP_KINDS, ShipCatalog, ShipCatalogError, ShipError, ShipJob, ShipKind, ShipSpec,
    ShipStats,
};
pub use state::{GameEvent, GameState, ResourceKind, Resources};
