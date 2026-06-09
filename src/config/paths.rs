//! Пути конфигов рядом с исполняемым файлом.

use std::path::PathBuf;

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Зашифрованный файл серверов: только name/ip/port/key (переносимый секрет).
pub fn servers_path() -> PathBuf {
    exe_dir().join("servers.enc")
}

/// Остальная конфигурация (группы и пр.) — открытый toml, без секретов.
pub fn settings_path() -> PathBuf {
    exe_dir().join("settings.toml")
}

/// Тема оформления чарта — отдельный переносимый файл (можно делиться).
pub fn theme_path() -> PathBuf {
    exe_dir().join("theme.toml")
}

/// Раскладка окон (позиции/размеры/свёрнутость/активная вкладка + откреплённые
/// окна) — отдельный переносимый файл рядом с exe.
pub fn layout_path() -> PathBuf {
    exe_dir().join("layout.toml")
}

/// SQLite-БД с отчётами по закрытым ордерам (`ClosedSellOrderReport`).
pub fn reports_db_path() -> PathBuf {
    exe_dir().join("reports.sqlite")
}

/// Папка логов рядом с exe (команды/отчёты ядра для диагностики).
pub fn logs_dir() -> PathBuf {
    exe_dir().join("logs")
}

/// Старый объединённый зашифрованный конфиг (для одноразовой миграции).
pub fn legacy_enc_path() -> PathBuf {
    exe_dir().join("config.enc")
}

/// Совсем старый открытый конфиг (для одноразовой миграции).
pub fn legacy_toml_path() -> PathBuf {
    PathBuf::from("config.toml")
}
