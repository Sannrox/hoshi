//! NPC resource exchange plus a regional order book, contracts and shipments.
//!
//! Instant home barter (`quote` / `trade_market`) stays the over-the-counter
//! path. Limit and market orders, delivery contracts and inter-region
//! shipments go through [`GameState::place_order`], [`GameState::accept_contract`],
//! [`GameState::fulfil_contract`] and [`GameState::ship_goods`]. Crystal and
//! deuterium are priced in metal. Matching is price-time and deterministic.

use crate::combat::next_u64;
use crate::fleets::Coord;
use crate::ships::{Fleet, ShipKind};
use crate::state::{GameState, ResourceKind, Resources};
use std::collections::BTreeMap;
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
    pub hub_galaxy: u16,
    pub hub_system: u16,
    pub hub_position: u16,
    pub hub_metal: f64,
    pub hub_crystal: f64,
    pub hub_deuterium: f64,
    pub npc_order_size: f64,
    pub contract_penalty: f64,
    pub order_ttl_secs: u64,
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
            crystal_mean: 1.5,
            deuterium_mean: 6.0,
            cargo_free: 50.0,
            hub_galaxy: 1,
            hub_system: 1,
            hub_position: 16,
            hub_metal: 10_000.0,
            hub_crystal: 10_000.0,
            hub_deuterium: 10_000.0,
            npc_order_size: 50.0,
            contract_penalty: 0.2,
            order_ttl_secs: 86_400,
        }
    }

    pub fn mean(&self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => self.metal_mean,
            ResourceKind::Silicon => self.crystal_mean,
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                self.deuterium_mean
            }
        }
    }

    pub fn hub(&self) -> Coord {
        Coord {
            galaxy: self.hub_galaxy,
            system: self.hub_system,
            position: self.hub_position,
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
    QuoteGood,
    UnknownOrder,
    UnknownContract,
    NotAccepted,
    AlreadyAccepted,
    Closed,
    SameLocation,
    CannotFly,
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
            MarketError::QuoteGood => {
                write!(f, "market: metal is the quote; pick crystal or deuterium")
            }
            MarketError::UnknownOrder => write!(f, "market: unknown order"),
            MarketError::UnknownContract => write!(f, "market: unknown contract"),
            MarketError::NotAccepted => write!(f, "market: accept the contract first"),
            MarketError::AlreadyAccepted => write!(f, "market: contract already taken"),
            MarketError::Closed => write!(f, "market: contract is closed"),
            MarketError::SameLocation => write!(f, "market: origin and destination are the same"),
            MarketError::CannotFly => write!(f, "market: shipment needs a legal flight"),
        }
    }
}

impl std::error::Error for MarketError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn as_key(self) -> &'static str {
        match self {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "buy" => Some(OrderSide::Buy),
            "sell" => Some(OrderSide::Sell),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraderId {
    Player,
    Npc,
}

impl TraderId {
    pub fn as_key(self) -> &'static str {
        match self {
            TraderId::Player => "player",
            TraderId::Npc => "npc",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "player" => Some(TraderId::Player),
            "npc" => Some(TraderId::Npc),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractKind {
    Delivery,
    Procurement,
}

impl ContractKind {
    pub fn as_key(self) -> &'static str {
        match self {
            ContractKind::Delivery => "delivery",
            ContractKind::Procurement => "procurement",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "delivery" => Some(ContractKind::Delivery),
            "procurement" => Some(ContractKind::Procurement),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    Open,
    Accepted,
    Fulfilled,
    Failed,
}

impl ContractStatus {
    pub fn as_key(self) -> &'static str {
        match self {
            ContractStatus::Open => "open",
            ContractStatus::Accepted => "accepted",
            ContractStatus::Fulfilled => "fulfilled",
            ContractStatus::Failed => "failed",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "open" => Some(ContractStatus::Open),
            "accepted" => Some(ContractStatus::Accepted),
            "fulfilled" => Some(ContractStatus::Fulfilled),
            "failed" => Some(ContractStatus::Failed),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, ContractStatus::Fulfilled | ContractStatus::Failed)
    }
}

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
    pub iron: f64,
    pub silicon: f64,
    pub deuterium: f64,
    pub metal_hist: Vec<f64>,
    pub crystal_hist: Vec<f64>,
    pub deuterium_hist: Vec<f64>,
}

impl MarketState {
    pub fn from_config(cfg: &MarketConfig) -> Self {
        let mut state = Self {
            iron: cfg.metal_mean,
            silicon: cfg.crystal_mean,
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
            ResourceKind::Iron | ResourceKind::RareEarths => self.iron,
            ResourceKind::Silicon => self.silicon,
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                self.deuterium
            }
        }
    }

    pub fn set_price(&mut self, kind: ResourceKind, value: f64) {
        match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => self.iron = value,
            ResourceKind::Silicon => self.silicon = value,
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                self.deuterium = value
            }
        }
    }

    pub fn history(&self, kind: ResourceKind) -> &[f64] {
        match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => &self.metal_hist,
            ResourceKind::Silicon => &self.crystal_hist,
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                &self.deuterium_hist
            }
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
        for kind in ResourceKind::QUOTED {
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
        for kind in ResourceKind::QUOTED {
            let price = self.price(kind);
            let hist = match kind {
                ResourceKind::Iron | ResourceKind::RareEarths => &mut self.metal_hist,
                ResourceKind::Silicon => &mut self.crystal_hist,
                ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                    &mut self.deuterium_hist
                }
            };
            hist.push(price);
            if hist.len() > cap {
                let drop = hist.len() - cap;
                hist.drain(..drop);
            }
        }
    }
}

/// Per-region warehouses, books, contracts and in-flight cargo.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MarketBoard {
    pub regions: BTreeMap<Coord, RegionMarket>,
    pub orders: BTreeMap<u64, RestingOrder>,
    pub contracts: BTreeMap<u64, MarketContract>,
    pub shipments: BTreeMap<u64, Shipment>,
    pub next_order_id: u64,
    pub next_contract_id: u64,
    pub next_shipment_id: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionMarket {
    pub coord: Coord,
    pub npc_warehouse: Resources,
    pub player_warehouse: Resources,
    pub metal_hist: Vec<f64>,
    pub crystal_hist: Vec<f64>,
    pub deuterium_hist: Vec<f64>,
    pub last_crystal: Option<f64>,
    pub last_deuterium: Option<f64>,
}

