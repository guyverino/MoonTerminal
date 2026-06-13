//! Окно «Стратегии» (порт egui `src/strategies/*` + `window/strategies_window.rs`).
//! Отдельное ОС-окно, 4 панели (дерево → секции → параметры): дерево ядро→папка→
//! стратегия с поиском/фильтрами/чекбоксами (стейджинг) и «Применить» (старт/стоп),
//! секции схемы выбранной стратегии (затемнение неактивных), плашки параметров
//! (read-only, YES/NO, «…» для длинных значений). Зависимости полей/разделов — из
//! `assets/param_deps.toml` (hot-reload, [`rules`]). Читает живой `Backend` (store
//! по ядрам), «Применить» шлёт `session.apply_strategies` (синхр. галок + старт/стоп).

mod filter;
mod rules;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    popover::Popover,
    v_flex, Root, Sizable, StyledExt,
};

use crate::{hex, Backend};
use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection, StrategyRow};
use moon_core::palette;
use moon_core::session::{CoreId, CoreStore};

use filter::StrategyFilter;
use rules::{Rules, Values};

/// Длиннее этого (символов) — значение считаем «длинным»: обрезаем и даём «…».
const LONG_VALUE: usize = 28;

pub type Key = (CoreId, u64);

/// Цвет палитры с альфой → `Rgba` (0xRRGGBBAA). Для подсветки выбора/затемнения.
fn hexa(c: [u8; 3], a: u8) -> Rgba {
    rgba((c[0] as u32) << 24 | (c[1] as u32) << 16 | (c[2] as u32) << 8 | a as u32)
}

/// Состояние окна «Стратегии» (порт egui `StrategiesState` + рендер 4 панелей).
pub struct StrategiesView {
    backend: Entity<Backend>,
    /// Текстовое поле поиска (gpui-component Input) — значение читаем в фильтр.
    search: Entity<InputState>,
    /// Фильтры дерева (вид/направление/только активные); `search` синхр. из инпута.
    filter: StrategyFilter,
    /// Текущая (первичная) стратегия — источник схемы/секций (ядро, id).
    selected: Option<Key>,
    /// Множественный выбор (ядро, id) — подсветка + объединённый показ параметров.
    sel: HashSet<Key>,
    /// Якорь для range-выбора по Shift.
    anchor: Option<Key>,
    /// Плоский порядок видимых стратегий прошлого кадра — для Shift-диапазона.
    flat_order: Vec<Key>,
    /// Индекс выбранной секции в схеме её вида. НЕ сбрасывается при смене стратегии,
    /// только клампится при выходе за диапазон.
    selected_section: usize,
    /// Стейджинг чекбоксов: (ядро, id) → желаемый checked. Уходит на сервер по
    /// старт/стоп отмеченных, затем очищается.
    staged: HashMap<Key, bool>,
    /// Открытое окошко просмотра длинного значения поля: (имя поля, значение).
    popup: Option<(String, String)>,
    /// Раскрытые ядра в дереве.
    expanded_cores: HashSet<CoreId>,
    /// Раскрытые папки в дереве: (ядро, путь).
    expanded_folders: HashSet<(CoreId, String)>,
    /// Правила зависимостей полей (param_deps.toml; hot-reload).
    rules: Rules,
    /// Показывать только активные параметры (галка над параметрами).
    only_active_params: bool,
    focus: FocusHandle,
}

