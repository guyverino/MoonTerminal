//! Форматирование чисел для UI/feed.

/// Компактное число: точность `decimals`, хвостовые нули и точка срезаются
/// ("1.500000" → "1.5", "2.000000" → "2").
pub fn compact(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}
