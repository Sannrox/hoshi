//! The shard's NPC regional market: one host-owned book per region that
//! every tenant's orders rest on and fill against.
//!
//! Local saves keep their private [`crate::MarketBoard`]. On a shard the
//! host holds this book inside [`crate::ShardCommons`]: orders carry the
//! placing tenant, fills settle into that tenant's `GameState`, and the NPC
//! quotes and the price walk tick once on the host clock from an RNG seeded
//! by the shard map seed. Matching and pricing are the local code
//! ([`match_order`], [`npc_tickets`]); only settlement differs.

use crate::fleets::Coord;
use crate::intent::IntentError;
use crate::market::{
    MarketConfig, MarketError, MarketState, NPC_GOODS, NPC_VENUE_ORDERS, OrderSide, OrderTicket,
    RegionMarket, RestingOrder, TraderId, covers, match_order, npc_tickets,
};
use crate::shard::ShardError;
use crate::state::{GameState, ResourceKind, Resources};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Who placed an order on the host book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ShardTrader {
    Npc,
    Tenant(String),
}

/// The host's regional books. Closed until the first
/// [`crate::ShardSession::join`] opens it with the shard map seed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShardMarket {
    book: Option<Book>,
}

#[derive(Debug, Clone, PartialEq)]
struct Book {
    /// One price walk for the shard, like a local save's `market`.
    prices: MarketState,
    /// NPC warehouses and price history per region. `player_warehouse`
    /// stays empty here: a tenant's goods live in its own save.
    regions: BTreeMap<Coord, RegionMarket>,
    orders: BTreeMap<u64, RestingOrder<ShardTrader>>,
    next_order_id: u64,
    rng: u64,
    /// Host time of the last NPC tick.
    last_tick: u64,
    /// Host time the book has run through.
    advanced_to: u64,
}

/// One region's book as a tenant reads it. Other orders carry no identity.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardBook {
    pub region: Coord,
    pub at: u64,
    /// Silicon's mid in iron.
    pub silicon: f64,
    /// Deuterium's mid in iron; water ice and helium-3 trade on it too.
    pub deuterium: f64,
    /// Silicon and deuterium price history in the region, one entry per
    /// NPC tick. Empty until the region has a book.
    pub silicon_hist: Vec<f64>,
    pub deuterium_hist: Vec<f64>,
    /// Resting orders, by id.
    pub orders: Vec<ShardBookOrder>,
}

/// One resting order on a [`ShardBook`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShardBookOrder {
    pub id: u64,
    pub side: OrderSide,
    pub good: ResourceKind,
    pub price: f64,
    pub remaining: f64,
    /// The reading tenant placed it.
    pub own: bool,
}

impl ShardMarket {
    pub fn is_open(&self) -> bool {
        self.book.is_some()
    }

    /// Open the book at `now` with `seed` for the NPC walk. An open book is
    /// kept, so a reconnect or a restart does not reseed it.
    pub(crate) fn open(&mut self, seed: u64, now: u64, cfg: &MarketConfig) {
        if self.book.is_some() {
            return;
        }
        let prices = MarketState::from_config(cfg);
        let hub = cfg.hub();
        let regions = BTreeMap::from([(hub, RegionMarket::open(hub, &prices, cfg))]);
        self.book = Some(Book {
            prices,
            regions,
            orders: BTreeMap::new(),
            next_order_id: 1,
            rng: seed,
            last_tick: now,
            advanced_to: now,
        });
    }

    /// Earliest book event due by `now` and not yet run: an NPC tick or a
    /// tenant order's expiry. The host stops catch-up there.
    pub(crate) fn next_due(&self, now: u64, cfg: &MarketConfig) -> Option<u64> {
        let book = self.book.as_ref().filter(|_| cfg.enabled)?;
        let tick = book.last_tick.saturating_add(cfg.tick_secs.max(1));
        book.orders
            .values()
            .filter(|o| matches!(o.trader, ShardTrader::Tenant(_)))
            .filter_map(|o| o.expires_at)
            .chain(std::iter::once(tick))
            .filter(|&at| at > book.advanced_to && at <= now)
            .min()
    }

