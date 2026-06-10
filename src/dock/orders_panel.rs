//! Вкладка «Ордера»: таблица открытых ордеров группы (все ядра), на всю ширину.
//! Колонки (без шапки, подпись — внутри ячейки): Ядро · Сторона · Токен · Size ·
//! SL · TS · Vstop · Bprice · CurPrice · Fill · Strat.
//!
//! Сторона: BUY (лонг, ждёт) — зелёным, SHORT (шорт, ждёт) — красным, SELL
//! (исполнился — позиция открыта/продаётся) — синим.
//!
//! Клик по строке открывает чарт монеты на вкладке Main (фулскрин) НА ЯДРЕ ордера —
//! чтобы сразу торговать. ПКМ по таблице вызывает меню фильтра/сортировки.

use crate::feed::OrderRow;
use crate::session::CoreId;
use crate::shell::theme;

/// Одна строка таблицы ордеров с привязкой к ядру-источнику.
#[derive(Clone)]
pub struct OrderEntry {
    pub core: CoreId,
    pub core_name: String,
    /// Quote ядра (`USDT`/…) — режем из символа в показе токена (`ADAUSDT` → `ADA`).
    pub quote: String,
    pub row: OrderRow,
}

/// Первичный ключ сортировки (тогл-группа в меню ПКМ).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrimarySort {
    SellFirst,
    BuyFirst,
    Creation,
}

impl PrimarySort {
    pub fn to_u8(self) -> u8 {
        match self {
            PrimarySort::Creation => 0,
            PrimarySort::SellFirst => 1,
            PrimarySort::BuyFirst => 2,
        }
    }
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => PrimarySort::SellFirst,
            2 => PrimarySort::BuyFirst,
            _ => PrimarySort::Creation,
        }
    }
}

impl OrderKind {
    pub fn to_u8(self) -> u8 {
        match self {
            OrderKind::All => 0,
            OrderKind::Real => 1,
            OrderKind::Emu => 2,
        }
    }
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => OrderKind::Real,
            2 => OrderKind::Emu,
            _ => OrderKind::All,
        }
    }
}

/// Источник ордеров: все ядра группы или конкретное ядро (поле-список сверху).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OrdersSource {
    All,
    Core(CoreId),
}

/// Фильтр по типу ордера (поле-список): все / реальные / эмуляторные.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    All,
    Real,
    Emu,
}

/// Состояние вида таблицы ордеров (источник + тип + фильтр + сортировка). Своё у окна.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OrdersViewState {
    pub source: OrdersSource,
    pub kind: OrderKind,
    pub only_current_market: bool,
    pub primary: PrimarySort,
    pub newest_first: bool,
}

impl Default for OrdersViewState {
    fn default() -> Self {
        Self {
            source: OrdersSource::All,
            kind: OrderKind::All,
            only_current_market: false,
            primary: PrimarySort::Creation,
            newest_first: true,
        }
    }
}

const ROW_H: f32 = 18.0;
const COL_SPACING: f32 = 8.0;

/// Горизонтальное выравнивание содержимого ячейки.
#[derive(Clone, Copy)]
enum Al {
    Left,
    Center,
    Right,
}

/// Вход (покупка) заполнен.
fn executed(r: &OrderRow) -> bool {
    r.fill_pct >= 99.95
}

/// SELL — исполненный ЛОНГ: куплен (вход заполнен) и выставлен на продажу. НЕ шорт.
fn is_sell(r: &OrderRow) -> bool {
    !r.is_short && executed(r)
}

/// BUY — лонг, ещё не исполнен (ждёт покупки).
fn is_buy(r: &OrderRow) -> bool {
    !r.is_short && !executed(r)
}

