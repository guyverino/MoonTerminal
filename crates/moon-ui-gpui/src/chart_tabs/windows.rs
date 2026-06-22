//! Откреп-вкладки чартов: жизненный цикл их ОС-окон (создание/восстановление/репин,
//! персист геометрии и масштаба) и хост-вид окна `DetachedChartHost`. Вынесено из
//! `chart_tabs` как отдельная подсистема выносных окон — сама полоска вкладок про неё
//! знает лишь через несколько `pub(super)`-методов, дёргаемых из event/observe путей.

use gpui::prelude::FluentBuilder;
use gpui::*;
use rust_i18n::t;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonInputEvent,
    MoonInputState, MoonPalette, MoonWindowFrame, MoonWindowFrameControls, Root, h_flex, v_flex,
};

use super::layout_popup::render_layout_popup;
use super::stack::{resolve_layout, resolve_mode};
use super::{AddChartStack, ChartTabs, Tab, chart_pane_label};
use crate::Backend;
use crate::chart_persist::{self, StackLayoutMode};
use crate::design;
use moon_core::config::ChartBucket;

impl ChartTabs {
    /// Дабл-клик по чарту AddToChart-вкладки → открыть монету на Main + переключиться.
    /// Собрать откреплённые окна чартов ЭТОЙ группы: восстановить (разминимизировать), показать
    /// и каскадом вернуть на первичный монитор. Спасение, если окна свёрнуты/спрятаны/уехали за
    /// экран (они независимы и не ходят за Main). Кнопка в полосе вкладок Main-окна группы.
    pub(super) fn gather_windows(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let handles: Vec<_> = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == group)
            .map(|(_, h)| *h)
            .collect();
        for (i, handle) in handles.into_iter().enumerate() {
            let _ = handle.update(cx, |_, window, _| {
                crate::windowing::reset_window_onscreen(window, i);
                window.activate_window();
            });
        }
    }

    /// Отцепить AddToChart-вкладку в отдельное ОС-окно (убрать из стрипа).
    pub(super) fn detach(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Tab::Add(n, bucket) = tab.clone() else {
            return;
        };
        let Some(pos) = self
            .add
            .iter()
            .position(|(num, c, _)| *num == n && *c == bucket)
        else {
            return;
        };
        let (_, _, panel) = self.add.remove(pos);
        if self.active == tab {
            self.active = Tab::Main;
            self.sync_seen_for_active(cx);
            self.sync_active_scale(cx);
            self.sync_inactive_chart_visibility(cx);
            self.persist_scales(cx);
        }
        // Геометрия: сохранённая (если уже откреплялась) или дефолт-каскад.
        let geom = self
            .spec_geom(cx, n, &bucket)
            .unwrap_or(chart_persist::WinGeom {
                x: 200,
                y: 160,
                w: 900,
                h: 620,
            });
        // Пометить вкладку откреплённой в charts.json (восстановится окном на след. запуске).
        self.upsert_spec(cx, n, &bucket, |s| s.detached = Some(geom));
        moon_core::detect_diag::line(&format!(
            "[detach] n={n} bucket={bucket:?} → detached=Some({},{},{},{})",
            geom.x, geom.y, geom.w, geom.h
        ));
        self.open_chart_window(n, panel, bucket, geom, false, cx);
        cx.notify();
    }

    /// Открыть ОС-окно откреп-вкладки (общий код detach и восстановления при загрузке). Панель
    /// держим в `detached` (ingest наполняет её по num/core); `gpu_canvas` переезжает вместе
    /// с GPUI scene окна.
    /// Хост (`DetachedChartHost`) сам пишет геометрию и просит репин по закрытию. Окно трекаем
    /// по группе (закрытие окна группы закроет его — main.rs on_window_closed).
    fn open_chart_window(
        &mut self,
        n: u32,
        panel: Entity<AddChartStack>,
        bucket: ChartBucket,
        geom: chart_persist::WinGeom,
        restored: bool,
        cx: &mut Context<Self>,
    ) {
        panel.update(cx, |p, pcx| p.set_scene_visible(false, pcx));
        self.detached.push((n, bucket.clone(), panel.clone()));
        // КРИТИЧНО для мультимонитора: без display_id окно создаётся на PRIMARY, и если
        // сохранённые bounds вне primary — gpui откатывается на default_bounds() (центр + дефолт-
        // размер). Поэтому ищем монитор, СОДЕРЖАЩИЙ сохранённую точку, и передаём его display_id —
        // тогда bounds валидны для него и окно встаёт точно (см. retrieve_window_placement).
        let origin = point(px(geom.x as f32), px(geom.y as f32));
        let display_id = cx
            .displays()
            .into_iter()
            .find(|d| d.bounds().contains(&origin))
            .map(|d| d.id());
        let mut opts = crate::windowing::detached_chart_window_options(
            format!(
                "MoonTerminal — {}",
                chart_pane_label(&self.backend, &self.group, n, &bucket, cx)
            ),
            WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(geom.w as f32), px(geom.h as f32)),
            }),
            display_id,
        );
        // Цвет clear окна — из темы (фон чарта). Тело окна прозрачное (own-pass UnderScene нельзя
        // перекрывать), поэтому подложку под/между чартами даёт именно clear; без этого он белый.
        let bg = self.theme.bg;
        opts.window_clear_color = Some(gpui::rgb(
            ((bg[0] as u32) << 16) | ((bg[1] as u32) << 8) | bg[2] as u32,
        ));
        let backend = self.backend.clone();
        let group = self.group.clone();
        // Для восстановленного окна — сохранённый логический размер, чтобы скорректировать
        // DPICHANGED-сжатие на первом render (см. DetachedChartHost.restore_size).
        let restore_size = restored.then(|| size(px(geom.w as f32), px(geom.h as f32)));
        let opened = cx.open_window(opts, move |window, cx| {
            crate::windowing::configure_chart_clear_color(window, cx);
            let host = cx.new(|cx| {
                DetachedChartHost::new(
                    panel,
                    backend,
                    group,
                    n,
                    bucket,
                    restored,
                    restore_size,
                    window,
                    cx,
                )
            });
            cx.new(|cx| Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
        });
        if let Ok(handle) = opened {
            let group = self.group.clone();
            self.backend.update(cx, |b, _| {
                b.detached_chart_windows.push((group, handle));
            });
        }
    }

    /// Геометрия сохранённого откреп-окна вкладки (если есть в charts.json).
    fn spec_geom(
        &self,
        cx: &App,
        num: u32,
        bucket: &ChartBucket,
    ) -> Option<chart_persist::WinGeom> {
        self.backend
            .read(cx)
            .chart_specs
            .iter()
            .find(|s| s.group == self.group && s.num == num && s.bucket() == *bucket)
            .and_then(|s| s.detached)
    }

    /// Найти/создать спеку вкладки (group/num/bucket), применить мутатор, пометить dirty.
    pub(super) fn upsert_spec(
        &self,
        cx: &mut Context<Self>,
        num: u32,
        bucket: &ChartBucket,
        f: impl FnOnce(&mut chart_persist::ChartTabSpec),
    ) {
        let group = self.group.clone();
        self.backend.update(cx, |b, _| {
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.bucket() == *bucket)
            {
                f(s);
            } else {
                let mut s = chart_persist::ChartTabSpec {
                    group,
                    num,
                    core: None,
                    bucket: Some(bucket.clone()),
                    scale: None,
                    detached: None,
                    layout_mode: None,
                    layout_height: None,
                };
                f(&mut s);
                b.chart_specs.push(s);
            }
            b.chart_specs_dirty = true;
        });
    }

    /// Дренаж репина откреп-вкладок: хост закрыли (пользователь) → панель detached→add, спека
    /// → НЕ откреплена. Зовётся из backend observe. (На выходе приложения запрос не обработается → спека
    /// остаётся откреплённой → окно восстановится на след. запуске — как у detached.rs.)
    pub(super) fn drain_chart_repin(&mut self, cx: &mut Context<Self>) {
        // На выходе из приложения НЕ репиним: закрытие откреп-окон при quit не должно сбрасывать
        // detached (иначе окна не восстановятся). Финальный сейв уже сделан в on_app_quit.
        if self.backend.read(cx).quitting {
            return;
        }
        let group = self.group.clone();
        let reqs: Vec<(u32, ChartBucket)> = self.backend.update(cx, |b, _| {
            let mut out = Vec::new();
            b.chart_repin_request.retain(|(g, n, c)| {
                if *g == group {
                    out.push((*n, c.clone()));
                    false
                } else {
                    true
                }
            });
            out
        });
        for (n, bucket) in reqs {
            if let Some(p) = self
                .detached
                .iter()
                .position(|(num, c, _)| *num == n && *c == bucket)
            {
                let (num, c, pnl) = self.detached.remove(p);
                self.add.push((num, c, pnl));
                self.add.sort_by_key(|(num, c, _)| (*num, c.clone()));
            }
            self.upsert_spec(cx, n, &bucket, |s| s.detached = None);
            moon_core::detect_diag::line(&format!(
                "[repin] n={n} bucket={bucket:?} → detached=None (окно закрыли/репин)"
            ));
            cx.notify();
        }
    }

    /// Сохранить масштаб каждой вкладки в charts.json (upsert при изменении). Main = num 0.
    pub(super) fn persist_scales(&self, cx: &mut Context<Self>) {
        let mut items: Vec<(u32, ChartBucket, Option<f32>)> =
            vec![(0, ChartBucket::Shared, self.main.read(cx).scale())];
        for (n, c, p) in &self.add {
            items.push((*n, c.clone(), p.read(cx).scale()));
        }
        for (n, c, p) in &self.detached {
            items.push((*n, c.clone(), p.read(cx).scale()));
        }
        for (num, bucket, scale) in items {
            let (cur, exists) = {
                let specs = &self.backend.read(cx).chart_specs;
                let found = specs
                    .iter()
                    .find(|s| s.group == self.group && s.num == num && s.bucket() == bucket);
                (found.and_then(|s| s.scale), found.is_some())
            };
            if cur != scale && (scale.is_some() || exists) {
                self.upsert_spec(cx, num, &bucket, move |s| s.scale = scale);
            }
        }
    }

    /// Восстановить отложенные откреп-окна (charts.json). Открывать ОС-окна В render НЕЛЬЗЯ
    /// (рушит element-арену gpui: «ArenaRef after Arena was cleared»). Вызов идёт из
    /// конструктора ChartTabs, а фактическое открытие откладываем через `cx.defer`.
    pub(super) fn restore_detached(&mut self, cx: &mut Context<Self>) {
        if self.restore_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.restore_pending);
        let this = cx.entity();
        cx.defer(move |app| {
            this.update(app, |this, cx| {
                let (epoch, theme) = (this.epoch, this.theme.clone());
                // Откреп-чарты всегда independent: owned-связь поднимает Main при клике
                // по графику на мультимониторе. Taskbar скрывается policy + Windows fallback.
                for (n, bucket, geom, scale) in pending {
                    let backend = this.backend.clone();
                    let panel = cx.new(|_| {
                        AddChartStack::new(backend, n, bucket.clone(), epoch, theme.clone())
                    });
                    if scale.is_some() {
                        panel.update(cx, |p, pcx| p.set_scale(scale, pcx));
                    }
                    this.open_chart_window(n, panel, bucket, geom, true, cx);
                }
                cx.notify();
            });
        });
    }
}

