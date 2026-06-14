//! Окно настроек (порт egui `src/settings/*` + `window/settings_window.rs`).
//! Отдельное ОС-окно, редактирует ЖИВОЙ `Backend.config`: правки темы применяются
//! к чарту сразу (группы-окна читают config каждый кадр и пере-рендерят offscreen),
//! «Сохранить» пишет на диск (`AppConfig::save`).
//!
//! Разбито по вкладкам (как egui-оригинал): [`interface`] (тема), [`general`] (общие),
//! [`lines`] (стиль ордер-линий), [`connections`] (ядра/группы). Здесь — каркас:
//! `SettingsView` (состояние + поля редакторов), таб-бар, футер «Сохранить», общие
//! UI-хелперы (`slider_row`/`section`/`color_row`/`separator`) и `open`. Сами вкладки
//! и их состояние — в подмодулях (`impl SettingsView` расщеплён по файлам).

mod connections;
mod general;
mod interface;
mod lines;

use std::collections::HashSet;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    color_picker::{ColorPicker, ColorPickerState},
    h_flex,
    select::{SelectEvent, SelectState},
    slider::{Slider, SliderState},
    v_flex, IndexPath, Root, StyledExt,
};

use crate::icons::IconSet;
use crate::{hex, Backend};
use moon_core::config::{AppConfig, Language};
use moon_core::market::MarketDataMode;
use moon_core::palette;
use moon_core::session::SessionManager;

use connections::ConnRow;
use interface::Iface;
use lines::Lines;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Connections,
    General,
    Interface,
    Lines,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Connections, Tab::General, Tab::Interface, Tab::Lines];
    fn title(self) -> &'static str {
        match self {
            Tab::Connections => "Подключения",
            Tab::General => "Общие",
            Tab::Interface => "Интерфейс",
            Tab::Lines => "Линии",
        }
    }
}

/// Hsla (из color-picker) → sRGB [u8;3] для ChartTheme/OrdersStyle.
pub(super) fn hsla_u8(h: Hsla) -> [u8; 3] {
    let c: Rgba = h.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

/// Строка слайдера (порт egui `Slider::new(..).text(label)`): сам слайдер, справа —
/// подпись и текущее значение. Инлайн, на высоту одного ряда (как на стенде).
pub(super) fn slider_row(label: &str, st: &Entity<SliderState>, cx: &App) -> impl IntoElement {
    let val = st.read(cx).value().start();
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(div().w(px(220.0)).child(Slider::new(st)))
        .child(div().text_color(rgb(hex(palette::TEXT))).child(label.to_string()))
        .child(div().text_color(rgb(hex(palette::TEXT_2))).child(format!("{val:.2}")))
}

/// Разделитель секций (порт egui `ui.separator()`).
pub(super) fn separator() -> impl IntoElement {
    // gpui-component 0.5.2 убрала Divider — тонкая горизонтальная линия своим div'ом.
    div().my_1().h(px(1.0)).bg(rgb(0x2A2D31))
}

/// Секционный заголовок (порт egui `section()`): жирная подпись с отступом сверху.
pub(super) fn section(title: &str) -> impl IntoElement {
    div().mt_2().mb_1().font_bold().child(title.to_string())
}

/// Строка цвета (порт egui `color_row`): свотч-пикер, затем подпись справа.
pub(super) fn color_row(label: &str, st: &Entity<ColorPickerState>) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(ColorPicker::new(st))
        .child(div().child(label.to_string()))
}

/// Метки режима источника данных (вкладка «Подключения») — точные строки локали.
const MODE_LABELS: [(&str, MarketDataMode); 2] = [
    ("Дедуп (провайдер на биржу)", MarketDataMode::Dedup),
    ("По ядрам (без дедупа)", MarketDataMode::PerCore),
];

pub struct SettingsView {
    backend: Entity<Backend>,
    active: Tab,
    /// Статус сохранения: (текст, ошибка?).
    status: Option<(String, bool)>,
    iface: Iface,
    lines: Lines,
    /// Per-server editor-стейты (вкладка «Подключения»); пересоздаётся при add/del.
    conn: Vec<ConnRow>,
    /// Выпадающий выбор языка (вкладка «Общие»).
    lang: Entity<SelectState<Vec<SharedString>>>,
    /// Выпадающий выбор источника данных (вкладка «Подключения»).
    mode: Entity<SelectState<Vec<SharedString>>>,
    /// Какие блоки-линии раскрыты (вкладка «Линии», порт CollapsingHeader).
    open_lines: HashSet<&'static str>,
    /// Кэш иконок групп (вкладка «Подключения»).
    icons: IconSet,
    /// Для какой группы открыт пикер иконок (None = закрыт). Порт egui `picking`.
    picking: Option<String>,
}