/// Рендер таблицы ордеров. `current` — (ядро, маркет) текущего Main-фуллскрина (для
/// фильтра «только текущий маркет»). Возвращает (ядро, маркет) при клике по строке.
pub fn ui(
    ui: &mut egui::Ui,
    entries: &[OrderEntry],
    cores: &[(CoreId, String)],
    view: &mut OrdersViewState,
    current: Option<(CoreId, &str)>,
) -> Option<(CoreId, String)> {
    // Не делаем текст ячеек выделяемым — иначе над ними I-beam-курсор (как в поле
    // ввода). Над колонкой токена курсор выставим «рукой» вручную.
    ui.style_mut().interaction.selectable_labels = false;

    let mut shown: Vec<&OrderEntry> = entries
        .iter()
        .filter(|e| {
            let by_source = match view.source {
                OrdersSource::All => true,
                OrdersSource::Core(id) => e.core == id,
            };
            let by_kind = match view.kind {
                OrderKind::All => true,
                OrderKind::Real => !e.row.emulator,
                OrderKind::Emu => e.row.emulator,
            };
            by_source
                && by_kind
                && (!view.only_current_market
                    || match current {
                        Some((c, m)) => e.core == c && e.row.market == m,
                        None => true,
                    })
        })
        .collect();
    sort_entries(&mut shown, view);

    // Поле-список источника (как в логе): «Все ядра» + каждое ядро группы.
    ui.horizontal(|ui| {
        let cur = match view.source {
            OrdersSource::All => t!("orders.all_cores").to_string(),
            OrdersSource::Core(id) => cores
                .iter()
                .find(|(c, _)| *c == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| t!("orders.all_cores").to_string()),
        };
        egui::ComboBox::from_id_salt("orders_source")
            .selected_text(cur)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(matches!(view.source, OrdersSource::All), t!("orders.all_cores"))
                    .clicked()
                {
                    view.source = OrdersSource::All;
                }
                for (id, name) in cores {
                    let sel = matches!(view.source, OrdersSource::Core(c) if c == *id);
                    if ui.selectable_label(sel, name).clicked() {
                        view.source = OrdersSource::Core(*id);
                    }
                }
            });

        // Тип ордеров: Все / Реальные / Эмуляторные.
        let kind_label = match view.kind {
            OrderKind::All => t!("orders.kind.all"),
            OrderKind::Real => t!("orders.kind.real"),
            OrderKind::Emu => t!("orders.kind.emu"),
        };
        egui::ComboBox::from_id_salt("orders_kind")
            .selected_text(kind_label)
            .show_ui(ui, |ui| {
                for (k, key) in [
                    (OrderKind::All, "orders.kind.all"),
                    (OrderKind::Real, "orders.kind.real"),
                    (OrderKind::Emu, "orders.kind.emu"),
                ] {
                    if ui.selectable_label(view.kind == k, t!(key)).clicked() {
                        view.kind = k;
                    }
                }
            });

        // Счётчик показанных — после полей-списков.
        ui.label(
            egui::RichText::new(format!("{}", shown.len()))
                .size(theme::LABEL_SIZE)
                .color(theme::TEXT_3),
        );
        if view.only_current_market {
            ui.label(
                egui::RichText::new(format!("· {}", t!("orders.only_current")))
                    .size(theme::LABEL_SIZE)
                    .color(theme::TEXT_3),
            );
        }
    });
    ui.add_space(2.0);

    let mut clicked: Option<(CoreId, String)> = None;
    // (rect колонки токена, ядро, маркет) — клик/курсор-рука ТОЛЬКО над ней.
    let mut hits: Vec<(egui::Rect, CoreId, String)> = Vec::new();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .drag_to_scroll(false)
        .show(ui, |ui| {
            // Ширину считаем ВНУТРИ scroll — здесь доступная ширина уже без полосы
            // прокрутки, иначе сумма колонок шире вьюпорта и правый край обрежется.
            let w = Widths::compute(ui.available_width());
            ui.spacing_mut().item_spacing.x = COL_SPACING;
            let grid = egui::Grid::new("orders_grid")
                .striped(true)
                .num_columns(11)
                .min_col_width(0.0)
                .spacing([COL_SPACING, 4.0])
                .show(ui, |ui| {
                    if shown.is_empty() {
                        ui.label(egui::RichText::new(t!("orders.empty")).weak());
                        ui.end_row();
                    }
                    for e in &shown {
                        let token_rect = data_row(ui, e, &w);
                        hits.push((token_rect, e.core, e.row.market.clone()));
                    }
                });

            // Единый обработчик поверх ячеек (Label не-интерактивны): ЛКМ ТОЛЬКО по
            // колонке токена → открыть чарт; ПКМ в любом месте → меню.
            let area = ui.interact(
                grid.response.rect,
                ui.id().with("orders_area"),
                egui::Sense::click(),
            );
            // Курсор-рука, когда над колонкой монеты (намёк, что кликабельно).
            if let Some(pos) = area.hover_pos() {
                if hits.iter().any(|(rc, _, _)| rc.contains(pos)) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
            if area.clicked() {
                if let Some(pos) = area.interact_pointer_pos() {
                    if let Some((_, c, m)) = hits.iter().find(|(rc, _, _)| rc.contains(pos)) {
                        clicked = Some((*c, m.clone()));
                    }
                }
            }
            area.context_menu(|ui| menu(ui, view));
            super::tabs::fill_rest(ui);
        });

    clicked
}

