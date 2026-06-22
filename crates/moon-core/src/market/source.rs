use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use moonproto::state::{
    LastPricePoint, MarkPricePoint, OrderBookKind, SeqRingCursor, TradeHistoryRow,
};
use moonproto::MoonTime;

use crate::feed::{Level, MarketDirty, OrderBook, PricePoint, SharedMoonClient, Side, Tick};
use crate::session::CoreId;

use super::{MarketView, SharedMarketStore};

const ORDERBOOK_PULL_PERIOD_MS: u64 = 200;
const MARKET_DIAG_FLOOR: Duration = Duration::from_millis(1000);

fn market_diag_enabled() -> bool {
    std::env::var_os("MOON_MARKET_DIAG").is_some() || std::env::var_os("MOON_RENDER_DIAG").is_some()
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

fn bump_generation(revisions: &mut HashMap<CoreId, u64>, provider: CoreId) {
    let entry = revisions.entry(provider).or_insert(0);
    *entry = entry.wrapping_add(1);
}

fn bump_market_revision(
    revisions: &mut HashMap<(CoreId, String), u64>,
    provider: CoreId,
    market: &str,
) {
    let entry = revisions.entry((provider, market.to_string())).or_insert(0);
    *entry = entry.wrapping_add(1);
}

fn mix_pair(a: u64, b: u64) -> u64 {
    a.wrapping_mul(0x9e37_79b1_85eb_ca87).rotate_left(17) ^ b
}

#[derive(Default)]
struct MarketPullCursor {
    book_phase_ms: Option<u64>,
    last_book_slot: Option<u64>,
    last_book_revision: Option<u64>,
}

#[derive(Default)]
pub struct ChartHistoryCursor {
    trades: Option<SeqRingCursor>,
    last_prices: Option<SeqRingCursor>,
    mark_prices: Option<SeqRingCursor>,
    last_price: Option<f32>,
    trade_rows: Vec<TradeHistoryRow>,
    scan_trade_rows: Vec<TradeHistoryRow>,
    last_price_rows: Vec<LastPricePoint>,
    mark_price_rows: Vec<MarkPricePoint>,
}

impl ChartHistoryCursor {
    pub fn reset(&mut self) {
        self.trades = None;
        self.last_prices = None;
        self.mark_prices = None;
        self.last_price = None;
        self.trade_rows.clear();
        self.scan_trade_rows.clear();
        self.last_price_rows.clear();
        self.mark_price_rows.clear();
    }
}

#[derive(Default)]
pub struct ChartHistoryBuffers {
    pub ticks: Vec<Tick>,
    pub last_points: Vec<PricePoint>,
    pub mark_points: Vec<PricePoint>,
}

impl ChartHistoryBuffers {
    fn clear(&mut self) {
        self.ticks.clear();
        self.last_points.clear();
        self.mark_points.clear();
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChartHistoryRead {
    pub provider: CoreId,
    pub revision: u64,
    pub combo_capacity: usize,
    pub price_line_capacity: usize,
    pub combo_left_rel_ms: Option<f32>,
    pub combo_reset: bool,
    pub price_lines_changed: bool,
    pub clipped: bool,
    pub caught_up: bool,
    pub tick_price_range: Option<(f32, f32)>,
    pub last_price: Option<f32>,
}

struct MarketDataSourceInner {
    store: SharedMarketStore,
    clients: HashMap<CoreId, SharedMoonClient>,
    core_provider: HashMap<CoreId, CoreId>,
    cursors: HashMap<(CoreId, String), MarketPullCursor>,
    market_revisions: HashMap<(CoreId, String), u64>,
    provider_generations: HashMap<CoreId, u64>,
    started_at: Instant,
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
                market_revisions: HashMap::new(),
                provider_generations: HashMap::new(),
                started_at: Instant::now(),
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
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.clients.insert(core, client);
        inner.cursors.retain(|(provider, _), _| *provider != core);
        bump_generation(&mut inner.provider_generations, core);
    }

    pub fn set_provider_map(&self, core_provider: &HashMap<CoreId, CoreId>) {
        let mut inner = self.inner.write().expect("market source poisoned");
        inner.core_provider = core_provider.clone();

        let active_providers: HashSet<CoreId> = inner.core_provider.values().copied().collect();
        inner
            .cursors
            .retain(|(provider, _), _| active_providers.contains(provider));
        inner
            .market_revisions
            .retain(|(provider, _), _| active_providers.contains(provider));
    }

    pub fn reset_market(&self, provider: CoreId, market: &str) {
        let store = {
            let mut inner = self.inner.write().expect("market source poisoned");
            inner.cursors.remove(&(provider, market.to_string()));
            bump_market_revision(&mut inner.market_revisions, provider, market);
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
            bump_market_revision(&mut inner.market_revisions, provider, market);
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
            bump_generation(&mut inner.provider_generations, provider);
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
            inner.market_revisions.clear();
            inner.provider_generations.clear();
            inner.store.clone()
        };
        store.write().expect("market store poisoned").clear();
    }

    pub fn mark_dirty(&self, provider: CoreId, dirty: &[MarketDirty]) {
        if dirty.is_empty() {
            return;
        }
        let mut inner = self.inner.write().expect("market source poisoned");
        for item in dirty {
            bump_market_revision(&mut inner.market_revisions, provider, &item.market);
        }
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
        let (provider, client, store, elapsed_ms) = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(provider) = inner.core_provider.get(&core).copied() else {
                if market_diag_enabled()
                    && market_diag_due(format!("no-provider:{core}:{market}"), MARKET_DIAG_FLOOR)
                {
                    market_diag(format!("refresh core={core} market={market}: no provider"));
                }
                return false;
            };
            let Some(client) = inner.clients.get(&provider).and_then(SharedMoonClient::get) else {
                if market_diag_enabled()
                    && market_diag_due(format!("no-client:{provider}:{market}"), MARKET_DIAG_FLOOR)
                {
                    market_diag(format!(
                        "refresh core={core} provider={provider} market={market}: no client"
                    ));
                }
                return false;
            };
            (
                provider,
                client,
                inner.store.clone(),
                inner.started_at.elapsed().as_millis() as u64,
            )
        };

        let Some(snapshot) = client.snapshot_versioned() else {
            if market_diag_enabled()
                && market_diag_due(
                    format!("no-snapshot:{provider}:{market}"),
                    MARKET_DIAG_FLOOR,
                )
            {
                market_diag(format!(
                    "refresh core={core} provider={provider} market={market}: no snapshot"
                ));
            }
            return false;
        };

        let key = (provider, market.to_string());
        let mut book_update: Option<OrderBook> = None;
        let mut has_book_snapshot = false;

        {
            let mut inner = self.inner.write().expect("market source poisoned");
            if inner.core_provider.get(&core).copied() != Some(provider) {
                return false;
            }
            let cursor = inner.cursors.entry(key).or_default();

            let phase_ms = *cursor.book_phase_ms.get_or_insert_with(|| {
                cadence_phase_ms(provider, market, ORDERBOOK_PULL_PERIOD_MS)
            });
            let book_slot = cadence_slot(elapsed_ms, phase_ms, ORDERBOOK_PULL_PERIOD_MS);
            let book_due = book_slot.is_some_and(|slot| cursor.last_book_slot != Some(slot));
            if book_due {
                if let Some(book) = snapshot.order_book(market, OrderBookKind::Futures) {
                    has_book_snapshot = true;
                    let revision = book.revision();
                    if cursor.last_book_revision != Some(revision) {
                        cursor.last_book_revision = Some(revision);
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
                    }
                }
                cursor.last_book_slot = book_slot;
            }
        }

        let mut store = store.write().expect("market store poisoned");
        if store.view(provider, market).is_none() {
            if market_diag_enabled()
                && market_diag_due(format!("no-view:{provider}:{market}"), MARKET_DIAG_FLOOR)
            {
                market_diag(format!(
                    "refresh core={core} provider={provider} market={market}: no store view \
                     readers trades=false last=false mark=false \
                     book={has_book_snapshot} pulled ticks={} last={} mark={} book={:?}",
                    0,
                    0,
                    0,
                    book_update.as_ref().map(|b| (b.bids.len(), b.asks.len()))
                ));
            }
            return false;
        }

        let mut changed = false;
        if let Some(book) = book_update {
            store.apply_book(provider, market, &book);
            changed = true;
        }
        if market_diag_enabled()
            && market_diag_due(format!("refresh:{provider}:{market}"), MARKET_DIAG_FLOOR)
        {
            let (ring_len, ring_total, book_len, last_price) = store
                .view(provider, market)
                .map(|v| (0, 0, v.book.len(), v.last_price))
                .unwrap_or((0, 0, 0, None));
            market_diag(format!(
                "refresh core={core} provider={provider} market={market}: changed={changed} \
                 readers trades=false last=false mark=false \
                 book={has_book_snapshot} pulled ticks={} last={} mark={} \
                 view ring_len={ring_len} ring_total={ring_total} book_len={book_len} last_price={last_price:?}",
                0,
                0,
                0
            ));
        }
        changed
    }

    /// Cheap hot-path revision for a consumer core. This reads one monotonic
    /// MoonProto snapshot number and does not clone the snapshot or drain rings.
    pub fn snapshot_revision(&self, core: CoreId) -> Option<(CoreId, u64)> {
        let (provider, client) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let client = inner.clients.get(&provider)?.get()?;
            (provider, client)
        };
        Some((provider, client.snapshot_revision().unwrap_or(0)))
    }

    /// Cheap per-market wake revision for a consumer core.
    ///
    /// This is terminal-owned causality, not a MoonProto storage policy:
    /// feed threads mark the markets touched by domain events, and visible
    /// charts compare this one number before pulling retained rows or books.
    pub fn market_revision(&self, core: CoreId, market: &str) -> Option<(CoreId, u64)> {
        let inner = self.inner.read().expect("market source poisoned");
        let provider = inner.core_provider.get(&core).copied()?;
        let generation = inner
            .provider_generations
            .get(&provider)
            .copied()
            .unwrap_or(0);
        let revision = inner
            .market_revisions
            .get(&(provider, market.to_string()))
            .copied()
            .unwrap_or(0);
        Some((provider, mix_pair(generation, revision)))
    }

    pub fn read_chart_history_into(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        from_rel_ms: f32,
        to_rel_ms: f32,
        force_reset: bool,
        scan_price: bool,
        cursor: &mut ChartHistoryCursor,
        out: &mut ChartHistoryBuffers,
    ) -> Option<ChartHistoryRead> {
        out.clear();
        let (provider, client) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let client = inner.clients.get(&provider)?.get()?;
            (provider, client)
        };
        let snapshot = client.snapshot_versioned()?;
        let revision = client.snapshot_revision().unwrap_or(0);
        let readers = snapshot.market_history_readers(market)?;
        let from_time = moon_time_from_rel_ms(epoch_ms, from_rel_ms);
        let to_time = moon_time_from_rel_ms(epoch_ms, to_rel_ms.max(from_rel_ms + 1.0));
        let mut read = ChartHistoryRead {
            provider,
            revision,
            caught_up: true,
            ..ChartHistoryRead::default()
        };

        let trade_reader = readers.futures_trades.or(readers.spot_trades);
        if let Some(reader) = trade_reader {
            read.combo_capacity = reader.capacity();
            let reset = force_reset || cursor.trades.is_none();
            if reset {
                reader.copy_time_range(
                    from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.trade_rows,
                );
                cursor.trades = Some(reader.cursor_from_now());
                read.combo_reset = true;
                read.caught_up = true;
            } else if let Some(cur) = cursor.trades.as_mut() {
                let meta = reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.trade_rows);
                read.clipped |= meta.clipped;
                read.caught_up &= meta.caught_up;
                if meta.clipped {
                    reader.copy_time_range(
                        from_time,
                        to_time,
                        reader.capacity(),
                        &mut cursor.trade_rows,
                    );
                    cursor.trades = Some(reader.cursor_from_now());
                    read.combo_reset = true;
                }
            }
            rows_to_ticks(&cursor.trade_rows, &mut out.ticks);
            read.combo_left_rel_ms = out
                .ticks
                .first()
                .map(|tick| (tick.time_ms - epoch_ms) as f32);
            if let Some(t) = out.ticks.last() {
                cursor.last_price = Some(t.price);
            } else if cursor.last_price.is_none() {
                cursor.trade_rows.clear();
                reader.copy_last(1, &mut cursor.trade_rows);
                if let Some(row) = cursor.trade_rows.last() {
                    cursor.last_price = Some(row.price);
                }
            }
            if scan_price {
                reader.copy_time_range(
                    from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.scan_trade_rows,
                );
                read.tick_price_range = trade_price_range(&cursor.scan_trade_rows);
            }
        } else {
            cursor.trades = None;
            cursor.last_price = None;
        }

        if let Some(reader) = readers.last_prices {
            read.price_line_capacity = read.price_line_capacity.max(reader.capacity());
            let reset = force_reset || cursor.last_prices.is_none();
            let mut changed = reset;
            if reset {
                cursor.last_prices = Some(reader.cursor_from_now());
            } else if let Some(cur) = cursor.last_prices.as_mut() {
                let meta =
                    reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.last_price_rows);
                read.clipped |= meta.clipped;
                read.caught_up &= meta.caught_up;
                changed = meta.copied > 0 || meta.clipped;
            }
            if changed {
                reader.copy_time_range(
                    from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.last_price_rows,
                );
                last_rows_to_points(&cursor.last_price_rows, &mut out.last_points);
                read.price_lines_changed = true;
            }
        } else {
            cursor.last_prices = None;
        }

        if let Some(reader) = readers.mark_prices {
            read.price_line_capacity = read.price_line_capacity.max(reader.capacity());
            let reset = force_reset || cursor.mark_prices.is_none();
            let mut changed = reset;
            if reset {
                cursor.mark_prices = Some(reader.cursor_from_now());
            } else if let Some(cur) = cursor.mark_prices.as_mut() {
                let meta =
                    reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.mark_price_rows);
                read.clipped |= meta.clipped;
                read.caught_up &= meta.caught_up;
                changed = meta.copied > 0 || meta.clipped;
            }
            if changed {
                reader.copy_time_range(
                    from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.mark_price_rows,
                );
                mark_rows_to_points(&cursor.mark_price_rows, &mut out.mark_points);
                read.price_lines_changed = true;
            }
        } else {
            cursor.mark_prices = None;
        }

        read.last_price = cursor.last_price;
        Some(read)
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