/// Хост-вид окна откреплённой чарт-вкладки: шапка (масштаб + «закрыть все графики») + панель.
/// Сам пишет геометрию окна в charts.json (`observe_window_bounds`) и просит репин по закрытию
/// (`on_release` → `chart_repin_request`, дренит ChartTabs).
struct DetachedChartHost {
    panel: Entity<AddChartStack>,
    backend: Entity<Backend>,
    group: String,
    num: u32,
    bucket: ChartBucket,
    /// Можно ли сохранять геометрию из `observe_window_bounds`. У ВОССТАНОВЛЕННОГО окна сперва
    /// false: авто-размещение gpui на не-primary DPI читается со сдвигом ×scale, и пересохранять
    /// его НЕЛЬЗЯ (иначе позиция уезжает с каждым запуском). Армируется через ~1.5с — дальше
    /// пишем только реальные перемещения пользователя. У свежего детача — сразу true.
    persist_armed: bool,
    /// Логический размер для коррекции на ПЕРВОМ render восстановленного окна: gpui создаёт окно
    /// на primary, и `WM_DPICHANGED` при переезде на монитор с другим DPI пере-масштабирует
    /// РАЗМЕР (позиция уже верная) → форсим сохранённый логический размер один раз. None у детача.
    restore_size: Option<Size<Pixels>>,
    /// Кнопку окна из таскбара убираем `ITaskbarList::DeleteTab` на первых рендерах (когда окно
    /// уже показано и кнопка создана). Окно при этом остаётся обычным independent → FancyZones его
    /// видит. Несколько тиков — подстраховка от гонки «кнопка ещё не появилась».
    taskbar_hide_ticks: u8,
    /// Открыт ли попап настроек раскладки (кнопка ⚙ в шапке выносного окна).
    layout_popup_open: bool,
    /// Был ли курсор уже внутри попапа (для авто-скрытия по уходу — см. ChartTabs).
    layout_popup_hovered: bool,
    /// Поле ввода высоты слота в попапе раскладки (Blur/Enter → применить к этой вкладке).
    layout_height_input: Entity<MoonInputState>,
}

