use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use moonproto::state::{OrderBookKind, SeqRingCursor};

use crate::feed::{
    Level, OrderBook, PriceLineKind, PricePoint, SharedMoonClient, Side, Tick,
};
use crate::session::CoreId;

use super::{MarketView, SharedMarketStore};

const MARKET_PULL_BATCH: usize = 8192;
const ORDERBOOK_PULL_FLOOR: Duration = Duration::from_millis(50);
const MARKET_DIAG_FLOOR: Duration = Duration::from_millis(1000);

fn market_diag_enabled() -> bool {
    std::env::var_os("MOON_MARKET_DIAG").is_some()
        || std::env::var_os("MOON_RENDER_DIAG").is_some()
}

fn market_diag_due(key: impl Into<String>, floor: Duration) -> bool {
    if !market_diag_enabled() {
        return false;
    }
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let key = key.into();
    let now = Instant::now();
    let mut last = LAST
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("market diag lock poisoned");
    match last.get(&key).copied() {
        Some(prev) if now.duration_since(prev) < floor => false,
        _ => {
            last.insert(key, now);
            true
        }
    }
}

fn market_diag(msg: impl std::fmt::Display) {
    if market_diag_enabled() {
        log::info!("[market_diag] {msg}");
    }
}

#[derive(Default)]
struct MarketPullCursor {
    trades: Option<SeqRingCursor>,
    last_prices: Option<SeqRingCursor>,
    mark_prices: Option<SeqRingCursor>,
    last_book_at: Option<Instant>,
}

struct MarketDataSourceInner {
    store: SharedMarketStore,
    clients: HashMap<CoreId, SharedMoonClient>,
    core_provider: HashMap<CoreId, CoreId>,
    cursors: HashMap<(CoreId, String), MarketPullCursor>,
}

/// UI-agnostic market read-model bridge.
///
/// Feed threads publish only `SharedMoonClient` slots and lightweight wakes.
/// Consumers call this source when they are about to render: it pulls retained
/// MoonProto snapshot rows through per-consumer cursors into the shared
/// `MarketStore`, then exposes a read-only view by consumer core/market.
#[derive(Clone)]
pub struct MarketDataSource {
    inner: Arc<RwLock<MarketDataSourceInner>>,
}