impl StrategiesView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("поиск"));
        // Печать в поиске → перерисовать (значение читаем из инпута в render).
        cx.subscribe(&search, |_this, _e, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();

        // Новые снимки стратегий/схемы (дренаж backend) → перерисовка; заодно
        // hot-reload правил зависимостей (param_deps.toml) — правка файла видна на лету.
        cx.observe(&backend, |this, _b, cx| {
            this.rules.reload_if_changed();
            cx.notify();
        })
        .detach();

        Self {
            backend,
            search,
            filter: StrategyFilter::default(),
            selected: None,
            sel: HashSet::new(),
            anchor: None,
            flat_order: Vec::new(),
            selected_section: 0,
            staged: HashMap::new(),
            popup: None,
            expanded_cores: HashSet::new(),
            expanded_folders: HashSet::new(),
            rules: Rules::load(),
            // По умолчанию неактивные параметры скрыты (галка включена).
            only_active_params: true,
            focus: cx.focus_handle(),
        }
    }

    // ── Выбор ───────────────────────────────────────────────────────────────

    /// Клик по стратегии с учётом модификаторов: Shift — диапазон от якоря (по
    /// `order`), Ctrl/Cmd — добавить/убрать по одной, без модификатора — выбрать одну.
    fn apply_click(&mut self, key: Key, order: &[Key], shift: bool, command: bool) {
        if shift {
            if let Some(a) = self.anchor {
                let ia = order.iter().position(|k| *k == a);
                let ib = order.iter().position(|k| *k == key);
                if let (Some(ia), Some(ib)) = (ia, ib) {
                    let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
                    self.sel = order[lo..=hi].iter().copied().collect();
                } else {
                    self.sel = std::iter::once(key).collect();
                }
            } else {
                self.sel = std::iter::once(key).collect();
                self.anchor = Some(key);
            }
        } else if command {
            if !self.sel.remove(&key) {
                self.sel.insert(key);
            }
            self.anchor = Some(key);
        } else {
            self.sel.clear();
            self.sel.insert(key);
            self.anchor = Some(key);
        }
        // Первичная (источник схемы/секций) — всегда кликнутая. Раздел не сбрасываем.
        self.selected = Some(key);
    }

    // ── Действия (старт/стоп отмеченных) ─────────────────────────────────────

    /// «Старт/стоп отмеченных»: на ядро — изменённые галки (diff стейджинга против
    /// серверного checked) + команда старт/стоп, если у ядра есть отмеченная стратегия
    /// или есть правки галок. Шлёт через `session.apply_strategies`, чистит стейджинг.
    fn apply_start_stop(&mut self, cores: &[(CoreId, String)], start: bool, cx: &mut Context<Self>) {
        // Собрать действия (читаем store), затем применить (повторный borrow backend).
        let mut actions: Vec<(CoreId, Vec<(u64, bool)>, bool)> = Vec::new();
        {
            let b = self.backend.read(cx);
            let store = b.session.store();
            for (core, _) in cores {
                let Some(cd) = store.core(*core) else { continue };
                let mut checks = Vec::new();
                let mut has_checked = false;
                for r in &cd.strategies {
                    let eff = self.staged.get(&(*core, r.id)).copied().unwrap_or(r.checked);
                    if eff != r.checked {
                        checks.push((r.id, eff));
                    }
                    if eff {
                        has_checked = true;
                    }
                }
                if !checks.is_empty() || has_checked {
                    actions.push((*core, checks, start));
                }
            }
        }
        if actions.is_empty() {
            return;
        }
        let b = self.backend.read(cx);
        for (core, checks, st) in actions {
            b.session.apply_strategies(core, checks, Some(st));
        }
        self.staged.clear();
        cx.notify();
    }

    /// Развернуть все узлы (если `collapsed`) или свернуть все (иначе).
    fn expand_collapse_toggle(&mut self, cores: &[(CoreId, String)], store: &CoreStore, collapsed: bool) {
        if collapsed {
            for (c, _) in cores {
                self.expanded_cores.insert(*c);
                if let Some(cd) = store.core(*c) {
                    for r in &cd.strategies {
                        // Раскрываем каждый уровень пути (накопительные префиксы).
                        let mut acc = String::new();
                        for part in r.folder_path.split(['/', '\\']).filter(|s| !s.is_empty()) {
                            if !acc.is_empty() {
                                acc.push('/');
                            }
                            acc.push_str(part);
                            self.expanded_folders.insert((*c, acc.clone()));
                        }
                    }
                }
            }
        } else {
            self.expanded_cores.clear();
            self.expanded_folders.clear();
        }
    }

    // ── Панель 1: дерево ──────────────────────────────────────────────────────

    fn tree_panel(
        &self,
        store: &CoreStore,
        cores: &[(CoreId, String)],
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let accent = rgb(hex(palette::ACCENT));
        let border = rgb(hex(palette::LIFT_HOVER));

        // Поиск временно раскрывает всё; своё состояние раскрытия не трогаем.
        let force_open = self.filter.searching();

        // Узлы ядер → дерево.
        let mut list = v_flex().w_full().gap_0();
        for (core_id, core_name) in cores {
            let Some(cd) = store.core(*core_id) else { continue };
            if cd.strategies.is_empty() || !cd.strategies.iter().any(|r| self.filter.matches(r)) {
                continue;
            }
            let open = force_open || self.expanded_cores.contains(core_id);
            let total = cd.strategies.iter().filter(|r| self.filter.counts(r)).count();
            let active = cd.strategies.iter().filter(|r| self.filter.counts(r) && r.checked).count();
            let label = format!("{}  {}  {}/{}", if open { "▼" } else { "▶" }, core_name, active, total);
            let cid = *core_id;
            list = list.child(
                div()
                    .id(SharedString::from(format!("core-{cid}")))
                    .w_full()
                    .py_0p5()
                    .cursor_pointer()
                    .font_bold()
                    .text_color(accent)
                    .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.expanded_cores, cid);
                        cx.notify();
                    })),
            );
            if !open {
                continue;
            }
            // Вложенное дерево папок (отступ слева — как egui ui.indent).
            let root = build_node(cd.strategies.iter().filter(|r| self.filter.matches(r)));
            let mut prefix: Vec<String> = Vec::new();
            let mut kids: Vec<AnyElement> = Vec::new();
            self.render_node(&root, &cd.strategies, *core_id, &mut prefix, force_open, order, built, &mut kids, cx);
            let mut body = v_flex().w_full().pl_3().gap_0();
            for k in kids {
                body = body.child(k);
            }
            list = list.child(body);
        }

        // Поиск + фильтр вида + фильтр направления.
        let kinds = kinds_present(cores, store);
        let kind_text = self
            .filter
            .kind
            .and_then(|k| kinds.iter().find(|(o, _)| *o == k))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "все типы".to_string());
        let dir_text = match self.filter.dir {
            None => "все".to_string(),
            Some(true) => "SHORT".to_string(),
            Some(false) => "LONG".to_string(),
        };

        let collapsed = self.expanded_cores.is_empty() && self.expanded_folders.is_empty();
        let cores_owned: Arc<Vec<(CoreId, String)>> = Arc::new(cores.to_vec());

        v_flex()
            .w(px(300.0))
            .h_full()
            .border_r_1()
            .border_color(border)
            // ── Фильтры сверху ──
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_1()
                            .items_center()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.search).small().cleanable(true)))
                            .child(self.combo_kind(kind_text, kinds, cx))
                            .child(self.combo_dir(dir_text, cx)),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                Checkbox::new("flt-active")
                                    .label("только активные")
                                    .checked(self.filter.only_active)
                                    .on_click(cx.listener(|this, ch: &bool, _, cx| {
                                        this.filter.only_active = *ch;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("expand-all")
                                    .ghost()
                                    .xsmall()
                                    .label(if collapsed { "▼" } else { "▲" })
                                    .tooltip(if collapsed { "Развернуть всё" } else { "Свернуть всё" })
                                    .on_click({
                                        let cores = cores_owned.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            let store = this.backend.read(cx).session.store();
                                            let coll = this.expanded_cores.is_empty() && this.expanded_folders.is_empty();
                                            // store borrow tied to cx; clone cores for &-call.
                                            let cores_v = cores.as_ref().clone();
                                            this.expand_collapse_toggle(&cores_v, store, coll);
                                            cx.notify();
                                        })
                                    }),
                            ),
                    ),
            )
            .child(div().w_full().h(px(1.0)).bg(border))
            // ── Прокручиваемый список ──
            .child(div().id("strat-tree-scroll").flex_1().w_full().overflow_y_scroll().p_2().child(list))
            // ── Нижняя панель действий ──
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(self.action_bar(cores_owned, store, cx))
            .into_any_element()
    }

    /// Комбобокс фильтра вида (попап-список: «все типы» + присутствующие виды).
    fn combo_kind(&self, current: String, kinds: Vec<(u8, String)>, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        Popover::new("strat-kind-filter")
            .trigger(Button::new("kind-btn").outline().xsmall().label(format!("{current} ▾")))
            .content(move |_state, _window, _cx| {
                let view = view.clone();
                let mut col = v_flex().gap_0p5().p_1().min_w(px(150.0));
                col = col.child(combo_item("kind-all", "все типы", {
                    let view = view.clone();
                    move |app| {
                        view.update(app, |this, c| {
                            this.filter.kind = None;
                            c.notify();
                        });
                    }
                }));
                for (ord, name) in &kinds {
                    let ord = *ord;
                    let view = view.clone();
                    col = col.child(combo_item(format!("kind-{ord}"), name, move |app| {
                        view.update(app, |this, c| {
                            this.filter.kind = Some(ord);
                            c.notify();
                        });
                    }));
                }
                col
            })
            .into_any_element()
    }

    /// Комбобокс фильтра направления (все/LONG/SHORT).
    fn combo_dir(&self, current: String, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        Popover::new("strat-dir-filter")
            .trigger(Button::new("dir-btn").outline().xsmall().label(format!("{current} ▾")))
            .content(move |_state, _window, _cx| {
                let opts: [(&str, Option<bool>); 3] = [("все", None), ("LONG", Some(false)), ("SHORT", Some(true))];
                let mut col = v_flex().gap_0p5().p_1().min_w(px(110.0));
                for (label, val) in opts {
                    let view = view.clone();
                    col = col.child(combo_item(format!("dir-{label}"), label, move |app| {
                        view.update(app, |this, c| {
                            this.filter.dir = val;
                            c.notify();
                        });
                    }));
                }
                col
            })
            .into_any_element()
    }

    /// Нижняя панель действий: старт/стоп отмеченных + счётчик стейджинга.
    fn action_bar(&self, cores: Arc<Vec<(CoreId, String)>>, _store: &CoreStore, cx: &Context<Self>) -> AnyElement {
        // Кнопки видимы всегда (как egui); пустое действие — no-op в apply_start_stop.
        let cs = cores.clone();
        let mut row = h_flex().w_full().p_2().gap_2().items_center();
        row = row.child(
            Button::new("start-checked").primary().xsmall().label("▶ отмеченных").on_click({
                let cs = cs.clone();
                cx.listener(move |this, _, _, cx| {
                    let cores_v = cs.as_ref().clone();
                    this.apply_start_stop(&cores_v, true, cx);
                })
            }),
        );
        row = row.child(
            Button::new("stop-checked").outline().xsmall().label("■ отмеченных").on_click({
                let cs = cs.clone();
                cx.listener(move |this, _, _, cx| {
                    let cores_v = cs.as_ref().clone();
                    this.apply_start_stop(&cores_v, false, cx);
                })
            }),
        );
        if !self.staged.is_empty() {
            row = row.child(
                div()
                    .text_xs()
                    .text_color(rgb(hex(palette::ACCENT)))
                    .child(format!("изменений: {}", self.staged.len())),
            );
        }
        row.into_any_element()
    }

    /// Рекурсивно собирает элементы узла: подпапки (сворачиваемые, с активн./всего),
    /// затем стратегии прямо в этой папке.
    #[allow(clippy::too_many_arguments)]
    fn render_node(
        &self,
        node: &FolderNode,
        strategies: &[StrategyRow],
        core_id: CoreId,
        prefix: &mut Vec<String>,
        force_open: bool,
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        out: &mut Vec<AnyElement>,
        cx: &Context<Self>,
    ) {
        for (name, child) in &node.children {
            prefix.push(name.clone());
            let path_key = prefix.join("/");
            let fkey = (core_id, path_key.clone());
            let fopen = force_open || self.expanded_folders.contains(&fkey);
            let (active, total) = folder_counts(strategies, &self.filter, prefix);
            let flabel = format!("{}  {name}  {active}/{total}", if fopen { "▼" } else { "▶" });
            let fkey_click = fkey.clone();
            out.push(
                div()
                    .id(SharedString::from(format!("folder-{core_id}-{path_key}")))
                    .w_full()
                    .py_0p5()
                    .cursor_pointer()
                    .text_color(rgb(hex(palette::TEXT)))
                    .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
                    .child(flabel)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.expanded_folders, fkey_click.clone());
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if fopen {
                let mut kids: Vec<AnyElement> = Vec::new();
                self.render_node(child, strategies, core_id, prefix, force_open, order, built, &mut kids, cx);
                let mut body = v_flex().w_full().pl_3().gap_0();
                for k in kids {
                    body = body.child(k);
                }
                out.push(body.into_any_element());
            }
            prefix.pop();
        }
        for r in &node.strategies {
            out.push(self.strategy_row(core_id, r, order, built, cx));
        }
    }

    /// Одна строка стратегии: чекбокс (стейджинг) · индикатор запуска · имя (выбор).
    fn strategy_row(
        &self,
        core: CoreId,
        r: &StrategyRow,
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let key = (core, r.id);
        built.push(key);
        let server = r.checked;
        let val = self.staged.get(&key).copied().unwrap_or(server);

        // Подсветка — для всех выбранных (мультивыбор), иначе для первичной.
        let highlighted = if self.sel.is_empty() {
            self.selected == Some(key)
        } else {
            self.sel.contains(&key)
        };
        let dot = if server { palette::GREEN } else { palette::TEXT_3 };
        let type_col = if r.is_short { palette::RED } else { palette::TEXT_3 };

        let order_c = order.clone();
        let mut name_row = div()
            .id(SharedString::from(format!("strat-{core}-{}", r.id)))
            .flex_1()
            .min_w_0()
            .px_1()
            .rounded(px(2.0))
            .cursor_pointer()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().flex_1().min_w_0().truncate().text_color(rgb(hex(palette::TEXT))).child(r.name.clone()))
                    .child(div().text_xs().text_color(rgb(hex(type_col))).child(r.kind.clone())),
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                let m = e.modifiers();
                this.apply_click(key, &order_c, m.shift, m.secondary());
                cx.notify();
            }));
        if highlighted {
            name_row = name_row.bg(hexa(palette::ACCENT, 0x33));
        } else {
            name_row = name_row.hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))));
        }

        h_flex()
            .w_full()
            .items_center()
            .gap_1()
            .py_0p5()
            .child(
                Checkbox::new(SharedString::from(format!("chk-{core}-{}", r.id)))
                    .checked(val)
                    .on_click(cx.listener(move |this, ch: &bool, _, cx| {
                        let v = *ch;
                        if v == server {
                            this.staged.remove(&key);
                        } else {
                            this.staged.insert(key, v);
                        }
                        cx.notify();
                    })),
            )
            .child(div().text_color(rgb(hex(dot))).child("●"))
            .child(name_row)
            .into_any_element()
    }

    // ── Панель 2: разделы (секции) ────────────────────────────────────────────

    fn sections_panel(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
        let border = rgb(hex(palette::LIFT_HOVER));
        let mut col = v_flex()
            .w(px(285.0))
            .h_full()
            .border_r_1()
            .border_color(border)
            .p_2()
            .gap_1()
            .child(div().font_bold().child("Разделы"))
            .child(div().w_full().h(px(1.0)).bg(border));

        let Some(sections) = selected_sections(self, store) else {
            return col
                .child(div().mt_2().text_color(rgb(hex(palette::TEXT_2))).child("выберите стратегию в дереве"))
                .into_any_element();
        };
        if sections.is_empty() {
            return col
                .child(div().mt_2().text_color(rgb(hex(palette::TEXT_2))).child("схема не получена"))
                .into_any_element();
        }
        let values = selected_values(self, store);

        // Порядок: сначала активные, потом неактивные; внутри групп — порядок схемы.
        let mut order: Vec<(usize, bool)> = sections
            .iter()
            .enumerate()
            .map(|(i, sec)| (i, section_active(&self.rules, &values, sec)))
            .collect();
        order.sort_by_key(|(_, active)| !active);

        let mut list = v_flex().w_full().gap_0();
        for (i, active) in order {
            let sec = &sections[i];
            let on = self.selected_section == i;
            let tcol = if !active { palette::TEXT_3 } else { palette::TEXT };
            let mut row = div()
                .id(SharedString::from(format!("sec-{i}")))
                .w_full()
                .px_1()
                .py_0p5()
                .rounded(px(2.0))
                .cursor_pointer()
                .text_color(rgb(hex(tcol)))
                .child(sec.title.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected_section = i;
                    cx.notify();
                }));
            if on {
                row = row.bg(hexa(palette::ACCENT, 0x33));
            } else {
                row = row.hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))));
            }
            list = list.child(row);
        }
        col = col.child(div().id("strat-sections-scroll").flex_1().w_full().overflow_y_scroll().child(list));
        col.into_any_element()
    }

    // ── Панель 3: параметры выбранной секции ────────────────────────────────

    fn params_panel(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
        let mut col = v_flex().flex_1().h_full().p_2().gap_1();

        if selected_row(self, store).is_none() {
            return col
                .child(div().mt_2().text_color(rgb(hex(palette::TEXT_2))).child("выберите стратегию в дереве"))
                .into_any_element();
        }
        let Some(sections) = selected_sections(self, store) else {
            return col.into_any_element();
        };
        let Some(sec) = sections.get(self.selected_section) else {
            return col.into_any_element();
        };
        let values = selected_values(self, store);
        // Объединённый показ по всем выбранным (любых видов).
        let rows = multi_rows(self, store);
        let multi = rows.len() > 1;
        let common = common_fields(self, store);
        let differ = kinds_differ(self, store);

        // Заголовок раздела + счётчик (полей / выбрано) справа.
        let count = if multi {
            format!("выбрано: {}", rows.len())
        } else {
            format!("полей: {}", sec.fields.len())
        };
        col = col
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(div().font_bold().child(sec.title.clone()))
                    .child(div().text_xs().text_color(rgb(hex(palette::TEXT_2))).child(count)),
            )
            .child(
                Checkbox::new("params-only-active")
                    .label("только активные")
                    .checked(self.only_active_params)
                    .on_click(cx.listener(|this, ch: &bool, _, cx| {
                        this.only_active_params = *ch;
                        cx.notify();
                    })),
            )
            .child(div().w_full().h(px(1.0)).bg(rgb(hex(palette::LIFT_HOVER))));

        // Порядок полей — как в схеме. Значения берём из снимка по имени.
        let mut list = v_flex().w_full().gap_0();
        for f in &sec.fields {
            let lname = f.name.to_lowercase();
            if multi && lname == "strategyname" {
                continue;
            }
            if let Some(c) = &common {
                if !c.contains(&lname) {
                    continue;
                }
            }
            if differ && lname == "signaltype" {
                continue;
            }
            let active = self.rules.field_active(&f.name, &values);
            if self.only_active_params && !active {
                continue;
            }
            let merged = merged_value(&rows, f);
            list = list.child(self.field_row(f, merged, active, cx));
        }
        col = col.child(div().id("strat-params-scroll").flex_1().w_full().overflow_y_scroll().child(list));
        col.into_any_element()
    }

    /// Строка поля: имя слева, значение справа. `active=false` — приглушаем тёмным.
    /// `merged=None` — значения у выбранных различаются (помечаем «≠», без значения).
    fn field_row(&self, f: &SchemaField, merged: Option<String>, active: bool, cx: &Context<Self>) -> AnyElement {
        let name_col = if active { palette::TEXT_2 } else { palette::TEXT_3 };
        let val_col = if active { palette::TEXT } else { palette::TEXT_3 };

        let value_el: AnyElement = match merged {
            None => div()
                .font_bold()
                .text_color(rgb(hex(palette::ACCENT)))
                .child("≠")
                .into_any_element(),
            Some(value) => match f.ui {
                SchemaFieldUi::Checkbox => {
                    let on = is_on(&value);
                    let col = if active && on { palette::GREEN } else { palette::TEXT_3 };
                    div().font_bold().text_color(rgb(hex(col))).child(if on { "YES" } else { "NO" }).into_any_element()
                }
                _ => {
                    let long = value.chars().count() > LONG_VALUE;
                    if long {
                        let short: String = value.chars().take(LONG_VALUE).collect();
                        let name = f.name.clone();
                        let full = value.clone();
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new(SharedString::from(format!("more-{}", f.name)))
                                    .ghost()
                                    .xsmall()
                                    .label("…")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.popup = Some((name.clone(), full.clone()));
                                        cx.notify();
                                    })),
                            )
                            .child(div().text_color(rgb(hex(val_col))).child(format!("{short}…")))
                            .into_any_element()
                    } else {
                        div().text_color(rgb(hex(val_col))).child(value).into_any_element()
                    }
                }
            },
        };

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .py_0p5()
            .gap_2()
            .child(div().text_color(rgb(hex(name_col))).child(f.name.clone()))
            .child(value_el)
            .into_any_element()
    }

    /// Окошко просмотра длинного значения (read-only) — оверлей поверх окна.
    fn popup_overlay(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (name, val) = self.popup.clone()?;
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000073))
                .child(
                    v_flex()
                        .w(px(460.0))
                        .max_h(px(360.0))
                        .bg(rgb(hex(palette::SURFACE_1)))
                        .border_1()
                        .border_color(rgb(hex(palette::LIFT_HOVER)))
                        .rounded(px(6.0))
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(rgb(hex(palette::LIFT_HOVER)))
                                .child(div().font_bold().child(name))
                                .child(
                                    Button::new("popup-close").ghost().xsmall().label("×").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.popup = None;
                                            cx.notify();
                                        }),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .id("popup-scroll")
                                .flex_1()
                                .w_full()
                                .overflow_y_scroll()
                                .p_3()
                                .text_color(rgb(hex(palette::TEXT)))
                                .child(val),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl EventEmitter<()> for StrategiesView {}
