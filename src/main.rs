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
mod win_taskbar;
mod window;
mod workspace;

use winit::event_loop::{ControlFlow, EventLoop};

use crate::app::App;
use crate::config::AppConfig;

fn main() -> anyhow::Result<()> {
    // Глушим info-шум зависимостей (wgpu/naga/winit), оставляем только наши логи.
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("warn,moon_terminal=info,moonproto::crypted=error"),
    )
    .init();

    let cfg = AppConfig::load()?;
    // Применяем язык интерфейса до создания окон (дефолт — системная локаль).
    rust_i18n::set_locale(cfg.language.code());
    log::info!("ядер в конфиге: {}", cfg.servers.len());

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    // SessionManager поднимает по ядру на каждый сервер конфига.
    let mut app = App::new(cfg);
    event_loop.run_app(&mut app)?;
    Ok(())
}
