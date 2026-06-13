//! Состояние вида графика — порт интерактива из moonweb (ChartInteraction +
//! CoordManager + YZoomController), с поведением «свободно листаем, через 3 с
//! возврат к лайву»:
//!   X (время): зум колесом, пан ЛКМ/Shift-колесо. Пан НЕ снимает Live — он
//!              лишь «удерживает» вид (manual_until), затем возврат к «сейчас».
//!   Y (цена):  авто-центрирование по цене с мёртвой зоной 10% масштаба
//!              (не дёргается на каждом тике) + масштаб Авто/фикс-процент.

use super::transform::ChartUniform;

/// Прямоугольник в пикселях (top-left origin).
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

// --- Константы поведения ---
/// Скорость подгона авто-масштаба к видимому диапазону (доля за кадр).
const AUTO_LERP: f32 = 0.15;
/// Мёртвая зона центровки: пока цена в пределах ±BUFFER*range от центра — не
/// двигаем (иначе дёргалось бы каждый кадр). Буфер = доля текущего масштаба.
const CENTER_BUFFER: f32 = 0.10;
/// Скорость возврата центра к цене, когда она вышла за буфер (доля за кадр).
const TICK_LERP: f32 = 0.10;
/// Пикселей вертикального drag ПКМ на удвоение/деление диапазона Y.
const YSCALE_PX_PER_2X: f32 = 150.0;
/// Сколько держать ручной вид после последнего действия, мс (затем — к лайву).
const MANUAL_HOLD_MS: f64 = 3000.0;
/// Гистерезис диапазона Y: render_range держим, пока «гладкая» цель не уйдёт за
/// ±15% — тогда снап к цели. Между снапами масштаб Y постоянен (нужно, чтобы
/// scrollable canvas Stage 2 оставался валиден кадрами между прыжками).
const RANGE_HYST: f32 = 1.15;
/// Порог сдвига центра Y в пикселях: пока цена не уехала дальше — центр стоит.
const CENTER_SNAP_PX: f32 = 8.0;
/// Минимальное видимое окно времени, мс. Зум по X не даёт окну схлопнуться
/// меньше — иначе секунда занимает весь экран и follow по правому краю гонит
/// график (тот самый «улёт»). Порог-«упор» из ТЗ: ~1 с.
const MIN_WINDOW_MS: f32 = 1_000.0;
/// Максимальное видимое окно времени, мс (зум по X не растягивает больше). 1 час.
const MAX_WINDOW_MS: f32 = 3_600_000.0;
/// Дефолтное видимое окно при первом открытии, мс. Под него подгоняется зум по
/// реальной ширине зоны графика (1 минута; сетка 30 вертикалей → ~2 с на риску).
const DEFAULT_WINDOW_MS: f32 = 60_000.0;

pub struct ChartView {
    /// Фиксированная точка отсчёта времени (unix ms), задаётся при старте.
    pub epoch_ms: f64,
    /// Зум по X: пикселей на миллисекунду.
    pub px_per_ms: f32,
    /// Время (unix ms) у правого «сейчас»-якоря области.
    pub right_time_ms: f64,
    /// Авто-следование за правым краем по времени (live).
    pub follow: bool,
    /// Доля ширины окна, оставляемая справа как «будущее» (аналог xRange*0.9).
    pub right_margin_frac: f32,

    /// Цена в центре области.
    pub center_price: f32,
    /// Видимый диапазон цены (единицы цены, сверху-вниз области).
    pub price_range: f32,
    /// Авто-подгон диапазона цены (кнопка «Авто»).
    pub auto_price: bool,
    /// Последний фикс-процент (range = center*percent), для дрейф-режима.
    pub scale_percent: f32,
    /// Производное: пикселей на единицу цены (кэш для пана/хит-теста).
    pub px_per_price: f32,

