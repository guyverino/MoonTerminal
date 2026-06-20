//! Откреп-вкладки чартов: жизненный цикл их ОС-окон (создание/восстановление/репин,
//! персист геометрии и масштаба) и хост-вид окна `DetachedChartHost`. Вынесено из
//! `chart_tabs` как отдельная подсистема выносных окон — сама полоска вкладок про неё
//! знает лишь через несколько `pub(super)`-методов, дёргаемых из render.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonPalette, MoonWindowFrame, MoonWindowFrameControls, Root, h_flex,
    v_flex,
};

use super::{ChartTabs, Tab, chart_pane_label};
use crate::Backend;
use crate::chart_persist;
use crate::design;
use crate::panels::ChartPanel;
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
    pub(super) fn detach(
        &mut self,
        tab: Tab,
        owner: Option<AnyWindowHandle>,
        cx: &mut Context<Self>,
    ) {
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
        self.open_chart_window(n, bucket, panel, geom, false, owner, cx);
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
        bucket: ChartBucket,
        panel: Entity<ChartPanel>,
        geom: chart_persist::WinGeom,
        restored: bool,
        owner: Option<AnyWindowHandle>,
        cx: &mut Context<Self>,
    ) {
        self.detached.push((n, bucket.clone(), panel.clone()));
        panel.update(cx, |p, _| p.set_scene_visible(false));
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
        let opts = crate::windowing::detached_window_options(
            format!(
                "MoonTerminal — {}",
                chart_pane_label(&self.backend, &self.group, n, &bucket, cx)
            ),
            WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(geom.w as f32), px(geom.h as f32)),
            }),
            display_id,
            owner,
        );
        let backend = self.backend.clone();
        let group = self.group.clone();
        // Для восстановленного окна — сохранённый логический размер, чтобы скорректировать
        // DPICHANGED-сжатие на первом render (см. DetachedChartHost.restore_size).
        let restore_size = restored.then(|| size(px(geom.w as f32), px(geom.h as f32)));
        let opened = cx.open_window(opts, move |window, cx| {
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
    fn upsert_spec(
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
                };
                f(&mut s);
                b.chart_specs.push(s);
            }
            b.chart_specs_dirty = true;
        });
    }

    /// Дренаж репина откреп-вкладок: хост закрыли (пользователь) → панель detached→add, спека
    /// → НЕ откреплена. Зовётся из render. (На выходе приложения запрос не обработается → спека
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
    /// (рушит element-арену gpui: «ArenaRef after Arena was cleared»). Откладываем через
    /// `cx.defer` — закрытие выполнится ПОСЛЕ цикла рендера, когда открытие окон безопасно.
    pub(super) fn restore_detached(&mut self, owner: AnyWindowHandle, cx: &mut Context<Self>) {
        if self.restore_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.restore_pending);
        let this = cx.entity();
        cx.defer(move |app| {
            this.update(app, |this, cx| {
                let (epoch, theme) = (this.epoch, this.theme.clone());
                // Восстановленный откреп-чарт — owned-окно СВОЕЙ группы (как при runtime-detach).
                // Owner = окно ЭТОГО ChartTabs (= групп-окно), берём его handle в render. НЕ через
                // group_windows: на старте оно может быть ещё не вставлено к моменту defer →
                // owner=None → Independent → отдельная кнопка в таскбаре.
                let owner = Some(owner);
                for (n, bucket, geom, scale) in pending {
                    let backend = this.backend.clone();
                    let panel = cx.new(|c| {
                        ChartPanel::new_addto(backend, n, bucket.clone(), epoch, theme.clone(), c)
                    });
                    if scale.is_some() {
                        panel.update(cx, |p, pcx| p.set_scale(scale, pcx));
                    }
                    this.open_chart_window(n, bucket, panel, geom, true, owner, cx);
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
    panel: Entity<ChartPanel>,
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
}

impl DetachedChartHost {
    fn new(
        panel: Entity<ChartPanel>,
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
        Self {
            panel,
            backend,
            group,
            num,
            bucket,
            persist_armed: !restored,
            restore_size,
        }
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
        // Откреп-чарты независимы (не owned) → ОС даёт им кнопку в таскбаре. Прячем её через
        // WS_EX_TOOLWINDOW (Windows). Идемпотентно: реальная смена стиля происходит один раз.
        crate::windowing::hide_window_from_taskbar(window);
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
        // Шапка — ТОЛЬКО у выносных окон вкладок (в основном доке её нет): масштаб слева,
        // «закрыть все графики» справа.
        v_flex()
            .size_full()
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
                    .child(crate::controls::scale_dropdown_for_panel(
                        scale,
                        panel.clone(),
                        p,
                    ))
                    .child(
                        div()
                            .id("detached-close-all")
                            .px(design::ui_px(cx, 8.0))
                            .h(design::fit_h_px(cx, 22.0, 13.0, 4.5))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(design::ui_px(cx, 3.0))
                            .text_size(design::text_px(cx, 11.0))
                            .text_color(rgba(0xC8CCD0FF))
                            .bg(rgba(0x00000059))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xE04848CC)).text_color(rgb(0xFFFFFF)))
                            .child("Закрыть все графики")
                            .on_mouse_down(MouseButton::Left, move |_e, _w, app| {
                                close_all_panel.update(app, |p, cx| p.close_all_panes(cx));
                            }),
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
                    .child(self.panel.clone()),
            )
    }
}
