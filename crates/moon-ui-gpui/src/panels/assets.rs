//! Панель/окно «Активы». Сверху — полоса ядер (баланс USDT) + таблица позиций/балансов
//! по всем ядрам охвата (стоимость/итоги в USDT, фильтр >1 USDT + галка «показать всё»).
//! Снизу (только в отдельном окне) — список ядер слева (свободно/итого) и 3 контейнера
//! кошельков (Спот/Фьючерсы/Квартальные) справа: перетаскивание монеты между ними
//! открывает диалог количества (дефолт — всё свободное) и выполняет перенос.
//!
//! Один и тот же `AssetsView` живёт двумя способами:
//! - как dock-панель в окне группы (`AssetsScope::Group`) — активы ядер группы;
//! - как глобальное singleton-окно (`AssetsScope::All`, открывается кнопкой «⧉») —
//!   активы ВСЕХ подключённых ядер. Дедуп окна — в `Backend.assets_window` (как «Стратегии»).

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonCheckbox, MoonCheckboxSize,
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonInput, MoonInputState,
    MoonPalette, MoonTone, MoonWindowFrame, Panel, PanelEvent, PanelState, Root, h_flex, v_flex,
};

use crate::Backend;
use crate::design;
use moon_core::feed::{AssetRow, TransferAssetRow, WalletKind};
use moon_core::session::CoreId;

/// Высота титлбара окна «Активы» (как у окна «Стратегии»).
const ASSETS_HEADER_H: f32 = 32.0;

/// Область охвата панели «Активы».
#[derive(Clone)]
enum AssetsScope {
    /// Dock-панель окна группы — ядра этой группы.
    Group(String),
    /// Глобальное окно — все подключённые ядра.
    All,
}

/// Полезная нагрузка drag&drop переноса актива между кошельками.
#[derive(Clone)]
struct AssetDrag {
    core: CoreId,
    asset: String,
    from: WalletKind,
    /// Свободное количество монеты (дефолт для диалога — перенести всё).
    free: f64,
}

/// Ожидающий подтверждения перенос (открыт диалог количества).
#[derive(Clone)]
struct PendingTransfer {
    core: CoreId,
    asset: String,
    from: WalletKind,
    to: WalletKind,
    /// Свободное количество (максимум / дефолт).
    free: f64,
}

/// Превью под курсором при перетаскивании листа.
struct AssetDragPreview {
    label: SharedString,
}

impl Render for AssetDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .px_2()
            .py_1()
            .rounded(px(4.0))
            .bg(rgb(p.shell_high))
            .border_1()
            .border_color(rgb(p.blue))
            .text_color(rgb(p.text))
            .text_size(design::text_px(cx, 10.5))
            .font_family(design::mono())
            .child(self.label.clone())
    }
}

/// Строка таблицы активов с привязкой к ядру + посчитанная USDT-стоимость.
struct AssetEntry {
    core_name: String,
    row: AssetRow,
    /// Текущая стоимость в USDT.
    value: f64,
}

/// Подытог по ядру: баланс свободно/итого в USDT (для полосы ядер и левого списка).
struct CoreAgg {
    id: CoreId,
    name: String,
    /// Свободный баланс в USDT (btc_total * курс).
    free: f64,
    /// Итоговый баланс в USDT (btc_full * курс, с нереализ. PnL).
    total: f64,
}

fn num(v: f64) -> String {
    moon_core::util::fmt::adaptive(v)
}

