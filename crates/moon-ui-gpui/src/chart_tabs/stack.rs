//! Общий слой вертикального стека чартов (Main + AddToChart): единый тип записи,
//! хелперы масштаба/очистки и 3-режимная раскладка (FIT/SCROLL/COMPRESS), параметризованная
//! фабрикой плитки. Нюансы Main (fullscreen / active / ПКМ-возврат) остаются в `MainChartStack`.

use gpui::*;
use moon_ui::{MoonScrollbarVisibility, MoonVirtualList, MoonVirtualListScrollHandle, v_flex};

use crate::Backend;
use crate::chart_persist::StackLayoutMode;
use crate::panels::ChartPanel;
use moon_core::session::CoreId;

/// Одна запись стека: рынок ядра + его отдельный `ChartPanel`.
pub(super) struct ChartStackEntry {
    pub core: CoreId,
    pub market: String,
    pub panel: Entity<ChartPanel>,
}

/// Глобальный дефолт раскладки из конфига: `(scroll, compress, высота_слота)`. Применяется к
/// вкладкам, у которых нет своей per-tab настройки.
pub(super) fn stack_layout_cfg(b: &Backend) -> (bool, bool, f32) {
    (
        b.config.charts_stack_scroll,
        b.config.charts_stack_compress,
        b.config.chart_stack_height.clamp(120, 2000) as f32,
    )
}

/// Разрешить раскладку стека: per-tab настройка вкладки (`mode`/`height`), а где её нет —
/// глобальный дефолт конфига. Возвращает `(scroll, compress, высота_слота)`.
pub(super) fn resolve_layout(
    mode: Option<StackLayoutMode>,
    height: Option<u16>,
    b: &Backend,
) -> (bool, bool, f32) {
    let (def_scroll, def_compress, def_h) = stack_layout_cfg(b);
    let (scroll, compress) = mode.map_or((def_scroll, def_compress), |m| m.to_scroll_compress());
    let h = height.map_or(def_h, |h| h.clamp(120, 2000) as f32);
    (scroll, compress, h)
}

/// Текущий режим вкладки как `StackLayoutMode` (per-tab или из глобального дефолта) — для
/// показа выбранного пункта в попапе настроек.
pub(super) fn resolve_mode(mode: Option<StackLayoutMode>, b: &Backend) -> StackLayoutMode {
    mode.unwrap_or_else(|| {
        StackLayoutMode::from_scroll_compress(b.config.charts_stack_scroll, b.config.charts_stack_compress)
    })
}

/// Применить масштаб ко всем панелям стека.
pub(super) fn set_panels_scale<S: 'static>(
    entries: &[ChartStackEntry],
    pct: Option<f32>,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_scale(pct, pcx));
    }
}

/// Убрать из стека панели без графиков. Возвращает true, если состав изменился.
pub(super) fn retain_nonempty_panels(entries: &mut Vec<ChartStackEntry>, cx: &App) -> bool {
    let before = entries.len();
    entries.retain(|e| e.panel.read(cx).pane_count() > 0);
    entries.len() != before
}

/// 3-режимная вертикальная раскладка стека (режим — из Настроек):
///  • scroll=false               → FIT: панели делят высоту окна;
///  • scroll=true, compress=false → SCROLL: фикс. высота `cfg_h`, `MoonVirtualList` со скроллом;
///  • scroll=true, compress=true  → COMPRESS: фикс. высота, без скролла, сжатие при переполнении.
///
/// `panel_at` достаёт панель по индексу, `tile` строит одну плитку (Main — с ПКМ-возвратом,
/// Add — простую). FIT/COMPRESS итерируют переданный `s` (это `&self` вызывающего стека), а
/// SCROLL берёт панели через weak-entity в App-контексте — поэтому own-entity не читается
/// через `cx` (иначе RefCell-паника «already mutably borrowed» во время render).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_chart_stack<S, P, T>(
    base_id: &str,
    s: &S,
    entity: Entity<S>,
    count: usize,
    scroll: bool,
    compress: bool,
    cfg_h: f32,
    scroll_handle: &MoonVirtualListScrollHandle,
    border: Rgba,
    panel_at: P,
    tile: T,
) -> AnyElement
where
    S: Render + 'static,
    P: Fn(&S, usize) -> Option<Entity<ChartPanel>> + Copy + 'static,
    T: Fn(&S, usize, Entity<ChartPanel>, Option<f32>, bool, Rgba, Entity<S>) -> AnyElement
        + Copy
        + 'static,
{
    if scroll && !compress {
        // SCROLL: фикс. высота, виртуальный список со скроллбаром. Плитку строим через
        // weak-entity (фабрика `MoonVirtualList` отдаёт `App`, а не `Context`).
        let weak = entity.downgrade();
        let list = MoonVirtualList::new(
            format!("{base_id}-vlist"),
            count,
            cfg_h,
            move |ix, _window, app| {
                let Some(ent) = weak.upgrade() else {
                    return div().into_any_element();
                };
                let s = ent.read(app);
                let Some(panel) = panel_at(s, ix) else {
                    return div().into_any_element();
                };
                tile(s, ix, panel, Some(cfg_h), false, border, ent.clone())
            },
        )
        .track_scroll(scroll_handle)
        .surface(false)
        .border(false)
        .radius(0.0)
        .scrollbar_visibility(MoonScrollbarVisibility::Scrolling);
        return div()
            .id(format!("{base_id}-scroll"))
            .relative()
            .size_full()
            .child(list)
            .into_any_element();
    }

    // FIT / COMPRESS: v_flex на всю высоту окна, без скролла.
    let mut tiles: Vec<AnyElement> = Vec::with_capacity(count);
    for ix in 0..count {
        let Some(panel) = panel_at(s, ix) else {
            continue;
        };
        let (height, flex) = if compress {
            (Some(cfg_h), false)
        } else {
            (None, true)
        };
        tiles.push(tile(s, ix, panel, height, flex, border, entity.clone()));
    }
    div()
        .id(format!("{base_id}-fit"))
        .relative()
        .size_full()
        .overflow_hidden()
        .child(v_flex().size_full().children(tiles))
        .into_any_element()
}
