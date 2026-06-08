//! Feed: граница между ядром и UI. Backend-поток (на ядро) шлёт `FeedMsg` в UI.
//! UI никогда не вызывает moonproto напрямую. Режим один — live.

pub mod live;
pub mod types;

pub use types::*;

use std::sync::mpsc::{Receiver, Sender};

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
            if let Err(e) = live::run(&server, &tx, &cmd_rx, reports.as_ref()) {
                log::error!("live backend упал: {e:#}");
                let _ = tx.send(FeedMsg::Status(ConnStatus::Failed(e.to_string())));
            }
        })
        .expect("spawn feed thread");
    FeedHandle {
        rx,
        cmd_tx,
        _join: join,
    }
}