impl Focusable for StrategiesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for StrategiesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Поиск читаем из инпута в фильтр (единый источник).
        self.filter.search = self.search.read(cx).value().to_string();

        // Список ядер (id, имя) — все подключённые, как egui (session.sessions()).
        let cores: Vec<(CoreId, String)> = {
            let b = self.backend.read(cx);
            b.session.sessions().iter().map(|s| (s.id, s.name.clone())).collect()
        };

        // Клампим выбранный раздел в диапазон (как sections::show).
        {
            let store = self.backend.read(cx).session.store();
            if let Some(secs) = selected_sections(self, store) {
                if self.selected_section >= secs.len() {
                    self.selected_section = 0;
                }
            }
        }

        // Плоский порядок прошлого кадра (для Shift-диапазона) + новый накапливаем.
        let order = Arc::new(self.flat_order.clone());
        let mut built: Vec<Key> = Vec::new();

        let store = self.backend.read(cx).session.store();
        let tree = self.tree_panel(store, &cores, &order, &mut built, cx);
        let sections = self.sections_panel(store, cx);
        let params = self.params_panel(store, cx);
        let overlay = self.popup_overlay(cx);

        // Сохранить порядок текущего кадра (store-borrow держит cx, не self).
        self.flat_order = built;

        let mut root = v_flex()
            .size_full()
            .relative()
            .bg(rgb(hex(palette::BG)))
            .text_color(rgb(hex(palette::TEXT)))
            .text_sm()
            .track_focus(&self.focus)
            .child(h_flex().flex_1().w_full().min_h_0().child(tree).child(sections).child(params));
        if let Some(overlay) = overlay {
            root = root.child(overlay);
        }
        root
    }
}

