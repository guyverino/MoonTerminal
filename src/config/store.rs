//! Низкоуровневое чтение/запись файлов конфига: шифрование servers.enc и
//! открытый settings.toml. Тут НЕТ доменной логики (слияние/uid — в `reconcile`).
//!
//! Важно: битый settings.toml не теряем молча — уводим в `.bak` и продолжаем с
//! дефолта. Иначе одно неверное значение обнулило бы все группы и галки серверов.

use std::path::Path;

use anyhow::Context;

use super::crypto;
use super::paths;
use super::schema::{ServersFile, SettingsFile};

/// Расшифровать и разобрать servers.enc. Ошибка чтения/дешифровки — фатальна
/// (это секреты пользователя, молча терять нельзя).
pub fn read_servers() -> anyhow::Result<ServersFile> {
    let bytes = std::fs::read(paths::servers_path()).context("чтение servers.enc")?;
    let plain = crypto::decrypt(&bytes)?;
    let sf = toml::from_str(std::str::from_utf8(&plain)?).context("разбор servers.enc")?;
    Ok(sf)
}

/// Зашифровать и записать servers.enc.
pub fn write_servers(sf: &ServersFile) -> anyhow::Result<()> {
    let enc = crypto::encrypt(toml::to_string(sf)?.as_bytes())?;
    std::fs::write(paths::servers_path(), enc).context("запись servers.enc")?;
    Ok(())
}

/// Прочитать settings.toml. Нет файла → дефолт (первый запуск). Битый файл →
/// увести в `.bak`, залогировать и вернуть дефолт (данные не теряются молча).
pub fn read_settings() -> SettingsFile {
    let path = paths::settings_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return SettingsFile::default(),
    };
    match toml::from_str(&text) {
        Ok(sf) => sf,
        Err(e) => {
            log::error!("settings.toml повреждён ({e}); увожу в .bak и начинаю с дефолта");
            backup_corrupt(&path);
            SettingsFile::default()
        }
    }
}

/// Записать settings.toml (открытый, человекочитаемый TOML, без секретов).
pub fn write_settings(sf: &SettingsFile) -> anyhow::Result<()> {
    std::fs::write(paths::settings_path(), toml::to_string_pretty(sf)?)
        .context("запись settings.toml")?;
    Ok(())
}

/// Переименовать битый settings.toml → settings.toml.bak (не затираем молча).
fn backup_corrupt(path: &Path) {
    let bak = path.with_extension("toml.bak");
    if let Err(e) = std::fs::rename(path, &bak) {
        log::warn!("не удалось увести битый settings.toml в .bak: {e}");
    }
}