impl DetachedChartHost {
    fn new(
        panel: Entity<AddChartStack>,
        backend: Entity<Backend>,
        group: String,
        num: u32,
        bucket: ChartBucket,
        restored: bool,
        restore_size: Option<Size<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Геометрия окна (causal bounds event) → charts.json («то же место» при загрузке).
        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_geometry(window, cx);
        })
        .detach();
        // Восстановленное окно: НИКОГДА не пересохраняем геометрию автоматически. gpui на
        // не-primary DPI читает позицию со сдвигом ×scale (баг размещения, см. заметку для
        // MoonUI GPUI), и если её сохранить — на след. запуске окно уезжает ещё → улетает за
        // экран → дефолт (компаундинг). Поэтому сохранённую позицию НЕ трогаем: рестор кладёт
        // окно на исходное место и держит стабильно. (Свежий детач — persist_armed=true.)
        // Закрытие окна → репин в стрип (дренит ChartTabs). На выходе приложения запрос не
        // обработается → спека остаётся откреплённой → окно восстановится на след. запуске.
        let (g, n, c) = (group.clone(), num, bucket.clone());
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, _| {
                b.chart_repin_request.push((g.clone(), n, c.clone()));
            });
        })
        .detach();
        // Поле высоты попапа раскладки: Blur (клик вне) / Enter → применить к этой вкладке.
        let layout_height_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &layout_height_input,
            |this, inp, ev: &MoonInputEvent, cx| {
                if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                    return;
                }
                let raw = inp.read(cx).value().to_string();
                if let Ok(h) = raw.trim().parse::<u16>() {
                    let mode = this.panel.read(cx).layout_mode();
                    this.apply_layout(mode, Some(h.clamp(120, 2000)), cx);
                }
            },
        )
        .detach();
        // Восстановить сохранённую раскладку вкладки из charts.json в панель.
        let (group2, num2, bucket2) = (group.clone(), num, bucket.clone());
        let saved = backend.read(cx).chart_specs.iter().find_map(|s| {
            (s.group == group2 && s.num == num2 && s.bucket() == bucket2)
                .then(|| (s.layout_mode, s.layout_height))
        });
        if let Some((m, h)) = saved {
            if m.is_some() || h.is_some() {
                panel.update(cx, |p, pcx| p.set_layout(m, h, pcx));
            }
        }
        Self {
            panel,
            backend,
            group,
            num,
            bucket,
            persist_armed: !restored,
            restore_size,
            taskbar_hide_ticks: 8,
            layout_popup_open: false,
            layout_popup_hovered: false,
            layout_height_input,
        }
    }

    /// Открыть/закрыть попап раскладки; при открытии — заполнить высоту текущим значением.
    fn toggle_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layout_popup_open = !self.layout_popup_open;
        self.layout_popup_hovered = false;
        if self.layout_popup_open {
            let (m, h) = {
                let p = self.panel.read(cx);
                (p.layout_mode(), p.layout_height())
            };
            let (_, _, hh) = resolve_layout(m, h, self.backend.read(cx));
            let val = format!("{}", hh as u16);
            self.layout_height_input
                .update(cx, |st, c| st.set_value(val, window, c));
        }
        cx.notify();
    }

    /// Применить раскладку к панели вкладки и сохранить в charts.json.
    fn apply_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        self.panel.update(cx, |p, c| p.set_layout(mode, height, c));
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.bucket() == bucket)
            {
                s.layout_mode = mode;
                s.layout_height = height;
            } else {
                bk.chart_specs.push(chart_persist::ChartTabSpec {
                    group,
                    num,
                    core: None,
                    bucket: Some(bucket),
                    scale: None,
                    detached: None,
                    layout_mode: mode,
                    layout_height: height,
                });
            }
            bk.chart_specs_dirty = true;
        });
        cx.notify();
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        // У восстановленного окна сохранение пока заглушено (см. persist_armed): не даём авто-
        // размещению gpui (со сдвигом ×scale на не-primary DPI) перезаписать сохранённую позицию.
        if !self.persist_armed {
            return;
        }
        let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
            moon_core::detect_diag::line(&format!(
                "[geom] n={} НЕ Windowed → геометрия не сохранена",
                self.num
            ));
            return;
        };
        let geom = chart_persist::WinGeom { x, y, w, h };
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let found = self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.group == group && s.num == num && s.bucket() == bucket)
            {
                let cur = s.detached.map(|g| (g.x, g.y, g.w, g.h));
                if cur != Some((geom.x, geom.y, geom.w, geom.h)) {
                    s.detached = Some(geom);
                    bk.chart_specs_dirty = true;
                }
                true
            } else {
                false
            }
        });
        moon_core::detect_diag::line(&format!(
            "[geom] n={num} bucket={bucket:?} → x={} y={} w={} h={} (spec_found={found})",
            geom.x, geom.y, geom.w, geom.h
        ));
    }
}