/// Разбить целую часть на тройки пробелом: "1111" → "1 111".
fn group_thousands(int: &str) -> String {
    let len = int.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// Денежный формат USDT: тысячи через пробел, 1 знак после запятой (через `,`),
/// знак `$` в конце. Пример: `1 111,1$`.
fn money(v: f64) -> String {
    let neg = v < 0.0;
    let s = format!("{:.1}", v.abs()); // "1111.1"
    let (int, frac) = s.split_once('.').unwrap_or((s.as_str(), "0"));
    format!(
        "{}{},{frac}$",
        if neg { "-" } else { "" },
        group_thousands(int)
    )
}

/// Человекочитаемая категория рынка: listed-байт + quote.
fn kind_label(row: &AssetRow) -> String {
    let k = match row.listed {
        1 => "spot",
        2 => "fut",
        3 => "both",
        _ => "?",
    };
    format!("{k}·{}", row.quote)
}

/// Окно/панель «Активы».
pub struct AssetsView {
    backend: Entity<Backend>,
    scope: AssetsScope,
    /// true = вид живёт в отдельном ОС-окне (рисует свой титлбар/контролы и дерево
    /// переноса); false = dock-вкладка (только таблица позиций).
    windowed: bool,
    /// Выбранное ядро для нижних контейнеров кошельков.
    selected_core: Option<CoreId>,
    /// Показывать ВСЁ (иначе только балансы >1 USDT с известной ценой).
    show_all: bool,
    /// Открытый диалог переноса (количество) + поле ввода.
    pending_transfer: Option<PendingTransfer>,
    transfer_input: Option<Entity<MoonInputState>>,
    /// Сигнатура данных прошлого кадра (assets_rev/transfer_rev ядер) — гейт перерисовки.
    last_sig: u64,
    /// Секундное ведро (~1 Гц) для обновления цен/стоимости.
    last_sec: u64,
    last_notify_ms: f64,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl AssetsView {
    fn new(
        backend: Entity<Backend>,
        scope: AssetsScope,
        windowed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Перерисовка по дренажу backend — только при изменении активов (rev) или раз в сек.
        cx.observe(&backend, |this, backend, cx| {
            let now = moon_chart::paint::now_unix_ms();
            let sig = this.assets_sig(backend.read(cx));
            let sec = (now as u64) / 1000;
            let changed = sig != this.last_sig || sec != this.last_sec;
            if changed && now - this.last_notify_ms >= 250.0 {
                this.last_sig = sig;
                this.last_sec = sec;
                this.last_notify_ms = now;
                cx.notify();
            }
        })
        .detach();

        // Только отдельное окно сохраняет свою геометрию (dock-панель живёт в окне группы).
        if windowed {
            cx.observe_window_bounds(window, |this, window, cx| {
                let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
                    return;
                };
                this.backend.update(cx, |b, _| {
                    if b.layout.assets_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                        b.layout.assets_window =
                            Some(moon_core::config::layout::GeomRect { x, y, w, h });
                        b.layout_dirty = true;
                    }
                });
            })
            .detach();
        }

        let mut this = Self {
            backend,
            scope,
            windowed,
            selected_core: None,
            show_all: false,
            pending_transfer: None,
            transfer_input: None,
            last_sig: 0,
            last_sec: 0,
            last_notify_ms: 0.0,
            dock: None,
            focus: cx.focus_handle(),
        };
        // Выбрать первое ядро охвата и запросить его transfer-активы для дерева.
        let first = this
            .scope_cores(this.backend.read(cx))
            .first()
            .map(|(id, _)| *id);
        if let Some(core) = first {
            this.selected_core = Some(core);
            this.backend.read(cx).session.refresh_transfer_assets(core);
        }
        this
    }

    /// Реконструкция dock-панели из `docks.json` (группа из state) — вкладка, без дерева.
    pub fn restored_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, AssetsScope::Group(group), false, window, cx)
    }

    /// Ядра охвата (id, имя): группа → ядра группы; глобально → все подключённые.
    fn scope_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        b.session
            .sessions()
            .iter()
            .filter(|s| match &self.scope {
                AssetsScope::Group(g) => &s.group == g,
                AssetsScope::All => true,
            })
            .map(|s| (s.id, s.name.clone()))
            .collect()
    }

    /// Сигнатура активов охвата (assets_rev/transfer_rev ядер) — гейт перерисовки.
    fn assets_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        self.scope_cores(b)
            .iter()
            .filter_map(|(id, _)| store.core(*id))
            .fold(0u64, |a, c| {
                a.wrapping_mul(31)
                    .wrapping_add(c.assets_rev)
                    .wrapping_mul(31)
                    .wrapping_add(c.transfer_rev)
            })
    }

    /// Строки таблицы по всем ядрам охвата (с USDT-стоимостью), отсортированные по
    /// убыванию стоимости. По умолчанию — только >1 USDT (или открытая позиция); галка
    /// «показать всё» снимает фильтр.
    fn collect(&self, b: &Backend) -> Vec<AssetEntry> {
        let store = b.session.store();
        let mut out = Vec::new();
        for (id, name) in self.scope_cores(b) {
            let Some(cd) = store.core(id) else { continue };
            for row in &cd.assets.rows {
                let value = row.value_usdt;
                let keep = self.show_all || value > 1.0 || row.pos_size != 0.0;
                if !keep {
                    continue;
                }
                out.push(AssetEntry {
                    core_name: name.clone(),
                    row: row.clone(),
                    value,
                });
            }
        }
        sort_by_value(&mut out);
        out
    }

    /// Балансы по каждому ядру охвата (свободно/итого в USDT, посчитаны на ядре).
    fn per_core(&self, b: &Backend) -> Vec<CoreAgg> {
        let store = b.session.store();
        self.scope_cores(b)
            .into_iter()
            .map(|(id, name)| {
                let mut free = 0.0;
                let mut total = 0.0;
                if let Some(cd) = store.core(id) {
                    // USDT-баланс уже посчитан на ядре с учётом базовой валюты.
                    free = cd.assets.global.free_usdt;
                    total = cd.assets.global.total_usdt;
                }
                CoreAgg {
                    id,
                    name,
                    free,
                    total,
                }
            })
            .collect()
    }
}