impl RegionMarket {
    pub fn history(&self, kind: ResourceKind) -> &[f64] {
        match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => &self.metal_hist,
            ResourceKind::Silicon => &self.crystal_hist,
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                &self.deuterium_hist
            }
        }
    }

    fn push_price(&mut self, kind: ResourceKind, price: f64, cap: usize) {
        let hist = match kind {
            ResourceKind::Iron | ResourceKind::RareEarths => &mut self.metal_hist,
            ResourceKind::Silicon => &mut self.crystal_hist,
            ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                &mut self.deuterium_hist
            }
        };
        hist.push(price);
        if hist.len() > cap.max(1) {
            let drop = hist.len() - cap.max(1);
            hist.drain(..drop);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RestingOrder {
    pub id: u64,
    pub region: Coord,
    pub trader: TraderId,
    pub side: OrderSide,
    pub good: ResourceKind,
    pub remaining: f64,
    pub price: f64,
    pub escrow_metal: f64,
    pub escrow_good: f64,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub is_market: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketContract {
    pub id: u64,
    pub kind: ContractKind,
    pub region: Coord,
    pub good: ResourceKind,
    pub amount: f64,
    pub remaining: f64,
    pub price: f64,
    pub deadline: u64,
    pub penalty: f64,
    pub issuer: TraderId,
    pub holder: Option<TraderId>,
    pub status: ContractStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shipment {
    pub id: u64,
    pub origin: Coord,
    pub dest: Coord,
    pub cargo: Resources,
    pub fuel: f64,
    pub launched_at: u64,
    pub arrives_at: u64,
    pub contract_id: Option<u64>,
    pub trader: TraderId,
    pub delivered: bool,
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

fn traded_good(kind: ResourceKind) -> Result<ResourceKind, MarketError> {
    match kind {
        ResourceKind::Iron | ResourceKind::RareEarths => Err(MarketError::QuoteGood),
        ResourceKind::Silicon
        | ResourceKind::Deuterium
        | ResourceKind::WaterIce
        | ResourceKind::Helium3 => Ok(kind),
    }
}

impl GameState {
    pub(crate) fn next_board_at(&self) -> Option<u64> {
        if !self.config.market.enabled {
            return None;
        }
        let mut next = None;
        for order in self.board.orders.values() {
            if order.trader != TraderId::Player {
                continue;
            }
            if let Some(at) = order.expires_at.filter(|&at| at > self.last_tick_unix) {
                next = min_opt(next, Some(at));
            }
        }
        for ship in self.board.shipments.values() {
            if ship.trader == TraderId::Player
                && !ship.delivered
                && ship.arrives_at > self.last_tick_unix
            {
                next = min_opt(next, Some(ship.arrives_at));
            }
        }
        for contract in self.board.contracts.values() {
            if contract.holder == Some(TraderId::Player)
                && contract.status == ContractStatus::Accepted
                && contract.deadline > self.last_tick_unix
            {
                next = min_opt(next, Some(contract.deadline));
            }
        }
        next
    }

    pub(crate) fn advance_board_at(&mut self, at: u64) {
        if !self.config.market.enabled {
            return;
        }
        self.expire_orders_at(at);
        self.arrive_shipments_at(at);
        self.fail_contracts_at(at);
    }

    pub(crate) fn on_market_tick(&mut self, at: u64) {
        self.expire_open_contracts_at(at);
        self.ensure_region(self.config.market.hub());
        self.ensure_region(self.home);
        self.expire_npc_orders_at(at);
        self.record_region_mids();
        self.npc_market_tick(at);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn place_order(
        &mut self,
        region: Coord,
        side: OrderSide,
        good: ResourceKind,
        amount: f64,
        limit: Option<f64>,
        ttl_secs: u64,
        now_unix: u64,
    ) -> Result<u64, MarketError> {
        self.catch_up(now_unix);
        self.place_order_as(
            TraderId::Player,
            region,
            side,
            good,
            amount,
            limit,
            ttl_secs,
            self.last_tick_unix,
        )
    }

    pub fn accept_contract(&mut self, id: u64, now_unix: u64) -> Result<(), MarketError> {
        self.catch_up(now_unix);
        if !self.config.market.enabled {
            return Err(MarketError::Disabled);
        }
        let now = self.last_tick_unix;
        let contract = self
            .board
            .contracts
            .get_mut(&id)
            .ok_or(MarketError::UnknownContract)?;
        if contract.status != ContractStatus::Open {
            return Err(if contract.status.is_terminal() {
                MarketError::Closed
            } else {
                MarketError::AlreadyAccepted
            });
        }
        if now >= contract.deadline {
            contract.status = ContractStatus::Failed;
            return Err(MarketError::Closed);
        }
        contract.holder = Some(TraderId::Player);
        contract.status = ContractStatus::Accepted;
        self.push_event(now, format!("market: accepted contract #{id}"));
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Trade,
            actor: "player".into(),
            action: "accept_contract".into(),
            objects: vec![format!("contract:{id}")],
            outcome: "accepted".into(),
            ..crate::HistoryDraft::default()
        });
        Ok(())
    }

    pub fn fulfil_contract(&mut self, id: u64, now_unix: u64) -> Result<(), MarketError> {
        self.catch_up(now_unix);
        self.fulfil_contract_inner(id, self.last_tick_unix)
    }

    pub fn ship_goods(
        &mut self,
        origin: Coord,
        dest: Coord,
        cargo: Resources,
        contract_id: Option<u64>,
        now_unix: u64,
    ) -> Result<u64, MarketError> {
        self.catch_up(now_unix);
        if !self.config.market.enabled {
            return Err(MarketError::Disabled);
        }
        if origin == dest {
            return Err(MarketError::SameLocation);
        }
        if !cargo_ok(cargo) {
            return Err(MarketError::BadAmount);
        }
        let now = self.last_tick_unix;
        if let Some(cid) = contract_id {
            let contract = self
                .board
                .contracts
                .get(&cid)
                .ok_or(MarketError::UnknownContract)?;
            if contract.holder != Some(TraderId::Player)
                || contract.status != ContractStatus::Accepted
            {
                return Err(MarketError::NotAccepted);
            }
            if contract.region != dest {
                return Err(MarketError::Closed);
            }
            let qty = cargo.get(contract.good);
            if qty <= 1e-9 {
                return Err(MarketError::Insufficient {
                    missing: contract.remaining,
                    kind: contract.good,
                });
            }
            if qty > contract.remaining + 1e-9 {
                return Err(MarketError::BadAmount);
            }
        }
        let mut ships = Fleet::default();
        let n = cargo_ships(self, cargo);
        ships.add(ShipKind::SmallCargo, n);
        let (secs, fuel) = self
            .flight_from(origin, dest, ships, 100)
            .map_err(|_| MarketError::CannotFly)?;
        let have_fuel = self.trader_stock(TraderId::Player, origin).deuterium;
        if have_fuel + 1e-9 < fuel {
            return Err(MarketError::Insufficient {
                missing: fuel - have_fuel,
                kind: ResourceKind::Deuterium,
            });
        }
        self.debit_trader(TraderId::Player, origin, cargo.with_fuel(fuel))?;
        self.ensure_region(origin);
        self.ensure_region(dest);
        let id = self.board.next_shipment_id.max(1);
        self.board.next_shipment_id = id.saturating_add(1);
        self.board.shipments.insert(
            id,
            Shipment {
                id,
                origin,
                dest,
                cargo,
                fuel,
                launched_at: now,
                arrives_at: now.saturating_add(secs),
                contract_id,
                trader: TraderId::Player,
                delivered: false,
            },
        );
        self.push_event(
            now,
            format!("market: shipment #{id} {origin} -> {dest} eta {secs}s"),
        );
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Trade,
            actor: "player".into(),
            action: "ship_goods".into(),
            objects: vec![
                format!("shipment:{id}"),
                origin.to_string(),
                dest.to_string(),
            ],
            outcome: format!("fuel {fuel:.1}"),
            mass: cargo.total(),
            ..crate::HistoryDraft::default()
        });
        Ok(id)
    }

    pub fn credit_market_delivery(&mut self, dest: Coord, cargo: Resources, at: u64) {
        self.ensure_region(dest);
        self.credit_trader(TraderId::Player, dest, cargo);
        self.try_auto_fulfil_at(dest, at);
        self.push_event(at, format!("market: delivered cargo to {dest}"));
    }

    #[allow(clippy::too_many_arguments)]
    fn place_order_as(
        &mut self,
        trader: TraderId,
        region: Coord,
        side: OrderSide,
        good: ResourceKind,
        amount: f64,
        limit: Option<f64>,
        ttl_secs: u64,
        now: u64,
    ) -> Result<u64, MarketError> {
        if !self.config.market.enabled {
            return Err(MarketError::Disabled);
        }
        self.ensure_region(region);
        let good = traded_good(good)?;
        if !amount.is_finite() || amount <= 0.0 {
            return Err(MarketError::BadAmount);
        }
        let is_market = limit.is_none();
        let price = match limit {
            Some(p) => {
                if !p.is_finite() || p <= 0.0 {
                    return Err(MarketError::BadAmount);
                }
                p
            }
            None => match side {
                OrderSide::Buy => f64::MAX,
                OrderSide::Sell => 0.0,
            },
        };
        let (escrow_metal, escrow_good) = match side {
            OrderSide::Buy => (self.buy_order_escrow(good, amount, limit), 0.0),
            OrderSide::Sell => (0.0, amount),
        };
        if side == OrderSide::Buy {
            self.debit_trader(
                trader,
                region,
                Resources {
                    iron: escrow_metal,
                    ..Resources::ZERO
                },
            )?;
        } else {
            let mut pay = Resources::ZERO;
            pay.set(good, escrow_good);
            self.debit_trader(trader, region, pay)?;
        }
        self.ensure_region(region);
        let id = self.board.next_order_id.max(1);
        self.board.next_order_id = id.saturating_add(1);
        let ttl = if ttl_secs == 0 {
            self.config.market.order_ttl_secs
        } else {
            ttl_secs
        };
        let expires_at = now.saturating_add(ttl);
        self.board.orders.insert(
            id,
            RestingOrder {
                id,
                region,
                trader,
                side,
                good,
                remaining: amount,
                price,
                escrow_metal,
                escrow_good,
                created_at: now,
                expires_at: Some(expires_at),
                is_market,
            },
        );
        self.match_incoming(id, now);
        if let Some(order) = self.board.orders.get(&id)
            && order.is_market
            && order.remaining > 1e-9
        {
            let leftover = order.id;
            self.cancel_order(leftover, now);
        }
        if trader == TraderId::Player {
            self.push_event(
                now,
                format!(
                    "market: {} {} {:.0} {} at {}",
                    side.as_key(),
                    if is_market { "market" } else { "limit" },
                    amount,
                    good.label(),
                    region
                ),
            );
            self.record_history(crate::HistoryDraft {
                at: now,
                kind: crate::HistoryKind::Trade,
                actor: "player".into(),
                action: "place_order".into(),
                objects: vec![
                    format!("order:{id}"),
                    region.to_string(),
                    good.as_key().into(),
                ],
                outcome: side.as_key().into(),
                ..crate::HistoryDraft::default()
            });
        }
        Ok(id)
    }

    fn match_incoming(&mut self, incoming_id: u64, now: u64) {
        let Some(incoming) = self.board.orders.get(&incoming_id).cloned() else {
            return;
        };
        let mut rest: Vec<u64> = self
            .board
            .orders
            .values()
            .filter(|o| {
                o.id != incoming.id
                    && o.region == incoming.region
                    && o.good == incoming.good
                    && o.side != incoming.side
                    && o.trader != incoming.trader
                    && o.remaining > 1e-9
                    && crosses(&incoming, o)
            })
            .map(|o| o.id)
            .collect();
        rest.sort_by(|&a, &b| {
            let oa = &self.board.orders[&a];
            let ob = &self.board.orders[&b];
            match incoming.side {
                OrderSide::Buy => oa
                    .price
                    .partial_cmp(&ob.price)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(oa.id.cmp(&ob.id)),
                OrderSide::Sell => ob
                    .price
                    .partial_cmp(&oa.price)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(oa.id.cmp(&ob.id)),
            }
        });
        for rest_id in rest {
            let Some(incoming) = self.board.orders.get(&incoming_id).cloned() else {
                break;
            };
            if incoming.remaining <= 1e-9 {
                break;
            }
            let Some(resting) = self.board.orders.get(&rest_id).cloned() else {
                continue;
            };
            if resting.remaining <= 1e-9 || !crosses(&incoming, &resting) {
                continue;
            }
            let mid = self.metal_price(incoming.good);
            let price = fill_price(&incoming, &resting, mid);
            let qty = incoming.remaining.min(resting.remaining);
            if qty <= 1e-9 || !price.is_finite() || price <= 0.0 {
                continue;
            }
            self.settle_fill(incoming_id, rest_id, qty, price, now);
        }
        self.drop_empty_orders();
    }

    fn settle_fill(&mut self, left_id: u64, right_id: u64, mut qty: f64, price: f64, now: u64) {
        let (buy_id, sell_id) = {
            let left = &self.board.orders[&left_id];
            if left.side == OrderSide::Buy {
                (left_id, right_id)
            } else {
                (right_id, left_id)
            }
        };
        let buy = self.board.orders[&buy_id].clone();
        let sell = self.board.orders[&sell_id].clone();
        if !price.is_finite() || price <= 0.0 {
            return;
        }
        let affordable = buy.escrow_metal / price;
        if qty > affordable + 1e-12 {
            qty = affordable;
        }
        if qty <= 1e-9 {
            return;
        }
        let metal = qty * price;
        let refund = if buy.is_market {
            0.0
        } else {
            ((buy.price - price) * qty).max(0.0)
        };
        if let Some(order) = self.board.orders.get_mut(&buy_id) {
            order.remaining = (order.remaining - qty).max(0.0);
            order.escrow_metal = (order.escrow_metal - metal - refund).max(0.0);
        }
        if let Some(order) = self.board.orders.get_mut(&sell_id) {
            order.remaining = (order.remaining - qty).max(0.0);
            order.escrow_good = (order.escrow_good - qty).max(0.0);
        }
        let mut good_lot = Resources::ZERO;
        good_lot.set(buy.good, qty);
        self.credit_trader(buy.trader, buy.region, good_lot);
        if refund > 1e-12 {
            self.credit_trader(
                buy.trader,
                buy.region,
                Resources {
                    iron: refund,
                    ..Resources::ZERO
                },
            );
        }
        self.credit_trader(
            sell.trader,
            sell.region,
            Resources {
                iron: metal,
                ..Resources::ZERO
            },
        );
        if let Some(region) = self.board.regions.get_mut(&buy.region) {
            match buy.good {
                ResourceKind::Silicon => region.last_crystal = Some(price),
                ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                    region.last_deuterium = Some(price)
                }
                ResourceKind::Iron | ResourceKind::RareEarths => {}
            }
        }
        let impact = TradeQuote {
            pay: ResourceKind::Iron,
            recv: buy.good,
            pay_amount: metal,
            recv_amount: qty,
            fee: 0.0,
            slip: 0.0,
        };
        self.market.apply_trade_impact(&impact, &self.config.market);
        if buy.trader == TraderId::Player || sell.trader == TraderId::Player {
            self.apply_faction_delta("traders", |f| f.trade_delta, "trade", now);
            self.note_daily(crate::DailyAction::Trade, now);
            self.push_event(
                now,
                format!(
                    "market: fill {:.1} {} @ {:.3} metal in {}",
                    qty,
                    buy.good.label(),
                    price,
                    buy.region
                ),
            );
        }
    }

    fn drop_empty_orders(&mut self) {
        let gone: Vec<u64> = self
            .board
            .orders
            .iter()
            .filter(|(_, o)| o.remaining <= 1e-9)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some(order) = self.board.orders.remove(&id) {
                self.refund_order(&order);
            }
        }
    }

    fn cancel_order(&mut self, id: u64, _now: u64) {
        if let Some(order) = self.board.orders.remove(&id) {
            self.refund_order(&order);
        }
    }

    fn expire_orders_at(&mut self, at: u64) {
        let expired: Vec<u64> = self
            .board
            .orders
            .iter()
            .filter(|(_, o)| o.expires_at == Some(at))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(order) = self.board.orders.remove(&id) {
                let trader = order.trader;
                self.refund_order(&order);
                if trader == TraderId::Player {
                    self.push_event(at, format!("market: order #{} expired", order.id));
                }
            }
        }
    }

    fn expire_npc_orders_at(&mut self, at: u64) {
        let expired: Vec<u64> = self
            .board
            .orders
            .iter()
            .filter(|(_, o)| o.trader == TraderId::Npc && o.expires_at.is_some_and(|exp| exp <= at))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(order) = self.board.orders.remove(&id) {
                self.refund_order(&order);
            }
        }
    }

    fn refund_order(&mut self, order: &RestingOrder) {
        if order.escrow_metal > 1e-12 {
            self.credit_trader(
                order.trader,
                order.region,
                Resources {
                    iron: order.escrow_metal,
                    ..Resources::ZERO
                },
            );
        }
        if order.escrow_good > 1e-12 {
            let mut lot = Resources::ZERO;
            lot.set(order.good, order.escrow_good);
            self.credit_trader(order.trader, order.region, lot);
        }
    }

    fn arrive_shipments_at(&mut self, at: u64) {
        let due: Vec<u64> = self
            .board
            .shipments
            .iter()
            .filter(|(_, s)| !s.delivered && s.arrives_at == at)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            let Some(mut ship) = self.board.shipments.remove(&id) else {
                continue;
            };
            self.credit_trader(ship.trader, ship.dest, ship.cargo);
            ship.delivered = true;
            let contract_id = ship.contract_id;
            let dest = ship.dest;
            self.push_event(
                at,
                format!("market: shipment #{} arrived at {dest}", ship.id),
            );
            self.board.shipments.insert(id, ship);
            if let Some(cid) = contract_id {
                let _ = self.fulfil_contract_inner(cid, at);
            } else {
                self.try_auto_fulfil_at(dest, at);
            }
        }
    }

    /// An open contract nobody accepted before its deadline leaves the board.
    /// Nobody holds it, so no penalty is due and nothing is worth keeping;
    /// the NPC tick posts fresh ones. Runs on the market tick, not at each
    /// deadline.
    fn expire_open_contracts_at(&mut self, at: u64) {
        self.board
            .contracts
            .retain(|_, c| !(c.status == ContractStatus::Open && c.deadline <= at));
    }

    fn fail_contracts_at(&mut self, at: u64) {
        let due: Vec<u64> = self
            .board
            .contracts
            .iter()
            .filter(|(_, c)| c.status == ContractStatus::Accepted && c.deadline == at)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            let Some(contract) = self.board.contracts.get(&id).cloned() else {
                continue;
            };
            if contract.remaining <= 1e-9 {
                if let Some(c) = self.board.contracts.get_mut(&id) {
                    c.status = ContractStatus::Fulfilled;
                    c.remaining = 0.0;
                }
                continue;
            }
            let holder = contract.holder.unwrap_or(TraderId::Player);
            let pay_at = if holder == TraderId::Player {
                self.home
            } else {
                contract.region
            };
            let have = self.trader_stock(holder, pay_at).iron;
            let take = contract.penalty.min(have).max(0.0);
            if take > 1e-12 {
                let _ = self.debit_trader(
                    holder,
                    pay_at,
                    Resources {
                        iron: take,
                        ..Resources::ZERO
                    },
                );
                self.credit_trader(
                    contract.issuer,
                    contract.region,
                    Resources {
                        iron: take,
                        ..Resources::ZERO
                    },
                );
            }
            if let Some(c) = self.board.contracts.get_mut(&id) {
                c.status = ContractStatus::Failed;
            }
            self.push_event(
                at,
                format!("market: contract #{id} missed; penalty {take:.0}"),
            );
            self.note_contract_incident(id, at);
        }
    }

    fn fulfil_contract_inner(&mut self, id: u64, now: u64) -> Result<(), MarketError> {
        if !self.config.market.enabled {
            return Err(MarketError::Disabled);
        }
        let contract = self
            .board
            .contracts
            .get(&id)
            .cloned()
            .ok_or(MarketError::UnknownContract)?;
        if contract.status != ContractStatus::Accepted {
            return Err(if contract.status == ContractStatus::Open {
                MarketError::NotAccepted
            } else {
                MarketError::Closed
            });
        }
        let holder = contract.holder.unwrap_or(TraderId::Player);
        let have = self
            .trader_stock(holder, contract.region)
            .get(contract.good);
        let qty = have.min(contract.remaining);
        if qty <= 1e-9 {
            return Err(MarketError::Insufficient {
                missing: contract.remaining,
                kind: contract.good,
            });
        }
        let mut lot = Resources::ZERO;
        lot.set(contract.good, qty);
        self.debit_trader(holder, contract.region, lot)?;
        self.credit_trader(contract.issuer, contract.region, lot);
        let pay = qty * contract.price;
        let issuer_metal = self
            .trader_stock(contract.issuer, contract.region)
            .iron
            .min(pay);
        if issuer_metal > 1e-12 {
            let _ = self.debit_trader(
                contract.issuer,
                contract.region,
                Resources {
                    iron: issuer_metal,
                    ..Resources::ZERO
                },
            );
            self.credit_trader(
                holder,
                contract.region,
                Resources {
                    iron: issuer_metal,
                    ..Resources::ZERO
                },
            );
        }
        if let Some(c) = self.board.contracts.get_mut(&id) {
            c.remaining = (c.remaining - qty).max(0.0);
            if c.remaining <= 1e-9 {
                c.status = ContractStatus::Fulfilled;
                c.remaining = 0.0;
            }
        }
        self.push_event(
            now,
            format!(
                "market: fulfilled {:.1} {} on contract #{id}",
                qty,
                contract.good.label()
            ),
        );
        self.record_history(crate::HistoryDraft {
            at: now,
            kind: crate::HistoryKind::Trade,
            actor: "player".into(),
            action: "fulfil_contract".into(),
            objects: vec![format!("contract:{id}")],
            outcome: format!("{qty:.1} {}", contract.good.as_key()),
            ..crate::HistoryDraft::default()
        });
        if holder == TraderId::Player && issuer_metal > 0.0 {
            let mut paid = Resources::ZERO;
            paid.iron = issuer_metal;
            let amount = self.value_resources(paid);
            self.credit_income(amount, crate::finance::IncomeKind::Contract);
        }
        Ok(())
    }

    fn try_auto_fulfil_at(&mut self, dest: Coord, at: u64) {
        let ids: Vec<u64> = self
            .board
            .contracts
            .values()
            .filter(|c| {
                c.region == dest
                    && c.status == ContractStatus::Accepted
                    && c.holder == Some(TraderId::Player)
            })
            .map(|c| c.id)
            .collect();
        for id in ids {
            let _ = self.fulfil_contract_inner(id, at);
        }
    }

    fn npc_market_tick(&mut self, at: u64) {
        let hub = self.config.market.hub();
        let home = self.home;
        let size = self.config.market.npc_order_size;
        if size <= 0.0 {
            return;
        }
        for venue in [hub, home] {
            self.ensure_region(venue);
            let live_npc: usize = self
                .board
                .orders
                .values()
                .filter(|o| o.trader == TraderId::Npc && o.region == venue && o.remaining > 1e-9)
                .count();
            if live_npc >= 4 {
                continue;
            }
            for good in [ResourceKind::Silicon, ResourceKind::Deuterium] {
                let mid = self.metal_price(good);
                let has_good = self.trader_stock(TraderId::Npc, venue).get(good);
                let has_metal = self.trader_stock(TraderId::Npc, venue).iron;
                if has_good + 1e-9 >= size {
                    let _ = self.place_order_as(
                        TraderId::Npc,
                        venue,
                        OrderSide::Sell,
                        good,
                        size.min(has_good),
                        Some(mid * 1.02),
                        self.config.market.order_ttl_secs,
                        at,
                    );
                }
                if has_metal + 1e-9 >= size * mid * 0.98 {
                    let _ = self.place_order_as(
                        TraderId::Npc,
                        venue,
                        OrderSide::Buy,
                        good,
                        size,
                        Some(mid * 0.98),
                        self.config.market.order_ttl_secs,
                        at,
                    );
                }
            }
        }
        let open = self
            .board
            .contracts
            .values()
            .filter(|c| !c.status.is_terminal())
            .count();
        if open < 2 {
            self.post_npc_contract(ContractKind::Delivery, ResourceKind::Silicon, at);
            self.post_npc_contract(ContractKind::Procurement, ResourceKind::Deuterium, at);
        }
    }

    fn post_npc_contract(&mut self, kind: ContractKind, good: ResourceKind, at: u64) {
        let hub = self.config.market.hub();
        let amount = self.config.market.npc_order_size;
        if amount <= 0.0 {
            return;
        }
        let price = self.metal_price(good);
        let penalty = amount * price * self.config.market.contract_penalty.max(0.0);
        let deadline = at.saturating_add(self.config.market.tick_secs.saturating_mul(12));
        let id = self.board.next_contract_id.max(1);
        self.board.next_contract_id = id.saturating_add(1);
        self.board.contracts.insert(
            id,
            MarketContract {
                id,
                kind,
                region: hub,
                good,
                amount,
                remaining: amount,
                price,
                deadline,
                penalty,
                issuer: TraderId::Npc,
                holder: None,
                status: ContractStatus::Open,
            },
        );
    }

    fn record_region_mids(&mut self) {
        let cap = self.config.market.history_len.max(1);
        let mids: Vec<(ResourceKind, f64)> = ResourceKind::QUOTED
            .iter()
            .map(|&kind| (kind, self.market.price(kind)))
            .collect();
        for region in self.board.regions.values_mut() {
            for (kind, mid) in &mids {
                let px = match kind {
                    ResourceKind::Silicon => region.last_crystal.unwrap_or(*mid),
                    ResourceKind::Deuterium | ResourceKind::WaterIce | ResourceKind::Helium3 => {
                        region.last_deuterium.unwrap_or(*mid)
                    }
                    ResourceKind::Iron | ResourceKind::RareEarths => *mid,
                };
                region.push_price(*kind, px, cap);
            }
        }
    }

    fn ensure_region(&mut self, coord: Coord) {
        if self.board.regions.contains_key(&coord) {
            return;
        }
        let cap = self.config.market.history_len.max(1);
        let mut region = RegionMarket {
            coord,
            npc_warehouse: Resources::ZERO,
            player_warehouse: Resources::ZERO,
            metal_hist: Vec::new(),
            crystal_hist: Vec::new(),
            deuterium_hist: Vec::new(),
            last_crystal: None,
            last_deuterium: None,
        };
        region.npc_warehouse = if coord == self.config.market.hub() {
            Resources::from_parts(
                self.config.market.hub_metal,
                self.config.market.hub_crystal,
                self.config.market.hub_deuterium,
            )
        } else {
            Resources::ZERO
        };
        for kind in ResourceKind::QUOTED {
            region.push_price(kind, self.market.price(kind), cap);
        }
        self.board.regions.insert(coord, region);
    }

    fn metal_price(&self, good: ResourceKind) -> f64 {
        let metal = self.market.price(ResourceKind::Iron).max(1e-9);
        (self.market.price(good) / metal).max(1e-9)
    }

    pub(crate) fn buy_order_escrow(
        &self,
        good: ResourceKind,
        amount: f64,
        limit: Option<f64>,
    ) -> f64 {
        match limit {
            Some(price) if price.is_finite() && price > 0.0 => amount * price,
            Some(_) => 0.0,
            None => amount * self.metal_price(good) * self.config.market.max_mult.max(1.0),
        }
    }

    pub(crate) fn shipment_fuel(&self, origin: Coord, dest: Coord, cargo: Resources) -> f64 {
        if origin == dest || !cargo_ok(cargo) {
            return 0.0;
        }
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, cargo_ships(self, cargo));
        self.flight_from(origin, dest, ships, 100)
            .map(|(_, fuel)| fuel)
            .unwrap_or(0.0)
    }

    fn trader_stock(&self, trader: TraderId, coord: Coord) -> Resources {
        match trader {
            TraderId::Npc => self
                .board
                .regions
                .get(&coord)
                .map(|r| r.npc_warehouse)
                .unwrap_or(Resources::ZERO),
            TraderId::Player => {
                let warehouse = self
                    .board
                    .regions
                    .get(&coord)
                    .map(|r| r.player_warehouse)
                    .unwrap_or(Resources::ZERO);
                if coord == self.home {
                    self.resources.saturating_add(warehouse)
                } else if let Some(colony) = self.colonies.iter().find(|c| c.coord == coord) {
                    colony.resources.saturating_add(warehouse)
                } else {
                    warehouse
                }
            }
        }
    }

    fn debit_trader(
        &mut self,
        trader: TraderId,
        coord: Coord,
        cost: Resources,
    ) -> Result<(), MarketError> {
        let have = self.trader_stock(trader, coord);
        for kind in ResourceKind::ALL {
            let need = cost.get(kind);
            if need > 1e-12 && have.get(kind) + 1e-9 < need {
                return Err(MarketError::Insufficient {
                    missing: need - have.get(kind),
                    kind,
                });
            }
        }
        if trader == TraderId::Player {
            self.take_player_at(coord, cost);
        } else {
            self.apply_trader_delta(trader, coord, |stock| {
                *stock = stock.saturating_sub(cost);
            });
        }
        Ok(())
    }

    fn take_player_at(&mut self, coord: Coord, mut cost: Resources) {
        let stock = if coord == self.home {
            Some(&mut self.resources)
        } else {
            self.colonies
                .iter_mut()
                .find(|c| c.coord == coord)
                .map(|c| &mut c.resources)
        };
        if let Some(stock) = stock {
            let mut from = Resources::ZERO;
            for kind in ResourceKind::ALL {
                from.set(kind, cost.get(kind).min(stock.get(kind)));
            }
            *stock = stock.saturating_sub(from);
            cost = cost.saturating_sub(from);
        }
        if cost.total() > 1e-12 {
            self.ensure_region(coord);
            if let Some(region) = self.board.regions.get_mut(&coord) {
                region.player_warehouse = region.player_warehouse.saturating_sub(cost);
            }
        }
    }

    fn credit_trader(&mut self, trader: TraderId, coord: Coord, found: Resources) {
        if trader == TraderId::Player
            && (coord == self.home || self.colonies.iter().any(|c| c.coord == coord))
        {
            let before = self.trader_stock(TraderId::Player, coord);
            self.credit_at(coord, found);
            let after = self.trader_stock(TraderId::Player, coord);
            let mut overflow = Resources::ZERO;
            for kind in ResourceKind::ALL {
                let gained = after.get(kind) - before.get(kind);
                overflow.set(kind, (found.get(kind) - gained).max(0.0));
            }
            if overflow.total() > 1e-12 {
                self.ensure_region(coord);
                if let Some(region) = self.board.regions.get_mut(&coord) {
                    region.player_warehouse = region.player_warehouse.saturating_add(overflow);
                }
            }
            return;
        }
        self.ensure_region(coord);
        self.apply_trader_delta(trader, coord, |stock| {
            *stock = stock.saturating_add(found);
        });
    }

    fn apply_trader_delta(
        &mut self,
        trader: TraderId,
        coord: Coord,
        f: impl FnOnce(&mut Resources),
    ) {
        match trader {
            TraderId::Npc => {
                self.ensure_region(coord);
                if let Some(region) = self.board.regions.get_mut(&coord) {
                    f(&mut region.npc_warehouse);
                }
            }
            TraderId::Player => {
                if coord == self.home {
                    f(&mut self.resources);
                } else if let Some(colony) = self.colonies.iter_mut().find(|c| c.coord == coord) {
                    f(&mut colony.resources);
                } else {
                    self.ensure_region(coord);
                    if let Some(region) = self.board.regions.get_mut(&coord) {
                        f(&mut region.player_warehouse);
                    }
                }
            }
        }
    }
}

