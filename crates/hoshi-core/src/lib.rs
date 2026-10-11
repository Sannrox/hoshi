//! hoshi-core: game rules, state, timers and offline progression.

mod balance;
mod buildings;
mod clock;
mod fleets;
mod research;
mod save;
mod ships;
mod state;

pub use balance::{
    BalanceConfig, BuildingCatalog, BuildingSpec, EnergyProducerSpec, ProducerSpec, StorageSpec,
};
pub use buildings::{BuildError, BuildJob, BuildingKind, Buildings};
pub use clock::{Clock, FakeClock, SystemClock};
pub use fleets::{
    Coord, FleetError, FlightConfig, Mission, MissionPhase, distance, flight_secs, fuel_cost,
};
pub use research::{
    BuildingPrereq, ModifierKind, PrereqGap, ResearchError, ResearchJob, TechModifier, TechPrereq,
    TechSpec, TechTree, TechTreeError,
};
pub use save::{
    SCHEMA_VERSION, SaveError, default_save_path, load_or_new, load_state, open_save,
    resolve_save_path, save_state,
};
pub use ships::{
    Fleet, ShipCatalog, ShipCatalogError, ShipError, ShipJob, ShipKind, ShipSpec, ShipStats,
};
pub use state::{GameState, ResourceKind, Resources};