/// Сортировка строк по убыванию USDT-стоимости (самые большие сверху).
fn sort_by_value(out: &mut [AssetEntry]) {
    out.sort_by(|a, b| {
        b.value
            .partial_cmp(&a.value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

impl AssetsView {
    /// Верхняя панель управления: счётчик, галка «показать всё», итоги (Σ стоимость / Σ PnL).
    fn controls(
        &self,
        count: usize,
        total_value: f64,
        total_pnl: f64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let pnl_tone = if total_pnl > 0.0 {
            p.green
        } else if total_pnl < 0.0 {
            p.red
        } else {
            p.text_muted
        };

        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child(format!("{count}")),
            )
            .child(
                MoonCheckbox::new("assets-show-all")
                    .label("показать всё")
                    .checked(self.show_all)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|this, ch: &bool, _, cx| {
                        if this.show_all != *ch {
                            this.show_all = *ch;
                            cx.notify();
                        }
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_soft))
                    .child(format!("Σ {}", money(total_value))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pnl_tone))
                    .child(format!("PnL {}", money(total_pnl))),
            )
    }

    /// Полоса ядер (горизонтальная, с переносом): по ядру — имя + баланс в USDT (округлён).
    fn core_strip(&self, aggs: &[CoreAgg], cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let mut row = h_flex()
            .w_full()
            .flex_none()
            .flex_wrap()
            .gap_2()
            .px_2()
            .py_1();
        for a in aggs {
            row = row.child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(rgb(p.shell_high))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.text))
                            .child(a.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.text_soft))
                            .child(money(a.total)),
                    ),
            );
        }
        row
    }

    /// Нижняя секция: слева список ядер (имя + свободно/итого, выбор), справа —
    /// таблица активов выбранного ядра (полные колонки) + дерево кошельков с переносом.
    fn bottom(&self, b: &Backend, cores: &[(CoreId, String)], cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        // Эффективный выбор: сохранённый, если он есть в охвате, иначе первое ядро.
        let selected = self
            .selected_core
            .filter(|c| cores.iter().any(|(id, _)| id == c))
            .or_else(|| cores.first().map(|(id, _)| *id));
        let aggs = self.per_core(b);

        // ── Левая колонка: список ядер (имя + свободно/итого USDT) ──
        let mut list = v_flex().w_full().gap_0();
        for (id, name) in cores {
            let cid = *id;
            let active = selected == Some(cid);
            let (free, total) = aggs
                .iter()
                .find(|a| a.id == cid)
                .map(|a| (a.free, a.total))
                .unwrap_or((0.0, 0.0));
            let mut item = h_flex()
                .id(SharedString::from(format!("asset-core-{cid}")))
                .w_full()
                .h(design::fit_h_px(cx, 24.0, 13.0, 5.0))
                .px(design::ui_px(cx, 8.0))
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .text_color(rgb(p.text))
                .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.text_soft))
                        .child(format!("{} / {}", money(free), money(total))),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.selected_core != Some(cid) {
                        this.selected_core = Some(cid);
                        this.backend.read(cx).session.refresh_transfer_assets(cid);
                        cx.notify();
                    }
                }));
            if active {
                item = item.bg(rgb(p.panel)).text_color(rgb(p.blue));
            } else {
                item = item.hover(|s| s.bg(rgb(p.shell_high)));
            }
            list = list.child(item);
        }

        let left = v_flex()
            .w(px(240.0))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 4.0))
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child("Ядра · своб/итого"),
            )
            .child(
                div()
                    .id("asset-core-list")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .child(list),
            );

        // ── Правая часть: 3 контейнера кошельков (Спот/Фьючерсы/Квартальные) ──
        let right = match selected {
            Some(core) => self.wallets_section(b, core, cx).into_any_element(),
            None => div()
                .p_4()
                .text_color(rgb(p.text_muted))
                .child("нет ядер")
                .into_any_element(),
        };

        h_flex()
            .w_full()
            .h(px(380.0))
            .flex_none()
            .border_t_1()
            .border_color(rgb(p.border))
            .child(left)
            .child(div().flex_1().h_full().min_w_0().child(right))
    }

    /// Секция кошельков ядра: заголовок (+ ↻ refresh) и 3 контейнера в ряд.
    fn wallets_section(&self, b: &Backend, core: CoreId, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .h_full()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 4.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.text_muted))
                            .child("Кошельки · перетащи монету между контейнерами"),
                    )
                    .child(
                        MoonButton::new("assets-refresh-transfer")
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .label("↻")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend.read(cx).session.refresh_transfer_assets(core);
                                cx.notify();
                            }))
                            .render(),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .children(
                        WalletKind::ALL
                            .into_iter()
                            .map(|kind| self.wallet_column(b, core, kind, cx)),
                    ),
            )
    }

    /// Один контейнер кошелька (Спот/Фьючерсы/Квартальные): монеты (draggable) и
    /// drop-таргет. Бросок монеты из другого кошелька открывает диалог количества.
    fn wallet_column(
        &self,
        b: &Backend,
        core: CoreId,
        kind: WalletKind,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let all_items = b
            .session
            .store()
            .core(core)
            .map(|c| c.transfer_assets.wallet(kind).to_vec())
            .unwrap_or_default();
        // Фильтр >1 USDT (нет цены → скрыто), сортировка по убыванию стоимости.
        let mut items: Vec<&TransferAssetRow> = all_items
            .iter()
            .filter(|a| self.show_all || a.value_usdt > 1.0)
            .collect();
        items.sort_by(|a, b| {
            b.value_usdt
                .partial_cmp(&a.value_usdt)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut list = v_flex().w_full().gap_0().p(px(4.0));
        if items.is_empty() {
            list = list.child(
                div()
                    .px(design::ui_px(cx, 6.0))
                    .py(px(2.0))
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child("—"),
            );
        }
        for a in items {
            let drag = AssetDrag {
                core,
                asset: a.currency.clone(),
                from: kind,
                free: a.amount,
            };
            let preview_label: SharedString = format!("{} {}", a.currency, num(a.amount)).into();
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "coin-{core}-{}-{}",
                        kind.to_u8(),
                        a.currency
                    )))
                    .w_full()
                    .h(design::fit_h_px(cx, 26.0, 12.0, 6.0))
                    .px(design::ui_px(cx, 6.0))
                    .rounded(px(3.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_grab()
                    .text_color(rgb(p.text))
                    // Заметная подсветка строки при наведении (видно, что потащишь).
                    .hover(|s| s.bg(rgba(0x3b82f626)).border_color(rgb(p.blue)))
                    .border_1()
                    .border_color(rgba(0x00000000))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.blue))
                            .child(a.currency.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(p.text_muted))
                            .child(num(a.amount)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(p.text_soft))
                            .child(money(a.value_usdt)),
                    )
                    .on_drag(drag, move |_d, _pos, _w, cx| {
                        cx.new(|_| AssetDragPreview {
                            label: preview_label.clone(),
                        })
                    }),
            );
        }

        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .px(design::ui_px(cx, 6.0))
                    .py(design::ui_px(cx, 3.0))
                    .bg(rgb(p.shell_high))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.text_soft))
                    .child(format!("{} ({})", kind.label(), all_items.len())),
            )
            .child(
                div()
                    .id(SharedString::from(format!("wallet-col-{core}-{}", kind.to_u8())))
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .drag_over::<AssetDrag>(|s, _drag, _w, _cx| s.bg(rgba(0x3b82f622)))
                    .on_drop(cx.listener(move |this, drag: &AssetDrag, window, cx| {
                        if drag.core == core && drag.from != kind {
                            this.open_transfer_dialog(drag, kind, window, cx);
                        }
                    }))
                    .child(list),
            )
    }

    /// Открыть диалог количества для переноса монеты (дефолт — всё свободное).
    fn open_transfer_dialog(
        &mut self,
        drag: &AssetDrag,
        to: WalletKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_qty = num(drag.free);
        let input = cx.new(|cx| MoonInputState::new(window, cx).default_value(&default_qty));
        self.pending_transfer = Some(PendingTransfer {
            core: drag.core,
            asset: drag.asset.clone(),
            from: drag.from,
            to,
            free: drag.free,
        });
        self.transfer_input = Some(input);
        cx.notify();
    }

    /// Подтвердить перенос: прочитать количество из поля, выполнить и закрыть диалог.
    fn confirm_transfer(&mut self, cx: &mut Context<Self>) {
        let Some(pt) = self.pending_transfer.clone() else {
            return;
        };
        let qty = self
            .transfer_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .and_then(|s| s.trim().replace(',', ".").parse::<f64>().ok())
            .unwrap_or(0.0);
        if qty > 0.0 {
            self.backend.read(cx).session.transfer_asset(
                pt.core,
                pt.asset.clone(),
                qty,
                pt.from,
                pt.to,
            );
        }
        self.close_transfer_dialog(cx);
    }

    fn close_transfer_dialog(&mut self, cx: &mut Context<Self>) {
        self.pending_transfer = None;
        self.transfer_input = None;
        cx.notify();
    }

    /// Оверлей-диалог количества переноса (по центру окна).
    fn transfer_dialog(&self, pt: &PendingTransfer, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let title = format!(
            "Перенос {}: {} → {}",
            pt.asset,
            pt.from.label(),
            pt.to.label()
        );
        let mut card = v_flex()
            .w(px(320.0))
            .gap(design::ui_px(cx, 10.0))
            .p(design::ui_px(cx, 14.0))
            .rounded(px(8.0))
            .bg(rgb(p.shell_high))
            .border_1()
            .border_color(rgb(p.border))
            .child(div().text_color(rgb(p.text)).child(title))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child(format!("свободно: {}", num(pt.free))),
            );
        if let Some(input) = &self.transfer_input {
            card = card.child(MoonInput::new("transfer-amount").state(input).small());
        }
        card = card.child(
            h_flex()
                .w_full()
                .gap_2()
                .justify_end()
                .child(
                    MoonButton::new("transfer-cancel")
                        .outline()
                        .size(MoonButtonSize::Action)
                        .label("Отмена")
                        .on_click(cx.listener(|this, _, _, cx| this.close_transfer_dialog(cx)))
                        .render(),
                )
                .child(
                    MoonButton::new("transfer-confirm")
                        .primary()
                        .size(MoonButtonSize::Action)
                        .label("Перенести")
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_transfer(cx)))
                        .render(),
                ),
        );

        // Затемнённый фон на всё окно + карточка по центру.
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            .child(card)
    }
}

