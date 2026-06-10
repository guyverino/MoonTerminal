//! Feed: граница между ядром и UI. Backend-поток (на ядро) шлёт `FeedMsg` в UI.
//! UI никогда не вызывает moonproto напрямую. Режим один — live.

pub mod live;
mod report;
mod strategies;
pub mod types;

pub use types::*;

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

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

/// Поднимает live-backend для одного ядра (подключение есть всегда; подписка — по команде).
/// `reports` — канал к SQLite-writer'у (None = БД недоступна, отчёты не пишем).
pub fn spawn(server: ServerConfig, reports: Option<ReportTx>) -> FeedHandle {
    let (tx, rx) = std::sync::mpsc::channel();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<CoreCmd>();
    let join = std::thread::Builder::new()
        .name(format!("feed-{}", server.id))
        .spawn(move || {
            // Авто-реконнект на уровне приложения: если live::run упал (например,
            // НЕ удалось первичное подключение — moonproto умеет реконнект только
            // ПОСЛЕ успешного connect), повторяем с нарастающим backoff. Штатный
            // выход (Ok = координатор/UI ушёл) — завершаемся.
            let mut backoff = Duration::from_secs(2);
            loop {
                match live::run(&server, &tx, &cmd_rx, reports.as_ref()) {
                    Ok(()) => break,
                    Err(e) => {
                        log::error!(
                            "live backend «{}» упал: {e:#}; реконнект через {:?}",
                            server.name,
                            backoff
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
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(30));
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
