//! Общее для док-панелей. Сейчас — кнопка открепления (⧉): её код был дословно
//! повторён в Orders/Report/Log/Stub (отличалось лишь имя панели).

use gpui::*;
use moon_ui::{DockArea, MoonButton, MoonButtonSize};

use crate::Backend;
use crate::detached::DetachedSpec;

/// Кнопка тулбара «открепить в окно» (⧉): убирает панель из своего дока и открывает
/// её отдельным окном, записывая спеку в `backend.detached`. `name` — стабильное имя
/// панели (как у `panel_name`/`remove_panel_by_name`/`DetachedSpec`).
pub fn detach_button(
    name: &'static str,
    group: String,
    backend: Entity<Backend>,
    dock: Option<WeakEntity<DockArea>>,
) -> AnyElement {
    MoonButton::new(SharedString::from(format!("detach-{name}")))
        .ghost()
        .size(MoonButtonSize::Action)
        .label("⧉")
        .on_click(move |_, window, app| {
            // Убрать себя из дока.
            if let Some(dock) = dock.as_ref().and_then(|d| d.upgrade()) {
                dock.update(app, |area, cx| {
                    area.remove_panel_by_name(name, window, cx);
                });
            }
            // Открыть окно открепления + записать спеку.
            let spec = DetachedSpec::new(group.clone(), name.to_string());
            crate::detached::spawn(app, &backend, &spec, Some(window.window_handle()));
            backend.update(app, |b, _| {
                b.detached.push(spec);
                b.detached_dirty = true;
            });
        })
        .render()
        .into_any_element()
}
