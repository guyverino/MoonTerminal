//! SessionManager: по одному backend-потоку (ядру) на каждый сервер из конфига.
//!
//! Данные ядер делятся на два плана:
//! - АККАУНТНЫЙ (статус/ордера/детекты/стратегии) — свой у каждого ядра, лежит в
//!   `CoreStore` по CoreId;
//! - РЫНОЧНЫЙ (крестики/стакан) — общий для биржи, дедуплицируется по ядру-провайдеру
//!   и лежит в `MarketStore` (см. `crate::market`).
//!
//! Координатор (см. `coordinator.rs`) узнаёт биржу каждого ядра из `Identity`,
//! избирает провайдера на биржу и шлёт ядрам рыночную роль командой `SetMarket`.

pub mod coordinator;
pub mod order_lines;
pub mod store;

pub use store::{CoreId, CoreStore};

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use moonproto::state::{LastPricePoint, MarkPricePoint, SeqRingReader, TradeHistoryRow};
use moonproto::MoonClient;

use crate::config::AppConfig;
use crate::db::ReportTx;
use crate::feed::{self, ConnStatus, CoreCmd, ExchangeId, FeedHandle, FeedMsg, FeedWakeTx};
use crate::market::{MarketDataMode, MarketDataSource, MarketStore, MarketView, SharedMarketStore};

pub struct CoreSession {
    pub id: CoreId,
    pub name: String,
    pub group: String,
    handle: FeedHandle,
}

/// Сводка подключений для статус-бара: сколько ядер готово из общего числа +
/// список «лежащих» (имя, статус) для всплывающей подсказки.
pub struct ConnSummary {
    pub ready: usize,
    pub total: usize,
    /// Не-Ready ядра: (имя, статус). Для тултипа «кто не подключён и почему».
    pub down: Vec<(String, ConnStatus)>,
}

pub struct SessionManager {
    sessions: Vec<CoreSession>,
    feed_wake: Option<FeedWakeTx>,
    /// Аккаунтный план: статус/ордера/детекты/стратегии по ядру. Снаружи —
    /// только чтение через [`SessionManager::store`]; мутирует лишь сам менеджер.
    store: CoreStore,
    /// Рыночный план: общий буфер вне GPUI entity. Live-feed только будит; данные
    /// в буфер тянет `MarketDataSource` из MoonProto snapshots.
    market: SharedMarketStore,
    /// Pull/read-model bridge shared by UI listener and native chart frames.
    market_source: MarketDataSource,
    /// Режим источника рыночных данных (рубильник; пока дефолт Dedup).
    mode: MarketDataMode,
    /// Ядро → биржа (из `Identity`). Без идентичности провайдер не назначается.
    core_key: HashMap<CoreId, ExchangeId>,
    /// Ядро → ядро-провайдер его рыночных данных (dedup: один на биржу; per-core: сам).
    core_provider: HashMap<CoreId, CoreId>,
    /// Биржа → избранный провайдер (для удержания/failover в режиме Dedup).
    providers: HashMap<ExchangeId, CoreId>,
    /// Провайдер → обслуживаемые рынки (union открытых чартов + linger).
    wanted: HashMap<CoreId, HashSet<String>>,
    /// (провайдер, рынок) → дедлайн снятия после закрытия последнего чарта (linger).
    pending_drop: HashMap<(CoreId, String), Instant>,
    /// Последняя посланная ядру роль — чтобы не слать дубликаты команд.
    last_cmd: HashMap<CoreId, (bool, Vec<String>)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DrainStats {
    /// At least one feed message was applied to session state.
    pub any: bool,
    /// Data visible to chart GPU state changed: market ticks/book/price-lines or order lines.
    pub chart_data: bool,
    /// Slow GPUI chrome/account state changed and the Backend entity should be notified.
    pub ui_state: bool,
}

fn log_diag_market_history_after_fill(client: &MoonClient, market: &str, provider: CoreId) {
    let Some(snapshot) = client.snapshot_versioned() else {
        log::warn!("diag history fill verify: no snapshot provider={provider} market={market}");
        return;
    };
    let revision = client.snapshot_revision().unwrap_or(0);
    let Some(readers) = snapshot.market_history_readers(market) else {
        log::warn!(
            "diag history fill verify: no history readers provider={provider} market={market} rev={revision}"
        );
        return;
    };
    log::info!(
        "diag history fill verify: provider={provider} market={market} rev={revision} {} {} {}",
        trade_reader_summary("futures_trades", readers.futures_trades),
        trade_reader_summary("spot_trades", readers.spot_trades),
        price_reader_summary("last", readers.last_prices),
    );
    log::info!(
        "diag history fill verify: provider={provider} market={market} rev={revision} {}",
        mark_reader_summary("mark", readers.mark_prices),
    );
}

fn trade_reader_summary(name: &str, reader: Option<SeqRingReader<TradeHistoryRow>>) -> String {
    let Some(reader) = reader else {
        return format!("{name}=none");
    };
    let bounds = reader.bounds();
    let first = (bounds.len > 0)
        .then(|| reader.read_at_seq(bounds.oldest_seq))
        .flatten()
        .map(|r| format!("{}:{:.4}", r.unix_millis(), r.price))
        .unwrap_or_else(|| "-".to_string());
    let mut last = Vec::new();
    reader.copy_last(1, &mut last);
    let last = last
        .last()
        .map(|r| format!("{}:{:.4}", r.unix_millis(), r.price))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "{name}=len:{}/{} seq:[{}..{}) first:{} last:{}",
        bounds.len, bounds.capacity, bounds.oldest_seq, bounds.next_seq, first, last
    )
}

