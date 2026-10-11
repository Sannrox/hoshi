//! hoshi-core: game rules, state, timers and offline progression.

mod balance;
mod clock;
mod state;

pub use balance::{BalanceConfig, EnergyProducerSpec, ProducerSpec, StorageSpec};
pub use clock::{Clock, FakeClock, SystemClock};
pub use state::{Buildings, GameState, ResourceKind, Resources};

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