    /// Whether a book event is due by `now` while `identity` has an order
    /// on the book. A lone catch-up of that tenant would run past a fill
    /// or refund the host owes it, so `submit` refuses until a settle.
    pub(crate) fn due_for(&self, identity: &str, now: u64, cfg: &MarketConfig) -> bool {
        let resting = self.book.as_ref().is_some_and(|book| {
            book.orders
                .values()
                .any(|o| matches!(&o.trader, ShardTrader::Tenant(id) if id == identity))
        });
        resting && self.next_due(now, cfg).is_some()
    }

    /// Run the book to `at`: expire tenant orders, then each NPC tick due.
    /// Every empire has stepped to `at` first, so refunds and fills land
    /// at that time. A tenant the host has not loaded keeps its orders off
    /// the matching and its expiry waits until it is back.
    pub(crate) fn advance(
        &mut self,
        at: u64,
        empires: &mut BTreeMap<String, GameState>,
        cfg: &MarketConfig,
    ) {
        let Some(book) = self.book.as_mut() else {
            return;
        };
        if cfg.enabled {
            book.expire_tenant_orders(at, empires, cfg);
            let step = cfg.tick_secs.max(1);
            while book.last_tick.saturating_add(step) <= at {
                let tick = book.last_tick.saturating_add(step);
                book.npc_tick(tick, empires, cfg);
                book.last_tick = tick;
            }
        }
        book.advanced_to = book.advanced_to.max(at);
    }

    /// Place `ticket` for tenant `identity` at `now` and match it. The
    /// caller has stepped every empire to `now`.
    pub(crate) fn place(
        &mut self,
        identity: &str,
        ticket: OrderTicket,
        empires: &mut BTreeMap<String, GameState>,
        now: u64,
        cfg: &MarketConfig,
    ) -> Result<u64, ShardError> {
        let book = self.book.as_mut().ok_or(ShardError::NoBook)?;
        book.place_as(
            ShardTrader::Tenant(identity.to_string()),
            ticket,
            empires,
            now,
            cfg,
        )
    }

    /// `region` as `identity` reads it at `at`. `None` while the book is closed.
    pub(crate) fn read(&self, identity: &str, region: Coord, at: u64) -> Option<ShardBook> {
        let book = self.book.as_ref()?;
        let hist = |kind| {
            book.regions
                .get(&region)
                .map(|r| r.history(kind).to_vec())
                .unwrap_or_default()
        };
        Some(ShardBook {
            region,
            at,
            silicon: book.prices.metal_price(ResourceKind::Silicon),
            deuterium: book.prices.metal_price(ResourceKind::Deuterium),
            silicon_hist: hist(ResourceKind::Silicon),
            deuterium_hist: hist(ResourceKind::Deuterium),
            orders: book
                .orders
                .values()
                .filter(|o| o.region == region)
                .map(|o| ShardBookOrder {
                    id: o.id,
                    side: o.side,
                    good: o.good,
                    price: o.price,
                    remaining: o.remaining,
                    own: matches!(&o.trader, ShardTrader::Tenant(id) if id == identity),
                })
                .collect(),
        })
    }
}