impl EventEmitter<PanelEvent> for AssetsView {}
impl Focusable for AssetsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for AssetsView {
    fn panel_name(&self) -> &'static str {
        "Assets"
    }
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Активы")
    }
    fn dump(&self, _cx: &App) -> PanelState {
        let group = match &self.scope {
            AssetsScope::Group(g) => g.clone(),
            AssetsScope::All => String::new(),
        };
        crate::dock_persist::panel_state_with_group("Assets", &group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    /// Кнопка «⧉»: открыть ГЛОБАЛЬНОЕ окно «Активы» (все ядра, singleton) — в отличие
    /// от Orders это не per-group detach, а отдельное окно (как «Стратегии»).
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        let backend = self.backend.clone();
        Some(vec![
            MoonButton::new("assets-open-global")
                .ghost()
                .size(MoonButtonSize::Action)
                .label("⧉")
                .on_click(move |_, window, app| {
                    open(backend.clone(), Some(window.window_handle()), app);
                })
                .render()
                .into_any_element(),
        ])
    }
}

impl Render for AssetsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let b = self.backend.read(cx);
        let cores = self.scope_cores(b);
        let entries = self.collect(b);
        let p = MoonPalette::active(cx);
        let windowed = self.windowed;

        let count = entries.len();
        let total_value: f64 = entries.iter().map(|e| e.value).sum();
        let total_pnl: f64 = entries
            .iter()
            .map(|e| e.row.profit_b + e.row.profit_l + e.row.profit_s)
            .sum();

        let aggs = self.per_core(b);
        let controls = self.controls(count, total_value, total_pnl, cx);
        let core_strip = self.core_strip(&aggs, cx);
        // Дерево переноса (список ядер + кошельки) — только в отдельном ОКНЕ; во вкладке
        // показываем открытые позиции/балансы по всем ядрам охвата (таблица на всю ширину).
        let tree_section = windowed.then(|| self.bottom(b, &cores, cx).into_any_element());
        let table = assets_table("assets-table", entries, cx);

        // Ширина окна для хит-оверлея титлбара (drag/resize/контролы) — как у «Стратегий».
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(bb)
            | WindowBounds::Maximized(bb)
            | WindowBounds::Fullscreen(bb) => f32::from(bb.size.width),
        };

        let mut root = v_flex()
            .id("assets-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::text_px(cx, 10.5))
            .bg(rgb(p.table_body))
            .when(windowed, |this| this.child(assets_header(p, cx)))
            .child(controls)
            .child(core_strip)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(table)
            .children(tree_section);
        if windowed {
            root = root.child(
                MoonWindowFrame::tool("assets-window-frame-hit", chrome_width)
                    .header_height(ASSETS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            );
        }
        // Модальный диалог количества переноса — поверх всего.
        if let Some(pt) = self.pending_transfer.clone() {
            root = root.child(self.transfer_dialog(&pt, cx));
        }
        root
    }
}