impl SettingsView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let iface = interface::build(&backend, window, cx);
        let lines = lines::build(&backend, window, cx);
        let conn = connections::build_conn(&backend, window, cx);

        // Язык — выпадающий список (порт egui ComboBox). Init = текущий язык draft.
        let (cur_lang, cur_mode) = {
            let b = backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (d.language, d.market_mode)
        };
        let lang_items: Vec<SharedString> = Language::ALL.iter().map(|l| l.label().into()).collect();
        let lang_idx = Language::ALL.iter().position(|l| *l == cur_lang).unwrap_or(0);
        let lang = cx.new(|cx| {
            SelectState::new(lang_items, Some(IndexPath::default().row(lang_idx)), window, cx)
        });
        cx.subscribe(&lang, |this, _e, ev: &SelectEvent<Vec<SharedString>>, cx| {
            let SelectEvent::Confirm(v) = ev;
            if let Some(val) = v {
                if let Some(l) = Language::ALL.iter().find(|l| l.label() == val.as_ref()) {
                    let l = *l;
                    this.backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            p.language = l;
                            bcx.notify();
                        }
                    });
                }
            }
        })
        .detach();

        // Источник данных — выпадающий список (порт egui ComboBox).
        let mode_items: Vec<SharedString> = MODE_LABELS.iter().map(|(l, _)| (*l).into()).collect();
        let mode_idx = MODE_LABELS.iter().position(|(_, m)| *m == cur_mode).unwrap_or(0);
        let mode = cx.new(|cx| {
            SelectState::new(mode_items, Some(IndexPath::default().row(mode_idx)), window, cx)
        });
        cx.subscribe(&mode, |this, _e, ev: &SelectEvent<Vec<SharedString>>, cx| {
            let SelectEvent::Confirm(v) = ev;
            if let Some(val) = v {
                if let Some((_, m)) = MODE_LABELS.iter().find(|(l, _)| *l == val.as_ref()) {
                    let m = *m;
                    this.backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            p.market_mode = m;
                            bcx.notify();
                        }
                    });
                }
            }
        })
        .detach();

        // Живой статус ядер (точки в «Подключениях») + n/8 фид-кнопки → перерисовка
        // окна настроек на каждый дренаж backend.
        cx.observe(&backend, |_this, _b, cx| cx.notify()).detach();

        // Закрытие окна (drop view) → сбросить draft: чарт откатывается к config
        // (отмена несохранённых правок) — как egui (draft discarded on close).
        cx.on_release(|this, app| {
            this.backend.update(app, |b, cx| {
                b.preview = None;
                cx.notify();
            });
        })
        .detach();
        Self {
            backend,
            active: Tab::Connections,
            status: None,
            iface,
            lines,
            conn,
            lang,
            mode,
            open_lines: HashSet::new(),
            icons: IconSet::discover(),
            picking: None,
        }
    }

    /// Коммит draft → config + запись на диск (валидация внутри AppConfig::save).
    /// draft остаётся (правки продолжаются), как egui (Save не закрывает окно). При
    /// успехе — применяем изменения (порт egui `App::render_settings`).
    fn save(&mut self, cx: &mut Context<Self>) {
        // Снимок «до» для diff (структура/режим/язык/лог/чарты). Коммитим draft, пишем
        // на диск (save может выровнять uid'ы), затем сравниваем с актуальным config.
        let before = self.backend.read(cx).config.clone();
        let res = self.backend.update(cx, |b, _| {
            if let Some(p) = &b.preview {
                b.config = p.clone();
            }
            b.config.save()
        });
        match res {
            Ok(()) => {
                self.status = Some(("Сохранено".into(), false));
                self.apply_settings(&before, cx);
            }
            Err(e) => self.status = Some((e.to_string(), true)),
        }
        cx.notify();
    }

    /// Применить сохранённые настройки (порт egui `App::render_settings` хвост).
    /// • лог-настройки — живо (set_file_logging + чистка);
    /// • структурные изменения серверов/групп → рестарт `SessionManager` + пересоздание
    ///   окон групп; • смена режима рынка — живо (`set_market_mode`); • смена «чарт на
    ///   ядро» без структурных изменений → тоже пересборка окон (новые чарт-вкладки).
    /// Язык в GPUI-хроме захардкожен (нет i18n-слоя) — меняем только сохранённое
    /// значение, перетесселяции нет.
    fn apply_settings(&mut self, before: &AppConfig, cx: &mut Context<Self>) {
        let after = self.backend.read(cx).config.clone();

        // Файловый лог — применяем живо: включили запись или сократили срок → чистим.
        if before.log_to_file != after.log_to_file
            || before.log_retention_days != after.log_retention_days
        {
            moon_core::applog::set_file_logging(after.log_to_file, after.log_retention_days);
            moon_core::applog::purge_old();
        }

        let struct_changed = before.structural_sig() != after.structural_sig();
        let mode_changed = before.market_mode != after.market_mode;
        let split_changed = before.charts_split_by_core != after.charts_split_by_core;

        if struct_changed {
            // Рестарт сессий по новому конфигу + пересоздание окон групп (их число/состав
            // зависит от серверов/групп). epoch сохраняем прежний.
            self.backend.update(cx, |b, _| {
                let mut s =
                    SessionManager::start(&b.config, b.epoch, b.reports.as_ref().map(|h| &h.tx));
                s.set_market_mode(b.config.market_mode);
                b.session = s;
                b.desired.clear();
            });
            self.rebuild_group_windows(cx);
        } else if mode_changed {
            // Режим рынка — живо: ядра остаются на связи, координатор пере-выберет
            // провайдеров на следующем тике.
            self.backend.update(cx, |b, _| b.session.set_market_mode(b.config.market_mode));
        }

        // Сменили «отдельная чарт-вкладка на ядро» (без структурного ребилда, который и
        // так всё пересоздаёт) → пересобираем окна, чтобы чарт-вкладки собрались в новом
        // режиме (egui чистил chart-tabs; в GPUI вкладки живут в окне — пересоздаём окно).
        if !struct_changed && split_changed {
            self.rebuild_group_windows(cx);
        }
    }

    /// Закрыть все окна групп и открыть заново по актуальному конфигу (порт egui
    /// `needs_rebuild`). Геометрия восстановится из сохранённой раскладки.
    fn rebuild_group_windows(&mut self, cx: &mut Context<Self>) {
        let (handles, cfg, epoch, layout) = self.backend.update(cx, |b, _| {
            let handles: Vec<WindowHandle<Root>> = b.group_windows.values().copied().collect();
            b.group_windows.clear();
            (handles, b.config.clone(), b.epoch, b.layout.clone())
        });
        for h in handles {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for (i, g) in crate::groups(&cfg).into_iter().enumerate() {
            crate::spawn_group_window(cx, &self.backend, &cfg, g, epoch, &layout, i as f32 * 40.0);
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = rgb(hex(palette::SURFACE_1));
        let panel = rgb(hex(palette::BG));
        let border = rgb(hex(palette::LIFT_HOVER));
        let accent = rgb(hex(palette::ACCENT));
        let muted = rgb(hex(palette::TEXT_2));

        // ── Полоска вкладок ─────────────────────────────────────────────────
        let mut tabs = h_flex().w_full().gap_1().px_2().bg(panel).border_b_1().border_color(border);
        for t in Tab::ALL {
            let on = self.active == t;
            let (tc, bb) = if on { (accent, accent) } else { (muted, panel) };
            tabs = tabs.child(
                div()
                    .id(t.title())
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .text_color(tc)
                    .border_b_2()
                    .border_color(bb)
                    .child(t.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = t;
                        cx.notify();
                    })),
            );
        }

        // ── Тело активной вкладки ───────────────────────────────────────────
        let content = match self.active {
            Tab::Interface => self.interface_tab(cx).into_any_element(),
            Tab::General => self.general_tab(cx).into_any_element(),
            Tab::Lines => self.lines_tab(cx).into_any_element(),
            Tab::Connections => self.connections_tab(cx).into_any_element(),
        };
        // Тело прокручивается (вкладки выше высоты окна): stateful div + overflow_y_scroll.
        let body = div()
            .id("settings-body")
            .flex_1()
            .w_full()
            .overflow_y_scroll()
            .child(v_flex().w_full().p_4().gap_2().child(content));

        // ── Подвал: Сохранить + статус ──────────────────────────────────────
        let status_el = match &self.status {
            Some((msg, err)) => div()
                .text_color(if *err { rgb(hex(palette::RED)) } else { rgb(hex(palette::GREEN)) })
                .child(msg.clone()),
            None => div(),
        };
        let footer = h_flex()
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .items_center()
            .bg(panel)
            .border_t_1()
            .border_color(border)
            .child(
                Button::new("save")
                    .primary()
                    .label("Сохранить")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .child(status_el);

        v_flex()
            .size_full()
            .bg(bg)
            .text_color(rgb(hex(palette::TEXT)))
            .text_sm()
            .child(tabs)
            .child(body)
            .child(footer)
    }
}

/// Открыть окно настроек (отдельное ОС-окно). Заводит draft = копия config (его
/// правят вкладки, чарт показывает его живьём). Повторный клик при уже открытом
/// окне игнорируем (draft уже есть) — иначе два окна делили бы один draft.
pub fn open(backend: Entity<Backend>, cx: &mut App) {
    if backend.read(cx).preview.is_some() {
        return;
    }
    backend.update(cx, |b, _| b.preview = Some(b.config.clone()));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(160.0), px(120.0)),
            size: size(px(860.0), px(580.0)),
        })),
        titlebar: Some(TitlebarOptions {
            title: Some("MoonTerminal — Настройки".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(opts, |window, cx| {
        let view = cx.new(|cx| SettingsView::new(backend, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .ok();
}
