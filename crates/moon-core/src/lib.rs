//! MoonTerminal backend core — UI-агностичное ядро терминала.
//!
//! Здесь живёт всё, что не зависит от движка отображения (egui/wgpu/winit и
//! будущего GPUI): поток данных от ядра MoonBot, мульти-ядра/дедуп маркет-данных,
//! конфиг с секретами, локальная БД отчётов и доменные типы. Общение с UI — через
//! `feed::FeedMsg` (ядро → UI) и `feed::CoreCmd` (UI → ядро); UI никогда не зовёт
//! транспорт (moonproto) напрямую.
//!
//! Делится между текущей egui-оболочкой (`moon-terminal`) и будущей GPUI-оболочкой
//! — обе зависят от core, но не наоборот.

pub mod applog;
pub mod config;
pub mod data;
pub mod db;
pub mod feed;
pub mod market;
pub mod metrics;
pub mod palette;
pub mod session;
pub mod symbol;
pub mod util;
