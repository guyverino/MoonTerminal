//! Хэш-сигнатуры хрома окна группы — дёшево ловят, изменилось ли видимое
//! содержимое (рынок/статус/цена/ордера/подключения), чтобы решить, гнать ли
//! egui-тесселяцию заново или переиспользовать кэш. Живые счётчики статус-бара
//! (fps/present/CPU/RAM) сюда НЕ входят — их освежает EGUI_THROTTLE.

use std::hash::{Hash, Hasher};

use crate::feed::ConnStatus;

/// Сигнатура содержимого хрома: меняется только при смене рынка/статуса/цены
/// (до копеек) / набора ордеров.
pub(super) fn chrome_sig(
    market: &str,
    status: &ConnStatus,
    last_price: Option<f32>,
    orders_sig: u64,
    conn_sig: u64,
) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    market.hash(&mut h);
    let (code, txt): (u8, &str) = match status {
        ConnStatus::Connecting => (0, ""),
        ConnStatus::Stage(s) => (1, s.as_str()),
        ConnStatus::Ready => (2, ""),
        ConnStatus::Failed(e) => (3, e.as_str()),
        ConnStatus::Disconnected => (4, ""),
    };
    code.hash(&mut h);
    txt.hash(&mut h);
    last_price
        .map(|p| (p * 100.0).round() as i64)
        .unwrap_or(i64::MIN)
        .hash(&mut h);
    orders_sig.hash(&mut h);
    conn_sig.hash(&mut h);
    h.finish()
}

/// Хэш сводки подключений (ready/total + список упавших) — чтобы статус-бар
/// перерисовывался при смене статуса ЛЮБОГО ядра, а не только активного.
pub(super) fn conn_summary_sig(summary: &crate::session::ConnSummary) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    summary.ready.hash(&mut h);
    summary.total.hash(&mut h);
    for (name, st) in &summary.down {
        name.hash(&mut h);
        match st {
            ConnStatus::Connecting => 0u8.hash(&mut h),
            ConnStatus::Stage(s) => {
                1u8.hash(&mut h);
                s.hash(&mut h);
            }
            ConnStatus::Ready => 2u8.hash(&mut h),
            ConnStatus::Failed(e) => {
                3u8.hash(&mut h);
                e.hash(&mut h);
            }
            ConnStatus::Disconnected => 4u8.hash(&mut h),
        }
    }
    h.finish()
}