// ── Чистые помощники (порт `strategies/mod.rs`) ──────────────────────────────

/// Поиск строки стратегии в store.
fn row(store: &CoreStore, core: CoreId, id: u64) -> Option<&StrategyRow> {
    store.core(core)?.strategies.iter().find(|s| s.id == id)
}

/// Выбранная строка стратегии (по `selected`).
fn selected_row<'a>(st: &StrategiesView, store: &'a CoreStore) -> Option<&'a StrategyRow> {
    let (core, id) = st.selected?;
    row(store, core, id)
}

/// Ключи выбранных стратегий (мультивыбор) или первичная, если выбор пуст.
fn selected_keys(st: &StrategiesView) -> Vec<Key> {
    if st.sel.is_empty() {
        st.selected.into_iter().collect()
    } else {
        st.sel.iter().copied().collect()
    }
}

/// Строки ВСЕХ выбранных стратегий (любых видов) — для объединённого показа.
fn multi_rows<'a>(st: &StrategiesView, store: &'a CoreStore) -> Vec<&'a StrategyRow> {
    selected_keys(st).iter().filter_map(|(c, id)| row(store, *c, *id)).collect()
}

/// У выбранных РАЗНЫЕ виды стратегий? (тогда SignalType менять нельзя — скрываем).
fn kinds_differ(st: &StrategiesView, store: &CoreStore) -> bool {
    let mut kind: Option<u8> = None;
    for (c, id) in selected_keys(st) {
        if let Some(r) = row(store, c, id) {
            match kind {
                None => kind = Some(r.kind_ordinal),
                Some(k) if k != r.kind_ordinal => return true,
                _ => {}
            }
        }
    }
    false
}

