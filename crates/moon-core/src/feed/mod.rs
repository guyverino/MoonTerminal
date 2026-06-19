//! Feed: граница между ядром и UI. Backend-поток (на ядро) шлёт `FeedMsg` в UI.
//! UI никогда не вызывает moonproto напрямую. Режим один — live.

pub mod live;
mod report;
mod strategies;
pub mod synth;
pub mod types;

pub use types::*;

use std::sync::mpsc::{Receiver, SendError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use moonproto::MoonClient;

use crate::config::ServerConfig;
use crate::db::ReportTx;
use crate::market::SharedMarketStore;

pub type FeedRx = Receiver<FeedMsg>;
pub type FeedWakeTx = Sender<()>;

#[derive(Clone, Default)]
pub struct SharedMoonClient {
    inner: Arc<RwLock<Option<Arc<MoonClient>>>>,
}

impl SharedMoonClient {
    pub(crate) fn set(&self, client: Option<Arc<MoonClient>>) {
        *self.inner.write().expect("moon client slot poisoned") = client;
    }

    pub fn get(&self) -> Option<Arc<MoonClient>> {
        self.inner
            .read()
            .expect("moon client slot poisoned")
            .clone()
    }
}

#[derive(Clone)]
pub struct FeedTx {
    data: Sender<FeedMsg>,
    wake: Option<FeedWakeTx>,
}

impl FeedTx {
    fn new(data: Sender<FeedMsg>, wake: Option<FeedWakeTx>) -> Self {
        Self { data, wake }
    }

    pub fn send(&self, msg: FeedMsg) -> Result<(), SendError<FeedMsg>> {
        self.data.send(msg)?;
        if let Some(wake) = &self.wake {
            let _ = wake.send(());
        }
        Ok(())
    }
}

/// Команды координатора → backend ядра. Задают рыночную РОЛЬ ядра.
#[derive(Debug, Clone)]
pub enum CoreCmd {
    /// Желаемая рыночная роль ядра (полное состояние, не дельта).
    ///
    /// `provider=true` → ядро ретейнит ВСЕ трейды биржи (`subscribe_all_trades`) и
    /// обслуживает рынки из `markets`: подписывает их стакан и читает их крестики,
    /// помечая именем рынка. `provider=false` → никаких рыночных подписок (ядро
    /// отдаёт только аккаунтный план: ордера/детекты/стратегии).
    SetMarket {
        provider: bool,
        markets: Vec<String>,
    },
    /// Действие со стратегиями ядра. Сначала синхронизирует галки (`set_checked`
    /// по каждой паре + `send_checked_delta`), затем, если задано, шлёт «старт
    /// отмеченных» (`start_stop=Some(true)`) или «стоп отмеченных» (`Some(false)`).
    /// `checks` — только изменённые галки; `start_stop=None` — лишь синхронизация.
    StrategiesAction {
        checks: Vec<(u64, bool)>,
        start_stop: Option<bool>,
    },
    /// Редактирование полей стратегий ОДНОГО ядра: на каждую стратегию свой набор
    /// `(id, changes)` (имя→строка). ВАЖНО: все правки ядра идут ОДНОЙ командой — на стороне
    /// feed клонируем полный снимок, патчим все указанные стратегии и шлём ОДИН
    /// `sync_local_strategies`. Раздельные команды на каждую стратегию нельзя: `sync` целиком
    /// заменяет набор, и второй sync перетёр бы правку первого (применялось бы к одной).
    EditStrategyFields {
        edits: Vec<(u64, Vec<(String, String)>)>,
    },
}

#[derive(Clone)]
pub struct CoreCmdTx {
    data: Sender<CoreCmd>,
    wake: Sender<()>,
}

impl CoreCmdTx {
    fn new(data: Sender<CoreCmd>, wake: Sender<()>) -> Self {
        Self { data, wake }
    }

    pub fn send(&self, cmd: CoreCmd) -> Result<(), SendError<CoreCmd>> {
        self.data.send(cmd)?;
        let _ = self.wake.send(());
        Ok(())
    }
}

impl Drop for CoreCmdTx {
    fn drop(&mut self) {
        let _ = self.wake.send(());
    }
}

/// Хэндл backend-потока. Дроп закрывает каналы → поток завершается.
pub struct FeedHandle {
    pub rx: FeedRx,
    pub cmd_tx: CoreCmdTx,
    pub client: SharedMoonClient,
    _join: std::thread::JoinHandle<()>,
}

/// Базовый шаг backoff и его потолок (между попытками первичного коннекта).
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Сколько `live::run` должен продержаться, чтобы счесть коннект стабильным и
/// сбросить backoff на минимум (редкий разрыв после долгой работы ≠ штормящий хост).
const STABLE_AFTER: Duration = Duration::from_secs(60);

/// Случайный множитель в диапазоне 0.75..1.25 (джиттер ±25%). Разносит во времени
/// синхронные реконнекты множества ядер (упал хост → 200 ядер не бьются в такт).
fn jittered(d: Duration) -> Duration {
    let mut b = [0u8; 8];
    let _ = getrandom::getrandom(&mut b);
    let frac = (u64::from_le_bytes(b) % 1000) as f64 / 1000.0; // 0.0..1.0
    d.mul_f64(0.75 + frac * 0.5)
}

/// Поднимает live-backend для одного ядра (подключение есть всегда; подписка — по команде).
/// `reports` — канал к SQLite-writer'у (None = БД недоступна, отчёты не пишем).
pub fn spawn(
    server: ServerConfig,
    chart_memory_percent: u16,
    reports: Option<ReportTx>,
    wake: Option<FeedWakeTx>,
    market: Option<SharedMarketStore>,
) -> FeedHandle {
    let (data_tx, rx) = std::sync::mpsc::channel();
    let tx = FeedTx::new(data_tx, wake);
    let (cmd_data_tx, cmd_rx) = std::sync::mpsc::channel::<CoreCmd>();
    let (run_wake_tx, run_wake_rx) = std::sync::mpsc::channel::<()>();
    let cmd_tx = CoreCmdTx::new(cmd_data_tx, run_wake_tx.clone());
    let client = SharedMoonClient::default();
    let thread_client = client.clone();
    let join = std::thread::Builder::new()
        .name(format!("feed-{}", server.id))
        .spawn(move || {
            // Авто-реконнект на уровне приложения: если live::run упал (например,
            // НЕ удалось первичное подключение — moonproto умеет реконнект только
            // ПОСЛЕ успешного connect), повторяем с нарастающим backoff + джиттер.
            // Штатный выход (Ok = координатор/UI ушёл) — завершаемся.
            // Синт-ядро бенчмарка: гоним synth::run (без сети/реконнекта).
            if server.synthetic {
                let _ = synth::run(&server, &tx, &cmd_rx, market.as_ref());
                return;
            }
            let mut backoff = BACKOFF_MIN;
            loop {
                let started = Instant::now();
                match live::run(
                    &server,
                    chart_memory_percent,
                    &tx,
                    &cmd_rx,
                    &run_wake_tx,
                    &run_wake_rx,
                    reports.as_ref(),
                    thread_client.clone(),
                ) {
                    Ok(()) => break,
                    Err(e) => {
                        // Коннект продержался долго перед падением → не штормящий хост,
                        // лечим как свежий: сбрасываем backoff на минимум.
                        if started.elapsed() >= STABLE_AFTER {
                            backoff = BACKOFF_MIN;
                        }
                        let wait = jittered(backoff);
                        log::error!(
                            "live backend «{}» упал: {e:#}; реконнект через {:?}",
                            server.name,
                            wait
                        );
                        if tx
                            .send(FeedMsg::Status(ConnStatus::Failed(format!(
                                "{e} · переподключение…"
                            ))))
                            .is_err()
                        {
                            break; // UI закрыт
                        }
                        match run_wake_rx.recv_timeout(wait) {
                            Ok(()) => while run_wake_rx.try_recv().is_ok() {},
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                }
            }
        })
        .expect("spawn feed thread");
    FeedHandle {
        rx,
        cmd_tx,
        client,
        _join: join,
    }
}
