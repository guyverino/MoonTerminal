//! Файловый лог команд/отчётов ядра для диагностики — пишется в `logs/` рядом
//! с exe. Один общий файл, потокобезопасный (несколько feed-потоков). Нужен,
//! чтобы видеть СЫРЫЕ report-SQL (INSERT/UPDATE), которые шлёт ядро.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::paths;

static LOG: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

fn handle() -> Option<&'static Mutex<std::fs::File>> {
    LOG.get_or_init(|| {
        let dir = paths::logs_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("applog: не удалось создать {}: {e}", dir.display());
            return None;
        }
        let path = dir.join("commands.log");
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => {
                eprintln!("applog: команды пишутся в {}", path.display());
                Some(Mutex::new(f))
            }
            Err(e) => {
                eprintln!("applog: не удалось открыть {}: {e}", path.display());
                None
            }
        }
    })
    .as_ref()
}

fn ts() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // YYYY-MM-DD HH:MM:SS.mmm (UTC) через тот же civil-конвертер, что и в db.
    let secs = ms / 1000;
    let frac = ms % 1000;
    format!("{}.{:03}", crate::db::fmt_unix_secs(secs), frac)
}

/// Пишет строку команды в `logs/commands.log` (с UTC-временем).
pub fn command(line: &str) {
    if let Some(m) = handle() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "[{}] {line}", ts());
        }
    }
}