/// Титлбар окна «Активы» (drag-кластер слева + системные контролы справа).
fn assets_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("assets-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, ASSETS_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgb(p.shell_high))
        .border_b(px(1.0))
        .border_color(rgb(p.border))
        .child(
            MoonWindowFrame::tool("assets-titlebar-title", 0.0)
                .title_cluster("Активы", cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("assets-window-frame-visual", 0.0)
                    .header_height(ASSETS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

fn assets_columns() -> Vec<MoonDataTableColumn> {
    let numeric = |title: &str, w: f32| MoonDataTableColumn::new(title.to_lowercase(), title, w).right();
    vec![
        MoonDataTableColumn::new("core", "Ядро", 90.0),
        MoonDataTableColumn::new("coin", "Актив", 70.0),
        numeric("Кол-во", 90.0),
        numeric("Цена", 84.0),
        numeric("Стоим.$", 92.0),
        numeric("Поз.", 80.0),
        numeric("Поз.цена", 84.0),
        numeric("Профит", 86.0),
        MoonDataTableColumn::new("kind", "Рынок", 80.0),
    ]
}

fn assets_table(id: &'static str, entries: Vec<AssetEntry>, cx: &Context<AssetsView>) -> impl IntoElement {
    let empty = entries.is_empty();
    let rows = Rc::new(entries);
    let row_count = rows.len();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);

    div()
        .id(SharedString::from(format!("{id}-host")))
        .relative()
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .overflow_hidden()
        .bg(rgb(p.table_body))
        .child(
            MoonDataTable::new(id, row_count, move |ix, _window, _app| {
                assets_row(&table_rows[ix], p)
            })
            .columns(assets_columns())
            .header_height(design::TABLE_HEAD_H)
            .row_height(design::TABLE_ROW_H),
        )
        .when(empty, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(10.0))
                    .top(px(design::TABLE_HEAD_H))
                    .h(px(design::TABLE_ROW_H))
                    .flex()
                    .items_center()
                    .font_family(design::mono())
                    .text_size(design::text_px(cx, 10.5))
                    .text_color(rgb(p.text_muted))
                    .child("нет активов"),
            )
        })
}

