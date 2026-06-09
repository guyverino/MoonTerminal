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
pub mod store;

pub use store::{CoreId, CoreStore};

use std::collections::{HashMap, HashSet};
use std::time::Instant;

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
    pub sessions: Vec<CoreSession>,
    /// Аккаунтный план: статус/ордера/детекты/стратегии по ядру.
    pub store: CoreStore,
    /// Рыночный план: крестики/стакан по ядру-провайдеру (дедуп).
    pub market: MarketStore,
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

impl SessionManager {
    /// Поднимает live-сессии по всем серверам конфига. Нет серверов — нет сессий.
    /// `reports` — общий канал к SQLite-writer'у (клонируется на каждое ядро).
    pub fn start(config: &AppConfig, epoch_ms: f64, reports: Option<&ReportTx>) -> Self {
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
            let handle = feed::spawn(s, reports.cloned());
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

    /// Дренирует все каналы ядер. Аккаунтные сообщения → CoreStore; рыночные →
    /// MarketStore (по ядру-источнику, т.е. провайдеру); Identity → core_key.
    /// Зовётся раз в кадр перед `set_open`.
    pub fn drain(&mut self) {
        for sess in &self.sessions {
            while let Ok(msg) = sess.handle.rx.try_recv() {
                match msg {
                    FeedMsg::Identity(ex) => {
                        self.core_key.insert(sess.id, ex);
                    }
                    FeedMsg::Ticks { market, ticks } => {
                        self.market.apply_ticks(sess.id, &market, &ticks);
                    }
                    FeedMsg::OrderBook { market, book } => {
                        self.market.apply_book(sess.id, &market, &book);
                    }
                    other => {
                        if let Some(core) = self.store.core_mut(sess.id) {
                            core.apply(other);
                        }
                    }
                }
            }
        }
    }

    /// Снимок статусов подключения всех ядер (id → статус) — для бейджей в окне
    /// Настроек. Владеющая копия, чтобы не держать заём на сессию.
    pub fn status_map(&self) -> HashMap<CoreId, ConnStatus> {
        self.store.statuses().collect()
    }

    /// Сводка подключений по живым сессиям: ready/total + список не-Ready ядер
    /// (имя, статус) для статус-бара и его тултипа.
    pub fn conn_summary(&self) -> ConnSummary {
        let total = self.sessions.len();
        let mut ready = 0;
        let mut down = Vec::new();
        for s in &self.sessions {
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
        let handle = feed::spawn(server, reports.cloned());
        match self.sessions.iter_mut().find(|s| s.id == id) {
            Some(sess) => sess.handle = handle, // дроп старого хэндла → старый поток завершится
            None => self.sessions.push(CoreSession { id, name, group, handle }),
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
    pub fn apply_strategies(&self, core: CoreId, checks: Vec<(u64, bool)>, start_stop: Option<bool>) {
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
