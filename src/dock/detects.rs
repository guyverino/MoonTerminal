//! Лента детектов: горизонтальный ряд кнопок над чартом. Новые дорисовываются
//! справа, сдвигая старые влево (клиппинг по левому краю тулбара). Кнопка живёт
//! `KeepAlert` секунд стратегии-источника и только если у неё `SoundAlert=Yes`.
//! Клик по кнопке открывает чарт монеты и убирает кнопку.

use std::collections::{HashMap, VecDeque};

use crate::session::{CoreId, CoreStore};
use crate::workspace::CoreInfo;

/// Жёсткий лимит кнопок в ленте (страховка от наплыва; TTL обычно режет раньше).
const MAX_BUTTONS: usize = 48;

struct RibbonItem {
    core: CoreId,
    /// Полный символ рынка (для подписки при клике и тултипа).
    market: String,
    /// Подпись кнопки — монета без quote подключения (`ADAUSDT` → `ADA`).
    base: String,
    color: [u8; 3],
    /// Время прихода (unix ms) и срок жизни (мс) — из KeepAlert стратегии.
    born_ms: f64,
    ttl_ms: f64,
}

/// Состояние ленты: очередь кнопок + per-core курсор уже учтённых детектов.
#[derive(Default)]
pub struct DetectRibbon {
    items: VecDeque<RibbonItem>,
    last_seq: HashMap<CoreId, u64>,
}

impl DetectRibbon {
    /// Втягивает новые детекты ядер группы (seq > курсора) с `SoundAlert=Yes`.
    pub fn ingest(&mut self, cores: &[CoreInfo], store: &CoreStore) {
        for ci in cores {
            let Some(d) = store.core(ci.id) else { continue };
            let last = self.last_seq.get(&ci.id).copied().unwrap_or(0);
            // Детекты добавлены по возрастанию seq → идём с конца, пока свежие.
            let mut fresh: Vec<&crate::feed::DetectRow> = Vec::new();
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                fresh.push(det);
            }
            if fresh.is_empty() {
                continue;
            }
            self.last_seq.insert(ci.id, fresh[0].seq); // самый новый seq
            for det in fresh.iter().rev() {
                if !det.sound_alert {
                    continue;
                }
                let ttl_ms = (det.keep_alert_secs.max(1) as f64) * 1000.0;
                // Ключ кнопки — пара (ядро, монета). Тот же детект с ТОГО ЖЕ ядра →
                // не плодим кнопку, лишь продлеваем жизнь (новый KeepAlert). Та же
                // монета с ДРУГОГО ядра → отдельная кнопка цветом своего ядра.
                if let Some(it) = self
                    .items
                    .iter_mut()
                    .find(|it| it.core == ci.id && it.market == det.market)
                {
                    it.born_ms = det.time_ms;
                    it.ttl_ms = ttl_ms;
                    it.color = ci.color;
                } else {
                    self.items.push_back(RibbonItem {
                        core: ci.id,
                        market: det.market.clone(),
                        base: crate::symbol::base_symbol(&det.market, &ci.quote).to_string(),
                        color: ci.color,
                        born_ms: det.time_ms,
                        ttl_ms,
                    });
                }
            }
        }
        while self.items.len() > MAX_BUTTONS {
            self.items.pop_front();
        }
    }

    /// Убирает просроченные кнопки (now − born ≥ ttl).
    pub fn prune(&mut self, now_ms: f64) {
        self.items.retain(|it| now_ms - it.born_ms < it.ttl_ms);
    }

    /// Есть ли видимые кнопки (нужно гнать кадры для их TTL-истечения).
    pub fn has_items(&self) -> bool {
        !self.items.is_empty()
    }

    /// Рисует детекты вертикальной колонкой в правом доке (новые сверху). Каждая
    /// кнопка в стиле стенда: токен сверху, остаток жизни мелким снизу (`4s`),
    /// глоу-полоса снизу цветом ядра-источника (из настроек). Клик → (ядро,
    /// рынок) для открытия чарта; кнопка удаляется. Возвращает первый клик.
    pub fn show(&mut self, ui: &mut egui::Ui, now_ms: f64) -> Option<(CoreId, String)> {
        let mut clicked: Option<usize> = None;
        ui.spacing_mut().item_spacing.y = 6.0;
        // Новые детекты — сверху: идём с конца очереди (свежие) к началу.
        for (idx, it) in self.items.iter().enumerate().rev() {
            let secs_left = ((it.ttl_ms - (now_ms - it.born_ms)) / 1000.0).ceil().max(0.0) as u32;
            let glow = egui::Color32::from_rgb(it.color[0], it.color[1], it.color[2]);
            if detect_button(ui, &it.base, secs_left, glow)
                .on_hover_text(&it.market)
                .clicked()
            {
                clicked = Some(idx);
            }
        }
        clicked.map(|idx| {
            let it = self.items.remove(idx).expect("idx in range");
            (it.core, it.market)
        })
    }
}

