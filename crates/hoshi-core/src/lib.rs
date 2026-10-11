//! hoshi-core: game rules, state, timers and offline progression.

mod balance;
mod buildings;
mod clock;
mod state;

pub use balance::{
    BalanceConfig, BuildingCatalog, BuildingSpec, EnergyProducerSpec, ProducerSpec, StorageSpec,
};
pub use buildings::{BuildError, BuildJob, BuildingKind, Buildings};
pub use clock::{Clock, FakeClock, SystemClock};
pub use state::{GameState, ResourceKind, Resources};

/// Open (or create) the SQLite save at `path`. Schema arrives with the save/load issue.
pub fn open_save(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(path)?;
    conn.execute_batch("PRAGMA user_version = 0;")?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_opens_in_memory() {
        assert!(open_save(std::path::Path::new(":memory:")).is_ok());
    }
}