impl Book {
    fn place_as(
        &mut self,
        trader: ShardTrader,
        ticket: OrderTicket,
        empires: &mut BTreeMap<String, GameState>,
        now: u64,
        cfg: &MarketConfig,
    ) -> Result<u64, ShardError> {
        if !cfg.enabled {
            return Err(market_err(MarketError::Disabled));
        }
        self.ensure_region(ticket.region, cfg);
        let id = self.next_order_id.max(1);
        let order = RestingOrder::open(trader.clone(), ticket, id, now, &self.prices, cfg)
            .map_err(market_err)?;
        let escrow = order.escrow();
        match &trader {
            ShardTrader::Npc => {
                let npc = self.npc_stock(ticket.region, cfg);
                covers(*npc, escrow).map_err(market_err)?;
                *npc = npc.saturating_sub(escrow);
            }
            ShardTrader::Tenant(identity) => empires
                .get_mut(identity)
                .ok_or_else(|| ShardError::UnknownIdentity {
                    identity: identity.clone(),
                })?
                .take_order_escrow(ticket.region, escrow)
                .map_err(market_err)?,
        }
        self.next_order_id = id.saturating_add(1);
        let is_market = order.is_market;
        self.orders.insert(id, order);
        let matched = match_order(&mut self.orders, id, &mut self.prices, cfg, |t| match t {
            ShardTrader::Npc => true,
            ShardTrader::Tenant(identity) => empires.contains_key(identity),
        });
        for fill in &matched.fills {
            self.credit(&fill.buyer, fill.region, fill.goods(), empires, cfg);
            self.credit(&fill.buyer, fill.region, iron(fill.refund), empires, cfg);
            self.credit(&fill.seller, fill.region, iron(fill.metal()), empires, cfg);
            if let Some(region) = self.regions.get_mut(&fill.region) {
                region.note_fill(fill.good, fill.price);
            }
            for side in [&fill.buyer, &fill.seller] {
                if let ShardTrader::Tenant(identity) = side
                    && let Some(state) = empires.get_mut(identity)
                {
                    state.note_fill(fill.good, fill.qty, fill.price, fill.region, now);
                }
            }
        }
        for order in &matched.emptied {
            self.refund(order, empires, cfg);
        }
        if let Some(order) = self.orders.get(&id)
            && order.is_market
            && order.remaining > 1e-9
        {
            let unfilled = (order.remaining - ticket.amount).abs() <= 1e-9;
            if let Some(order) = self.orders.remove(&id) {
                self.refund(&order, empires, cfg);
            }
            if unfilled && trader != ShardTrader::Npc {
                return Err(market_err(MarketError::NoCounterparty {
                    region: ticket.region,
                    side: ticket.side,
                }));
            }
        }
        if let ShardTrader::Tenant(identity) = &trader
            && let Some(state) = empires.get_mut(identity)
        {
            state.note_order_placed(id, ticket, is_market, now);
        }
        Ok(id)
    }

    /// One NPC tick at `at`, in the local order: walk prices, open the hub
    /// and every tenant home, expire NPC orders, record history, then quote
    /// in each venue. NPC quotes match resting tenant orders like any other.
    fn npc_tick(&mut self, at: u64, empires: &mut BTreeMap<String, GameState>, cfg: &MarketConfig) {
        self.prices.tick(cfg, &mut self.rng);
        let mut venues = vec![cfg.hub()];
        for state in empires.values() {
            if !venues.contains(&state.home) {
                venues.push(state.home);
            }
        }
        for &venue in &venues {
            self.ensure_region(venue, cfg);
        }
        let expired: Vec<u64> = self
            .orders
            .values()
            .filter(|o| o.trader == ShardTrader::Npc && o.expires_at.is_some_and(|exp| exp <= at))
            .map(|o| o.id)
            .collect();
        for id in expired {
            if let Some(order) = self.orders.remove(&id) {
                self.refund(&order, empires, cfg);
            }
        }
        for region in self.regions.values_mut() {
            region.record_mid(&self.prices, cfg);
        }
        if cfg.npc_order_size <= 0.0 {
            return;
        }
        for venue in venues {
            let live = self
                .orders
                .values()
                .filter(|o| o.trader == ShardTrader::Npc && o.region == venue && o.remaining > 1e-9)
                .count();
            if live >= NPC_VENUE_ORDERS {
                continue;
            }
            for good in NPC_GOODS {
                let stock = *self.npc_stock(venue, cfg);
                for ticket in npc_tickets(venue, good, stock, &self.prices, cfg) {
                    let _ = self.place_as(ShardTrader::Npc, ticket, empires, at, cfg);
                }
            }
        }
    }

    /// Refund tenant orders past their expiry. An order of a tenant the
    /// host has not loaded stays until that tenant is back.
    fn expire_tenant_orders(
        &mut self,
        at: u64,
        empires: &mut BTreeMap<String, GameState>,
        cfg: &MarketConfig,
    ) {
        let expired: Vec<u64> = self
            .orders
            .values()
            .filter(|o| o.expires_at.is_some_and(|exp| exp <= at))
            .filter(|o| matches!(&o.trader, ShardTrader::Tenant(id) if empires.contains_key(id)))
            .map(|o| o.id)
            .collect();
        for id in expired {
            if let Some(order) = self.orders.remove(&id) {
                self.refund(&order, empires, cfg);
                if let ShardTrader::Tenant(identity) = &order.trader
                    && let Some(state) = empires.get_mut(identity)
                {
                    state.push_event(at, format!("market: order #{id} expired"));
                }
            }
        }
    }

