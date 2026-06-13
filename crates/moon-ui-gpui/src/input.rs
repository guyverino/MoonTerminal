//! Ввод чарт-области в GPUI — порт `src/chart/input.rs` egui-версии 1:1, но
//! winit-free: вместо winit-типов берёт plain `dy: f32` (колесо) и локальный
//! [`Btn`]. Всё в ДЕВАЙС-пикселях (та же шкала, в которой движок рендерит offscreen
//! и в которой живёт `ChartView`). Конвертацию из лог. px окна делает вызывающий
//! (Shell): `(pos − slot_origin) × scale_factor`.

use moon_chart::container::Container;
use moon_chart::paint::now_unix_ms;
use moon_chart::view::{ChartView, Rect};
use moon_chart::GLASS_ZONE_PX;
use moon_core::session::CoreId;

/// Кнопка мыши (вместо winit::MouseButton).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Left,
    Right,
}

#[derive(Default)]
pub struct ChartInput {
    /// Позиция перекрестия (девайс-px); None — вне зоны графика.
    pub cursor: Option<(f32, f32)>,
    /// Последняя позиция указателя (девайс-px).
    pub last_ptr: (f32, f32),
    /// Панель под курсором (индекс в контейнере).
    pub hovered_pane: Option<usize>,
    /// Раскладка панелей (девайс-px) прошлого кадра — hit-тест ввода.
    pub pane_rects: Vec<(usize, Rect)>,
    /// Двойной клик по чарту → отправить монету на Main (владелец забирает).
    pub pending_to_main: Option<(CoreId, String)>,

    lmb_down: bool,
    lmb_active: bool,
    drag_accum: (f32, f32),
    rmb_down: bool,
    /// ПКМ сдвинулся за порог → зум-перетаскивание, а не клик-тоггл фулскрина.
    rmb_moved: bool,
    rmb_start_y: f32,
    rmb_start_range: f32,
    rmb_start_center: f32,
    /// Время/позиция прошлого ЛКМ-нажатия — для детекта двойного клика.
    last_lmb_ms: f64,
    last_lmb_pos: (f32, f32),
}

impl ChartInput {
    /// Ширина rect панели под курсором (девайс-px) — для клампа зума по X.
    fn hovered_pane_w(&self, fallback: f32) -> f32 {
        self.pane_rects
            .iter()
            .find(|(i, _)| Some(*i) == self.hovered_pane)
            .map(|(_, r)| r.w)
            .unwrap_or(fallback)
    }

