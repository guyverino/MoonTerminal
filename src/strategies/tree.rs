//! Панель 1: дерево ядро→папка→стратегия с поиском, счётчиками запущ./всего,
//! индикатором запуска, чекбоксами (стейджинг) и кнопкой «Применить» (старт/стоп).

use super::{Key, StratAction, StrategiesOut, StrategiesState};
use crate::feed::StrategyRow;
use crate::session::{CoreId, CoreStore};
use crate::shell::theme;

pub fn show(
    ui: &mut egui::Ui,
    st: &mut StrategiesState,
    cores: &[(CoreId, String)],
    store: &CoreStore,
    out: &mut StrategiesOut,
) {
    // Нижняя панель действий ПОД левой колонкой: старт/стоп отмеченных (позже —
    // copy/delete/create). Добавляем первой, чтобы она прижалась к низу.
    egui::TopBottomPanel::bottom("strat-tree-actions").show_inside(ui, |ui| {
        ui.add_space(4.0);
        action_buttons(ui, st, cores, store, out);
        ui.add_space(4.0);
    });

    ui.add_space(6.0);
    // Поиск (половина ширины) + выпадающий фильтр по виду стратегии. Оба фильтра
    // действуют ОДНОВРЕМЕННО (И). Ширины фиксированные от available — без «расползания».
    let avail = ui.available_width();
    let search_w = (avail * 0.42).max(54.0);
    ui.horizontal(|ui| {
        let te = ui.add_sized(
            [search_w, 22.0],
            egui::TextEdit::singleline(&mut st.filter.search)
                .hint_text(t!("strat.search").to_string()),
        );
        // Круглый сброс «×» ВНУТРИ поля (линиями, без глифа-тофу).
        if !st.filter.search.is_empty() {
            let d = 14.0;
            let center = egui::pos2(te.rect.right() - d * 0.5 - 5.0, te.rect.center().y);
            let hit = egui::Rect::from_center_size(center, egui::vec2(d, d));
            let br = ui.interact(hit, ui.make_persistent_id("strat-search-clear"), egui::Sense::click());
            let p = ui.painter();
            p.circle_filled(center, d * 0.5, if br.hovered() { theme::LIFT_HOVER } else { theme::LIFT });
            let fg = if br.hovered() { theme::TEXT } else { theme::TEXT_3 };
            let s = 3.0;
            let stroke = egui::Stroke::new(1.3, fg);
            p.line_segment([center + egui::vec2(-s, -s), center + egui::vec2(s, s)], stroke);
            p.line_segment([center + egui::vec2(-s, s), center + egui::vec2(s, -s)], stroke);
            if br.clicked() {
                st.filter.search.clear();
            }
        }

        // Список видов стратегий (по присутствующим в дереве), фильтр по виду.
        let kinds = kinds_present(cores, store);
        let cur = st
            .filter
            .kind
            .and_then(|k| kinds.iter().find(|(o, _)| *o == k))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("strat.all_kinds").to_string());
        egui::ComboBox::from_id_salt("strat-kind-filter")
            .selected_text(cur)
            .width((avail * 0.34).max(64.0))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut st.filter.kind, None, t!("strat.all_kinds").to_string());
                for (ord, name) in &kinds {
                    ui.selectable_value(&mut st.filter.kind, Some(*ord), name);
                }
            });

        // Фильтр направления (все/LONG/SHORT) — узкий, ~1/5 ширины.
        let dir_txt = match st.filter.dir {
            None => t!("strat.all_dirs").to_string(),
            Some(true) => "SHORT".to_string(),
            Some(false) => "LONG".to_string(),
        };
        egui::ComboBox::from_id_salt("strat-dir-filter")
            .selected_text(dir_txt)
            .width((avail * 0.18).max(48.0))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut st.filter.dir, None, t!("strat.all_dirs").to_string());
                ui.selectable_value(&mut st.filter.dir, Some(false), "LONG");
                ui.selectable_value(&mut st.filter.dir, Some(true), "SHORT");
            });
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut st.filter.only_active, t!("strat.only_active").to_string());
        // Справа — маленькая квадратная кнопка «развернуть/свернуть всё» (▼/▲).
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let collapsed = st.expanded_cores.is_empty() && st.expanded_folders.is_empty();
            let h = ui.spacing().interact_size.y;
            let arrow = if collapsed { "▼" } else { "▲" };
            let tip = if collapsed {
                t!("strat.expand_all")
            } else {
                t!("strat.collapse_all")
            };
            if ui
                .add_sized([h, h], egui::Button::new(arrow))
                .on_hover_text(tip.to_string())
                .clicked()
            {
                expand_collapse_toggle(st, cores, store, collapsed);
            }
        });
    });
    ui.separator();

    // Поиск временно раскрывает всё; своё состояние раскрытия (expanded_*) при
    // этом не трогаем — после очистки дерево вернётся к прежней свёрнутости.
    let force_open = st.filter.searching();

    // Плоский порядок видимых стратегий прошлого кадра — для Shift-диапазона; новый
    // собираем по ходу отрисовки и сохраняем в конце.
    let order = st.flat_order.clone();
    let mut built: Vec<Key> = Vec::new();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (core_id, core_name) in cores {
                let Some(cd) = store.core(*core_id) else { continue };
                if cd.strategies.is_empty() || !cd.strategies.iter().any(|r| st.filter.matches(r)) {
                    continue;
                }

                // Узел ядра: активных/всего (с учётом фильтров типа и L/S), ▼/▶.
                let open = force_open || st.expanded_cores.contains(core_id);
                let total = cd.strategies.iter().filter(|r| st.filter.counts(r)).count();
                let active = cd
                    .strategies
                    .iter()
                    .filter(|r| st.filter.counts(r) && r.checked)
                    .count();
                let label = format!(
                    "{}  {}  {}/{}",
                    if open { "▼" } else { "▶" },
                    core_name,
                    active,
                    total
                );
                if ui
                    .selectable_label(
                        false,
                        egui::RichText::new(label).strong().color(theme::ACCENT),
                    )
                    .clicked()
                {
                    toggle(&mut st.expanded_cores, *core_id);
                }
                if !open {
                    continue;
                }

                ui.indent(("core_body", core_id), |ui| {
                    // Вложенное дерево папок: путь разбиваем по «/» и «\».
                    let root = build_node(cd.strategies.iter().filter(|r| st.filter.matches(r)));
                    let mut prefix: Vec<String> = Vec::new();
                    render_node(
                        ui,
                        st,
                        &root,
                        &cd.strategies,
                        *core_id,
                        &mut prefix,
                        force_open,
                        &order,
                        &mut built,
                    );
                });
            }
        });

    st.flat_order = built;
}