    fn refund(
        &mut self,
        order: &RestingOrder<ShardTrader>,
        empires: &mut BTreeMap<String, GameState>,
        cfg: &MarketConfig,
    ) {
        self.credit(&order.trader, order.region, order.escrow(), empires, cfg);
    }

    /// Credit `lot` to `trader` in `region`: the NPC warehouse, or the
    /// tenant's planet there and its own warehouse for the rest.
    fn credit(
        &mut self,
        trader: &ShardTrader,
        region: Coord,
        lot: Resources,
        empires: &mut BTreeMap<String, GameState>,
        cfg: &MarketConfig,
    ) {
        if lot.total() <= 1e-12 {
            return;
        }
        match trader {
            ShardTrader::Npc => {
                let npc = self.npc_stock(region, cfg);
                *npc = npc.saturating_add(lot);
            }
            ShardTrader::Tenant(identity) => {
                if let Some(state) = empires.get_mut(identity) {
                    state.credit_trader(TraderId::Player, region, lot);
                }
            }
        }
    }

    /// The NPC warehouse in `region`, opening the region first.
    fn npc_stock(&mut self, region: Coord, cfg: &MarketConfig) -> &mut Resources {
        let prices = &self.prices;
        &mut self
            .regions
            .entry(region)
            .or_insert_with(|| RegionMarket::open(region, prices, cfg))
            .npc_warehouse
    }

    fn ensure_region(&mut self, coord: Coord, cfg: &MarketConfig) {
        self.npc_stock(coord, cfg);
    }
}

fn market_err(err: MarketError) -> ShardError {
    ShardError::Intent(IntentError::Market(err))
}

fn iron(amount: f64) -> Resources {
    Resources {
        iron: amount,
        ..Resources::ZERO
    }
}

/// The open book inside the `ShardCommons` bytes (version 3).
#[derive(Serialize, Deserialize)]
pub(crate) struct BookDoc {
    rng: u64,
    last_tick: u64,
    advanced_to: u64,
    next_order_id: u64,
    prices: PricesDoc,
    regions: Vec<RegionDoc>,
    orders: Vec<OrderDoc>,
}

#[derive(Serialize, Deserialize)]
struct PricesDoc {
    iron: f64,
    silicon: f64,
    deuterium: f64,
    metal_hist: Vec<f64>,
    crystal_hist: Vec<f64>,
    deuterium_hist: Vec<f64>,
}

#[derive(Serialize, Deserialize)]
struct RegionDoc {
    coord: [u16; 3],
    npc: BTreeMap<String, f64>,
    metal_hist: Vec<f64>,
    crystal_hist: Vec<f64>,
    deuterium_hist: Vec<f64>,
    last_crystal: Option<f64>,
    last_deuterium: Option<f64>,
}

#[derive(Serialize, Deserialize)]
struct OrderDoc {
    id: u64,
    region: [u16; 3],
    trader: ShardTrader,
    side: String,
    good: String,
    remaining: f64,
    price: f64,
    escrow_metal: f64,
    escrow_good: f64,
    created_at: u64,
    expires_at: Option<u64>,
    is_market: bool,
}

fn row(coord: Coord) -> [u16; 3] {
    [coord.galaxy, coord.system, coord.position]
}

fn coord([galaxy, system, position]: [u16; 3]) -> Coord {
    Coord {
        galaxy,
        system,
        position,
    }
}