/// Имена полей (lowercase) в схеме ядра `core` для вида `ord`.
fn kind_field_set(store: &CoreStore, core: CoreId, ord: u8) -> HashSet<String> {
    store
        .core(core)
        .and_then(|cd| cd.schema.as_ref())
        .and_then(|sch| sch.kinds.iter().find(|k| k.ordinal == ord))
        .map(|k| k.sections.iter().flat_map(|s| &s.fields).map(|f| f.name.to_lowercase()).collect())
        .unwrap_or_default()
}

/// Поля (lowercase), которые есть у ВСЕХ выбранных стратегий (пересечение схем их
/// видов). None — выбрана одна (ограничения нет, показываем всё).
fn common_fields(st: &StrategiesView, store: &CoreStore) -> Option<HashSet<String>> {
    let keys = selected_keys(st);
    if keys.len() <= 1 {
        return None;
    }
    let mut acc: Option<HashSet<String>> = None;
    for (c, id) in keys {
        let Some(r) = row(store, c, id) else { continue };
        let set = kind_field_set(store, c, r.kind_ordinal);
        acc = Some(match acc {
            None => set,
            Some(a) => a.intersection(&set).cloned().collect(),
        });
    }
    acc
}

/// Значения полей выбранной стратегии: имя(lowercase) → значение(как есть) — для
/// вычисления зависимостей (depends_on). Несохранённые ядром поля добираем дефолтами.
fn selected_values(st: &StrategiesView, store: &CoreStore) -> Values {
    let mut v = Values::new();
    if let Some(row) = selected_row(st, store) {
        for (name, val) in &row.fields {
            v.insert(name.to_lowercase(), val.clone());
        }
        if let Some(sections) = selected_sections(st, store) {
            for sec in sections {
                for f in &sec.fields {
                    v.entry(f.name.to_lowercase()).or_insert_with(|| f.default.clone().unwrap_or_default());
                }
            }
        }
    }
    v
}