    /// Снапнутые (кусочно-постоянные) Y-параметры, которыми РЕАЛЬНО рисуем.
    /// Живой «гладкий» target — center_price/price_range; рендер берёт
    /// render_center/render_range и держит их стабильными между редкими
    /// прыжками → большинство кадров Y-маппинг не меняется (база для canvas).
    pub render_center: f32,
    pub render_range: f32,

    /// До этого момента (unix ms) вид удерживается вручную (нет авто-возврата).
    pub manual_until: f64,

    /// Полуразмер крестика, px.
    pub marker_half_px: f32,

    /// Ещё не подгоняли зум под дефолтное окно (делается раз, по реальной ширине
    /// зоны графика в первом кадре).
    x_init_pending: bool,
}

impl ChartView {
    pub fn new(epoch_ms: f64) -> Self {
        Self {
            epoch_ms,
            px_per_ms: 0.05, // ~20 секунд видимого окна на 1000 px
            right_time_ms: epoch_ms,
            follow: true,
            right_margin_frac: 0.10, // поле «будущего» справа как в moonweb (xRange*0.9)
            center_price: 0.0,
            price_range: 1.0,
            auto_price: true,
            scale_percent: 0.10,
            px_per_price: 0.5,
            render_center: 0.0,
            render_range: 1.0,
            manual_until: 0.0,
            marker_half_px: 3.5, // крест 7px (NormalX MoonBot)
            x_init_pending: true,
        }
    }

    /// Один раз подгоняет зум по X так, чтобы видимое окно было ровно
    /// DEFAULT_WINDOW_MS при реальной ширине зоны графика `area_w` (физ. px).
    /// Зовётся каждым кадром — срабатывает лишь на первом (когда ширина известна).
    pub fn ensure_default_window(&mut self, area_w: f32) {
        if self.x_init_pending && area_w >= 1.0 {
            self.px_per_ms = (area_w / DEFAULT_WINDOW_MS).clamp(0.0005, 5.0);
            self.x_init_pending = false;
        }
    }

    /// Лайв сейчас? (Live включён И не идёт ручное удержание после действия.)
    pub fn is_live(&self, now_ms: f64) -> bool {
        self.follow && now_ms >= self.manual_until
    }

    /// Якорит правый край к `edge_ms`, если идёт лайв. Smooth wall-clock режим
    /// (CHART_RENDERING_TZ): зовётся с now_ms → правый край = «сейчас», гладкий
    /// скролл по времени. Дешевизна кадра обеспечивается canvas UV-scroll +
    /// egui-mesh cache (Stage 2b/2c), а не пропуском кадров.
    pub fn follow_edge(&mut self, edge_ms: f64, now_ms: f64) {
        if self.is_live(now_ms) {
            self.right_time_ms = edge_ms;
        }
    }

    /// Отмечает ручное действие: удерживаем вид MANUAL_HOLD_MS, потом — к лайву.
    pub fn begin_manual(&mut self, now_ms: f64) {
        self.manual_until = now_ms + MANUAL_HOLD_MS;
    }

    /// Немедленный возврат к лайву (кнопка Live): к «сейчас», сброс удержания.
    pub fn resume_live(&mut self, now_ms: f64) {
        self.follow = true;
        self.manual_until = 0.0;
        self.right_time_ms = now_ms;
    }

    /// Сброс Y-вида для мгновенного переоткрытия на новой монете/цене: обнуляем
    /// центр/диапазон (живые и render), чтобы следующий update_y встал СРАЗУ на
    /// цену без плавного «добега» (lerp). Цена появляется через кадр-два после
    /// подписки — тогда вид мгновенно встаёт на неё, а не бежит от старой.
    pub fn reset_y(&mut self) {
        self.center_price = 0.0;
        self.price_range = 0.0;
        self.render_center = 0.0;
        self.render_range = 0.0;
    }

    /// Пиксель правого края по заданному времени (для триггера перерисовки).
    pub fn pixel_at(&self, edge_ms: f64) -> i64 {
        ((edge_ms - self.epoch_ms) * self.px_per_ms as f64).floor() as i64
    }