impl ShardMarket {
    /// The book for the commons bytes; `None` while it is closed.
    pub(crate) fn to_doc(&self) -> Option<BookDoc> {
        let book = self.book.as_ref()?;
        let p = &book.prices;
        Some(BookDoc {
            rng: book.rng,
            last_tick: book.last_tick,
            advanced_to: book.advanced_to,
            next_order_id: book.next_order_id,
            prices: PricesDoc {
                iron: p.iron,
                silicon: p.silicon,
                deuterium: p.deuterium,
                metal_hist: p.metal_hist.clone(),
                crystal_hist: p.crystal_hist.clone(),
                deuterium_hist: p.deuterium_hist.clone(),
            },
            regions: book
                .regions
                .values()
                .map(|r| RegionDoc {
                    coord: row(r.coord),
                    npc: ResourceKind::ALL
                        .iter()
                        .filter(|&&kind| r.npc_warehouse.get(kind) != 0.0)
                        .map(|&kind| (kind.as_key().to_string(), r.npc_warehouse.get(kind)))
                        .collect(),
                    metal_hist: r.metal_hist.clone(),
                    crystal_hist: r.crystal_hist.clone(),
                    deuterium_hist: r.deuterium_hist.clone(),
                    last_crystal: r.last_crystal,
                    last_deuterium: r.last_deuterium,
                })
                .collect(),
            orders: book
                .orders
                .values()
                .map(|o| OrderDoc {
                    id: o.id,
                    region: row(o.region),
                    trader: o.trader.clone(),
                    side: o.side.as_key().to_string(),
                    good: o.good.as_key().to_string(),
                    remaining: o.remaining,
                    price: o.price,
                    escrow_metal: o.escrow_metal,
                    escrow_good: o.escrow_good,
                    created_at: o.created_at,
                    expires_at: o.expires_at,
                    is_market: o.is_market,
                })
                .collect(),
        })
    }

