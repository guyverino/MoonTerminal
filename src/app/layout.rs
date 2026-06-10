//! Раскладка окон App (layout.toml): снятие геометрии/состояния живых окон
//! групп и окон открепления, дебаунс-сохранение делает about_to_wait.

use std::time::Instant;

use winit::window::WindowId;

use super::App;

impl App {
    /// Снять геометрию+состояние окна группы `id` в раскладку (по имени группы).
    pub(super) fn update_group_layout(&mut self, id: WindowId) {
        if let Some(h) = self.windows.get(&id) {
            if let Ok(pos) = h.window.outer_position() {
                let size = h.window.inner_size();
                let (primary, newest, only_current, kind) = h.workspace.dock.orders_layout();
                self.layout.groups.insert(
                    h.workspace.group.clone(),
                    crate::config::GroupLayout {
                        x: pos.x,
                        y: pos.y,
                        w: size.width,
                        h: size.height,
                        maximized: h.window.is_maximized(),
                        collapsed: h.workspace.dock.collapsed(),
                        tab: h.workspace.dock.tab.idx() as u8,
                        dock_h: h.workspace.dock.dock_height(),
                        orders_primary: primary,
                        orders_newest_first: newest,
                        orders_only_current: only_current,
                        orders_kind: kind,
                    },
                );
            }
        }
    }

    /// Пересобрать список откреплённых окон в раскладке из живых окон открепления.
    /// Заодно обновляет карту запомненной геометрии (по ключу) — чтобы повторное
    /// открепление вставало на то же место даже после закрытия.
    pub(super) fn update_detached_layout(&mut self) {
        let mut list = Vec::new();
        let mut geoms: Vec<(String, crate::config::GeomRect)> = Vec::new();
        for p in self.detached.values() {
            let Ok(pos) = p.window.outer_position() else {
                continue;
            };
            let Some(group) = self.windows.get(&p.owner).map(|h| h.workspace.group.clone()) else {
                continue; // владелец закрыт — не сохраняем сироту
            };
            let size = p.window.inner_size();
            let rect = crate::config::GeomRect { x: pos.x, y: pos.y, w: size.width, h: size.height };
            list.push(crate::config::DetachedLayout {
                tab: p.tab.idx() as u8,
                owner_group: group,
                x: pos.x,
                y: pos.y,
                w: size.width,
                h: size.height,
            });
            geoms.push((self.detached_geom_key(p.global, p.tab, p.owner), rect));
        }
        self.layout.detached = list;
        for (k, r) in geoms {
            self.layout.detached_geom.insert(k, r);
        }
    }

    /// Снять раскладку со ВСЕХ живых окон и записать layout.toml.
    pub(super) fn save_layout(&mut self) {
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for id in ids {
            self.update_group_layout(id);
        }
        self.update_detached_layout();
        self.layout.save();
        self.layout_dirty = false;
        self.last_layout_save = Instant::now();
    }
}
