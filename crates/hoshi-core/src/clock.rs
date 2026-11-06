//! Injectable clocks so ticks and offline catch-up stay deterministic in tests.

use std::fmt;

/// Source of time for the game.
pub trait Clock {
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> u64;
}

/// `factor` was 0. A scaled clock needs a factor of at least 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockScaleError;

impl fmt::Display for ClockScaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "clock factor must be at least 1")
    }
}

impl std::error::Error for ClockScaleError {}

/// Game time on a scaled shard: `epoch + factor * (real_now - epoch)`.
///
/// Factor 1 is the identity. `epoch` and `factor` are fixed; there is no
/// setter. Command rate limits stay on the inner clock.
#[derive(Debug, Clone)]
pub struct ScaledClock<C> {
    inner: C,
    epoch: u64,
    factor: u32,
}

impl<C> ScaledClock<C> {
    /// Wrap `inner`. `factor` must be at least 1.
    pub fn new(inner: C, epoch: u64, factor: u32) -> Result<Self, ClockScaleError> {
        if factor == 0 {
            return Err(ClockScaleError);
        }
        Ok(Self {
            inner,
            epoch,
            factor,
        })
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn factor(&self) -> u32 {
        self.factor
    }

    pub fn inner(&self) -> &C {
        &self.inner
    }
}

impl<C: Clock> Clock for ScaledClock<C> {
    fn now_unix(&self) -> u64 {
        scale_unix(self.inner.now_unix(), self.epoch, self.factor)
    }
}

/// Game unix from a wall unix: `epoch + factor * (real - epoch)`.
/// Times before `epoch` stay before it so factor 1 is the identity there too.
pub fn scale_unix(real_unix: u64, epoch: u64, factor: u32) -> u64 {
    let factor = u64::from(factor.max(1));
    if real_unix >= epoch {
        epoch.saturating_add((real_unix - epoch).saturating_mul(factor))
    } else {
        epoch.saturating_sub((epoch - real_unix).saturating_mul(factor))
    }
}

/// Wall unix from a game unix: `epoch + (game - epoch) / factor`.
pub fn wall_unix(game_unix: u64, epoch: u64, factor: u32) -> u64 {
    let factor = u64::from(factor.max(1));
    if game_unix >= epoch {
        epoch.saturating_add((game_unix - epoch) / factor)
    } else {
        epoch.saturating_sub((epoch - game_unix) / factor)
    }
}

/// Remaining wall seconds until a game deadline.
pub fn wall_remaining(game_deadline: u64, game_now: u64, factor: u32) -> u64 {
    let factor = u64::from(factor.max(1));
    game_deadline.saturating_sub(game_now) / factor
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factor_one_is_the_identity() {
        let inner = FakeClock::new(1_000);
        let clock = ScaledClock::new(inner, 500, 1).unwrap();
        assert_eq!(clock.now_unix(), 1_000);
        clock.inner().advance(40);
        assert_eq!(clock.now_unix(), 1_040);
    }

    #[test]
    fn factor_scales_elapsed_from_epoch() {
        let inner = FakeClock::new(1_000);
        let clock = ScaledClock::new(inner, 1_000, 10).unwrap();
        assert_eq!(clock.now_unix(), 1_000);
        clock.inner().advance(7);
        assert_eq!(clock.now_unix(), 1_070);
        assert_eq!(wall_unix(1_070, 1_000, 10), 1_007);
        assert_eq!(wall_remaining(1_070, 1_000, 10), 7);
    }

    #[test]
    fn times_before_epoch_stay_before_epoch() {
        assert_eq!(scale_unix(990, 1_000, 1), 990);
        assert_eq!(scale_unix(990, 1_000, 10), 900);
        assert_eq!(wall_unix(990, 1_000, 1), 990);
        assert_eq!(wall_unix(900, 1_000, 10), 990);
        let inner = FakeClock::new(990);
        let clock = ScaledClock::new(inner, 1_000, 1).unwrap();
        assert_eq!(clock.now_unix(), 990);
    }

    #[test]
    fn zero_factor_is_refused() {
        assert_eq!(
            ScaledClock::new(FakeClock::new(0), 0, 0).unwrap_err(),
            ClockScaleError
        );
    }
}
