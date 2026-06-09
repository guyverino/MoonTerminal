//! Панель 1: дерево ядро→папка→стратегия с поиском, счётчиками запущ./всего,
//! индикатором запуска, чекбоксами (стейджинг) и кнопкой «Применить» (старт/стоп).

use super::{folders_of, matches, searching, Key, StratAction, StrategiesOut, StrategiesState};
use crate::session::{CoreId, CoreStore};
use crate::shell::theme;

pub fn show(
    ui: &mut egui::Ui,
    st: &mut StrategiesState,
    cores: &[(CoreId, String)],
    store: &CoreStore,
    out: &mut StrategiesOut,
) {
    ui.add_space(6.0);
    // Поиск во всю ширину (без вычислений от available_width — они давали
    // «расползание» панели на перерисовках). Сброс — круглая кнопка с крестиком
    // ВНУТРИ поля справа, нарисованная линиями (не глифом, иначе шрифт даёт тофу).
    let te = ui.add(
        egui::TextEdit::singleline(&mut st.search)
            .desired_width(f32::INFINITY)
            .hint_text(t!("strat.search").to_string()),
    );
    if !st.search.is_empty() {
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
            st.search.clear();
        }
    }

    // Старт/стоп отмеченных: галка — это ОТМЕТКА (не «работает»). Кнопка сначала
    // синхронизирует изменённые галки, затем шлёт ядрам «старт»/«стоп» отмеченных.
    let has_any = cores
        .iter()
        .any(|(c, _)| store.core(*c).is_some_and(|cd| !cd.strategies.is_empty()));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::BTN_GAP;
        // Кнопки темы проекта (seg_btn). Отключённые (нет ядер) — приглушаем.
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
    ui.separator();

    // Поиск временно раскрывает всё; своё состояние раскрытия (expanded_*) при
    // этом не трогаем — после очистки дерево вернётся к прежней свёрнутости.
    let force_open = searching(st);

    // Плоский порядок видимых стратегий прошлого кадра — для Shift-диапазона; новый
    // собираем по ходу отрисовки и сохраняем в конце.
    let order = st.flat_order.clone();
    let mut built: Vec<Key> = Vec::new();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (core_id, core_name) in cores {
                let Some(cd) = store.core(*core_id) else { continue };
                if cd.strategies.is_empty() || !cd.strategies.iter().any(|r| matches(st, r)) {
                    continue;
                }

                // Узел ядра: счётчик стратегий, ▼/▶, своё состояние раскрытия.
                let open = force_open || st.expanded_cores.contains(core_id);
                let label = format!(
                    "{}  {}  ·  {}",
                    if open { "▼" } else { "▶" },
                    core_name,
                    cd.strategies.len()
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
                    for folder in folders_of(&cd.strategies) {
                        let rows: Vec<_> = cd
                            .strategies
                            .iter()
                            .filter(|r| r.folder_path == folder && matches(st, r))
                            .collect();
                        if rows.is_empty() {
                            continue;
                        }
                        let total = rows.len();
                        let running = rows.iter().filter(|r| r.checked).count();
                        let fkey = (*core_id, folder.clone());
                        let fopen = force_open || st.expanded_folders.contains(&fkey);
                        let name = if folder.is_empty() {
                            t!("strat.root").to_string()
                        } else {
                            folder.clone()
                        };
                        let flabel =
                            format!("{}  {name}  {running}/{total}", if fopen { "▼" } else { "▶" });
                        if ui.selectable_label(false, flabel).clicked() {
                            toggle(&mut st.expanded_folders, fkey.clone());
                        }
                        if fopen {
                            ui.indent(("folder_body", core_id, &folder), |ui| {
                                for r in &rows {
                                    strategy_row(ui, st, *core_id, r, &order, &mut built);
                                }
                            });
                        }
                    }
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
        let resp = ui.selectable_label(highlighted, &r.name);
        if resp.clicked() {
            let m = ui.input(|i| i.modifiers);
            apply_click(st, key, order, m.shift, m.command);
        }
        resp.on_hover_text(format!("{} · {}", r.kind, if r.is_short { "SHORT" } else { "LONG" }));
    });
}