fn moon_time_from_rel_ms(epoch_ms: f64, rel_ms: f32) -> MoonTime {
    MoonTime::from_unix_millis((epoch_ms + rel_ms as f64).round() as i64)
}

fn rows_to_ticks(rows: &[TradeHistoryRow], out: &mut Vec<Tick>) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().map(|r| Tick {
        time_ms: r.unix_millis() as f64,
        price: r.price,
        qty: r.quantity(),
        side: if r.is_buy() { Side::Buy } else { Side::Sell },
    }));
}

fn last_rows_to_points(rows: &[LastPricePoint], out: &mut Vec<PricePoint>) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().map(|p| PricePoint {
        time_ms: p.unix_millis() as f64,
        price: p.price(),
    }));
}

fn mark_rows_to_points(rows: &[MarkPricePoint], out: &mut Vec<PricePoint>) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().map(|p| PricePoint {
        time_ms: p.unix_millis() as f64,
        price: p.price(),
    }));
}

fn trade_price_range(rows: &[TradeHistoryRow]) -> Option<(f32, f32)> {
    if rows.is_empty() {
        return None;
    }
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for r in rows {
        lo = lo.min(r.price);
        hi = hi.max(r.price);
    }
    Some((lo, hi))
}

fn cadence_phase_ms(provider: CoreId, market: &str, period_ms: u64) -> u64 {
    let mut sig = 0xcbf29ce484222325u64;
    sig ^= provider;
    sig = sig.wrapping_mul(0x100000001b3);
    for b in market.bytes() {
        sig ^= b as u64;
        sig = sig.wrapping_mul(0x100000001b3);
    }
    sig % period_ms.max(1)
}

fn cadence_slot(elapsed_ms: u64, phase_ms: u64, period_ms: u64) -> Option<u64> {
    if elapsed_ms < phase_ms {
        None
    } else {
        Some((elapsed_ms - phase_ms) / period_ms.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orderbook_cadence_phase_is_stable_and_bounded() {
        let a = cadence_phase_ms(1, "BTCUSDT", ORDERBOOK_PULL_PERIOD_MS);
        let b = cadence_phase_ms(1, "BTCUSDT", ORDERBOOK_PULL_PERIOD_MS);
        let c = cadence_phase_ms(1, "ETHUSDT", ORDERBOOK_PULL_PERIOD_MS);

        assert_eq!(a, b);
        assert!(a < ORDERBOOK_PULL_PERIOD_MS);
        assert!(c < ORDERBOOK_PULL_PERIOD_MS);
    }

    #[test]
    fn cadence_slot_waits_until_phase_then_advances_by_period() {
        assert_eq!(cadence_slot(99, 100, 200), None);
        assert_eq!(cadence_slot(100, 100, 200), Some(0));
        assert_eq!(cadence_slot(299, 100, 200), Some(0));
        assert_eq!(cadence_slot(300, 100, 200), Some(1));
    }
}
