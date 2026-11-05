//! Idle play loop: how long to block, when to draw, when to persist.

use std::time::Duration;

/// `event::poll` wait with combat replay off. Zero would spin.
pub const IDLE_POLL: Duration = Duration::from_secs(1);
/// Replay paints volleys at 10 fps.
pub const REPLAY_POLL: Duration = Duration::from_millis(100);

/// Idle persist cadence. Production ticks every second; rewriting the sqlite
/// save every 5s wrote hundreds of megabytes in a half hour. Player commands
/// and quit still save immediately.
pub const SAVE_IDLE_SECS: u64 = 60;

/// Reduced-motion / no-animation still refresh a clock this often.
pub const SLOW_CLOCK_SECS: u64 = 60;

pub fn poll_wait(replay_active: bool) -> Duration {
    if replay_active {
        REPLAY_POLL
    } else {
        IDLE_POLL
    }
}

/// Persist after a player command, or once [`SAVE_IDLE_SECS`] of game time
/// have passed since the last successful save. A live tactical round is not
/// stored; command saves wait until it ends so a crash can still reload the
/// scheduled raid. Idle persist and quit still resolve it.
pub fn save_due(
    now_unix: u64,
    last_save_unix: u64,
    after_command: bool,
    live_tactical: bool,
) -> bool {
    let command = after_command && !live_tactical;
    command || now_unix.saturating_sub(last_save_unix) >= SAVE_IDLE_SECS
}

/// Visible-frame identity. Live resource ticks and historical "ago" labels
/// are omitted so idle production does not repaint. Upcoming live events
/// (queues, raid, fleet, fight, merchant, open signals) are included.
/// Countdown text is omitted when reduced-motion / no-animation freeze
/// timers; those sessions pick up completions, raid warnings, keys, and
/// [`SLOW_CLOCK_SECS`]. Timer strings use the same formatter as the painted
/// label (`duration` for long raids and queues, `countdown` inside the raid
/// warning window and for fleet/signal ETAs) so a 1h raid does not redraw
/// every second.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameSig {
    pub history_next_id: u64,
    pub replay_frame: Option<usize>,
    pub raid_warning: bool,
    pub timer_label: Option<String>,
    pub slow_clock: u64,
    /// Live fight identity: `(next_at, actions.len())`. Presence alone would
    /// leave intermediate rounds undrawn while idle catch-up advances them.
    pub pending_tactical: Option<(u64, usize)>,
    pub story_pending: bool,
    pub away_open: bool,
    pub build_head: Option<u64>,
    pub research_head: Option<u64>,
    pub build_len: usize,
    pub research_queued: usize,
    pub ship_jobs: usize,
    pub ship_remaining: u32,
    pub ship_head: Option<u64>,
    pub merchant_until: Option<u64>,
    pub signal_head: Option<u64>,
    pub signals: usize,
    pub missions: usize,
    pub combat_logs: usize,
    pub ui_epoch: u64,
    pub updates: (bool, bool),
}

pub fn should_draw(previous: Option<&FrameSig>, now: &FrameSig) -> bool {
    previous != Some(now)
}

pub fn slow_clock(last_tick_unix: u64) -> u64 {
    last_tick_unix / SLOW_CLOCK_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_poll_stays_at_least_one_second_when_replay_is_off() {
        assert!(poll_wait(false) >= Duration::from_secs(1));
        assert_ne!(poll_wait(false), Duration::ZERO);
        assert_eq!(poll_wait(true), REPLAY_POLL);
    }

    #[test]
    fn two_idle_wakes_do_not_save_twice_inside_five_seconds() {
        let mut last_save = 1_000;
        let mut saves = 0;
        for now in [1_001, 1_002, 1_005] {
            if save_due(now, last_save, false, false) {
                saves += 1;
                last_save = now;
            }
        }
        assert_eq!(saves, 0, "idle persist is {SAVE_IDLE_SECS}s, not 5s");
        assert!(save_due(1_060, last_save, false, false));
        assert!(save_due(1_001, 1_000, true, false));
        assert!(!save_due(1_000, 1_000, false, false));
        assert!(!save_due(1_001, 1_000, true, true));
        assert!(save_due(1_060, 1_000, true, true));
    }

    #[test]
    fn should_draw_skips_an_unchanged_frame() {
        let frame = FrameSig {
            history_next_id: 1,
            slow_clock: 16,
            ..FrameSig::default()
        };
        assert!(should_draw(None, &frame));
        assert!(!should_draw(Some(&frame), &frame));
        let mut next = frame.clone();
        next.history_next_id = 2;
        assert!(should_draw(Some(&frame), &next));
        next = frame.clone();
        next.ui_epoch = 1;
        assert!(should_draw(Some(&frame), &next));
        next = frame.clone();
        next.build_head = Some(1_800);
        assert!(should_draw(Some(&frame), &next));
    }
}