/// Кастомная кнопка детекта в стиле стенда, но «живее» CSS:
/// • вся кнопка — один скруглённый меш-градиент: верхняя половина = цвет кнопки
///   (lift), от центра к низу плавно переходит в смесь lift + цвет ядра-источника;
/// • радиальный spotlight под курсором (следует за мышью при наведении);
/// • токен крупно ярко сверху, остаток `Ns` мелко тускло снизу.
/// Возвращает `Response` (клик/ховер/тултип).
fn detect_button(ui: &mut egui::Ui, base: &str, secs: u32, glow: egui::Color32) -> egui::Response {
    use crate::shell::theme;
    use egui::{vec2, Align2, Color32, Pos2, Rounding, Sense, Stroke};

    let h = 40.0;
    let radius = 6.0;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }

    let hovered = resp.hovered();
    // Клип по кнопке — чтобы spotlight не вылезал за её границы.
    let p = ui.painter_at(rect);

    // Скруглённый градиент на всю кнопку: верх = lift, низ (от центра) = lift+цвет
    // ядра. «Сила» — доля подмешиваемого цвета (мягко в покое, ярче на ховере).
    let lift = if hovered { theme::LIFT_HOVER } else { theme::LIFT };
    let tint_k = if hovered { 0.80 } else { 0.55 };
    let bottom = theme::lerp_color(lift, glow, tint_k);
    theme::rounded_grad(&p, rect, radius, lift, bottom, 0.5);

    // Spotlight под курсором — мягкое акцентное свечение, следует за мышью.
    if hovered {
        if let Some(c) = resp.hover_pos() {
            spotlight(&p, c, 34.0, Color32::from_rgba_unmultiplied(0xff, 0xb3, 0x47, 40));
        }
        ui.ctx().request_repaint(); // пока курсор на кнопке — гоним кадры для следования
    }

    // Рамка поверх (на ховере — акцентная вместо hairline).
    let border = if hovered {
        theme::ACCENT.gamma_multiply(0.6)
    } else {
        theme::HAIRLINE
    };
    p.rect_stroke(rect, Rounding::same(radius), Stroke::new(1.0, border));

    // Токен (яркий) сверху-слева. Остаток (мелко) снизу-слева — на цветном глоу,
    // поэтому цвет инвертируем по яркости нижнего цвета (тёмный текст на светлом
    // глоу и наоборот), чтобы отсчёт не сливался с градиентом.
    p.text(
        Pos2::new(rect.min.x + 8.0, rect.min.y + 6.0),
        Align2::LEFT_TOP,
        base,
        theme::font(),
        theme::TEXT,
    );
    let lum =
        0.299 * bottom.r() as f32 + 0.587 * bottom.g() as f32 + 0.114 * bottom.b() as f32;
    let secs_color = if lum > 140.0 {
        Color32::from_rgb(0x14, 0x14, 0x16)
    } else {
        theme::TEXT
    };
    p.text(
        Pos2::new(rect.min.x + 8.0, rect.max.y - 6.0),
        Align2::LEFT_BOTTOM,
        format!("{secs}s"),
        theme::label_font(),
        secs_color,
    );

    resp
}

/// Радиальное свечение: центр — `color`, край (радиус `r`) — прозрачно. Веер
/// треугольников вокруг центра (мягкий круг). Клип задаёт вызывающий painter.
fn spotlight(p: &egui::Painter, center: egui::Pos2, r: f32, color: egui::Color32) {
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    const N: u32 = 28;
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex { pos: center, uv: WHITE_UV, color });
    for k in 0..=N {
        let a = k as f32 / N as f32 * std::f32::consts::TAU;
        mesh.vertices.push(Vertex {
            pos: egui::Pos2::new(center.x + r * a.cos(), center.y + r * a.sin()),
            uv: WHITE_UV,
            color: egui::Color32::TRANSPARENT,
        });
    }
    for k in 0..N {
        mesh.indices.extend_from_slice(&[0, 1 + k, 2 + k]);
    }
    p.add(egui::Shape::mesh(mesh));
}
