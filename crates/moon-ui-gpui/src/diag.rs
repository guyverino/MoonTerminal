//! ПОСТОЯННАЯ диагностика частоты перерисовок (железное правило, см. AGENTS.md «UI Render
//! Diagnostics» + docs/ЕБАНИНА.md шапка). Глобальные атомарные счётчики дёргаются в
//! render/observe/notify; раз в ~1с дренаж-цикл снимает их и пишет строку в `render_diag.log`
//! (Hz по каждому пункту) + в log::info. Вопрос «где рисуем чаще чем надо» = РАНТАЙМ → читаем
//! этот лог, не гадаем по коду.
//!
//! ⚠️ ГРАБЛИ: гейт — env `MOON_RENDER_DIAG`, а НЕ `#[cfg(debug_assertions)]`! В этом проекте
//! `[profile.dev] debug-assertions = false` (Cargo.toml — снимают DX12 validation-слой ради
//! плавности), т.е. в рабочей dev-сборке `cfg(debug_assertions)` = false и счётчики бы исчезли.
//! Поэтому off-by-default через env (см. `enabled()`): инертно в dev и release, включаешь явно
//! `MOON_RENDER_DIAG=1` на время отладки — публичная сборка чистая, файл не пишется.
//!
//! NB: это РУЧНАЯ инструментация по узлам (забывается на новом узле). Целевое — чокпоинт во
//! фреймворке (render каждой вьюхи по имени типа) + own-pass-слои руками. Пока — вот так.

use std::sync::atomic::AtomicU64;

macro_rules! diag_counters {
    ($($name:ident => $label:literal),* $(,)?) => {
        $( pub static $name: AtomicU64 = AtomicU64::new(0); )*
        fn snapshot_and_reset() -> Vec<(&'static str, u64)> {
            use std::sync::atomic::Ordering;
            vec![ $( ($label, $name.swap(0, Ordering::Relaxed)) ),* ]
        }
    };
}

diag_counters!(
    ORDERS_RENDER     => "orders_render",
    SHELL_RENDER      => "shell_render",
    CHART_RENDER      => "chart_render",
    DETACHED_RENDER   => "detached_render",
    BACKEND_NOTIFY    => "backend_notify",
    CHART_PREPARE     => "chart_prepare",
    CHART_GPU_PREPARE => "chart_gpu_prepare",
    CHART_TASK_PREP   => "chart_task_prep",
    // own-pass present (реальная частота показа чарта) — раньше СЛЕПАЯ зона: present-rate
    // не измерялся вообще. CHART_CAM_STEP = сколько present'ов реально сдвинули камеру на
    // ≥1 пиксель ("рабочие" кадры). Соотношение CAM_STEP/PRESENT = экономия пиксельного
    // рубильника (адаптивна к зуму: на мелком масштабе почти все кадры пропускаются).
    CHART_PRESENT     => "chart_present",
    CHART_CAM_STEP    => "chart_cam_step",
    // ПОСЛОЙНЫЕ счётчики own-pass (мандат AGENTS.md «UI Render Diagnostics»): own-pass ВНЕ
    // GPUI-рендера → считаем руками в одной точке-чокпоинте (backend::render_d3d). *_DRAW =
    // отрисовка/блит слоя (раз на present); *_BAKE = перепекание текстуры-кэша (combo/стакан),
    // должно быть РЕДКО (по приходу данных/смене вида). BAKE ≈ DRAW = кэш не работает.
    CHART_BG_DRAW     => "bg_draw",
    CHART_GRID_DRAW   => "grid_draw",
    CHART_CURSOR_DRAW => "cursor_draw",
    CHART_COMBO_DRAW  => "combo_draw",
    CHART_COMBO_BAKE  => "combo_bake",
    CHART_BOOK_DRAW   => "orderbook_draw",
    CHART_BOOK_BAKE   => "orderbook_bake",
    CHART_USER_DRAW   => "userdata_draw",
    ORDERS_OBS_FIRE   => "orders_obs_fire",
    ORDERS_OBS_NOTIFY => "orders_obs_notify",
    SHELL_OBS_FIRE    => "shell_obs_fire",
    SHELL_OBS_NOTIFY  => "shell_obs_notify",
    CHART_OBS_FIRE    => "chart_obs_fire",
    CHART_OBS_NOTIFY  => "chart_obs_notify",
    CHART_OPEN_NOTIFY => "chart_open_notify",
    CHART_TTL_NOTIFY  => "chart_ttl_notify",
    CHART_INPUT_NOTIFY => "chart_input_notify",
    CHART_CURSOR_READOUT_NOTIFY => "chart_cursor_readout_notify",
    CHART_CANVAS_NOTIFY => "chart_canvas_notify",
);

/// Диагностика включается ТОЛЬКО при заданной env `MOON_RENDER_DIAG` (любое значение). По
/// умолчанию инертна в ЛЮБОЙ сборке (dev и release): ни счётчиков, ни файла render_diag.log —
/// включаешь явно на время отладки. (`cfg(debug_assertions)` тут НЕ годится: dev-профиль ставит
/// `debug-assertions = false` ради DX12 validation, см. шапку.) Читается один раз через OnceLock.
fn enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MOON_RENDER_DIAG").is_some())
}

#[inline]
pub fn bump(c: &AtomicU64) {
    if enabled() {
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Снять счётчики и дописать строку Hz в render_diag.log за прошедший интервал (no-op без env).
pub fn report(elapsed_ms: f64) {
    if !enabled() {
        return;
    }
    use std::io::Write;
    let snap = snapshot_and_reset();
    let hz = |c: u64| c as f64 * 1000.0 / elapsed_ms.max(1.0);
    let mut line = format!("[diag {:.0}ms]", elapsed_ms);
    for (label, c) in &snap {
        line.push_str(&format!(" {}={:.0}", label, hz(*c)));
    }
    log::info!("{line}");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("render_diag.log")
    {
        let _ = writeln!(f, "{line}");
    }
}