fn price_reader_summary(name: &str, reader: Option<SeqRingReader<LastPricePoint>>) -> String {
    let Some(reader) = reader else {
        return format!("{name}=none");
    };
    let bounds = reader.bounds();
    let first = (bounds.len > 0)
        .then(|| reader.read_at_seq(bounds.oldest_seq))
        .flatten()
        .map(|r| format!("{}:{:.4}", r.unix_millis(), r.price()))
        .unwrap_or_else(|| "-".to_string());
    let mut last = Vec::new();
    reader.copy_last(1, &mut last);
    let last = last
        .last()
        .map(|r| format!("{}:{:.4}", r.unix_millis(), r.price()))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "{name}=len:{}/{} seq:[{}..{}) first:{} last:{}",
        bounds.len, bounds.capacity, bounds.oldest_seq, bounds.next_seq, first, last
    )
}

fn mark_reader_summary(name: &str, reader: Option<SeqRingReader<MarkPricePoint>>) -> String {
    let Some(reader) = reader else {
        return format!("{name}=none");
    };
    let bounds = reader.bounds();
    let first = (bounds.len > 0)
        .then(|| reader.read_at_seq(bounds.oldest_seq))
        .flatten()
        .map(|r| format!("{}:{:.4}", r.unix_millis(), r.price()))
        .unwrap_or_else(|| "-".to_string());
    let mut last = Vec::new();
    reader.copy_last(1, &mut last);
    let last = last
        .last()
        .map(|r| format!("{}:{:.4}", r.unix_millis(), r.price()))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "{name}=len:{}/{} seq:[{}..{}) first:{} last:{}",
        bounds.len, bounds.capacity, bounds.oldest_seq, bounds.next_seq, first, last
    )
}

impl SessionManager {
    /// Поднимает live-сессии по всем серверам конфига. Нет серверов — нет сессий.
    /// `reports` — общий канал к SQLite-writer'у (клонируется на каждое ядро).
    pub fn start(
        config: &AppConfig,
        epoch_ms: f64,
        reports: Option<&ReportTx>,
        feed_wake: Option<FeedWakeTx>,
    ) -> Self {
        let market = MarketStore::shared(epoch_ms);
        let market_source = MarketDataSource::new(market.clone());
        let mut store = CoreStore::default();
        let mut sessions = Vec::new();
        for s in config
            .servers
            .iter()
            .filter(|s| s.active && config.group(&s.group).active)
            .cloned()
        {
            store.ensure(s.id);
            let id = s.id;
            let name = s.name.clone();
            let group = s.group.clone();
            let handle = feed::spawn(
                s,
                config.chart_memory_percent,
                reports.cloned(),
                feed_wake.clone(),
                Some(market.clone()),
            );
            market_source.set_client(id, handle.client.clone());
            sessions.push(CoreSession {
                id,
                name,
                group,
                handle,
            });
            log::info!("session up: core={id}");
        }
        if sessions.is_empty() {
            log::warn!("нет серверов в конфиге — добавь ядра в Настройках");
        }
        Self {
            sessions,
            feed_wake,
            store,
            market,
            market_source,
            mode: MarketDataMode::default(),
            core_key: HashMap::new(),
            core_provider: HashMap::new(),
            providers: HashMap::new(),
            wanted: HashMap::new(),
            pending_drop: HashMap::new(),
            last_cmd: HashMap::new(),
        }
    }

