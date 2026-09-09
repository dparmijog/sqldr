//! Guardrails shared by every frontend (CLI and TUI) that need to refuse
//! mutating statements on a `read_only` connection before it ever reaches
//! the network.

/// Flags statements that mutate data or schema based on their leading
/// keyword. Deliberately conservative: false positives (blocking a
/// harmless statement) are far cheaper than false negatives on a
/// production, read-only connection.
pub fn is_mutating(sql: &str) -> bool {
    let head = sql.trim_start().to_ascii_uppercase();
    const MUTATING: &[&str] = &[
        "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "REPLACE", "GRANT",
        "REVOKE",
    ];
    MUTATING.iter().any(|kw| head.starts_with(kw))
}

/// True when `sql` is an `UPDATE`/`DELETE` statement with no `WHERE`
/// clause — the classic "forgot the WHERE" accident. Callers should ask
/// for confirmation before running it, regardless of `read_only`.
pub fn needs_where_confirmation(sql: &str) -> bool {
    let head = sql.trim_start().to_ascii_uppercase();
    let is_update_or_delete = head.starts_with("UPDATE") || head.starts_with("DELETE");
    is_update_or_delete && !contains_keyword(sql, "WHERE")
}

/// Scans for a standalone keyword (e.g. `WHERE`, `LIMIT`), ignoring
/// occurrences inside string literals (`'...'`/`"..."`) so a literal like
/// `'nowhere'` doesn't count as `WHERE`. Best-effort, not a full SQL
/// parser: a match inside a subquery or `ON` clause still counts, which
/// only makes callers more lenient (fewer false positives), never less
/// safe/accurate for their purposes.
pub(crate) fn contains_keyword(sql: &str, keyword: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut in_string: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match in_string {
            Some(quote) => {
                if b == quote {
                    in_string = None;
                }
            }
            None => match b {
                b'\'' | b'"' => in_string = Some(b),
                _ if b.is_ascii_alphabetic() => {
                    let start = i;
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    if sql[start..i].eq_ignore_ascii_case(keyword) {
                        return true;
                    }
                    continue;
                }
                _ => {}
            },
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_without_where_needs_confirmation() {
        assert!(needs_where_confirmation("UPDATE widgets SET price = 0"));
        assert!(needs_where_confirmation("delete from widgets"));
    }

    #[test]
    fn update_with_where_does_not_need_confirmation() {
        assert!(!needs_where_confirmation("UPDATE widgets SET price = 0 WHERE id = 1"));
        assert!(!needs_where_confirmation("DELETE FROM widgets WHERE id = 1"));
    }

    #[test]
    fn where_inside_string_literal_does_not_count() {
        assert!(needs_where_confirmation(
            "UPDATE widgets SET name = 'nowhere to be found'"
        ));
    }

    #[test]
    fn select_and_insert_never_need_where_confirmation() {
        assert!(!needs_where_confirmation("SELECT * FROM widgets"));
        assert!(!needs_where_confirmation("INSERT INTO widgets (name) VALUES ('x')"));
    }
}