/// Ширины колонок (пропорционально доступной ширине). Веса учитывают инлайн-подписи.
struct Widths {
    core: f32,
    side: f32,
    token: f32,
    size: f32,
    sl: f32,
    ts: f32,
    vstop: f32,
    buy: f32,
    price: f32,
    fill: f32,
    strat: f32,
}

impl Widths {
    fn compute(avail: f32) -> Self {
        let (wc, ws, wt, wsz, wsl, wts, wv, wb, wp, wf, wstr) =
            (1.1, 1.1, 1.0, 1.3, 0.9, 0.9, 1.1, 1.5, 1.6, 1.1, 1.5);
        let total = wc + ws + wt + wsz + wsl + wts + wv + wb + wp + wf + wstr;
        let gaps = COL_SPACING * 10.0;
        let usable = (avail - gaps - 4.0).max(120.0);
        let u = |w: f32| (w / total * usable).max(8.0);
        Self {
            core: u(wc),
            side: u(ws),
            token: u(wt),
            size: u(wsz),
            sl: u(wsl),
            ts: u(wts),
            vstop: u(wv),
            buy: u(wb),
            price: u(wp),
            fill: u(wf),
            strat: u(wstr),
        }
    }
}

/// Одна строка ордера. Возвращает rect колонки токена (для клика/курсора по ней).
fn data_row(ui: &mut egui::Ui, e: &OrderEntry, w: &Widths) -> egui::Rect {
    let r = &e.row;

    cell_text(ui, w.core, Al::Left, &e.core_name, theme::TEXT_2);

    // Сторона: SELL (исполненный лонг, синий) / SHORT (красный) / BUY (зелёный).
    // Эмуляторный ордер помечаем «(E)».
    let (side, col) = if is_sell(r) {
        ("SELL", theme::BLUE)
    } else if r.is_short {
        ("SHORT", theme::RED)
    } else {
        ("BUY", theme::GREEN)
    };
    let side = if r.emulator {
        format!("{side} (E)")
    } else {
        side.to_string()
    };
    cell_text(ui, w.side, Al::Left, &side, col);

    // Токен без quote (`ADAUSDT` → `ADA`) — справа, акцентом (намёк на клик).
    let token = crate::symbol::base_symbol(&r.market, &e.quote);
    let token_rect = cell_text(ui, w.token, Al::Right, token, theme::ACCENT);

    cell_job(ui, w.size, Al::Right, "Sz:", &fmt4(r.size), theme::TEXT_2);
    cell_onoff(ui, w.sl, "SL:", r.sl_on);
    cell_onoff(ui, w.ts, "TS:", r.ts_on);
    cell_onoff(ui, w.vstop, "Vstop:", r.vstop_on);
    cell_job(ui, w.buy, Al::Right, "Buy:", &fmt4(r.buy_price), theme::TEXT_2);
    cell_job(ui, w.price, Al::Right, "Cur.P:", &fmt4(r.price as f64), theme::TEXT_2);
    cell_job(ui, w.fill, Al::Right, "Fill:", &format!("{:.0}%", r.fill_pct), theme::TEXT_2);
    cell_text(ui, w.strat, Al::Right, &r.strat, theme::TEXT_2);
    ui.end_row();
    token_rect
}

fn fmt4(v: f64) -> String {
    format!("{v:.4}")
}

fn layout_of(al: Al) -> egui::Layout {
    match al {
        Al::Left => egui::Layout::left_to_right(egui::Align::Center),
        Al::Right => egui::Layout::right_to_left(egui::Align::Center),
        // Горизонтальный центр + вертикальный центр, без растягивания текста.
        Al::Center => {
            egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Center)
        }
    }
}