impl Render for DetachedChartHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Коррекция размера восстановленного окна (один раз): окно уже на целевом мониторе с
        // верным scale → форсим сохранённый логический размер, перебивая DPICHANGED-сжатие.
        if let Some(sz) = self.restore_size.take() {
            window.resize(sz);
        }
        // Убрать кнопку из таскбара (DeleteTab), оставив окно independent → FancyZones его видит.
        // Несколько первых рендеров — на случай, если кнопка появляется чуть позже показа окна.
        if self.taskbar_hide_ticks > 0 {
            crate::windowing::hide_window_from_taskbar(window);
            self.taskbar_hide_ticks -= 1;
        }
        let p = MoonPalette::active(cx);
        // Масштаб — СВОЙ у этой панели (по-вкладочно), правится прямо в неё.
        let scale = self.panel.read(cx).scale();
        let panel = self.panel.clone();
        let close_all_panel = self.panel.clone();
        let title = chart_pane_label(&self.backend, &self.group, self.num, &self.bucket, cx);
        let frame = MoonWindowFrame::detached_chart("detached-chart-window-frame", 0.0)
            .header_height(34.0)
            .controls(MoonWindowFrameControls::Close)
            .show_controls(design::show_custom_window_controls());
        // Попап настроек раскладки этой вкладки (поверх тела, под кнопкой ⚙).
        let layout_popup = self.layout_popup_open.then(|| {
            let entity = cx.entity();
            let current = resolve_mode(self.panel.read(cx).layout_mode(), self.backend.read(cx));
            div()
                .id("detached-layout-popup-wrap")
                .occlude()
                .absolute()
                .right(px(6.0))
                .top(px(36.0))
                // Авто-скрытие: закрываем по уходу курсора, но только ПОСЛЕ первого входа.
                .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                    if *hovered {
                        this.layout_popup_hovered = true;
                    } else if this.layout_popup_hovered {
                        this.layout_popup_open = false;
                        cx.notify();
                    }
                }))
                .child(render_layout_popup(
                    "detached-layout",
                    current,
                    &self.layout_height_input,
                    p,
                    cx,
                    move |mode, app| {
                        entity.update(app, |this, cx| {
                            let h = this.panel.read(cx).layout_height();
                            this.apply_layout(Some(mode), h, cx);
                        });
                    },
                ))
        });
        // Шапка — ТОЛЬКО у выносных окон вкладок (в основном доке её нет): масштаб слева,
        // «закрыть все графики» справа.
        v_flex()
            .size_full()
            .relative()
            .child(
                h_flex()
                    .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .pl(design::ui_px(cx, design::titlebar_leading_inset()))
                    .pr(design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.shell_high))
                    .child(
                        frame
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    .child(crate::controls::scale_dropdown_for_add_stack(
                        scale,
                        panel.clone(),
                        p,
                    ))
                    .child({
                        let entity = cx.entity();
                        let open = self.layout_popup_open;
                        MoonButton::new("detached-layout-settings")
                            .label("⚙")
                            .tooltip(t!("chart.layout.tip").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(if open {
                                MoonButtonVariant::Blue
                            } else {
                                MoonButtonVariant::Ghost
                            })
                            .selected(open)
                            .on_click(move |_, window, app| {
                                entity.update(app, |this, cx| this.toggle_layout_popup(window, cx));
                            })
                            .render()
                    })
                    .child(
                        MoonButton::new("detached-close-all")
                            .label("🗑")
                            .tooltip(t!("chartwin.clear").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(MoonButtonVariant::Ghost)
                            .on_click(move |_, _w, app| {
                                close_all_panel.update(app, |p, cx| p.close_all_panes(cx));
                            })
                            .render(),
                    )
                    .when(design::show_custom_window_controls(), |this| {
                        this.child(frame.visual_controls(cx))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    // БЕЗ .bg(): own-pass чарта — слой UnderScene (под сценой), любой непрозрачный
                    // фон тела его перекрывает (видны лишь оси — они OverScene). Подложку под/между
                    // чартами закрывает тёмный clear окна (правка форка MoonUI), белого нет.
                    .child(self.panel.clone()),
            )
            .children(layout_popup)
    }
}
