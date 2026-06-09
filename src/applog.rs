//! Лог приложения: (1) файловый лог сырых команд/отчётов ядра в `logs/commands.log`
//! рядом с exe — для диагностики report-SQL (INSERT/UPDATE), которые шлёт ядро;
//! (2) общий in-memory кольцевой буфер для вкладки «Лог» нижнего дока.
//!
//! Буфер наполняют два источника: [`command`] (сырые report-SQL ядра) и
//! [`TeeLogger`] — обёртка над `env_logger`, дублирующая каждую напечатанную
//! `log::`-запись в буфер (и дальше отдающая её env_logger'у в консоль/файл).
//! Потокобезопасно (несколько feed-потоков + UI-поток читает снимок).

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::paths;

static LOG: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

/// Максимум строк в кольцевом буфере (старые вытесняются).
const RING_CAP: usize = 5000;

/// Одна строка лога для вкладки «Лог».
#[derive(Clone)]
pub struct LogLine {
    /// Время UTC, `YYYY-MM-DD HH:MM:SS.mmm`.
    pub ts: String,
    pub level: log::Level,
    /// Источник: target лог-записи или `core.cmd` для сырых команд ядра.
    pub target: String,
    pub msg: String,
}

static RING: OnceLock<Mutex<VecDeque<LogLine>>> = OnceLock::new();
/// Монотонный счётчик добавлений — дёшево ловит «появились новые строки» (host
/// форсит кадр, когда активна вкладка «Лог»). См. [`revision`].
static REVISION: AtomicU64 = AtomicU64::new(0);

fn ring() -> &'static Mutex<VecDeque<LogLine>> {
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_CAP)))
}

/// Добавить строку в кольцевой буфер (с вытеснением самой старой).
fn push(line: LogLine) {
    if let Ok(mut r) = ring().lock() {
        if r.len() >= RING_CAP {
            r.pop_front();
        }
        r.push_back(line);
    }
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Ревизия буфера (число добавлений). Сравнивай для детекта новых строк.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

/// Снимок последних `max` строк (по порядку старые→новые) для вкладки «Лог».
pub fn snapshot(max: usize) -> Vec<LogLine> {
    ring()
        .lock()
        .map(|r| {
            let start = r.len().saturating_sub(max);
            r.iter().skip(start).cloned().collect()
        })
        .unwrap_or_default()
}

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

/// Пишет строку команды в `logs/commands.log` (с UTC-временем) и в in-memory
/// буфер вкладки «Лог».
pub fn command(line: &str) {
    let ts = ts();
    if let Some(m) = handle() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "[{ts}] {line}");
        }
    }
    push(LogLine {
        ts,
        level: log::Level::Info,
        target: "core.cmd".to_string(),
        msg: line.to_string(),
    });
}

/// Логгер-обёртка: дублирует напечатанные записи в кольцевой буфер и делегирует
/// форматирование/вывод внутреннему `env_logger`. Ставится глобально в `main`.
pub struct TeeLogger {
    inner: env_logger::Logger,
}

impl TeeLogger {
    pub fn new(inner: env_logger::Logger) -> Self {
        Self { inner }
    }
}

impl log::Log for TeeLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        // В буфер кладём только то, что реально проходит фильтр env_logger (то же,
        // что попадёт в консоль) — иначе буфер забьётся trace-шумом зависимостей.
        if self.inner.matches(record) {
            push(LogLine {
                ts: ts(),
                level: record.level(),
                target: record.target().to_string(),
                msg: record.args().to_string(),
            });
        }
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}