/// Переключает наличие ключа в множестве (раскрыт/свёрнут).
fn toggle<T: std::cmp::Eq + std::hash::Hash>(set: &mut std::collections::HashSet<T>, key: T) {
    if !set.remove(&key) {
        set.insert(key);
    }
}

/// Виды стратегий, присутствующие в дереве: (ordinal, имя), отсортировано по имени.
/// «Все типы» добавляется первым пунктом отдельно (в комбобоксе).
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

/// Кнопки действий под колонкой: старт/стоп отмеченных (позже — copy/delete/create).
fn action_buttons(
    ui: &mut egui::Ui,
    st: &mut StrategiesState,
    cores: &[(CoreId, String)],
    store: &CoreStore,
    out: &mut StrategiesOut,
) {
    let has_any = cores
        .iter()
        .any(|(c, _)| store.core(*c).is_some_and(|cd| !cd.strategies.is_empty()));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::BTN_GAP;
        let start = theme::seg_btn(ui, &t!("strat.start_checked"), false, None, false);
        if has_any && start.clicked() {
            out.actions = gather_actions(st, cores, store, true);
            st.staged.clear();
        }
        let stop = theme::seg_btn(ui, &t!("strat.stop_checked"), false, None, false);
        if has_any && stop.clicked() {
            out.actions = gather_actions(st, cores, store, false);
            st.staged.clear();
        }
        if !st.staged.is_empty() {
            ui.label(
                egui::RichText::new(t!("strat.staged", n = st.staged.len()).to_string())
                    .color(theme::ACCENT)
                    .small(),
            );
        }
    });
}