fn assets_row(e: &AssetEntry, p: MoonPalette) -> MoonDataRow {
    let r = &e.row;
    let pnl = r.profit_b + r.profit_l + r.profit_s;
    let pnl_tone = if pnl > 0.0 {
        MoonTone::Positive
    } else if pnl < 0.0 {
        MoonTone::Danger
    } else {
        MoonTone::Muted
    };
    let pos = if r.pos_size != 0.0 {
        num(r.pos_size)
    } else {
        String::new()
    };
    let pos_price = if r.pos_size != 0.0 {
        num(r.pos_price)
    } else {
        String::new()
    };
    let _ = p;
    MoonDataRow::new([
        MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
        MoonDataCell::text(r.coin.clone()).tone(MoonTone::Accent).weight(500.0),
        MoonDataCell::text(num(r.qty)),
        MoonDataCell::text(num(r.price)),
        MoonDataCell::text(money(e.value)),
        MoonDataCell::text(pos),
        MoonDataCell::text(pos_price),
        MoonDataCell::text(money(pnl)).tone(pnl_tone),
        MoonDataCell::text(kind_label(r)).tone(MoonTone::Muted),
    ])
}

/// Открыть глобальное окно «Активы» (singleton, все ядра). Дедуп — в `Backend.assets_window`.
pub fn open(backend: Entity<Backend>, _owner: Option<AnyWindowHandle>, cx: &mut App) {
    // Уже открыто → сфокусировать.
    if let Some(handle) = backend.read(cx).assets_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let saved = backend.read(cx).layout.assets_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(140.0), px(110.0)),
            size: size(px(1180.0), px(720.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    let display_id = saved.and_then(|g| {
        let origin = point(px(g.x as f32), px(g.y as f32));
        cx.displays()
            .into_iter()
            .find(|d| d.bounds().contains(&origin))
            .map(|d| d.id())
    });
    let mut opts = crate::windowing::standalone_window_options(
        "MoonTerminal — Активы",
        WindowBounds::Windowed(bounds),
        Some(size(px(900.0), px(560.0))),
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        let view = cx.new(|cx| AssetsView::new(b, AssetsScope::All, true, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.assets_window = Some(handle));
    }
}
