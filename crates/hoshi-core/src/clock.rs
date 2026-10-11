//! Injectable clocks so ticks and offline catch-up stay deterministic in tests.

/// Source of time for the game.
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

/// Test clock. Mutation goes through [`Cell`] so [`Clock`] can take `&self`.
#[derive(Debug)]
pub struct FakeClock {
    now: std::cell::Cell<u64>,
}

impl FakeClock {
    pub fn new(now_unix: u64) -> Self {
        Self {
            now: std::cell::Cell::new(now_unix),
        }
    }

    pub fn set(&self, now_unix: u64) {
        self.now.set(now_unix);
    }

    pub fn advance(&self, seconds: u64) {
        self.now.set(self.now.get().saturating_add(seconds));
    }
}

impl Clock for FakeClock {
    fn now_unix(&self) -> u64 {
        self.now.get()
    }
}