    /// Видимое окно по X: (время у левого края, ширина окна в мс).
    /// Единый источник X-геометрии для uniform и для куллинга видимых тиков.
    pub fn visible_x(&self, area_w: f32) -> (f32, f32) {
        let window_ms = area_w / self.px_per_ms.max(1e-6);
        let right_rel = (self.right_time_ms - self.epoch_ms) as f32 + window_ms * self.right_margin_frac;
        (right_rel - window_ms, window_ms)
    }

    // ── Масштаб Y (кнопки тулбара) ────────────────────────────────────────────

    /// Кнопка «Авто» — динамический подгон под видимый диапазон.
    pub fn set_auto(&mut self) {
        self.auto_price = true;
    }

    /// Фикс-процент: видимый диапазон = цена*percent (как ZoomBar moonweb).
    pub fn set_scale_percent(&mut self, percent: f32) {
        self.auto_price = false;
        self.scale_percent = percent;
        let base = if self.center_price.abs() > 1e-6 {
            self.center_price.abs()
        } else {
            self.price_range
        };
        self.price_range = (base * percent).max(1e-6);
    }

    // ── Пан / зум мышью ─────────────────────────────────────────────────────────
    // Пан НЕ снимает Live: ставит ручное удержание (begin_manual), через 3 с —
    // авто-возврат к «сейчас». Зум по X (колесо) — постоянный, без удержания.

    /// Пан по X на dx пикселей (drag ЛКМ / Shift-колесо).
    pub fn pan_x_px(&mut self, dx: f32, now_ms: f64) {
        let dt_ms = dx as f64 / self.px_per_ms.max(1e-6) as f64;
        self.right_time_ms -= dt_ms; // тянем вправо → смотрим в прошлое
        self.begin_manual(now_ms);
    }

    /// Пан по Y на dy пикселей (drag ЛКМ).
    pub fn pan_y_px(&mut self, dy: f32, now_ms: f64) {
        self.center_price += dy / self.px_per_price.max(1e-6);
        self.begin_manual(now_ms);
    }

    /// Зум по X вокруг правого края (колесо). Ограничиваем не px_per_ms напрямую,
    /// а ВИДИМОЕ окно времени: [MIN_WINDOW_MS, MAX_WINDOW_MS]. `area_w` — ширина
    /// зоны графика в физ. пикселях (та же шкала, что px_per_ms). Если ширина ещё
    /// неизвестна (нулевая, до первого кадра) — мягкий абсолютный фолбэк.
    pub fn zoom_x(&mut self, factor: f32, area_w: f32) {
        let next = self.px_per_ms * factor;
        let (lo, hi) = if area_w >= 1.0 {
            // window_ms = area_w / px_per_ms → больше px_per_ms = у́же окно.
            (area_w / MAX_WINDOW_MS, area_w / MIN_WINDOW_MS)
        } else {
            (0.0005, 5.0)
        };
        self.px_per_ms = next.clamp(lo, hi);
    }

    /// Зум по Y (drag ПКМ) от снимка на момент нажатия. up=zoom out, down=zoom in.
    pub fn rmb_zoom(&mut self, start_center: f32, start_range: f32, cum_dy: f32, now_ms: f64) {
        let factor = 2f32.powf(-cum_dy / YSCALE_PX_PER_2X);
        let r = (start_range * factor).clamp(start_range * 0.25, start_range * 4.0);
        self.center_price = start_center;
        self.price_range = r.max(1e-6);
        self.begin_manual(now_ms);
    }

    // ── Обновление шкалы цены раз в кадр ─────────────────────────────────────────

