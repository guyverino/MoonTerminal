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
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_palette::{
    IndexPath, MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant,
    MoonColorPicker, MoonColorPickerState, MoonPalette, MoonRect, MoonSelectEvent, MoonSelectItem,
    MoonSelectState, MoonSlider, MoonSliderState, MoonWindowChrome, MoonWindowChromeButton, Root,
    h_flex, rgba_from, v_flex,
};

use crate::{Backend, design};
use crate::icons::IconSet;
use moon_core::config::{AppConfig, Language};
use moon_core::market::MarketDataMode;
use moon_core::session::SessionManager;

use connections::ConnRow;
use interface::Iface;
use lines::Lines;

const SETTINGS_HEADER_H: f32 = 30.0;

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
pub(super) fn slider_row(label: &str, st: &Entity<MoonSliderState>, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let val = st.read(cx).value().end();
    h_flex()
        .w_full()
        .min_h(px(28.0))
        .gap(px(10.0))
        .items_center()
        .child(div().w(px(180.0)).child(MoonSlider::new(st).height(22.0)))
        .child(
            div()
                .w(px(210.0))
                .min_w_0()
                .truncate()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
        .child(
            div()
                .w(px(58.0))
                .text_right()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(format!("{val:.2}")),
        )
}

/// Разделитель секций (порт egui `ui.separator()`).
pub(super) fn separator(p: MoonPalette) -> impl IntoElement {
    div().my(px(8.0)).h(px(1.0)).bg(rgba_from(p.border, 1.0))
}

/// Секционный заголовок (порт egui `section()`): жирная подпись с отступом сверху.
pub(super) fn section(title: &str, p: MoonPalette) -> impl IntoElement {
    div()
        .mt(px(10.0))
        .mb(px(4.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgba_from(p.text, 1.0))
        .child(title.to_string())
}

/// Строка цвета (порт egui `color_row`): свотч-пикер, затем подпись справа.
pub(super) fn color_row(
    label: &str,
    st: &Entity<MoonColorPickerState>,
    p: MoonPalette,
) -> impl IntoElement {
    h_flex()
        .min_h(px(28.0))
        .gap(px(10.0))
        .items_center()
        .child(MoonColorPicker::new(st))
        .child(
            div()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.to_string()),
        )
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
    lang: Entity<MoonSelectState<Language>>,
    /// Выпадающий выбор источника данных (вкладка «Подключения»).
    mode: Entity<MoonSelectState<MarketDataMode>>,
    /// Какие блоки-линии раскрыты (вкладка «Линии», порт CollapsingHeader).
    open_lines: HashSet<&'static str>,
    /// Кэш иконок групп (вкладка «Подключения»).
    icons: IconSet,
    /// Для какой группы открыт пикер иконок (None = закрыт). Порт egui `picking`.
    picking: Option<String>,
    /// Сигнатура данных, которые реально читают настройки: draft/config + статусы.
    last_sig: u64,
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
        let lang_items = Language::ALL
            .iter()
            .map(|l| MoonSelectItem::new(*l, l.label()))
            .collect::<Vec<_>>();
        let lang_idx = Language::ALL
            .iter()
            .position(|l| *l == cur_lang)
            .unwrap_or(0);
        let lang = cx
            .new(|cx| MoonSelectState::new(lang_items, Some(IndexPath::new(lang_idx)), window, cx));
        cx.subscribe(&lang, |this, _e, ev: &MoonSelectEvent<Language>, cx| {
            if let MoonSelectEvent::Confirm(Some(language)) = ev {
                let language = *language;
                this.backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        p.language = language;
                        bcx.notify();
                    }
                });
            }
        })
        .detach();

        // Источник данных — выпадающий список (порт egui ComboBox).
        let mode_items = MODE_LABELS
            .iter()
            .map(|(label, mode)| MoonSelectItem::new(*mode, *label))
            .collect::<Vec<_>>();
        let mode_idx = MODE_LABELS
            .iter()
            .position(|(_, m)| *m == cur_mode)
            .unwrap_or(0);
        let mode = cx
            .new(|cx| MoonSelectState::new(mode_items, Some(IndexPath::new(mode_idx)), window, cx));
        cx.subscribe(
            &mode,
            |this, _e, ev: &MoonSelectEvent<MarketDataMode>, cx| {
                if let MoonSelectEvent::Confirm(Some(mode)) = ev {
                    let mode = *mode;
                    this.backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            p.market_mode = mode;
                            bcx.notify();
                        }
                    });
                }
            },
        )
        .detach();

        let initial_sig = settings_sig(backend.read(cx));
        cx.observe(&backend, |this, backend, cx| {
            let sig = settings_sig(backend.read(cx));
            if sig != this.last_sig {
                this.last_sig = sig;
                cx.notify();
            }
        })
        .detach();

        // Закрытие окна (drop view) → сбросить draft: чарт откатывается к config
        // (отмена несохранённых правок) — как egui (draft discarded on close).
        cx.on_release(|this, app| {
            this.backend.update(app, |b, cx| {
                b.preview = None;
                b.settings_window = None;
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
            last_sig: initial_sig,
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
            self.backend
                .update(cx, |b, _| b.session.set_market_mode(b.config.market_mode));
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = f32::from(window.viewport_size().width);

        // ── Полоска вкладок ─────────────────────────────────────────────────
        let mut tabs = h_flex()
            .w_full()
            .h(px(34.0))
            .gap(px(6.0))
            .px(px(8.0))
            .bg(rgba_from(p.shell_high, 1.0))
            .border_b_1()
            .border_color(rgba_from(p.border, 1.0));
        for t in Tab::ALL {
            let on = self.active == t;
            tabs = tabs.child(
                MoonButton::new(t.title())
                    .variant(if on {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .size(MoonButtonSize::Custom {
                        height: 24.0,
                        radius: 4.0,
                        font_size: 10.5,
                        line_height: 13.0,
                        gap: 5.0,
                    })
                    .width(118.0)
                    .selected(on)
                    .label(t.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = t;
                        cx.notify();
                    }))
                    .render(),
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
            .bg(rgba_from(p.shell, 1.0))
            .child(v_flex().w_full().p(px(18.0)).gap(px(10.0)).child(content));

        // ── Подвал: Сохранить + статус ──────────────────────────────────────
        let status_el = match &self.status {
            Some((msg, err)) => div()
                .text_color(rgba_from(if *err { p.red } else { p.green }, 1.0))
                .child(msg.clone()),
            None => div(),
        };
        let footer = h_flex()
            .w_full()
            .h(px(42.0))
            .gap(px(10.0))
            .px(px(10.0))
            .items_center()
            .bg(rgba_from(p.shell_high, 1.0))
            .border_t_1()
            .border_color(rgba_from(p.border, 1.0))
            .child(
                MoonButton::new("save")
                    .primary()
                    .small()
                    .width(110.0)
                    .label("Сохранить")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    .render(),
            )
            .child(status_el);

        v_flex()
            .size_full()
            .relative()
            .bg(rgba_from(p.shell, 1.0))
            .font_family("Geist Mono")
            .text_size(px(11.0))
            .line_height(px(14.0))
            .text_color(rgba_from(p.text, 1.0))
            .child(settings_header(p))
            .child(tabs)
            .child(body)
            .child(footer)
            .child(settings_window_chrome(chrome_width))
    }
}

fn settings_header(p: MoonPalette) -> impl IntoElement {
    h_flex()
        .id("settings-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(px(SETTINGS_HEADER_H))
        .justify_between()
        .pl(px(design::titlebar_leading_inset()))
        .pr(px(design::HEADER_PAD_X))
        .bg(rgba_from(p.shell_high, 1.0))
        .border_b(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .child(
            h_flex()
                .gap(px(8.0))
                .items_center()
                .child(
                    div()
                        .w(px(7.0))
                        .h(px(7.0))
                        .rounded(px(999.0))
                        .bg(rgba_from(p.blue, 1.0))
                        .shadow(vec![moon_palette::foundation::box_shadow(
                            px(0.0),
                            px(0.0),
                            px(8.0),
                            px(0.0),
                            rgba_from(p.blue, 0.34),
                        )]),
                )
                .child(
                    div()
                        .font_family("Inter")
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgba_from(p.text, 1.0))
                        .child("Настройки"),
                ),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(settings_window_buttons(p))
        })
}

fn settings_window_buttons(p: MoonPalette) -> impl IntoElement {
    h_flex()
        .h(px(22.0))
        .gap(px(2.0))
        .font_family("Geist Mono")
        .text_size(px(11.0))
        .child(settings_window_button("—", p.text_soft))
        .child(settings_window_button("×", p.orange))
}

fn settings_window_button(label: &'static str, color: u32) -> impl IntoElement {
    div()
        .w(px(26.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .text_color(rgba_from(color, 1.0))
        .hover(|s| s.bg(rgba_from(0xFFFFFF, 0.055)))
        .child(label)
}

fn settings_sig(b: &Backend) -> u64 {
    let cfg = b.preview.as_ref().unwrap_or(&b.config);
    let mut h = DefaultHasher::new();

    cfg.language.code().hash(&mut h);
    cfg.market_mode.code().hash(&mut h);
    cfg.charts_split_by_core.hash(&mut h);
    cfg.log_to_file.hash(&mut h);
    cfg.log_retention_days.hash(&mut h);
    format!("{:?}", cfg.theme).hash(&mut h);
    format!("{:?}", cfg.orders).hash(&mut h);

    cfg.servers.len().hash(&mut h);
    for s in &cfg.servers {
        s.id.hash(&mut h);
        s.uid.hash(&mut h);
        s.name.hash(&mut h);
        s.active.hash(&mut h);
        s.show_window.hash(&mut h);
        s.feed.orders.hash(&mut h);
        s.feed.detects.hash(&mut h);
        s.feed.reports.hash(&mut h);
        s.feed.balance.hash(&mut h);
        s.feed.strategies.hash(&mut h);
        s.feed.log.hash(&mut h);
        s.feed.alerts.hash(&mut h);
        s.feed.arb.hash(&mut h);
        // The key input owns its local repaint while typing; only empty/non-empty
        // affects surrounding settings layout.
        s.key.is_empty().hash(&mut h);
        s.group.hash(&mut h);
        s.market.hash(&mut h);
        s.color.hash(&mut h);
        s.synthetic.hash(&mut h);
    }

    cfg.groups.len().hash(&mut h);
    for g in &cfg.groups {
        g.name.hash(&mut h);
        g.active.hash(&mut h);
        g.icon.hash(&mut h);
    }

    let mut statuses = b.session.status_map().into_iter().collect::<Vec<_>>();
    statuses.sort_by_key(|(id, _)| *id);
    for (id, status) in statuses {
        id.hash(&mut h);
        format!("{status:?}").hash(&mut h);
    }

    h.finish()
}

fn settings_window_chrome(width: f32) -> impl IntoElement {
    let controls_w = 52.0;
    let controls_x = (width - controls_w - 12.0).max(0.0);
    let drag_x = design::titlebar_leading_inset();
    let drag_w = if design::show_custom_window_controls() {
        (width - controls_w - 20.0).max(0.0)
    } else {
        (width - drag_x).max(0.0)
    };

    let chrome = MoonWindowChrome::new(
        "settings-window-chrome",
        MoonRect::new(0.0, 0.0, width, SETTINGS_HEADER_H),
    )
    .drag_bounds(MoonRect::new(drag_x, 0.0, drag_w, SETTINGS_HEADER_H));

    if design::show_custom_window_controls() {
        chrome
            .controls_bounds(MoonRect::new(
                controls_x,
                0.0,
                controls_w,
                SETTINGS_HEADER_H,
            ))
            .button_width(26.0)
            .buttons([
                MoonWindowChromeButton::Minimize,
                MoonWindowChromeButton::Close,
            ])
            .render()
    } else {
        chrome.no_controls().render()
    }
}

/// Открыть окно настроек (отдельное ОС-окно). Заводит draft = копия config (его
/// правят вкладки, чарт показывает его живьём). Повторный клик при уже открытом
/// окне игнорируем (draft уже есть) — иначе два окна делили бы один draft.
pub fn open(backend: Entity<Backend>, cx: &mut App) {
    if let Some(handle) = backend.read(cx).settings_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    if backend.read(cx).preview.is_some() {
        return;
    }
    backend.update(cx, |b, _| b.preview = Some(b.config.clone()));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(160.0), px(120.0)),
            size: size(px(860.0), px(620.0)),
        })),
        titlebar: Some(TitlebarOptions {
            title: Some("MoonTerminal — Настройки".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        kind: WindowKind::Floating,
        app_id: Some("MoonTerminal".to_string()),
        window_min_size: Some(size(px(620.0), px(420.0))),
        window_decorations: design::platform_window_decorations(),
        ..Default::default()
    };
    let b = backend.clone();
    match cx.open_window(opts, move |window, cx| {
        let view = cx.new(|cx| SettingsView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        Ok(handle) => {
            backend.update(cx, |b, _| b.settings_window = Some(handle));
        }
        Err(_) => {
            backend.update(cx, |b, cx| {
                b.preview = None;
                b.settings_window = None;
                cx.notify();
            });
        }
    }
}