    /// Read a book from the commons bytes. An unknown side or resource key
    /// is refused.
    pub(crate) fn from_doc(doc: Option<BookDoc>) -> Result<Self, String> {
        let Some(doc) = doc else {
            return Ok(Self::default());
        };
        let good = |key: &str| ResourceKind::from_key(key).ok_or(format!("unknown good {key}"));
        let mut regions = BTreeMap::new();
        for r in doc.regions {
            let mut npc = Resources::ZERO;
            for (key, amount) in &r.npc {
                npc.set(good(key)?, *amount);
            }
            let at = coord(r.coord);
            regions.insert(
                at,
                RegionMarket {
                    coord: at,
                    npc_warehouse: npc,
                    player_warehouse: Resources::ZERO,
                    metal_hist: r.metal_hist,
                    crystal_hist: r.crystal_hist,
                    deuterium_hist: r.deuterium_hist,
                    last_crystal: r.last_crystal,
                    last_deuterium: r.last_deuterium,
                },
            );
        }
        let mut orders = BTreeMap::new();
        for o in doc.orders {
            let side = OrderSide::from_key(&o.side).ok_or(format!("unknown side {}", o.side))?;
            orders.insert(
                o.id,
                RestingOrder {
                    id: o.id,
                    region: coord(o.region),
                    trader: o.trader,
                    side,
                    good: good(&o.good)?,
                    remaining: o.remaining,
                    price: o.price,
                    escrow_metal: o.escrow_metal,
                    escrow_good: o.escrow_good,
                    created_at: o.created_at,
                    expires_at: o.expires_at,
                    is_market: o.is_market,
                },
            );
        }
        let p = doc.prices;
        Ok(Self {
            book: Some(Book {
                prices: MarketState {
                    iron: p.iron,
                    silicon: p.silicon,
                    deuterium: p.deuterium,
                    metal_hist: p.metal_hist,
                    crystal_hist: p.crystal_hist,
                    deuterium_hist: p.deuterium_hist,
                },
                regions,
                orders,
                next_order_id: doc.next_order_id,
                rng: doc.rng,
                last_tick: doc.last_tick,
                advanced_to: doc.advanced_to,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceConfig;
    use crate::clock::Clock;
    use crate::intent::Intent;
    use crate::shard::{
        ShardCommand, ShardCommons, ShardConfig, ShardMap, ShardOrder, ShardSession,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    const START: u64 = 1_000;
    const MAP: ShardMap = ShardMap {
        seed: 11,
        galaxies: 1,
        systems: 4,
        positions: 3,
    };

    /// Host clock the test moves; the session holds a handle to the same cell.
    #[derive(Clone)]
    struct HostClock(Rc<Cell<u64>>);

    impl Clock for HostClock {
        fn now_unix(&self) -> u64 {
            self.0.get()
        }
    }

    struct Host {
        clock: HostClock,
        shard: ShardSession<HostClock>,
        empires: BTreeMap<String, GameState>,
    }

    impl Host {
        fn at(&self, now: u64) {
            self.clock.0.set(now);
        }

        fn home(&self, identity: &str) -> Coord {
            self.empires[identity].home
        }

        fn place(
            &mut self,
            identity: &str,
            id: &str,
            ticket: OrderTicket,
        ) -> Result<(), ShardError> {
            let cmd = ShardOrder {
                identity: identity.into(),
                idempotency_id: id.into(),
                ticket,
            };
            self.shard.place_order(&mut self.empires, cmd)
        }

        fn book(&mut self, identity: &str, region: Coord) -> ShardBook {
            self.shard
                .book(identity, region, &mut self.empires)
                .unwrap()
        }

        fn stock(&self, identity: &str, region: Coord) -> Resources {
            self.empires[identity].player_stock_at(region)
        }
    }

    fn tenant() -> GameState {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.daily = crate::DailyConfig::empty();
        let mut state = GameState::with_config(START, cfg);
        state.resources.iron = 5_000.0;
        state.resources.silicon = 1_000.0;
        state
    }

    fn host(identities: &[&str]) -> Host {
        let clock = HostClock(Rc::new(Cell::new(START)));
        let mut host = Host {
            shard: ShardSession::new(clock.clone(), [7; 32], ShardConfig::default()),
            clock,
            empires: BTreeMap::new(),
        };
        for identity in identities {
            host.shard
                .join(identity, MAP, &mut host.empires, |_| tenant())
                .unwrap();
        }
        host
    }

    fn silicon(region: Coord, side: OrderSide, amount: f64, limit: Option<f64>) -> OrderTicket {
        OrderTicket {
            region,
            side,
            good: ResourceKind::Silicon,
            amount,
            limit,
            ttl_secs: 0,
        }
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }

    fn tick_secs() -> u64 {
        ShardConfig::default().market.tick_secs
    }

    #[test]
    fn one_tenants_resting_sell_fills_another_tenants_market_buy() {
        let mut host = host(&["a", "b"]);
        let region = host.home("a");
        // b's iron reached a's region by an earlier shipment.
        host.empires
            .get_mut("b")
            .unwrap()
            .credit_trader(TraderId::Player, region, iron(1_000.0));
        let a_before = host.stock("a", region);
        let b_before = host.stock("b", region);

        host.place(
            "a",
            "sell",
            silicon(region, OrderSide::Sell, 10.0, Some(2.0)),
        )
        .unwrap();
        close(host.stock("a", region).silicon, a_before.silicon - 10.0);
        host.place("b", "buy", silicon(region, OrderSide::Buy, 4.0, None))
            .unwrap();

        let a = host.stock("a", region);
        let b = host.stock("b", region);
        close(a.iron, a_before.iron + 8.0);
        close(b.iron, b_before.iron - 8.0);
        close(b.silicon, b_before.silicon + 4.0);

        let for_a = host.book("a", region);
        let for_b = host.book("b", region);
        let [seen_by_a] = for_a.orders.as_slice() else {
            panic!("one order on the book: {:?}", for_a.orders);
        };
        let [seen_by_b] = for_b.orders.as_slice() else {
            panic!("one order on the book: {:?}", for_b.orders);
        };
        assert!(seen_by_a.own && !seen_by_b.own);
        assert_eq!(seen_by_a.id, seen_by_b.id);
        close(seen_by_a.remaining, 6.0);
        close(seen_by_b.price, 2.0);
    }

    #[test]
    fn npc_prices_tick_once_on_the_host_clock_whoever_reads_them() {
        let ticks = 5;
        let end = START + ticks * tick_secs();
        let mut alone = host(&["a"]);
        let mut pair = host(&["a", "b"]);
        let mut stepped = host(&["a", "b"]);
        for n in 1..=ticks {
            stepped.at(START + n * tick_secs());
            stepped.shard.settle(&mut stepped.empires);
        }
        alone.at(end);
        pair.at(end);
        let hub = ShardConfig::default().market.hub();

        let read = pair.book("a", hub);
        assert_eq!(
            read.silicon_hist.len() as u64,
            1 + ticks,
            "one entry per tick"
        );
        assert_ne!(read.silicon_hist[0], read.silicon, "the walk moved");
        let by_b = pair.book("b", hub);
        assert_eq!(
            (by_b.silicon, &by_b.silicon_hist),
            (read.silicon, &read.silicon_hist)
        );
        assert_eq!(by_b.deuterium_hist, read.deuterium_hist);
        assert_eq!(
            alone.book("a", hub),
            read,
            "a second tenant does not tick it again"
        );
        assert_eq!(
            stepped.book("a", hub),
            read,
            "one gap reads like hourly settles"
        );
        assert_eq!(stepped.shard.commons(), pair.shard.commons());
    }

    #[test]
    fn an_expired_order_returns_its_escrow_to_the_tenant() {
        let mut host = host(&["a", "b"]);
        let region = host.home("a");
        let before = host.stock("a", region).silicon;
        let mut ticket = silicon(region, OrderSide::Sell, 10.0, Some(100.0));
        ticket.ttl_secs = 600;
        host.place("a", "sell", ticket).unwrap();
        close(host.stock("a", region).silicon, before - 10.0);
        host.at(START + 599);
        assert_eq!(host.book("a", region).orders.len(), 1);
        let held = host.stock("a", region).silicon;
        host.at(START + 600);
        assert!(host.book("a", region).orders.is_empty());
        let back = host.stock("a", region).silicon - held;
        assert!(
            (back - 10.0).abs() < 0.01,
            "escrow back plus one second of output: {back}"
        );
    }

    #[test]
    fn the_book_round_trips_through_commons_bytes_and_older_bytes_open_it_at_join() {
        let mut host = host(&["a", "b"]);
        let region = host.home("a");
        host.place(
            "a",
            "sell",
            silicon(region, OrderSide::Sell, 10.0, Some(100.0)),
        )
        .unwrap();
        host.at(START + tick_secs());
        host.shard.settle(&mut host.empires);
        let commons = host.shard.commons().clone();
        let bytes = commons.to_bytes().unwrap();
        assert_eq!(ShardCommons::from_bytes(&bytes).unwrap(), commons);

        let v2 = br#"{"version":2,"outposts":[],"sites":[],"debris":[]}"#;
        *host.shard.commons_mut() = ShardCommons::from_bytes(v2).unwrap();
        assert!(!host.shard.commons().market.is_open());
        let ticket = silicon(region, OrderSide::Sell, 1.0, Some(100.0));
        assert_eq!(host.place("a", "again", ticket), Err(ShardError::NoBook));
        host.shard
            .join("a", MAP, &mut host.empires, |_| {
                unreachable!("a reconnect builds nothing")
            })
            .unwrap();
        host.place("a", "again", ticket).unwrap();
    }

    #[test]
    fn a_lone_submit_waits_for_a_book_event_owed_to_its_tenant() {
        let mut host = host(&["a", "b"]);
        let region = host.home("a");
        let mut ticket = silicon(region, OrderSide::Sell, 10.0, Some(100.0));
        ticket.ttl_secs = 600;
        host.place("a", "sell", ticket).unwrap();
        host.at(START + 600);
        let cmd = |identity: &str| ShardCommand {
            identity: identity.into(),
            idempotency_id: "mine".into(),
            intent: Intent::Build {
                planet: host.home(identity),
                building: crate::BuildingKind::MetalMine,
                actor: crate::Actor::Player,
            },
            client_now_unix: None,
        };
        let (for_a, for_b) = (cmd("a"), cmd("b"));
        let b = host.empires.get_mut("b").unwrap();
        host.shard.submit(b, for_b).unwrap();
        let a = host.empires.get_mut("a").unwrap();
        assert_eq!(
            host.shard.submit(a, for_a.clone()),
            Err(ShardError::Unsettled {
                identity: "a".into()
            }),
            "a's escrow refund is due first"
        );
        host.shard.settle(&mut host.empires);
        let a = host.empires.get_mut("a").unwrap();
        host.shard.submit(a, for_a).unwrap();
    }

    #[test]
    fn submit_sends_market_orders_to_the_book() {
        let mut host = host(&["a"]);
        let region = host.home("a");
        let before = host.empires["a"].clone();
        let cmd = ShardCommand {
            identity: "a".into(),
            idempotency_id: "order".into(),
            intent: Intent::PlaceOrder {
                region,
                side: OrderSide::Sell,
                good: ResourceKind::Silicon,
                amount: 1.0,
                limit: Some(2.0),
                ttl_secs: 0,
            },
            client_now_unix: None,
        };
        let state = host.empires.get_mut("a").unwrap();
        assert_eq!(
            host.shard.submit(state, cmd),
            Err(ShardError::OrderNeedsBook)
        );
        assert_eq!(host.empires["a"], before);
    }
}