    /// Подгоняет центр/диапазон цены. Работает только в лайве (вне ручного
    /// удержания и при включённом Live) — иначе вид заморожен. Центрирование
    /// по цене с мёртвой зоной CENTER_BUFFER; масштаб Авто (фит видимого,
    /// симметрично цене) или фикс-процент (range = цена*percent).
    pub fn update_y(
        &mut self,
        now_ms: f64,
        area_h: f32,
        visible: Option<(f32, f32)>,
        last_price: Option<f32>,
    ) {
        if self.is_live(now_ms) {
            if let Some(p) = last_price {
                // 1) Масштаб (range).
                if self.auto_price {
                    if let Some((lo, hi)) = visible {
                        // Симметрично цене, чтобы при центровке по цене ни верх,
                        // ни низ видимых данных не обрезались. +10% запас.
                        let half = (p - lo).max(hi - p).max(p.abs() * 0.0005 + 1e-6);
                        let trange = half * 2.0 * 1.10;
                        if self.center_price == 0.0 || self.price_range <= 0.0 {
                            self.price_range = trange;
                        } else {
                            self.price_range += (trange - self.price_range) * AUTO_LERP;
                        }
                    }
                } else {
                    self.price_range = (p.abs() * self.scale_percent).max(1e-6);
                }

                // 2) Центровка по цене с мёртвой зоной 10% масштаба.
                if self.center_price == 0.0 {
                    self.center_price = p;
                } else if self.price_range > 1e-9 {
                    let drift = (p - self.center_price).abs() / self.price_range;
                    if drift > CENTER_BUFFER {
                        self.center_price += (p - self.center_price) * TICK_LERP;
                    }
                }
            }
        }
        if !(self.price_range > 1e-9) {
            self.price_range = self.center_price.abs() * 0.10 + 1.0;
        }
        // Снап Y. В лайве render_* кусочно-постоянны: range держим, пока цель не
        // ушла за ±RANGE_HYST; центр — пока цена не уехала > CENTER_SNAP_PX px.
        // В ручном режиме следуем точно за вводом (drag отзывчив; canvas
        // пере-бейкается — это transient на время взаимодействия).
        if self.is_live(now_ms) {
            let target = self.price_range.max(1e-9);
            if !(self.render_range > 1e-9)
                || target > self.render_range * RANGE_HYST
                || target < self.render_range / RANGE_HYST
            {
                self.render_range = target;
            }
            let ppp = (area_h / self.render_range.max(1e-9)).max(1e-6);
            if (self.center_price - self.render_center).abs() * ppp > CENTER_SNAP_PX {
                self.render_center = self.center_price;
            }
        } else {
            self.render_range = self.price_range.max(1e-9);
            self.render_center = self.center_price;
        }
        self.px_per_price = (area_h / self.render_range.max(1e-9)).max(1e-6);
    }

    /// Uniform для запекания крестиков в канвас (Stage 2c): фиксированный левый
    /// край времени `bake_time0` (rel ms) и viewport = весь канвас [0,0,W,H]. Y
    /// берём из render-параметров — те же, что на экране, поэтому при неизменном Y
    /// запечённая картинка совпадает с экранной и нужен лишь UV-сдвиг по X.
    pub fn bake_uniform(&self, bake_time0: f32, canvas_w: f32, area_h: f32) -> ChartUniform {
        let view_price0 = self.render_center - (area_h * 0.5) / self.px_per_price.max(1e-6);
        ChartUniform {
            viewport: [0.0, 0.0, canvas_w, area_h],
            resolution: [canvas_w, area_h],
            time_to_px: self.px_per_ms,
            price_to_px: self.px_per_price,
            view_time0: bake_time0,
            view_price0,
            marker_half_px: self.marker_half_px,
            _pad: 0.0,
        }
    }

    /// Собирает GPU-uniform для текущего вида и области.
    pub fn uniform(&self, area: Rect, resolution: [f32; 2]) -> ChartUniform {
        let (view_time0, _window_ms) = self.visible_x(area.w);
        let view_price0 = self.render_center - (area.h * 0.5) / self.px_per_price.max(1e-6);

        ChartUniform {
            viewport: [area.x, area.y, area.w, area.h],
            resolution,
            time_to_px: self.px_per_ms,
            price_to_px: self.px_per_price,
            view_time0,
            view_price0,
            marker_half_px: self.marker_half_px,
            _pad: 0.0,
        }
    }
}