/// Развернуть все узлы (если `collapsed`) или свернуть все (иначе).
fn expand_collapse_toggle(
    st: &mut StrategiesState,
    cores: &[(CoreId, String)],
    store: &CoreStore,
    collapsed: bool,
) {
    if collapsed {
        for (c, _) in cores {
            st.expanded_cores.insert(*c);
            if let Some(cd) = store.core(*c) {
                for r in &cd.strategies {
                    // Раскрываем каждый уровень пути (накопительные префиксы).
                    let mut acc = String::new();
                    for part in r.folder_path.split(['/', '\\']).filter(|s| !s.is_empty()) {
                        if !acc.is_empty() {
                            acc.push('/');
                        }
                        acc.push_str(part);
                        st.expanded_folders.insert((*c, acc.clone()));
                    }
                }
            }
        }
    } else {
        st.expanded_cores.clear();
        st.expanded_folders.clear();
    }
}

/// Строка «имя (слева, усечение «…») … тип (справа)». Тип цветом по направлению.
/// Возвращает Response (click) для выбора. Без всплывающей подсказки.
fn name_type_row(
    ui: &mut egui::Ui,
    name: &str,
    highlighted: bool,
    type_label: &str,
    type_col: egui::Color32,
) -> egui::Response {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    let avail = ui.available_width();
    let h = ui.spacing().interact_size.y.max(body.size + 4.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(avail, h), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let type_galley = ui.fonts(|f| f.layout_no_wrap(type_label.to_string(), small, type_col));
        let type_w = type_galley.size().x;
        let name_max = (rect.width() - type_w - 10.0).max(8.0);
        let mut job = egui::text::LayoutJob::single_section(
            name.to_string(),
            egui::TextFormat {
                font_id: body,
                color: theme::TEXT,
                ..Default::default()
            },
        );
        job.wrap = egui::text::TextWrapping {
            max_width: name_max,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let name_galley = ui.fonts(|f| f.layout_job(job));
        let p = ui.painter();
        if highlighted {
            p.rect_filled(rect, 2.0, theme::ACCENT.gamma_multiply(0.20));
        } else if resp.hovered() {
            p.rect_filled(rect, 2.0, theme::LIFT_HOVER);
        }
        p.galley(
            egui::pos2(rect.left() + 2.0, rect.center().y - name_galley.size().y / 2.0),
            name_galley,
            theme::TEXT,
        );
        p.galley(
            egui::pos2(rect.right() - type_w - 2.0, rect.center().y - type_galley.size().y / 2.0),
            type_galley,
            type_col,
        );
    }
    resp
}

/// Клик по стратегии с учётом модификаторов: Shift — диапазон от якоря (по
/// `order`), Ctrl/Cmd — добавить/убрать по одной, без модификатора — выбрать одну.
fn apply_click(st: &mut StrategiesState, key: Key, order: &[Key], shift: bool, command: bool) {
    if shift {
        if let Some(a) = st.anchor {
            let ia = order.iter().position(|k| *k == a);
            let ib = order.iter().position(|k| *k == key);
            if let (Some(ia), Some(ib)) = (ia, ib) {
                let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
                st.sel = order[lo..=hi].iter().copied().collect();
            } else {
                st.sel = std::iter::once(key).collect();
            }
        } else {
            st.sel = std::iter::once(key).collect();
            st.anchor = Some(key);
        }
    } else if command {
        if !st.sel.remove(&key) {
            st.sel.insert(key);
        }
        st.anchor = Some(key);
    } else {
        st.sel.clear();
        st.sel.insert(key);
        st.anchor = Some(key);
    }
    // Первичная (источник схемы/секций) — всегда кликнутая. Раздел не сбрасываем.
    st.selected = Some(key);
}

/// Собирает действия по ядрам для «старт/стоп отмеченных»: на ядро — изменённые
/// галки (diff стейджинга против серверного checked) + команда старт/стоп, если у
/// ядра есть хоть одна отмеченная стратегия или есть правки галок.
fn gather_actions(
    st: &StrategiesState,
    cores: &[(CoreId, String)],
    store: &CoreStore,
    start: bool,
) -> Vec<StratAction> {
    let mut actions = Vec::new();
    for (core, _) in cores {
        let Some(cd) = store.core(*core) else { continue };
        let mut checks = Vec::new();
        let mut has_checked = false;
        for r in &cd.strategies {
            let eff = st.staged.get(&(*core, r.id)).copied().unwrap_or(r.checked);
            if eff != r.checked {
                checks.push((r.id, eff));
            }
            if eff {
                has_checked = true;
            }
        }
        if !checks.is_empty() || has_checked {
            actions.push(StratAction {
                core: *core,
                checks,
                start_stop: Some(start),
            });
        }
    }
    actions
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
fn folder_counts(strategies: &[StrategyRow], st: &StrategiesState, prefix: &[String]) -> (usize, usize) {
    let mut active = 0;
    let mut total = 0;
    for r in strategies {
        if !st.filter.counts(r) {
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

/// Рекурсивно рисует узел: подпапки (сворачиваемые, с активн./всего), затем
/// стратегии прямо в этой папке. Корневой узел даёт стратегии без папки + верхние папки.
#[allow(clippy::too_many_arguments)]
fn render_node(
    ui: &mut egui::Ui,
    st: &mut StrategiesState,
    node: &FolderNode,
    strategies: &[StrategyRow],
    core_id: CoreId,
    prefix: &mut Vec<String>,
    force_open: bool,
    order: &[Key],
    built: &mut Vec<Key>,
) {
    for (name, child) in &node.children {
        prefix.push(name.clone());
        let path_key = prefix.join("/");
        let fkey = (core_id, path_key.clone());
        let fopen = force_open || st.expanded_folders.contains(&fkey);
        let (active, total) = folder_counts(strategies, st, prefix);
        let flabel = format!("{}  {name}  {active}/{total}", if fopen { "▼" } else { "▶" });
        if ui.selectable_label(false, flabel).clicked() {
            toggle(&mut st.expanded_folders, fkey);
        }
        if fopen {
            ui.indent(("folder_body", core_id, &path_key), |ui| {
                render_node(ui, st, child, strategies, core_id, prefix, force_open, order, built);
            });
        }
        prefix.pop();
    }
    for r in &node.strategies {
        strategy_row(ui, st, core_id, r, order, built);
    }
}

/// Одна строка стратегии: чекбокс (стейджинг) · индикатор запуска · имя (выбор).
/// `order` — плоский порядок прошлого кадра (для Shift-диапазона); `built` —
/// накапливаемый порядок текущего кадра.
fn strategy_row(
    ui: &mut egui::Ui,
    st: &mut StrategiesState,
    core: CoreId,
    r: &crate::feed::StrategyRow,
    order: &[Key],
    built: &mut Vec<Key>,
) {
    ui.horizontal(|ui| {
        let key = (core, r.id);
        built.push(key);
        let server = r.checked;
        let mut val = st.staged.get(&key).copied().unwrap_or(server);
        if ui.checkbox(&mut val, "").changed() {
            // Возврат к серверному состоянию убирает запись из стейджинга.
            if val == server {
                st.staged.remove(&key);
            } else {
                st.staged.insert(key, val);
            }
        }
        // Индикатор серверной отметки (checked). Отдельного флага «работает» в
        // протоколе нет — показываем актуальную галку с сервера (не стейджинг).
        let dot = if server { theme::GREEN } else { theme::TEXT_3 };
        ui.colored_label(dot, "●");

        // Подсветка — для всех выбранных (мультивыбор), иначе для первичной.
        let highlighted = if st.sel.is_empty() {
            st.selected == Some(key)
        } else {
            st.sel.contains(&key)
        };
        // Имя слева (укорачивается «…»), тип стратегии прижат справа; SHORT — красным.
        let type_col = if r.is_short { theme::RED } else { theme::TEXT_3 };
        let resp = name_type_row(ui, &r.name, highlighted, &r.kind, type_col);
        if resp.clicked() {
            let m = ui.input(|i| i.modifiers);
            apply_click(st, key, order, m.shift, m.command);
        }
    });
}
