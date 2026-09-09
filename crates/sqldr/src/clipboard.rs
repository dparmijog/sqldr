//! Copy-to-clipboard via the OSC 52 terminal escape sequence, plus the
//! cell/row text formats offered from the results pane (plain text, JSON,
//! CSV, `INSERT`).
//!
//! OSC 52 writes to the terminal's clipboard through the escape sequence
//! itself rather than a platform clipboard API, so it works identically
//! over SSH — matching this project's personal-use-from-the-terminal
//! focus. Most terminal emulators support it; some
//! require an explicit opt-in setting for security reasons.

use std::io::Write;

use base64::Engine;
use sqldr_core::{Row, Value};

/// Writes `text` to the system clipboard via OSC 52. Best-effort: a
/// terminal that ignores/ blocks OSC 52 will simply not update the
/// clipboard, which is why callers still show a status message rather than
/// relying solely on this succeeding silently.
pub fn copy(text: &str) -> std::io::Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut stdout = std::io::stdout();
    write!(stdout, "\x1b]52;c;{encoded}\x07")?;
    stdout.flush()
}

pub fn cell_text(value: &Value) -> String {
    value.to_string()
}

pub fn row_json(row: &Row) -> String {
    let mut map = serde_json::Map::with_capacity(row.cols.len());
    for (col, value) in row.cols.iter().zip(&row.values) {
        map.insert(col.clone(), value_to_json(value));
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap_or_default()
}

fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(s) => serde_json::Value::String(s.clone()),
        Value::Bytes(b) => serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(b)),
        Value::Other(s) => serde_json::Value::String(s.clone()),
    }
}

pub fn row_csv(row: &Row) -> String {
    row.values
        .iter()
        .map(|v| csv_field(&v.to_string()))
        .collect::<Vec<_>>()
        .join(",")
}

fn csv_field(field: &str) -> String {
    if field.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Renders `row` as a single-row `INSERT INTO table (...) VALUES (...);`.
/// Only meaningful when the row genuinely came from one table (a sidebar
/// preview); the caller is responsible for gating this on that.
pub fn row_insert(row: &Row, table: &str) -> String {
    let cols = row.cols.join(", ");
    let vals = row
        .values
        .iter()
        .map(sql_literal)
        .collect::<Vec<_>>()
        .join(", ");
    format!("INSERT INTO {table} ({cols}) VALUES ({vals});")
}

/// Renders `value` as a SQL literal — reused by `row_insert` and by
/// foreign-key navigation, which builds a `WHERE ref_col = <value>` from
/// a cell the cursor is sitting on.
pub fn sql_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => if *b { "TRUE".to_string() } else { "FALSE".to_string() },
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Bytes(b) => format!("0x{}", b.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
        Value::Other(s) => format!("'{}'", s.replace('\'', "''")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row() -> Row {
        Row {
            cols: vec!["id".into(), "name".into(), "note".into()],
            values: vec![Value::Int(1), Value::Text("bolt".into()), Value::Null],
        }
    }

    #[test]
    fn csv_quotes_fields_with_special_characters() {
        let row = Row {
            cols: vec!["name".into()],
            values: vec![Value::Text("a, \"quoted\" value".into())],
        };
        assert_eq!(row_csv(&row), "\"a, \"\"quoted\"\" value\"");
    }

    #[test]
    fn insert_escapes_single_quotes_and_marks_null() {
        let row = sample_row();
        assert_eq!(
            row_insert(&row, "widgets"),
            "INSERT INTO widgets (id, name, note) VALUES (1, 'bolt', NULL);"
        );
    }

    #[test]
    fn json_round_trips_types() {
        let row = sample_row();
        let json = row_json(&row);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["name"], "bolt");
        assert!(parsed["note"].is_null());
    }
}