/// Единый формат сегмента для ВСЕХ ячеек — шрифт/размер `TextFormat::default`
/// (как у «Vstop»), чтобы вся таблица была одним шрифтом.
fn fmt_seg(color: egui::Color32) -> egui::text::TextFormat {
    egui::text::TextFormat {
        color,
        ..Default::default()
    }
}

/// Ячейка с одноцветным текстом. Возвращает её прямоугольник.
fn cell_text(ui: &mut egui::Ui, w: f32, al: Al, text: &str, color: egui::Color32) -> egui::Rect {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.append(text, 0.0, fmt_seg(color));
    cell_job_raw(ui, w, al, job)
}

/// Ячейка «подпись:значение» (подпись тусклая, значение — своим цветом).
fn cell_job(ui: &mut egui::Ui, w: f32, al: Al, label: &str, value: &str, vcolor: egui::Color32) {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.append(label, 0.0, fmt_seg(theme::TEXT_3));
    job.append(value, 0.0, fmt_seg(vcolor));
    cell_job_raw(ui, w, al, job);
}

/// Рисует ячейку ТОЧНОЙ ширины `w` (через set_min_width — иначе egui усаживает
/// ячейку до контента и колонки выходят неравномерными). Возвращает её прямоугольник.
fn cell_job_raw(ui: &mut egui::Ui, w: f32, al: Al, job: egui::text::LayoutJob) -> egui::Rect {
    ui.allocate_ui_with_layout(egui::vec2(w, ROW_H), layout_of(al), |ui| {
        ui.set_min_width(w);
        ui.set_min_height(ROW_H);
        ui.add(egui::Label::new(job).truncate());
    })
    .response
    .rect
}

/// Ячейка флага по центру: «SL:ON» (зелёным) / «SL:OFF» (тускло).
fn cell_onoff(ui: &mut egui::Ui, w: f32, label: &str, on: bool) {
    let (v, c) = if on {
        ("ON", theme::GREEN)
    } else {
        ("OFF", theme::MUTED)
    };
    cell_job(ui, w, Al::Center, label, v, c);
}

fn sort_entries(entries: &mut [&OrderEntry], view: &OrdersViewState) {
    entries.sort_by(|a, b| {
        // Сначала группа по выбранной стороне (Sell/Buy всегда первыми), ВНУТРИ группы
        // — по времени (новые/старые). Та же сортировка по времени и в остальных.
        let ka = primary_key(view.primary, &a.row);
        let kb = primary_key(view.primary, &b.row);
        ka.cmp(&kb).then_with(|| {
            let c = a.row.uid.cmp(&b.row.uid);
            if view.newest_first {
                c.reverse()
            } else {
                c
            }
        })
    });
}

/// Ключ группировки по ОТОБРАЖАЕМОЙ стороне (с учётом исполнения → SELL). 0 = выше.
fn primary_key(p: PrimarySort, r: &OrderRow) -> u8 {
    match p {
        PrimarySort::Creation => 0,
        // SELL (исполненный лонг) первыми.
        PrimarySort::SellFirst => u8::from(!is_sell(r)),
        // BUY (лонг, ещё не исполнен) первыми.
        PrimarySort::BuyFirst => u8::from(!is_buy(r)),
    }
}

/// Меню ПКМ: фильтр (тогл) + две тогл-группы сортировки.
fn menu(ui: &mut egui::Ui, view: &mut OrdersViewState) {
    if ui
        .selectable_label(view.only_current_market, t!("orders.only_current"))
        .clicked()
    {
        view.only_current_market = true;
    }
    if ui
        .selectable_label(!view.only_current_market, t!("orders.show_all"))
        .clicked()
    {
        view.only_current_market = false;
    }
    ui.separator();
    for (variant, key) in [
        (PrimarySort::SellFirst, "orders.sort.sell"),
        (PrimarySort::BuyFirst, "orders.sort.buy"),
        (PrimarySort::Creation, "orders.sort.creation"),
    ] {
        if ui
            .selectable_label(view.primary == variant, t!(key))
            .clicked()
        {
            view.primary = variant;
        }
    }
    ui.separator();
    if ui
        .selectable_label(view.newest_first, t!("orders.sort.new"))
        .clicked()
    {
        view.newest_first = true;
    }
    if ui
        .selectable_label(!view.newest_first, t!("orders.sort.old"))
        .clicked()
    {
        view.newest_first = false;
    }
}
