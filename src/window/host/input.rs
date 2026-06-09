//! Ввод окна группы: тонкие обёртки над общим [`crate::chart::input::ChartInput`].
//! Здесь только специфика host'а: гейт по геометрии `chart_area` (egui не годится:
//! CentralPanel поверх чарта считается «областью под курсором»), дабл-клик→Main
//! только в нумерованных вкладках и форс egui над доком детектов.

use winit::event::{MouseButton, MouseScrollDelta};
use winit::window::CursorIcon;

use crate::chart::container::ContainerKind;

use super::WindowHost;

impl WindowHost {
    /// Указатель в зоне графика (а не над панелями egui)? Чисто геометрия.
    fn chart_input_ok(&self) -> bool {
        let (x, y) = self.input.last_ptr;
        let (cx, cy, cw, ch) = self.chart_area;
        x >= cx && x <= cx + cw && y >= cy && y <= cy + ch
    }

    /// Колесо: зум по X (или пан по X при зажатом Shift) — у панели под курсором.
    pub fn wheel(&mut self, delta: &MouseScrollDelta) {
        let gate = self.chart_input_ok();
        let fallback_w = self.chart_area.2;
        let ac = self.active_container;
        if self.input.wheel(delta, gate, &mut self.containers[ac], fallback_w) {
            self.dirty = true;
        }
    }

    /// Нажатие/отпускание кнопки мыши. Гейт по зоне графика (не по egui).
    pub fn mouse_button(&mut self, button: MouseButton, pressed: bool) {
        let gate = self.chart_input_ok();
        // Дабл-клик→Main — только во вкладках-номерах (Main и так Main).
        let allow_dbl = matches!(self.active().kind, ContainerKind::Chart { .. });
        let ac = self.active_container;
        if self
            .input
            .mouse_button(button, pressed, gate, allow_dbl, &mut self.containers[ac])
        {
            self.dirty = true;
        }
    }

    /// Движение курсора (физ. пиксели).
    pub fn pointer_moved(&mut self, x: f32, y: f32) {
        let (cx, cy, cw, ch) = self.chart_area;
        let active = x >= cx && x <= cx + cw && y >= cy && y <= cy + ch;
        let was_shown = self.input.cursor.is_some();
        self.input.cursor = if active { Some((x, y)) } else { None };
        // Панель под курсором (для маршрутизации пан/зум и крестика).
        self.input.hovered_pane = if active { self.input.pane_at(x, y) } else { None };
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

        // ЛКМ/ПКМ-перетаскивания (пан/зум) — общая логика.
        let ac = self.active_container;
        if self.input.pointer_drag(x, y, &mut self.containers[ac]) {
            self.dirty = true;
        }
    }
}
