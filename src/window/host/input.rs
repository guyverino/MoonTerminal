//! Ввод/интерактив окна группы: маршрутизация колеса/кнопок/движения курсора в
//! пан/зум панели под курсором. Гейт по зоне графика (геометрия `chart_area`),
//! а не по egui. Всё в физических пикселях. Состояние drag/курсора живёт в полях
//! [`super::WindowHost`] — здесь только его обработчики.

use winit::event::{MouseButton, MouseScrollDelta};
use winit::window::CursorIcon;

use crate::chart::container::ContainerKind;
use crate::chart::paint::now_unix_ms;

use super::WindowHost;

impl WindowHost {
    /// Указатель в зоне графика (а не над панелями egui)? Чисто геометрия:
    /// все интерактивные виджеты egui живут в панелях ВНЕ центрального rect.
    fn chart_input_ok(&self) -> bool {
        let (x, y) = self.last_ptr;
        let (cx, cy, cw, ch) = self.chart_area;
        x >= cx && x <= cx + cw && y >= cy && y <= cy + ch
    }

    /// Ширина rect панели под курсором (физ. px) — для клампа зума по X.
    fn hovered_pane_w(&self) -> f32 {
        self.pane_rects
            .iter()
            .find(|(i, _)| Some(*i) == self.hovered_pane)
            .map(|(_, r)| r.w)
            .unwrap_or(self.chart_area.2)
    }

    /// `view` панели под курсором (для пан/зум). None — курсор не над панелью.
    fn hovered_view_mut(&mut self) -> Option<&mut crate::chart::view::ChartView> {
        let idx = self.hovered_pane?;
        self.containers[self.active_container]
            .panes
            .get_mut(idx)
            .map(|p| &mut p.chart.view)
    }

    /// Двойной ЛКМ по чарту (не стакану) нумерованной вкладки → запомнить монету
    /// панели под курсором для открытия на Main фулскрин (render применит).
    fn try_dblclick_to_main(&mut self) {
        if !matches!(self.active().kind, ContainerKind::Chart { .. }) {
            return; // только во вкладках-номерах
        }
        let Some(idx) = self.hovered_pane else { return };
        let Some((_, r)) = self.pane_rects.iter().find(|(i, _)| *i == idx) else {
            return;
        };
        // В стакане (правая зона GLASS_ZONE_PX) дабл-клик игнорируем.
        let glass_w = crate::chart::GLASS_ZONE_PX.min(r.w * 0.5);
        if self.last_ptr.0 >= r.x + r.w - glass_w {
            return;
        }
        let info = self.containers[self.active_container]
            .panes
            .get(idx)
            .map(|p| (p.core, p.market.clone()));
        self.pending_to_main = info;
    }

