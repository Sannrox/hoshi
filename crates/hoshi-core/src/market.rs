//! NPC resource exchange. Prices are a mean-reverting walk with bounded shocks.

use crate::combat::next_u64;
use crate::state::ResourceKind;
use std::fmt;

/// Balance knobs for the NPC market. Live in `balance.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketConfig {
    pub enabled: bool,
    pub tick_secs: u64,
    pub reversion: f64,
    pub volatility: f64,
    pub shock_chance: f64,
    pub shock_size: f64,
    pub fee: f64,
    pub slippage: f64,
    pub impact_unit: f64,
    pub min_mult: f64,
    pub max_mult: f64,
    pub history_len: usize,
    pub metal_mean: f64,
    pub crystal_mean: f64,
    pub deuterium_mean: f64,
    /// Trades larger than this need home cargo capacity at least the pay amount.
    pub cargo_free: f64,
}

impl MarketConfig {
    pub fn inactive() -> Self {
        Self {
            enabled: false,
            ..Self::default_active()
        }
    }

    pub fn default_active() -> Self {
        Self {
            enabled: true,
            tick_secs: 3600,
            reversion: 0.15,
            volatility: 0.04,
            shock_chance: 0.05,
            shock_size: 0.12,
            fee: 0.04,
            slippage: 0.002,
            impact_unit: 100.0,
            min_mult: 0.5,
            max_mult: 2.0,
            history_len: 24,
            metal_mean: 1.0,
            crystal_mean: 2.0,
            deuterium_mean: 3.0,
            cargo_free: 50.0,
        }
    }

    pub fn mean(&self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Metal => self.metal_mean,
            ResourceKind::Crystal => self.crystal_mean,
            ResourceKind::Deuterium => self.deuterium_mean,
        }
    }
}

impl Default for MarketConfig {
    fn default() -> Self {
        Self::default_active()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarketError {
    Disabled,
    SameResource,
    BadAmount,
    Insufficient { missing: f64, kind: ResourceKind },
    NeedCargo { need: f64, have: f64 },
}

impl fmt::Display for MarketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MarketError::Disabled => write!(f, "market is off"),
            MarketError::SameResource => write!(f, "market: pick two different resources"),
            MarketError::BadAmount => write!(f, "market: amount must be positive and finite"),
            MarketError::Insufficient { missing, kind } => {
                write!(f, "market: missing {missing:.0} {}", kind.label())
            }
            MarketError::NeedCargo { need, have } => {
                write!(f, "market: need {need:.0} cargo, have {have:.0}")
            }
        }
    }
}

impl std::error::Error for MarketError {}

#[derive(Debug, Clone, PartialEq)]
pub struct TradeQuote {
    pub pay: ResourceKind,
    pub recv: ResourceKind,
    pub pay_amount: f64,
    pub recv_amount: f64,
    pub fee: f64,
    pub slip: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketState {
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
    pub metal_hist: Vec<f64>,
    pub crystal_hist: Vec<f64>,
    pub deuterium_hist: Vec<f64>,
}

impl MarketState {
    pub fn from_config(cfg: &MarketConfig) -> Self {
        let mut state = Self {
            metal: cfg.metal_mean,
            crystal: cfg.crystal_mean,
            deuterium: cfg.deuterium_mean,
            metal_hist: Vec::new(),
            crystal_hist: Vec::new(),
            deuterium_hist: Vec::new(),
        };
        state.push_history(cfg);
        state
    }

    pub fn price(&self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Metal => self.metal,
            ResourceKind::Crystal => self.crystal,
            ResourceKind::Deuterium => self.deuterium,
        }
    }

    pub fn set_price(&mut self, kind: ResourceKind, value: f64) {
        match kind {
            ResourceKind::Metal => self.metal = value,
            ResourceKind::Crystal => self.crystal = value,
            ResourceKind::Deuterium => self.deuterium = value,
        }
    }

    pub fn history(&self, kind: ResourceKind) -> &[f64] {
        match kind {
            ResourceKind::Metal => &self.metal_hist,
            ResourceKind::Crystal => &self.crystal_hist,
            ResourceKind::Deuterium => &self.deuterium_hist,
        }
    }

    pub fn quote(
        &self,
        pay: ResourceKind,
        recv: ResourceKind,
        amount: f64,
        cfg: &MarketConfig,
    ) -> Result<TradeQuote, MarketError> {
        if pay == recv {
            return Err(MarketError::SameResource);
        }
        if !amount.is_finite() || amount <= 0.0 {
            return Err(MarketError::BadAmount);
        }
        let px_pay = self.price(pay);
        let px_recv = self.price(recv);
        if px_pay <= 0.0 || px_recv <= 0.0 {
            return Err(MarketError::BadAmount);
        }
        let gross = amount * (px_pay / px_recv);
        let slip = (amount / cfg.impact_unit.max(1.0)) * cfg.slippage;
        let recv_amount = gross * (1.0 - cfg.fee) / (1.0 + slip.max(0.0));
        Ok(TradeQuote {
            pay,
            recv,
            pay_amount: amount,
            recv_amount,
            fee: cfg.fee,
            slip,
        })
    }

