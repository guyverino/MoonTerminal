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
use std::time::{Duration, Instant};

use crate::config::AppConfig;
use crate::db::ReportTx;
use crate::feed::{self, ConnStatus, CoreCmd, ExchangeId, FeedHandle, FeedMsg};
use crate::market::{MarketDataMode, MarketStore, MarketView};

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
    /// Аккаунтный план: статус/ордера/детекты/стратегии по ядру. Снаружи —
    /// только чтение через [`SessionManager::store`]; мутирует лишь сам менеджер.
    store: CoreStore,
    /// Рыночный план: крестики/стакан по ядру-провайдеру (дедуп). Полностью
    /// инкапсулирован: наружу — только через [`SessionManager::market_view`].
    market: MarketStore,
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
}

impl SessionManager {
    /// Поднимает live-сессии по всем серверам конфига. Нет серверов — нет сессий.
    /// `reports` — общий канал к SQLite-writer'у (клонируется на каждое ядро).
    pub fn start(config: &AppConfig, epoch_ms: f64, reports: Option<&ReportTx>) -> Self {
        let mut store = CoreStore::default();
        let mut sessions = Vec::new();
        for (i, s) in config
            .servers
            .iter()
            .filter(|s| s.active && config.group(&s.group).active)
            .cloned()
            .enumerate()
        {
            store.ensure(s.id);
            let id = s.id;
            let name = s.name.clone();
            let group = s.group.clone();
            // Стаггер начального коннекта: ядра уходят в сеть веером (150мс шаг,
            // потолок ~4с), а не залпом — иначе всплеск соединений/UDP-bind на старте.
            let startup_delay = Self::startup_stagger(i);
            let handle = feed::spawn(s, reports.cloned(), startup_delay);
            sessions.push(CoreSession {
                id,
                name,
                group,
                handle,
            });
            log::info!("session up: core={id} (delay {startup_delay:?})");
        }
        if sessions.is_empty() {
            log::warn!("нет серверов в конфиге — добавь ядра в Настройках");
        }
        Self {
            sessions,
            store,
            market: MarketStore::new(epoch_ms),
            mode: MarketDataMode::default(),
            core_key: HashMap::new(),
            core_provider: HashMap::new(),
            providers: HashMap::new(),
            wanted: HashMap::new(),
            pending_drop: HashMap::new(),
            last_cmd: HashMap::new(),
        }
    }

    /// Задержка перед первым коннектом i-го ядра при старте сессии: линейный шаг
    /// 150мс с потолком 4с (чтобы 200 ядер не растягивались на полминуты). Разносит
    /// первичные коннекты во времени — меньше всплеск соединений и UDP-bind на старте.
    fn startup_stagger(index: usize) -> Duration {
        const STEP_MS: u64 = 150;
        const CAP: Duration = Duration::from_secs(4);
        Duration::from_millis(STEP_MS * index as u64).min(CAP)
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
                    }
                    FeedMsg::Ticks { market, ticks } => {
                        self.market.apply_ticks(sess.id, &market, &ticks);
                        stats.chart_data = true;
                    }
                    FeedMsg::PriceLine {
                        market,
                        kind,
                        points,
                    } => {
                        self.market
                            .apply_price_line(sess.id, &market, kind, &points);
                        stats.chart_data = true;
                    }
                    FeedMsg::OrderBook { market, book } => {
                        self.market.apply_book(sess.id, &market, &book);
                        stats.chart_data = true;
                    }
                    FeedMsg::Orders(orders) => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            let before = core.orders_rev;
                            core.apply(FeedMsg::Orders(orders));
                            stats.chart_data |= core.orders_rev != before;
                        }
                    }
                    other => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            core.apply(other);
                        }
                    }
                }
            }
        }
        stats
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
        // Ручной реконнект — мгновенно, без стаггера.
        let handle = feed::spawn(server, reports.cloned(), Duration::ZERO);
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

    /// Редактирование полей стратегий ядра: одни и те же `changes` (имя→строка)
    /// применить к каждой стратегии из `ids` (полный снимок правится на стороне feed).
    /// Пока не вызывается: UI-этап полного редактирования полей ещё не сделан.
    #[allow(dead_code)]
    pub fn edit_strategies(&self, core: CoreId, ids: Vec<u64>, changes: Vec<(String, String)>) {
        if ids.is_empty() || changes.is_empty() {
            return;
        }
        if let Some(s) = self.sessions.iter().find(|s| s.id == core) {
            let _ = s
                .handle
                .cmd_tx
                .send(CoreCmd::EditStrategyFields { ids, changes });
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

    /// Рыночные данные для чарта ядра `core` на рынке `market`: резолвим провайдера
    /// ядра и читаем его view. None, пока провайдер не избран или данные не пришли.
    pub fn market_view(&self, core: CoreId, market: &str) -> Option<&MarketView> {
        let provider = *self.core_provider.get(&core)?;
        self.market.view(provider, market)
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
        self.market.clear();
        self.providers.clear();
        self.core_provider.clear();
        self.wanted.clear();
        self.pending_drop.clear();
        self.last_cmd.clear();
    }
}