    /// Дренирует все каналы ядер. Аккаунтные сообщения → CoreStore; рыночные →
    /// MarketStore (по ядру-источнику, т.е. провайдеру); Identity → core_key.
    /// Зовётся частым data-drain тиком перед `set_open`. Возвращает, что именно
    /// изменилось: общий UI-state и отдельно данные, которые могут менять GPU-пиксели чарта.
    pub fn drain(&mut self) -> DrainStats {
        let mut stats = DrainStats::default();
        for sess in &self.sessions {
            while let Ok(msg) = sess.handle.rx.try_recv() {
                stats.any = true;
                match msg {
                    FeedMsg::Identity(ex) => {
                        self.core_key.insert(sess.id, ex);
                        stats.ui_state = true;
                    }
                    FeedMsg::Ticks { market, ticks } => {
                        self.market
                            .write()
                            .expect("market store poisoned")
                            .apply_ticks(sess.id, &market, &ticks);
                        stats.chart_data = true;
                    }
                    FeedMsg::PriceLine {
                        market,
                        kind,
                        points,
                    } => {
                        self.market
                            .write()
                            .expect("market store poisoned")
                            .apply_price_line(sess.id, &market, kind, &points);
                        stats.chart_data = true;
                    }
                    FeedMsg::OrderBook { market, book } => {
                        self.market
                            .write()
                            .expect("market store poisoned")
                            .apply_book(sess.id, &market, &book);
                        stats.chart_data = true;
                    }
                    FeedMsg::MarketDataChanged => {
                        stats.chart_data = true;
                    }
                    FeedMsg::Orders(orders) => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            let before = core.orders_rev;
                            core.apply(FeedMsg::Orders(orders));
                            stats.chart_data |= core.orders_rev != before;
                            stats.ui_state = true;
                        }
                    }
                    other => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            core.apply(other);
                            stats.ui_state = true;
                        }
                    }
                }
            }
        }
        stats
    }

    /// Compatibility pull for non-frame consumers. Native chart frames call the same
    /// `MarketDataSource` directly, so a data event cannot miss the current platform tick.
    pub fn refresh_market_data_for_open(&self, desired: &[(CoreId, String)]) -> bool {
        self.market_source.refresh_for_open(desired)
    }

    /// Diagnostics-only stress fixture: ask the MoonProto provider for `core` to
    /// fill every retained history ring for `market` to its effective capacity.
    /// This deliberately stays behind the session boundary: GPUI/debug UI should
    /// not talk to MoonProto clients directly.
    pub fn diag_fill_market_history_to_capacity(
        &self,
        core: CoreId,
        market: &str,
        now_ms: i64,
        span_ms: i64,
    ) -> bool {
        let provider = self.core_provider.get(&core).copied().unwrap_or(core);
        let Some(sess) = self.sessions.iter().find(|s| s.id == provider) else {
            log::warn!(
                "diag history fill: no provider session for core={core} provider={provider}"
            );
            return false;
        };
        let Some(client) = sess.handle.client.get() else {
            log::warn!("diag history fill: no MoonProto client for provider={provider}");
            return false;
        };
        match client.diag_fill_market_history_to_capacity(market, now_ms, span_ms) {
            Ok(true) => {
                log::info!(
                    "diag history fill: market={market} core={core} provider={provider} span_ms={span_ms}"
                );
                log_diag_market_history_after_fill(&client, market, provider);
                true
            }
            Ok(false) => {
                log::warn!(
                    "diag history fill: MoonProto returned false for market={market} provider={provider}"
                );
                false
            }
            Err(err) => {
                log::warn!(
                    "diag history fill: MoonProto error for market={market} provider={provider}: {err}"
                );
                false
            }
        }
    }

    /// Снимок статусов подключения всех ядер (id → статус) — для бейджей в окне
    /// Настроек. Владеющая копия, чтобы не держать заём на сессию.
    pub fn status_map(&self) -> HashMap<CoreId, ConnStatus> {
        self.store.statuses().collect()
    }

    /// Сводка подключений ядер ОДНОЙ группы: ready/total + список не-Ready ядер
    /// (имя, статус). Группа = ОС-окно, поэтому каждый статус-бар показывает свою
    /// группу (3/3 + 7/7 при 10 ядрах в двух группах). Учитываются и headless-ядра
    /// группы — у них тоже есть сессия (show_window влияет лишь на наличие окна).
    pub fn conn_summary_group(&self, group: &str) -> ConnSummary {
        let mut total = 0;
        let mut ready = 0;
        let mut down = Vec::new();
        for s in self.sessions.iter().filter(|s| s.group == group) {
            total += 1;
            let st = self
                .store
                .core(s.id)
                .map(|d| d.status.clone())
                .unwrap_or(ConnStatus::Connecting);
            if st == ConnStatus::Ready {
                ready += 1;
            } else {
                down.push((s.name.clone(), st));
            }
        }
        ConnSummary { ready, total, down }
    }

    /// Переподключить одно ядро: гасит старый backend-поток (дроп хэндла закрывает
    /// его каналы) и поднимает новый по текущему конфигу. Сбрасывает рыночную роль
    /// ядра, чтобы провайдер переизбрался. Неактивные ядра/группы игнорирует.
    pub fn reconnect(&mut self, id: CoreId, config: &AppConfig, reports: Option<&ReportTx>) {
        let Some(server) = config.servers.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        if !(server.active && config.group(&server.group).active) {
            return;
        }
        let name = server.name.clone();
        let group = server.group.clone();
        let handle = feed::spawn(
            server,
            config.chart_memory_percent,
            reports.cloned(),
            self.feed_wake.clone(),
            Some(self.market.clone()),
        );
        self.market_source.set_client(id, handle.client.clone());
        match self.sessions.iter_mut().find(|s| s.id == id) {
            Some(sess) => sess.handle = handle, // дроп старого хэндла → старый поток завершится
            None => self.sessions.push(CoreSession {
                id,
                name,
                group,
                handle,
            }),
        }
        self.store.ensure(id);
        if let Some(core) = self.store.core_mut(id) {
            core.status = ConnStatus::Connecting;
        }
        // Сброс координации для ядра: пусть провайдер/роль переизберутся заново.
        self.core_key.remove(&id);
        self.core_provider.remove(&id);
        self.providers.retain(|_, prov| *prov != id);
        self.last_cmd.remove(&id);
        log::info!("reconnect: core={id}");
    }

    /// Действие со стратегиями ядра (из окна стратегий): единый путь команд через
    /// per-core канал. Сначала синхронизирует галки (`checks`), затем — старт/стоп
    /// отмеченных (`start_stop`). Пустое действие — no-op.
    pub fn apply_strategies(
        &self,
        core: CoreId,
        checks: Vec<(u64, bool)>,
        start_stop: Option<bool>,
    ) {
        if checks.is_empty() && start_stop.is_none() {
            return;
        }
        if let Some(s) = self.sessions.iter().find(|s| s.id == core) {
            let _ = s
                .handle
                .cmd_tx
                .send(CoreCmd::StrategiesAction { checks, start_stop });
        }
    }

    /// Редактирование полей стратегий ядра: на каждую стратегию свой `(id, changes)`. ВСЕ
    /// правки ядра уходят ОДНОЙ командой (полный снимок правится на стороне feed одним
    /// `sync_local_strategies`) — иначе при нескольких выбранных стратегиях одного ядра
    /// второй sync перетирал бы первый (применялось бы к одной).
    pub fn edit_strategies(&self, core: CoreId, edits: Vec<(u64, Vec<(String, String)>)>) {
        if edits.is_empty() {
            return;
        }
        if let Some(s) = self.sessions.iter().find(|s| s.id == core) {
            let _ = s.handle.cmd_tx.send(CoreCmd::EditStrategyFields { edits });
        }
    }

    /// Read-only доступ к аккаунтному плану (статусы/ордера/детекты/стратегии).
    /// Наружу отдаём только `&` — мутирует store исключительно сам менеджер.
    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    /// Живые сессии ядер (id/имя/группа) — read-only срез для UI.
    pub fn sessions(&self) -> &[CoreSession] {
        &self.sessions
    }

    pub fn feed_wake(&self) -> Option<FeedWakeTx> {
        self.feed_wake.clone()
    }

    /// Рыночные данные для чарта ядра `core` на рынке `market`: резолвим провайдера
    /// ядра и читаем его view. None, пока провайдер не избран или данные не пришли.
    pub fn with_market_view<R>(
        &self,
        core: CoreId,
        market: &str,
        f: impl FnOnce(Option<&MarketView>) -> R,
    ) -> R {
        self.market_source.with_market_view(core, market, f)
    }

    pub fn market_source(&self) -> MarketDataSource {
        self.market_source.clone()
    }

    /// Переключить режим источника рыночных данных (рубильник из Настроек). При
    /// реальной смене сбрасывает рыночный план целиком — провайдеры, обслуживаемые
    /// рынки и данные переизберутся/перельются с нуля на следующем `set_open`
    /// (старые ядра получат свежие роли, т.к. `last_cmd` очищен).
    pub fn set_market_mode(&mut self, mode: MarketDataMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.market_source.clear();
        self.providers.clear();
        self.core_provider.clear();
        self.wanted.clear();
        self.pending_drop.clear();
        self.last_cmd.clear();
    }
}