/// Раздел АКТИВЕН (не затемнён), если в нём осталось БОЛЬШЕ ОДНОГО активного поля.
fn section_active(rules: &Rules, values: &Values, sec: &SchemaSection) -> bool {
    sec.fields.iter().filter(|f| rules.field_active(&f.name, values)).count() > 1
}

/// Секции схемы для выбранной стратегии (по её виду). None — нет выбора/схемы.
fn selected_sections<'a>(st: &StrategiesView, store: &'a CoreStore) -> Option<&'a [SchemaSection]> {
    let (core, id) = st.selected?;
    let cd = store.core(core)?;
    let row = cd.strategies.iter().find(|s| s.id == id)?;
    let schema = cd.schema.as_ref()?;
    let kind = schema.kinds.iter().find(|k| k.ordinal == row.kind_ordinal)?;
    Some(&kind.sections)
}

/// Общее значение поля по всем строкам или None, если различаются.
fn merged_value(rows: &[&StrategyRow], f: &SchemaField) -> Option<String> {
    let mut it = rows.iter().map(|r| field_value(r, f));
    let first = it.next()?;
    if it.all(|v| v == first) {
        Some(first)
    } else {
        None
    }
}

/// Значение поля стратегии (по имени) или дефолт схемы.
fn field_value(row: &StrategyRow, f: &SchemaField) -> String {
    row.fields
        .iter()
        .find(|(n, _)| n == &f.name)
        .map(|(_, v)| v.clone())
        .or_else(|| f.default.clone())
        .unwrap_or_default()
}