    /// Колесо: зум по X (или пан по X при зажатом Shift) — у панели под курсором.
    pub fn wheel(&mut self, delta: &MouseScrollDelta) {
        if !self.chart_input_ok() {
            return;
        }
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => *y,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
        };
        if dy == 0.0 {
            return;
        }
        let shift = self.shift_down;
        let w = self.hovered_pane_w();
        let now = now_unix_ms();
        if let Some(view) = self.hovered_view_mut() {
            if shift {
                view.pan_x_px(-dy.signum() * 60.0, now);
            } else {
                let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
                view.zoom_x(factor, w);
            }
            self.dirty = true;
        }
    }

    /// Нажатие/отпускание кнопки мыши. Гейт по зоне графика (не по egui). ПКМ:
    /// короткий клик (без сдвига) = тоггл фулскрин↔тайл; ПКМ-drag = зум по цене.
    pub fn mouse_button(&mut self, button: MouseButton, pressed: bool) {
        match button {
            MouseButton::Left => {
                if pressed {
                    if !self.chart_input_ok() {
                        return; // клик по панели — не таскаем график
                    }
                    // Двойной ЛКМ по ЧАРТУ (не стакану) нумерованной вкладки →
                    // открыть монету на Main фулскрин (обработка в render).
                    let now = now_unix_ms();
                    let (px, py) = self.last_ptr;
                    let dbl = now - self.last_lmb_ms < 400.0
                        && (px - self.last_lmb_pos.0).abs() < 28.0
                        && (py - self.last_lmb_pos.1).abs() < 28.0;
                    self.last_lmb_ms = now;
                    self.last_lmb_pos = (px, py);
                    if dbl {
                        self.try_dblclick_to_main();
                    }
                    self.lmb_down = true;
                    self.lmb_active = false;
                    self.drag_accum = (0.0, 0.0);
                } else {
                    self.lmb_down = false;
                    self.lmb_active = false;
                }
            }
            MouseButton::Right => {
                if pressed {
                    if !self.chart_input_ok() {
                        return;
                    }
                    self.rmb_down = true;
                    self.rmb_moved = false;
                    self.rmb_start_y = self.last_ptr.1;
                    let snap = self.hovered_view_mut().map(|v| (v.price_range, v.center_price));
                    if let Some((r, c)) = snap {
                        self.rmb_start_range = r;
                        self.rmb_start_center = c;
                    }
                } else {
                    // Отпустили ПКМ без сдвига → клик: тоггл фулскрин/тайл (фокус —
                    // панель под курсором). Со сдвигом — это был зум по цене.
                    if self.rmb_down && !self.rmb_moved {
                        let focus = self.hovered_pane.unwrap_or(0);
                        if !self.active().is_empty() {
                            self.active_mut().toggle_mode(focus);
                        }
                    }
                    self.rmb_down = false;
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    /// Движение курсора (физ. пиксели).
    pub fn pointer_moved(&mut self, x: f32, y: f32) {
        let dx = x - self.last_ptr.0;
        let dy = y - self.last_ptr.1;
        self.last_ptr = (x, y);
        let (cx, cy, cw, ch) = self.chart_area;
        let active = x >= cx && x <= cx + cw && y >= cy && y <= cy + ch;
        let was_shown = self.cursor.is_some();
        self.cursor = if active { Some((x, y)) } else { None };
        // Панель под курсором (для маршрутизации пан/зум и крестика).
        self.hovered_pane = if active {
            self.pane_rects
                .iter()
                .find(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
                .map(|(i, _)| *i)
        } else {
            None
        };
        // Системный курсор-крестик над графиком; шлём только при смене состояния.
        if active != was_shown {
            self.window
                .set_cursor(if active { CursorIcon::Crosshair } else { CursorIcon::Default });
        }
        // Перекрестие живёт в зоне графика: движение над графиком или уход с него
        // → нужен кадр. Над панелями egui (не было показано и не показано) кадр
        // не форсируем — это и есть бывший «шторм» от движения мыши.
        if active || was_shown {
            self.dirty = true;
        }

        // Курсор над доком детектов → перегон egui (spotlight на кнопке следует за
        // мышью). Точечно: только эта узкая колонка, не все панели (без «шторма»).
        let (dx0, dy0, dw, dh) = self.detects_area;
        if x >= dx0 && x <= dx0 + dw && y >= dy0 && y <= dy0 + dh {
            self.egui_dirty = true;
            self.dirty = true;
        }

        // ЛКМ-перетаскивание: горизонталь → пан по времени, вертикаль → пан по цене.
        if self.lmb_down {
            if !self.lmb_active {
                self.drag_accum.0 += dx;
                self.drag_accum.1 += dy;
                if self.drag_accum.0.abs() > 5.0 || self.drag_accum.1.abs() > 5.0 {
                    self.lmb_active = true;
                }
            }
            if self.lmb_active {
                let now = now_unix_ms();
                if let Some(view) = self.hovered_view_mut() {
                    if dx != 0.0 {
                        view.pan_x_px(dx, now);
                    }
                    if dy != 0.0 {
                        view.pan_y_px(dy, now);
                    }
                }
            }
            self.dirty = true;
        }

        // ПКМ-перетаскивание: вертикальный зум по цене от снимка нажатия. Сдвиг за
        // порог помечает rmb_moved → на отпускании это зум, а не клик-тоггл.
        if self.rmb_down {
            let cum = y - self.rmb_start_y;
            if cum.abs() > 4.0 {
                self.rmb_moved = true;
            }
            let (c, r) = (self.rmb_start_center, self.rmb_start_range);
            let now = now_unix_ms();
            if let Some(view) = self.hovered_view_mut() {
                view.rmb_zoom(c, r, cum, now);
            }
            self.dirty = true;
        }
    }
}