impl MarketDataSource {
    pub fn new(store: SharedMarketStore) -> Self {
        Self {
            inner: Arc::new(RwLock::new(MarketDataSourceInner {
                store,
                clients: HashMap::new(),
                core_provider: HashMap::new(),
                cursors: HashMap::new(),
            })),
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn store(&self) -> SharedMarketStore {
        self.inner
            .read()
            .expect("market source poisoned")
            .store
            .clone()
    }

    pub fn set_client(&self, core: CoreId, client: SharedMoonClient) {
        self.inner
            .write()
            .expect("market source poisoned")
            .clients
            .insert(core, client);
    }

    pub fn set_provider_map(&self, core_provider: &HashMap<CoreId, CoreId>) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.core_provider = core_provider.clone();

        let active_providers: HashSet<CoreId> = inner.core_provider.values().copied().collect();
        inner
            .cursors
            .retain(|(provider, _), _| active_providers.contains(provider));
    }

    pub fn reset_market(&self, provider: CoreId, market: &str) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.cursors.remove(&(provider, market.to_string()));
            inner.store.clone()
        };
        market_diag(format!("reset_market provider={provider} market={market}"));
        store
            .write()
            .expect("market store poisoned")
            .reset(provider, market);
    }

    pub fn drop_market(&self, provider: CoreId, market: &str) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.cursors.remove(&(provider, market.to_string()));
            inner.store.clone()
        };
        store
            .write()
            .expect("market store poisoned")
            .drop_market(provider, market);
    }

    pub fn drop_provider(&self, provider: CoreId) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.cursors.retain(|(p, _), _| *p != provider);
            inner.store.clone()
        };
        store
            .write()
            .expect("market store poisoned")
            .drop_provider(provider);
    }

    pub fn clear(&self) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.core_provider.clear();
            inner.cursors.clear();
            inner.store.clone()
        };
        store.write().expect("market store poisoned").clear();
    }

    pub fn refresh_for_open(&self, desired: &[(CoreId, String)]) -> bool {
        let mut changed = false;
        let mut seen = HashSet::<(CoreId, String)>::new();
        for (core, market) in desired {
            if seen.insert((*core, market.clone())) {
                changed |= self.refresh_market(*core, market);
            }
        }
        changed
    }

    pub fn refresh_markets<'a>(
        &self,
        markets: impl IntoIterator<Item = (CoreId, &'a str)>,
    ) -> bool {
        let mut changed = false;
        let mut seen = HashSet::<(CoreId, String)>::new();
        for (core, market) in markets {
            if seen.insert((core, market.to_string())) {
                changed |= self.refresh_market(core, market);
            }
        }
        changed
    }

    pub fn refresh_market(&self, core: CoreId, market: &str) -> bool {
        let (provider, client, store) = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(provider) = inner.core_provider.get(&core).copied() else {
                if market_diag_due(
                    format!("no-provider:{core}:{market}"),
                    MARKET_DIAG_FLOOR,
                ) {
                    market_diag(format!("refresh core={core} market={market}: no provider"));
                }
                return false;
            };
            let Some(client) = inner.clients.get(&provider).and_then(SharedMoonClient::get) else {
                if market_diag_due(
                    format!("no-client:{provider}:{market}"),
                    MARKET_DIAG_FLOOR,
                ) {
                    market_diag(format!(
                        "refresh core={core} provider={provider} market={market}: no client"
                    ));
                }
                return false;
            };
            (provider, client, inner.store.clone())
        };

        let Some(snapshot) = client.snapshot_versioned() else {
            if market_diag_due(
                format!("no-snapshot:{provider}:{market}"),
                MARKET_DIAG_FLOOR,
            ) {
                market_diag(format!(
                    "refresh core={core} provider={provider} market={market}: no snapshot"
                ));
            }
            return false;
        };

        let key = (provider, market.to_string());
        let mut ticks: Vec<Tick> = Vec::new();
        let mut last_points: Vec<PricePoint> = Vec::new();
        let mut mark_points: Vec<PricePoint> = Vec::new();
        let mut book_update: Option<OrderBook> = None;
        let mut has_trades_reader = false;
        let mut has_last_reader = false;
        let mut has_mark_reader = false;
        let mut has_book_snapshot = false;

        {
            let mut inner = self.inner.write().expect("market source poisoned");
            if inner.core_provider.get(&core).copied() != Some(provider) {
                return false;
            }
            let cursor = inner.cursors.entry(key).or_default();

            if let Some(reader) = snapshot
                .market_history_readers(market)
                .and_then(|r| r.futures_trades)
            {
                has_trades_reader = true;
                let cur = cursor
                    .trades
                    .get_or_insert_with(|| reader.cursor_from_oldest());
                let mut rows = Vec::new();
                reader.copy_new_since(cur, MARKET_PULL_BATCH, &mut rows);
                ticks.extend(rows.iter().map(|r| Tick {
                    time_ms: r.unix_millis() as f64,
                    price: r.price,
                    qty: r.quantity(),
                    side: if r.is_buy() { Side::Buy } else { Side::Sell },
                }));
            }

            if let Some(reader) = snapshot
                .market_history_readers(market)
                .and_then(|r| r.last_prices)
            {
                has_last_reader = true;
                let cur = cursor
                    .last_prices
                    .get_or_insert_with(|| reader.cursor_from_oldest());
                let mut rows = Vec::new();
                reader.copy_new_since(cur, MARKET_PULL_BATCH, &mut rows);
                last_points.extend(rows.iter().map(|p| PricePoint {
                    time_ms: p.unix_millis() as f64,
                    price: p.price(),
                }));
            }

            if let Some(reader) = snapshot
                .market_history_readers(market)
                .and_then(|r| r.mark_prices)
            {
                has_mark_reader = true;
                let cur = cursor
                    .mark_prices
                    .get_or_insert_with(|| reader.cursor_from_oldest());
                let mut rows = Vec::new();
                reader.copy_new_since(cur, MARKET_PULL_BATCH, &mut rows);
                mark_points.extend(rows.iter().map(|p| PricePoint {
                    time_ms: p.unix_millis() as f64,
                    price: p.price(),
                }));
            }

            let book_due = cursor
                .last_book_at
                .is_none_or(|last| last.elapsed() >= ORDERBOOK_PULL_FLOOR);
            if book_due {
                if let Some(book) = snapshot.order_book(market, OrderBookKind::Futures) {
                    has_book_snapshot = true;
                    book_update = Some(OrderBook {
                        bids: book
                            .buys
                            .iter()
                            .map(|l| Level {
                                price: l.rate as f32,
                                qty: l.quantity as f32,
                            })
                            .collect(),
                        asks: book
                            .sells
                            .iter()
                            .map(|l| Level {
                                price: l.rate as f32,
                                qty: l.quantity as f32,
                            })
                            .collect(),
                    });
                    cursor.last_book_at = Some(Instant::now());
                }
            }
        }

        let mut store = store.write().expect("market store poisoned");
        if store.view(provider, market).is_none() {
            if market_diag_due(
                format!("no-view:{provider}:{market}"),
                MARKET_DIAG_FLOOR,
            ) {
                market_diag(format!(
                    "refresh core={core} provider={provider} market={market}: no store view \
                     readers trades={has_trades_reader} last={has_last_reader} mark={has_mark_reader} \
                     book={has_book_snapshot} pulled ticks={} last={} mark={} book={:?}",
                    ticks.len(),
                    last_points.len(),
                    mark_points.len(),
                    book_update
                        .as_ref()
                        .map(|b| (b.bids.len(), b.asks.len()))
                ));
            }
            return false;
        }

        let mut changed = false;
        if !ticks.is_empty() {
            store.apply_ticks(provider, market, &ticks);
            changed = true;
        }
        if !last_points.is_empty() {
            store.apply_price_line(provider, market, PriceLineKind::Last, &last_points);
            changed = true;
        }
        if !mark_points.is_empty() {
            store.apply_price_line(provider, market, PriceLineKind::Mark, &mark_points);
            changed = true;
        }
        if let Some(book) = book_update {
            store.apply_book(provider, market, &book);
            changed = true;
        }
        if market_diag_due(
            format!("refresh:{provider}:{market}"),
            MARKET_DIAG_FLOOR,
        ) {
            let (ring_len, ring_total, book_len, last_price) = store
                .view(provider, market)
                .map(|v| (v.ring.len(), v.ring.total_pushed(), v.book.len(), v.last_price))
                .unwrap_or((0, 0, 0, None));
            market_diag(format!(
                "refresh core={core} provider={provider} market={market}: changed={changed} \
                 readers trades={has_trades_reader} last={has_last_reader} mark={has_mark_reader} \
                 book={has_book_snapshot} pulled ticks={} last={} mark={} \
                 view ring_len={ring_len} ring_total={ring_total} book_len={book_len} last_price={last_price:?}",
                ticks.len(),
                last_points.len(),
                mark_points.len()
            ));
        }
        changed
    }

    pub fn with_market_view<R>(
        &self,
        core: CoreId,
        market: &str,
        f: impl FnOnce(Option<&MarketView>) -> R,
    ) -> R {
        let (provider, store) = {
            let inner = self.inner.read().expect("market source poisoned");
            (inner.core_provider.get(&core).copied(), inner.store.clone())
        };
        let store = store.read().expect("market store poisoned");
        f(provider.and_then(|p| store.view(p, market)))
    }
}