fn is_on(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on")
}

/// Переключает наличие ключа в множестве (раскрыт/свёрнут).
fn toggle<T: std::cmp::Eq + std::hash::Hash>(set: &mut HashSet<T>, key: T) {
    if !set.remove(&key) {
        set.insert(key);
    }
}

/// Виды стратегий, присутствующие в дереве: (ordinal, имя), отсортировано по имени.
fn kinds_present(cores: &[(CoreId, String)], store: &CoreStore) -> Vec<(u8, String)> {
    let mut map: std::collections::BTreeMap<u8, String> = std::collections::BTreeMap::new();
    for (c, _) in cores {
        if let Some(cd) = store.core(*c) {
            for r in &cd.strategies {
                map.entry(r.kind_ordinal).or_insert_with(|| r.kind.clone());
            }
        }
    }
    let mut v: Vec<(u8, String)> = map.into_iter().collect();
    v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    v
}

/// Узел дерева папок: подпапки (по имени) + стратегии прямо в этой папке.
#[derive(Default)]
struct FolderNode<'a> {
    children: std::collections::BTreeMap<String, FolderNode<'a>>,
    strategies: Vec<&'a StrategyRow>,
}

/// Строит вложенное дерево из путей стратегий (`/` и `\` — разделители).
fn build_node<'a>(it: impl Iterator<Item = &'a StrategyRow>) -> FolderNode<'a> {
    let mut root = FolderNode::default();
    for r in it {
        let mut node = &mut root;
        for part in r.folder_path.split(['/', '\\']).filter(|s| !s.is_empty()) {
            node = node.children.entry(part.to_string()).or_default();
        }
        node.strategies.push(r);
    }
    root
}

