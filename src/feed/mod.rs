//! Feed: граница между ядром и UI. Backend-поток (на ядро) шлёт `FeedMsg` в UI.
//! UI никогда не вызывает moonproto напрямую. Режим один — live.

pub mod live;
mod report;
mod strategies;
pub mod synth;
pub mod types;

pub use types::*;

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use crate::config::ServerConfig;
use crate::db::ReportTx;

pub type FeedRx = Receiver<FeedMsg>;
pub type FeedTx = Sender<FeedMsg>;

/// Команды координатора → backend ядра. Задают рыночную РОЛЬ ядра.
#[derive(Debug, Clone)]
pub enum CoreCmd {
    /// Желаемая рыночная роль ядра (полное состояние, не дельта).
    ///
    /// `provider=true` → ядро ретейнит ВСЕ трейды биржи (`subscribe_all_trades`) и
    /// обслуживает рынки из `markets`: подписывает их стакан и читает их крестики,
    /// помечая именем рынка. `provider=false` → никаких рыночных подписок (ядро
    /// отдаёт только аккаунтный план: ордера/детекты/стратегии).
    SetMarket { provider: bool, markets: Vec<String> },
    /// Действие со стратегиями ядра. Сначала синхронизирует галки (`set_checked`
    /// по каждой паре + `send_checked_delta`), затем, если задано, шлёт «старт
    /// отмеченных» (`start_stop=Some(true)`) или «стоп отмеченных» (`Some(false)`).
    /// `checks` — только изменённые галки; `start_stop=None` — лишь синхронизация.
    StrategiesAction {
        checks: Vec<(u64, bool)>,
        start_stop: Option<bool>,
    },
    /// Редактирование полей: одни и те же `changes` (имя→строка) применить к каждой
    /// стратегии из `ids`. На стороне feed клонируем полный снимок, правим поля по
    /// типу и шлём `sync_local_strategies`.
    /// Пока не конструируется: UI-этап полного редактирования полей ещё не сделан.
    #[allow(dead_code)]
    EditStrategyFields {
        ids: Vec<u64>,
        changes: Vec<(String, String)>,
    },
}

/// Хэндл backend-потока. Дроп закрывает каналы → поток завершается.
pub struct FeedHandle {
    pub rx: FeedRx,
    pub cmd_tx: Sender<CoreCmd>,
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
/// `startup_delay` — пауза перед ПЕРВЫМ коннектом: на старте сессии ядра разносятся
/// веером (см. `SessionManager::start`), чтобы не бить в сеть/UDP-bind все разом.
/// Ручной реконнект передаёт `Duration::ZERO` — он должен срабатывать мгновенно.
pub fn spawn(server: ServerConfig, reports: Option<ReportTx>, startup_delay: Duration) -> FeedHandle {
    let (tx, rx) = std::sync::mpsc::channel();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<CoreCmd>();
    let join = std::thread::Builder::new()
        .name(format!("feed-{}", server.id))
        .spawn(move || {
            // Стаггер начального коннекта: спим ДО первой попытки. Каждый поток ждёт
            // сам по себе, поэтому цикл start() не блокируется — коннекты расходятся
            // во времени. Нулевая задержка (ручной реконнект) = сразу в дело.
            if !startup_delay.is_zero() {
                std::thread::sleep(startup_delay);
            }
            // Авто-реконнект на уровне приложения: если live::run упал (например,
            // НЕ удалось первичное подключение — moonproto умеет реконнект только
            // ПОСЛЕ успешного connect), повторяем с нарастающим backoff + джиттер.
            // Штатный выход (Ok = координатор/UI ушёл) — завершаемся.
            // Синт-ядро бенчмарка: гоним synth::run (без сети/реконнекта).
            if server.synthetic {
                let _ = synth::run(&server, &tx, &cmd_rx);
                return;
            }
            let mut backoff = BACKOFF_MIN;
            loop {
                let started = Instant::now();
                match live::run(&server, &tx, &cmd_rx, reports.as_ref()) {
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
                        // Координатор/сессия ушли (cmd-канал закрыт) → не крутимся.
                        if matches!(cmd_rx.try_recv(), Err(TryRecvError::Disconnected)) {
                            break;
                        }
                        std::thread::sleep(wait);
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                }
            }
        })
        .expect("spawn feed thread");
    FeedHandle {
        rx,
        cmd_tx,
        _join: join,
    }
}