fn crosses(incoming: &RestingOrder, resting: &RestingOrder) -> bool {
    match incoming.side {
        OrderSide::Buy => incoming.price + 1e-12 >= resting.price,
        OrderSide::Sell => incoming.price <= resting.price + 1e-12,
    }
}

fn fill_price(incoming: &RestingOrder, resting: &RestingOrder, mid: f64) -> f64 {
    if resting.is_market && incoming.is_market {
        mid
    } else if resting.is_market {
        if incoming.price.is_finite() && incoming.price > 0.0 && incoming.price < f64::MAX / 4.0 {
            incoming.price
        } else {
            mid
        }
    } else {
        resting.price
    }
}

fn cargo_ok(cargo: Resources) -> bool {
    for kind in ResourceKind::ALL {
        let v = cargo.get(kind);
        if !v.is_finite() || v < 0.0 {
            return false;
        }
    }
    cargo.total() > 1e-9
}

fn cargo_ships(state: &GameState, cargo: Resources) -> u32 {
    let mass = cargo.total();
    let cap = state.ship_stats(ShipKind::SmallCargo).cargo.max(1.0);
    (mass / cap).ceil().max(1.0) as u32
}

fn min_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(x), Some(y)) => Some(x.min(y)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceConfig;

    #[test]
    fn prices_stay_inside_configured_bounds() {
        let cfg = MarketConfig::default_active();
        let mut market = MarketState::from_config(&cfg);
        let mut rng = 1u64;
        for _ in 0..10_000 {
            market.tick(&cfg, &mut rng);
        }
        for kind in ResourceKind::QUOTED {
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
            .quote(ResourceKind::Iron, ResourceKind::Silicon, 100.0, &cfg)
            .unwrap();
        let mut after = market.clone();
        after.apply_trade_impact(&buy, &cfg);
        let sell = after
            .quote(
                ResourceKind::Silicon,
                ResourceKind::Iron,
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
            .quote(ResourceKind::Iron, ResourceKind::Silicon, amount, &cfg)
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

    fn ready(now: u64) -> GameState {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market = MarketConfig::default_active();
        cfg.daily = crate::DailyConfig::empty();
        let mut state = GameState::with_config(now, cfg);
        state.resources.iron = 2_000.0;
        state.resources.silicon = 2_000.0;
        state.resources.deuterium = 5_000.0;
        state
    }

    fn stock_npc(state: &mut GameState, coord: Coord, good: ResourceKind, amount: f64) {
        state.ensure_region(coord);
        if let Some(region) = state.board.regions.get_mut(&coord) {
            region.npc_warehouse.set(good, amount);
        }
    }

    #[test]
    fn limit_orders_match_at_the_resting_price() {
        let mut state = ready(1_000);
        let home = Coord::HOME;
        stock_npc(&mut state, home, ResourceKind::Silicon, 10.0);
        state
            .place_order_as(
                TraderId::Npc,
                home,
                OrderSide::Sell,
                ResourceKind::Silicon,
                10.0,
                Some(2.0),
                86_400,
                1_000,
            )
            .unwrap();
        let before_metal = state.resources.iron;
        let before_crystal = state.resources.silicon;
        state
            .place_order(
                home,
                OrderSide::Buy,
                ResourceKind::Silicon,
                10.0,
                Some(2.5),
                86_400,
                1_000,
            )
            .unwrap();
        assert!((state.resources.silicon - (before_crystal + 10.0)).abs() < 1e-6);
        assert!((state.resources.iron - (before_metal - 20.0)).abs() < 1e-6);
        assert!(
            state
                .board
                .orders
                .values()
                .all(|o| o.remaining <= 1e-9 || o.trader != TraderId::Player)
        );
    }

    #[test]
    fn partial_fill_leaves_the_rest_on_the_book() {
        let mut state = ready(1_000);
        let home = Coord::HOME;
        stock_npc(&mut state, home, ResourceKind::Silicon, 4.0);
        state
            .place_order_as(
                TraderId::Npc,
                home,
                OrderSide::Sell,
                ResourceKind::Silicon,
                4.0,
                Some(2.0),
                86_400,
                1_000,
            )
            .unwrap();
        state
            .place_order(
                home,
                OrderSide::Buy,
                ResourceKind::Silicon,
                10.0,
                Some(2.0),
                86_400,
                1_000,
            )
            .unwrap();
        let leftover = state
            .board
            .orders
            .values()
            .find(|o| o.trader == TraderId::Player)
            .expect("resting buy");
        assert!((leftover.remaining - 6.0).abs() < 1e-6);
    }

    #[test]
    fn expired_order_refunds_escrow_in_catch_up() {
        let mut state = ready(1_000);
        let home = Coord::HOME;
        let metal = state.resources.iron;
        state
            .place_order(
                home,
                OrderSide::Buy,
                ResourceKind::Silicon,
                5.0,
                Some(2.0),
                10,
                1_000,
            )
            .unwrap();
        let escrowed = metal - state.resources.iron;
        assert!(escrowed > 9.0);
        let after_place = state.resources.iron;
        state.catch_up(1_010);
        assert!(state.resources.iron + 1e-6 >= after_place + escrowed);
        assert!(state.board.orders.is_empty());
    }

    #[test]
    fn same_seed_replays_the_same_fills() {
        fn run(seed: u64) -> (f64, f64, usize) {
            let mut cfg = BalanceConfig::default();
            cfg.raid.enabled = false;
            cfg.market = MarketConfig::default_active();
            let mut state = GameState::with_seed(1_000, cfg, seed);
            state.resources.iron = 50_000.0;
            state.resources.silicon = 50_000.0;
            state.resources.deuterium = 50_000.0;
            let home = Coord::HOME;
            state.catch_up(1_000 + state.config.market.tick_secs);
            let _ = state.place_order(
                home,
                OrderSide::Buy,
                ResourceKind::Silicon,
                8.0,
                Some(3.0),
                86_400,
                state.last_tick_unix,
            );
            (
                state.resources.iron,
                state.resources.silicon,
                state.board.orders.len(),
            )
        }
        assert_eq!(run(7), run(7));
    }

    #[test]
    fn shipment_takes_transit_time_and_fuel() {
        let mut state = ready(1_000);
        let hub = state.config.market.hub();
        let metal = state.resources.iron;
        let deut = state.resources.deuterium;
        let id = state
            .ship_goods(
                Coord::HOME,
                hub,
                Resources {
                    iron: 20.0,
                    ..Resources::ZERO
                },
                None,
                1_000,
            )
            .unwrap();
        assert!(state.resources.iron < metal);
        assert!(state.resources.deuterium < deut);
        let ship = &state.board.shipments[&id];
        assert!(ship.arrives_at > 1_000);
        assert!(!ship.delivered);
        let eta = ship.arrives_at;
        state.catch_up(eta);
        assert!(state.board.shipments[&id].delivered);
        let parked = state.board.regions[&hub].player_warehouse.iron;
        assert!((parked - 20.0).abs() < 1e-6);
    }

    #[test]
    fn missed_contract_pays_the_penalty() {
        let mut state = ready(1_000);
        let hub = state.config.market.hub();
        state.board.next_contract_id = 1;
        state.board.contracts.insert(
            1,
            MarketContract {
                id: 1,
                kind: ContractKind::Delivery,
                region: hub,
                good: ResourceKind::Silicon,
                amount: 10.0,
                remaining: 10.0,
                price: 2.0,
                deadline: 1_050,
                penalty: 40.0,
                issuer: TraderId::Npc,
                holder: Some(TraderId::Player),
                status: ContractStatus::Accepted,
            },
        );
        state.ensure_region(hub);
        let metal = state.resources.iron;
        state.catch_up(1_050);
        assert_eq!(state.board.contracts[&1].status, ContractStatus::Failed);
        let after = state.resources.iron;
        assert!(after < metal);
    }

    #[test]
    fn an_open_contract_nobody_took_expires_after_its_deadline() {
        let mut state = ready(1_000);
        let hub = state.config.market.hub();
        state.board.next_contract_id = 2;
        state.board.contracts.insert(
            1,
            MarketContract {
                id: 1,
                kind: ContractKind::Delivery,
                region: hub,
                good: ResourceKind::Silicon,
                amount: 10.0,
                remaining: 10.0,
                price: 2.0,
                deadline: 1_050,
                penalty: 40.0,
                issuer: TraderId::Npc,
                holder: None,
                status: ContractStatus::Open,
            },
        );
        let iron = state.resources.iron;
        state.catch_up(1_050 + state.config.market.tick_secs);
        assert!(!state.board.contracts.contains_key(&1));
        assert!(
            state.resources.iron >= iron,
            "no penalty for a contract nobody held"
        );
        state.catch_up(1_050 + 40 * state.config.market.tick_secs);
        assert!(
            state.board.contracts.len() <= 4,
            "expired contracts do not pile up: {}",
            state.board.contracts.len()
        );
    }

    #[test]
    fn trade_blocker_names_missing_stock_and_cargo_before_the_trade() {
        let mut state = ready(1_000);
        state.config.market.cargo_free = 100.0;
        state.resources.silicon = 4_000.0;
        let quote = state
            .quote_market(ResourceKind::Silicon, ResourceKind::Deuterium, 800.0)
            .unwrap();
        assert!(matches!(
            state.trade_blocker(&quote),
            Some(MarketError::NeedCargo { need, .. }) if (need - 800.0).abs() < 1e-9
        ));
        assert_eq!(state.max_trade_amount(ResourceKind::Silicon), 100.0);
        state.resources.deuterium = 95.0;
        let quote = state
            .quote_market(ResourceKind::Deuterium, ResourceKind::Iron, 800.0)
            .unwrap();
        assert!(matches!(
            state.trade_blocker(&quote),
            Some(MarketError::Insufficient {
                kind: ResourceKind::Deuterium,
                ..
            })
        ));
        let small = state
            .quote_market(ResourceKind::Silicon, ResourceKind::Deuterium, 50.0)
            .unwrap();
        assert!(state.trade_blocker(&small).is_none());
    }

    #[test]
    fn contract_shipment_must_carry_the_contracted_good() {
        let mut state = ready(1_000);
        let hub = state.config.market.hub();
        state.board.next_contract_id = 2;
        state.board.contracts.insert(
            1,
            MarketContract {
                id: 1,
                kind: ContractKind::Delivery,
                region: hub,
                good: ResourceKind::Silicon,
                amount: 10.0,
                remaining: 10.0,
                price: 2.0,
                deadline: 10_000,
                penalty: 40.0,
                issuer: TraderId::Npc,
                holder: Some(TraderId::Player),
                status: ContractStatus::Accepted,
            },
        );
        let wrong = state.ship_goods(
            Coord::HOME,
            hub,
            Resources {
                iron: 10.0,
                ..Resources::ZERO
            },
            Some(1),
            1_000,
        );
        assert!(matches!(
            wrong,
            Err(MarketError::Insufficient {
                kind: ResourceKind::Silicon,
                ..
            })
        ));
        let extra = state.ship_goods(
            Coord::HOME,
            hub,
            Resources {
                silicon: 20.0,
                ..Resources::ZERO
            },
            Some(1),
            1_000,
        );
        assert_eq!(extra, Err(MarketError::BadAmount));
    }

    #[test]
    fn npc_orders_use_the_same_place_order_path() {
        let mut state = ready(1_000);
        state.catch_up(1_000 + state.config.market.tick_secs);
        assert!(
            state
                .board
                .orders
                .values()
                .any(|o| o.trader == TraderId::Npc)
                || state
                    .board
                    .contracts
                    .values()
                    .any(|c| c.issuer == TraderId::Npc)
        );
    }

    #[test]
    fn fill_overflow_parks_in_the_region_warehouse() {
        let mut state = ready(1_000);
        let home = Coord::HOME;
        let cap = state.storage_cap(ResourceKind::Silicon);
        state.resources.silicon = cap;
        stock_npc(&mut state, home, ResourceKind::Silicon, 10.0);
        state
            .place_order_as(
                TraderId::Npc,
                home,
                OrderSide::Sell,
                ResourceKind::Silicon,
                10.0,
                Some(2.0),
                86_400,
                1_000,
            )
            .unwrap();
        let metal = state.resources.iron;
        state
            .place_order(
                home,
                OrderSide::Buy,
                ResourceKind::Silicon,
                10.0,
                Some(2.5),
                86_400,
                1_000,
            )
            .unwrap();
        assert!((state.resources.silicon - cap).abs() < 1e-6);
        assert!((state.board.regions[&home].player_warehouse.silicon - 10.0).abs() < 1e-6);
        assert!(state.resources.iron < metal);
    }

    #[test]
    fn market_buy_cannot_mint_metal_against_a_high_ask() {
        let mut state = ready(1_000);
        let home = Coord::HOME;
        stock_npc(&mut state, home, ResourceKind::Silicon, 10.0);
        state
            .place_order_as(
                TraderId::Npc,
                home,
                OrderSide::Sell,
                ResourceKind::Silicon,
                10.0,
                Some(1_000_000.0),
                86_400,
                1_000,
            )
            .unwrap();
        let pool = |s: &GameState| {
            let region = &s.board.regions[&home];
            let escrow: f64 = s.board.orders.values().map(|o| o.escrow_metal).sum();
            s.resources.iron + region.player_warehouse.iron + region.npc_warehouse.iron + escrow
        };
        let before = pool(&state);
        state
            .place_order(
                home,
                OrderSide::Buy,
                ResourceKind::Silicon,
                10.0,
                None,
                86_400,
                1_000,
            )
            .unwrap();
        assert!((pool(&state) - before).abs() < 1e-6);
    }

    #[test]
    fn warehouse_overflow_can_be_sold() {
        let mut state = ready(1_000);
        let home = Coord::HOME;
        state.ensure_region(home);
        state
            .board
            .regions
            .get_mut(&home)
            .unwrap()
            .player_warehouse
            .silicon = 10.0;
        state.resources.silicon = 0.0;
        state
            .place_order(
                home,
                OrderSide::Sell,
                ResourceKind::Silicon,
                5.0,
                Some(2.0),
                86_400,
                1_000,
            )
            .unwrap();
        assert!((state.board.regions[&home].player_warehouse.silicon - 5.0).abs() < 1e-6);
        assert!(state.resources.silicon.abs() < 1e-9);
    }

    #[test]
    fn only_the_hub_starts_with_npc_stock() {
        let mut state = ready(1_000);
        let hub = state.config.market.hub();
        let elsewhere = Coord {
            galaxy: 2,
            system: 3,
            position: 4,
        };
        state.ensure_region(hub);
        state.ensure_region(elsewhere);
        assert!(state.board.regions[&hub].npc_warehouse.iron > 0.0);
        assert_eq!(
            state.board.regions[&elsewhere].npc_warehouse,
            Resources::ZERO
        );
    }

    #[test]
    fn long_gap_equals_step_ticks_for_expiry() {
        let mut gap = ready(1_000);
        let mut step = ready(1_000);
        let home = Coord::HOME;
        gap.place_order(
            home,
            OrderSide::Buy,
            ResourceKind::Silicon,
            3.0,
            Some(2.0),
            25,
            1_000,
        )
        .unwrap();
        step.place_order(
            home,
            OrderSide::Buy,
            ResourceKind::Silicon,
            3.0,
            Some(2.0),
            25,
            1_000,
        )
        .unwrap();
        gap.catch_up(1_040);
        let mut t = 1_001;
        while t <= 1_040 {
            step.catch_up(t);
            t += 1;
        }
        assert!((gap.resources.iron - step.resources.iron).abs() < 1e-6);
        assert_eq!(gap.board.orders.len(), step.board.orders.len());
    }
}