/// Активных/всего (по фильтру типа/L/S) во всех стратегиях под путём `prefix`.
fn folder_counts(strategies: &[StrategyRow], filter: &StrategyFilter, prefix: &[String]) -> (usize, usize) {
    let mut active = 0;
    let mut total = 0;
    for r in strategies {
        if !filter.counts(r) {
            continue;
        }
        let parts: Vec<&str> = r.folder_path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
        if parts.len() >= prefix.len() && prefix.iter().zip(parts.iter()).all(|(a, b)| a.as_str() == *b) {
            total += 1;
            if r.checked {
                active += 1;
            }
        }
    }
    (active, total)
}

/// Один пункт попап-комбобокса (кликабельная строка).
fn combo_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .w_full()
        .px_2()
        .py_1()
        .cursor_pointer()
        .rounded(px(3.0))
        .text_color(rgb(hex(palette::TEXT)))
        .hover(|s| s.bg(rgb(hex(palette::LIFT_HOVER))))
        .child(label.into())
        .on_click(move |_, _window, app| on_click(app))
}

/// Открыть окно «Стратегии» (отдельное ОС-окно). Дедуп окон — в `Backend`.
pub fn open(backend: Entity<Backend>, cx: &mut App) {
    // Уже открыто → сфокусировать.
    if let Some(handle) = backend.read(cx).strategies_window {
        if handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return;
        }
    }
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(120.0), px(90.0)),
            size: size(px(1180.0), px(680.0)),
        })),
        titlebar: Some(TitlebarOptions {
            title: Some("MoonTerminal — Стратегии".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        let view = cx.new(|cx| StrategiesView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        backend.update(cx, |bk, _| bk.strategies_window = Some(handle));
    }
}
