//! MoonTerminal — wgpu chart + orderbook, egui shell, feed от ядра MoonBot.

// В release-сборке без консольного окна. В debug консоль остаётся — в неё идут логи.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Локализация: макрос t! доступен во всём крейте; строки берутся из locales/.
// fallback = "en" — если ключа нет в выбранном языке, подставится английский.
#[macro_use]
extern crate rust_i18n;
i18n!("locales", fallback = "en");

mod app;
mod applog;
mod chart;
mod config;
mod db;
mod dock;
mod feed;
mod gpu;
mod icons;
mod market;
mod metrics;
mod palette;
mod session;
mod settings;
mod shell;
mod strategies;
mod symbol;
mod util;
mod win_taskbar;
mod window;
mod workspace;

use winit::event_loop::{ControlFlow, EventLoop};

use crate::app::App;
use crate::config::AppConfig;

fn main() -> anyhow::Result<()> {
    // Глушим info-шум зависимостей (wgpu/naga/winit), оставляем только наши логи.
    // Строим env_logger как Logger (не .init()) и оборачиваем в TeeLogger — он
    // дублирует напечатанные записи в in-memory буфер вкладки «Лог».
    let env = env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("warn,moon_terminal=info,moonproto::crypted=error"),
    )
    .build();
    log::set_max_level(env.filter());
    if let Err(e) = log::set_boxed_logger(Box::new(crate::applog::TeeLogger::new(env))) {
        eprintln!("не удалось установить логгер: {e}");
    }

    let mut cfg = AppConfig::load()?;
    // Применяем язык интерфейса до создания окон (дефолт — системная локаль).
    rust_i18n::set_locale(cfg.language.code());
    // Файловый лог: режим из конфига + одноразовая чистка старых файлов при старте.
    crate::applog::set_file_logging(cfg.log_to_file, cfg.log_retention_days);
    crate::applog::purge_old();
    log::info!("ядер в конфиге: {}", cfg.servers.len());

    // Бенч-инъекция синт-ядра (MOON_SYNTH): добавляем синтетический сервер в группу
    // ПЕРВОГО активного сервера, чтобы его окно ингестило AddToChart-детекты синта.
    if std::env::var("MOON_SYNTH").is_ok() {
        let group = cfg
            .servers
            .iter()
            .find(|s| s.active)
            .map(|s| s.group.clone())
            .unwrap_or_else(|| "default".into());
        let id = cfg.servers.iter().map(|s| s.id).max().map(|m| m + 1).unwrap_or(0);
        cfg.servers.push(crate::config::ServerConfig {
            id,
            uid: 0,
            name: "SYNTH".into(),
            active: true,
            show_window: true, // НЕ создаёт 2-е окно: группа уже имеет окно от реальных ядер
            feed: Default::default(),
            key: Default::default(),
            group,
            market: "SYNTH0".into(),
            color: [0x47, 0xb3, 0xff],
            synthetic: true,
        });
        log::info!("bench: добавлено синт-ядро id={id} (MOON_SYNTH)");
    }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    // SessionManager поднимает по ядру на каждый сервер конфига.
    let mut app = App::new(cfg);
    event_loop.run_app(&mut app)?;
    Ok(())
}
