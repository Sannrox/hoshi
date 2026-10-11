//! Bounded game effects from bridge activity. Off until [`ActivityEffectsConfig::enabled`].

/// One mapping from a bridge signal to a game effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityChannel {
    PrMerged,
    CiGreen,
    CiRed,
    AgentLongRun,
    AgentStuck,
}

impl ActivityChannel {
    pub const ALL: [ActivityChannel; 5] = [
        ActivityChannel::PrMerged,
        ActivityChannel::CiGreen,
        ActivityChannel::CiRed,
        ActivityChannel::AgentLongRun,
        ActivityChannel::AgentStuck,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            ActivityChannel::PrMerged => "pr_merged",
            ActivityChannel::CiGreen => "ci_green",
            ActivityChannel::CiRed => "ci_red",
            ActivityChannel::AgentLongRun => "agent_long_run",
            ActivityChannel::AgentStuck => "agent_stuck",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "pr_merged" => ActivityChannel::PrMerged,
            "ci_green" => ActivityChannel::CiGreen,
            "ci_red" => ActivityChannel::CiRed,
            "agent_long_run" => ActivityChannel::AgentLongRun,
            "agent_stuck" => ActivityChannel::AgentStuck,
            _ => return None,
        })
    }

    pub fn index(self) -> usize {
        match self {
            ActivityChannel::PrMerged => 0,
            ActivityChannel::CiGreen => 1,
            ActivityChannel::CiRed => 2,
            ActivityChannel::AgentLongRun => 3,
            ActivityChannel::AgentStuck => 4,
        }
    }
}

/// Knobs for one channel. Unused fields stay at zero for that mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityChannelConfig {
    pub enabled: bool,
    pub cooldown_secs: u64,
    pub daily_cap: u32,
    pub payout_hours: f64,
    pub boost_factor: f64,
    pub boost_secs: u64,
    pub min_duration_secs: u64,
}

impl ActivityChannelConfig {
    fn payout(payout_hours: f64, cooldown_secs: u64, daily_cap: u32) -> Self {
        Self {
            enabled: true,
            cooldown_secs,
            daily_cap,
            payout_hours,
            boost_factor: 0.0,
            boost_secs: 0,
            min_duration_secs: 0,
        }
    }

    fn boost(
        boost_factor: f64,
        boost_secs: u64,
        cooldown_secs: u64,
        daily_cap: u32,
        min_duration_secs: u64,
    ) -> Self {
        Self {
            enabled: true,
            cooldown_secs,
            daily_cap,
            payout_hours: 0.0,
            boost_factor,
            boost_secs,
            min_duration_secs,
        }
    }

    fn raid(cooldown_secs: u64, daily_cap: u32, min_duration_secs: u64) -> Self {
        Self {
            enabled: true,
            cooldown_secs,
            daily_cap,
            payout_hours: 0.0,
            boost_factor: 0.0,
            boost_secs: 0,
            min_duration_secs,
        }
    }
}

/// Balance table for bridge effects. Individually disableable per channel.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityEffectsConfig {
    pub enabled: bool,
    pub pr_merged: ActivityChannelConfig,
    pub ci_green: ActivityChannelConfig,
    pub ci_red: ActivityChannelConfig,
    pub agent_long: ActivityChannelConfig,
    pub agent_stuck: ActivityChannelConfig,
}

impl ActivityEffectsConfig {
    pub fn get(&self, channel: ActivityChannel) -> &ActivityChannelConfig {
        match channel {
            ActivityChannel::PrMerged => &self.pr_merged,
            ActivityChannel::CiGreen => &self.ci_green,
            ActivityChannel::CiRed => &self.ci_red,
            ActivityChannel::AgentLongRun => &self.agent_long,
            ActivityChannel::AgentStuck => &self.agent_stuck,
        }
    }
}

impl Default for ActivityEffectsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            pr_merged: ActivityChannelConfig::payout(0.5, 300, 24),
            ci_green: ActivityChannelConfig::boost(0.25, 1800, 600, 12, 0),
            ci_red: ActivityChannelConfig::raid(3600, 3, 0),
            agent_long: ActivityChannelConfig::boost(0.5, 1800, 1800, 6, 1800),
            agent_stuck: ActivityChannelConfig::raid(3600, 3, 900),
        }
    }
}

/// A bridge event reduced to a typed pulse. The TUI maps feed lines here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActivityPulse {
    PrMerged { ts: u64 },
    WorkflowFinished { ts: u64, success: bool },
    AgentFinished { ts: u64, duration_secs: u64 },
    AgentWaiting { ts: u64, waiting_secs: u64 },
}

impl ActivityPulse {
    pub fn ts(self) -> u64 {
        match self {
            ActivityPulse::PrMerged { ts }
            | ActivityPulse::WorkflowFinished { ts, .. }
            | ActivityPulse::AgentFinished { ts, .. }
            | ActivityPulse::AgentWaiting { ts, .. } => ts,
        }
    }
}

/// Result of [`crate::GameState::apply_activity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityApply {
    Applied,
    Skipped,
}

/// Persisted cooldowns, daily counters, and active boosts.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityTracker {
    pub prod_boost_until: Option<u64>,
    pub prod_boost_factor: f64,
    pub research_boost_until: Option<u64>,
    pub research_boost_factor: f64,
    pub last_applied: [Option<u64>; 5],
    pub day_start: u64,
    pub day_count: [u32; 5],
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self {
            prod_boost_until: None,
            prod_boost_factor: 0.0,
            research_boost_until: None,
            research_boost_factor: 0.0,
            last_applied: [None; 5],
            day_start: 0,
            day_count: [0; 5],
        }
    }
}

impl ActivityTracker {
    /// Cooldown and daily cap. `daily_cap == 0` means no daily limit.
    pub fn admit(
        &mut self,
        channel: ActivityChannel,
        ts: u64,
        cooldown_secs: u64,
        daily_cap: u32,
    ) -> bool {
        let i = channel.index();
        if let Some(last) = self.last_applied[i]
            && cooldown_secs > 0
            && ts < last.saturating_add(cooldown_secs)
        {
            return false;
        }
        let day = day_start(ts);
        if day > self.day_start {
            self.day_start = day;
            self.day_count = [0; 5];
        }
        if daily_cap > 0 && day == self.day_start && self.day_count[i] >= daily_cap {
            return false;
        }
        self.last_applied[i] = Some(ts);
        if day == self.day_start {
            self.day_count[i] = self.day_count[i].saturating_add(1);
        }
        true
    }
}

pub(crate) fn day_start(ts: u64) -> u64 {
    ts / 86_400 * 86_400
}

pub(crate) fn resolve_channel(
    pulse: ActivityPulse,
    config: &ActivityEffectsConfig,
) -> Option<(ActivityChannel, u64)> {
    match pulse {
        ActivityPulse::PrMerged { ts } => Some((ActivityChannel::PrMerged, ts)),
        ActivityPulse::WorkflowFinished { ts, success } => {
            let channel = if success {
                ActivityChannel::CiGreen
            } else {
                ActivityChannel::CiRed
            };
            Some((channel, ts))
        }
        ActivityPulse::AgentFinished { ts, duration_secs } => {
            if duration_secs >= config.agent_long.min_duration_secs {
                Some((ActivityChannel::AgentLongRun, ts))
            } else {
                None
            }
        }
        ActivityPulse::AgentWaiting { ts, waiting_secs } => {
            if waiting_secs >= config.agent_stuck.min_duration_secs {
                Some((ActivityChannel::AgentStuck, ts))
            } else {
                None
            }
        }
    }
}
