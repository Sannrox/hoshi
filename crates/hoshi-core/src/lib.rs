//! hoshi-core: game rules, state, timers and offline progression.
//!
//! Placeholder skeleton. Rules land through the M1 issues.

use std::time::Duration;

/// Source of time for the game, injectable so ticks are deterministic in tests.
pub trait Clock {
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> u64;
}

/// Wall-clock implementation of [`Clock`].
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Minimal game state: one stockpile and the last time it was advanced.
#[derive(Debug, Clone, PartialEq)]
pub struct GameState {
    pub metal: f64,
    pub metal_per_hour: f64,
    pub last_tick_unix: u64,
}

impl GameState {
    pub fn new(now_unix: u64) -> Self {
        Self {
            metal: 500.0,
            metal_per_hour: 30.0,
            last_tick_unix: now_unix,
        }
    }

    /// Advance production to `now_unix`. Works the same for a one-second tick
    /// and for a multi-day offline gap.
    pub fn catch_up(&mut self, now_unix: u64) {
        let elapsed = Duration::from_secs(now_unix.saturating_sub(self.last_tick_unix));
        self.metal += self.metal_per_hour * elapsed.as_secs_f64() / 3600.0;
        self.last_tick_unix = self.last_tick_unix.max(now_unix);
    }
}

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
    fn offline_catch_up_equals_step_by_step() {
        let mut a = GameState::new(0);
        a.catch_up(7200);
        let mut b = GameState::new(0);
        for t in (60..=7200).step_by(60) {
            b.catch_up(t);
        }
        assert!((a.metal - b.metal).abs() < 1e-6);
        assert!((a.metal - 560.0).abs() < 1e-6);
    }

    #[test]
    fn clock_going_backwards_does_not_lose_resources() {
        let mut s = GameState::new(100);
        s.catch_up(50);
        assert_eq!(s.metal, 500.0);
        assert_eq!(s.last_tick_unix, 100);
    }

    #[test]
    fn save_opens_in_memory() {
        assert!(open_save(std::path::Path::new(":memory:")).is_ok());
    }
}