    /// `view` панели под курсором (для пан/зум). None — курсор не над панелью.
    pub fn hovered_view_mut<'c>(&self, container: &'c mut Container) -> Option<&'c mut ChartView> {
        let idx = self.hovered_pane?;
        container.panes.get_mut(idx).map(|p| &mut p.chart.view)
    }

    /// Двойной ЛКМ по чарту (не стакану) панели под курсором → запомнить монету.
    fn try_dblclick_to_main(&mut self, container: &Container) {
        let Some(idx) = self.hovered_pane else { return };
        let Some((_, r)) = self.pane_rects.iter().find(|(i, _)| *i == idx) else {
            return;
        };
        // В стакане (правая зона GLASS_ZONE_PX) дабл-клик игнорируем.
        let glass_w = GLASS_ZONE_PX.min(r.w * 0.5);
        if self.last_ptr.0 >= r.x + r.w - glass_w {
            return;
        }
        self.pending_to_main = container.panes.get(idx).map(|p| (p.core, p.market.clone()));
    }

    /// Колесо: зум по X (или пан по X при Shift) — у панели под курсором.
    /// `dy` — знак/величина прокрутки (lines), `gate_ok` — указатель в зоне графика.
    /// Возвращает «нужен кадр».
    pub fn wheel(
        &mut self,
        dy: f32,
        shift: bool,
        gate_ok: bool,
        container: &mut Container,
        fallback_w: f32,
    ) -> bool {
        if !gate_ok || dy == 0.0 {
            return false;
        }
        let w = self.hovered_pane_w(fallback_w);
        let now = now_unix_ms();
        if let Some(view) = self.hovered_view_mut(container) {
            if shift {
                view.pan_x_px(-dy.signum() * 60.0, now);
            } else {
                let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
                view.zoom_x(factor, w);
            }
            return true;
        }
        false
    }

    /// Нажатие/отпускание кнопки. `gate_ok` — указатель в зоне графика (гейтит
    /// только нажатия). ПКМ: короткий клик = тоггл фулскрин↔тайл; ПКМ-drag = зум
    /// цены. `allow_dbl_to_main` — разрешён ли дабл-клик→Main. Возврат: «нужен кадр».
    pub fn mouse_button(
        &mut self,
        button: Btn,
        pressed: bool,
        gate_ok: bool,
        allow_dbl_to_main: bool,
        container: &mut Container,
    ) -> bool {
        match button {
            Btn::Left => {
                if pressed {
                    if !gate_ok {
                        return false; // клик по UI-панелям — не таскаем график
                    }
                    let now = now_unix_ms();
                    let (px, py) = self.last_ptr;
                    let dbl = now - self.last_lmb_ms < 400.0
                        && (px - self.last_lmb_pos.0).abs() < 28.0
                        && (py - self.last_lmb_pos.1).abs() < 28.0;
                    self.last_lmb_ms = now;
                    self.last_lmb_pos = (px, py);
                    if dbl && allow_dbl_to_main {
                        self.try_dblclick_to_main(container);
                    }
                    self.lmb_down = true;
                    self.lmb_active = false;
                    self.drag_accum = (0.0, 0.0);
                } else {
                    self.lmb_down = false;
                    self.lmb_active = false;
                }
            }
            Btn::Right => {
                if pressed {
                    if !gate_ok {
                        return false;
                    }
                    self.rmb_down = true;
                    self.rmb_moved = false;
                    self.rmb_start_y = self.last_ptr.1;
                    let snap = self
                        .hovered_view_mut(container)
                        .map(|v| (v.price_range, v.center_price));
                    if let Some((r, c)) = snap {
                        self.rmb_start_range = r;
                        self.rmb_start_center = c;
                    }
                } else {
                    // Отпустили ПКМ без сдвига → клик: тоггл фулскрин/тайл.
                    if self.rmb_down && !self.rmb_moved && !container.is_empty() {
                        let focus = self.hovered_pane.unwrap_or(0);
                        container.toggle_mode(focus);
                    }
                    self.rmb_down = false;
                }
            }
        }
        true
    }

    /// Drag-часть движения: ЛКМ (пан X/Y) и ПКМ (вертикальный зум цены от снимка).
    /// Сам обновляет `last_ptr`. Возвращает «нужен кадр» (идёт перетаскивание).
    pub fn pointer_drag(&mut self, x: f32, y: f32, container: &mut Container) -> bool {
        let dx = x - self.last_ptr.0;
        let dy = y - self.last_ptr.1;
        self.last_ptr = (x, y);

        // ЛКМ: горизонталь → пан по времени, вертикаль → пан по цене.
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
                if let Some(view) = self.hovered_view_mut(container) {
                    if dx != 0.0 {
                        view.pan_x_px(dx, now);
                    }
                    if dy != 0.0 {
                        view.pan_y_px(dy, now);
                    }
                }
            }
        }

        // ПКМ: вертикальный зум по цене от снимка нажатия.
        if self.rmb_down {
            let cum = y - self.rmb_start_y;
            if cum.abs() > 4.0 {
                self.rmb_moved = true;
            }
            let (c, r) = (self.rmb_start_center, self.rmb_start_range);
            let now = now_unix_ms();
            if let Some(view) = self.hovered_view_mut(container) {
                view.rmb_zoom(c, r, cum, now);
            }
        }

        self.lmb_down || self.rmb_down
    }

    /// Синхронизировать зажатость кнопок из факта move-события. GPUI шлёт mouse_up
    /// только над элементом → отпускание вне слота теряется и drag «залипает».
    /// Move сообщает реально зажатую кнопку — сбрасываем залипшее состояние.
    pub fn sync_pressed(&mut self, left_held: bool, right_held: bool) {
        if !left_held {
            self.lmb_down = false;
            self.lmb_active = false;
        }
        if !right_held {
            self.rmb_down = false;
        }
    }

    /// Hit-тест панели под точкой по `pane_rects`.
    pub fn pane_at(&self, x: f32, y: f32) -> Option<usize> {
        self.pane_rects
            .iter()
            .find(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
            .map(|(i, _)| *i)
    }
}
