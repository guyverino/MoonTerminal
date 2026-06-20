//! Колонки/ячейки/заголовки таблицы «Отчёт»: построение колонок, форматирование
//! значений БД в текст+цвет, человекочитаемые заголовки и ширины.

use super::*;

pub(super) fn report_columns(vis: &[usize]) -> Vec<MoonDataTableColumn> {
    vis.iter()
        .map(|&i| {
            let col = db::DISPLAY_COLUMNS[i];
            let column =
                MoonDataTableColumn::new(col, header_for(col), width_for(col)).sortable(true);
            if is_numeric_report_column(col) {
                column.right()
            } else {
                column
            }
        })
        .collect()
}

pub(super) fn report_data_row(
    ri: usize,
    table: &ReportTable,
    vis: &[usize],
    p: MoonPalette,
) -> MoonDataRow {
    let mut cells = Vec::with_capacity(vis.len());
    if let Some(r) = table.rows.get(ri) {
        for &i in vis {
            let cname = table.cols[i];
            let val = r.get(i).unwrap_or(&Value::Null);
            cells.push(report_data_cell(cname, val, p));
        }
    }
    MoonDataRow::new(cells)
}

fn report_data_cell(col: &str, val: &Value, p: MoonPalette) -> MoonDataCell {
    let (text, color) = cell(col, val, p);
    let cell = MoonDataCell::text(text).font_size(10.0).line_height(13.0);
    if let Some(color) = color {
        cell.text_color(color)
    } else {
        cell
    }
}

fn is_numeric_report_column(col: &str) -> bool {
    matches!(
        col,
        "quantity"
            | "boughtq"
            | "buyprice"
            | "sellprice"
            | "spentbtc"
            | "gainedbtc"
            | "profitbtc"
            | "lev"
            | "db_id"
            | "taskid"
    )
}

/// Текст + цвет ячейки по имени колонки и значению (порт `cell`).
fn cell(col: &str, v: &Value, p: MoonPalette) -> (String, Option<u32>) {
    match col {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => {
            (as_i64(v).map(db::fmt_unix).unwrap_or_default(), None)
        }
        "isshort" => match as_i64(v) {
            Some(1) => ("Шорт".into(), Some(p.red)),
            Some(0) => ("Лонг".into(), Some(p.green)),
            _ => (String::new(), Some(p.text_soft)),
        },
        "emulator" => match as_i64(v) {
            Some(1) => ("эму".into(), Some(p.text_soft)),
            _ => (String::new(), None),
        },
        "profitbtc" | "gainedbtc" => {
            let n = as_f64(v);
            let color = match n {
                Some(x) if x > 0.0 => Some(p.green),
                Some(x) if x < 0.0 => Some(p.red),
                _ => None,
            };
            (n.map(|x| format!("{x:+.6}")).unwrap_or_default(), color)
        }
        _ => (value_to_string(v), None),
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(*i),
        Value::Real(r) => Some(*r as i64),
        _ => None,
    }
}
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Real(r) => Some(*r),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => moon_core::util::fmt::compact(*r, 8),
        Value::Text(t) => t.clone(),
        Value::Blob(_) => "<blob>".into(),
    }
}

/// Человекочитаемый заголовок колонки (порт `header_for`).
pub(super) fn header_for(col: &str) -> &str {
    match col {
        "buydate" => "Открыт (UTC)",
        "closedate" => "Закрыт (UTC)",
        "sellsetdate" => "Sell set",
        "last_update_at" => "Обновлён",
        "core_name" => "Ядро",
        "db_id" => "ID",
        "taskid" => "TaskID",
        "exorderid" => "ExOrderID",
        "coin" => "Монета",
        "isshort" => "Сторона",
        "quantity" => "Кол-во",
        "boughtq" => "Куплено",
        "buyprice" => "Покупка",
        "sellprice" => "Продажа",
        "spentbtc" => "Влож.BTC",
        "gainedbtc" => "Получ.BTC",
        "profitbtc" => "Профит BTC",
        "lev" => "Плечо",
        "strategyid" => "Strat",
        "channelname" => "Канал",
        "signaltype" => "Сигнал",
        "fname" => "Файл",
        "basecurrency" => "BaseCur",
        "emulator" => "Эму",
        "status" => "Статус",
        "sellreason" => "Причина",
        "comment" => "Коммент",
        other => other,
    }
}

fn width_for(col: &str) -> f32 {
    match col {
        "buydate" | "closedate" => 120.0,
        "sellsetdate" | "last_update_at" => 116.0,
        "comment" => 280.0,
        "sellreason" => 170.0,
        "channelname" | "signaltype" | "fname" | "exorderid" => 110.0,
        "core_name" | "coin" => 88.0,
        "profitbtc" | "gainedbtc" | "spentbtc" => 96.0,
        "lev" | "isshort" | "emulator" => 52.0,
        _ => 82.0,
    }
}