    /// One scheduled tick: mean-reverting noise, rare shock, clamp, record history.
    pub fn tick(&mut self, cfg: &MarketConfig, rng: &mut u64) {
        for kind in ResourceKind::ALL {
            let mean = cfg.mean(kind);
            let price = self.price(kind);
            let noise = unit_noise(rng) * cfg.volatility;
            let mut next = price + cfg.reversion * (mean - price) + price * noise;
            if unit_unit(rng) < cfg.shock_chance {
                let sign = if next_u64(rng).is_multiple_of(2) {
                    1.0
                } else {
                    -1.0
                };
                next *= 1.0 + sign * cfg.shock_size;
            }
            self.set_price(kind, clamp_price(next, mean, cfg));
        }
        self.push_history(cfg);
    }

    pub fn shock(&mut self, kind: ResourceKind, relative: f64, cfg: &MarketConfig) {
        if relative == 0.0 || !relative.is_finite() {
            return;
        }
        let mean = cfg.mean(kind);
        let next = self.price(kind) * (1.0 + relative);
        self.set_price(kind, clamp_price(next, mean, cfg));
        self.push_history(cfg);
    }

    pub fn apply_trade_impact(&mut self, quote: &TradeQuote, cfg: &MarketConfig) {
        let impact = (quote.pay_amount / cfg.impact_unit.max(1.0)) * cfg.slippage;
        let sold = self.price(quote.pay) * (1.0 - impact);
        let bought = self.price(quote.recv) * (1.0 + impact);
        self.set_price(quote.pay, clamp_price(sold, cfg.mean(quote.pay), cfg));
        self.set_price(quote.recv, clamp_price(bought, cfg.mean(quote.recv), cfg));
    }

    fn push_history(&mut self, cfg: &MarketConfig) {
        let cap = cfg.history_len.max(1);
        for kind in ResourceKind::ALL {
            let price = self.price(kind);
            let hist = match kind {
                ResourceKind::Metal => &mut self.metal_hist,
                ResourceKind::Crystal => &mut self.crystal_hist,
                ResourceKind::Deuterium => &mut self.deuterium_hist,
            };
            hist.push(price);
            if hist.len() > cap {
                let drop = hist.len() - cap;
                hist.drain(..drop);
            }
        }
    }
}

fn clamp_price(price: f64, mean: f64, cfg: &MarketConfig) -> f64 {
    let lo = mean * cfg.min_mult;
    let hi = mean * cfg.max_mult;
    price.max(lo).min(hi).max(1e-9)
}

/// Uniform in `[-1, 1]`.
fn unit_noise(rng: &mut u64) -> f64 {
    (next_u64(rng) % 2001) as f64 / 1000.0 - 1.0
}

/// Uniform in `[0, 1)`.
fn unit_unit(rng: &mut u64) -> f64 {
    (next_u64(rng) as f64) / ((u64::MAX as f64) + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_stay_inside_configured_bounds() {
        let cfg = MarketConfig::default_active();
        let mut market = MarketState::from_config(&cfg);
        let mut rng = 1u64;
        for _ in 0..10_000 {
            market.tick(&cfg, &mut rng);
        }
        for kind in ResourceKind::ALL {
            let mean = cfg.mean(kind);
            let p = market.price(kind);
            assert!(p >= mean * cfg.min_mult - 1e-9, "{kind:?} {p}");
            assert!(p <= mean * cfg.max_mult + 1e-9, "{kind:?} {p}");
        }
    }

    #[test]
    fn same_seed_replays_the_same_walk() {
        let cfg = MarketConfig::default_active();
        let mut a = MarketState::from_config(&cfg);
        let mut b = MarketState::from_config(&cfg);
        let mut ra = 99u64;
        let mut rb = 99u64;
        for _ in 0..50 {
            a.tick(&cfg, &mut ra);
            b.tick(&cfg, &mut rb);
        }
        assert_eq!(a, b);
        assert_eq!(ra, rb);
    }

    #[test]
    fn buy_then_sell_back_loses_value() {
        let cfg = MarketConfig::default_active();
        let market = MarketState::from_config(&cfg);
        let buy = market
            .quote(ResourceKind::Metal, ResourceKind::Crystal, 100.0, &cfg)
            .unwrap();
        let mut after = market.clone();
        after.apply_trade_impact(&buy, &cfg);
        let sell = after
            .quote(
                ResourceKind::Crystal,
                ResourceKind::Metal,
                buy.recv_amount,
                &cfg,
            )
            .unwrap();
        assert!(
            sell.recv_amount < 100.0,
            "round trip {} -> {}",
            100.0,
            sell.recv_amount
        );
    }

    #[test]
    fn quote_applies_fee_and_slippage() {
        let cfg = MarketConfig::default_active();
        let market = MarketState::from_config(&cfg);
        let amount = 100.0;
        let q = market
            .quote(ResourceKind::Metal, ResourceKind::Crystal, amount, &cfg)
            .unwrap();
        let gross = amount * (cfg.metal_mean / cfg.crystal_mean);
        let slip = (amount / cfg.impact_unit) * cfg.slippage;
        let expected = gross * (1.0 - cfg.fee) / (1.0 + slip);
        assert!(
            (q.recv_amount - expected).abs() < 1e-9,
            "{} vs {expected}",
            q.recv_amount
        );
        assert!(q.recv_amount < gross);
        assert_eq!(q.fee, cfg.fee);
        assert!((q.slip - slip).abs() < 1e-12);
    }
}
