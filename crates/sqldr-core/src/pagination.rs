//! Automatic `LIMIT`/`OFFSET` pagination for interactive query execution.
//!
//! Running an unbounded `SELECT` in a TUI can hang the connection and flood
//! memory pulling millions of rows. Every read query gets a default page
//! size automatically, with `OFFSET` bumped to page forward/backward —
//! unless the user already wrote their own `LIMIT`, which is always
//! respected as-is (no pagination controls offered for it).

pub const DEFAULT_PAGE_SIZE: u64 = 500;

/// True if `sql`'s leading keyword indicates a row-returning read query
/// (`SELECT`, or a `WITH` CTE feeding one) — the only statements it makes
/// sense to auto-paginate.
fn is_paginable_statement(sql: &str) -> bool {
    let head = sql.trim_start().to_ascii_uppercase();
    head.starts_with("SELECT") || head.starts_with("WITH")
}

/// Scans for a `LIMIT` keyword at parenthesis depth 0 — i.e. one that
/// applies to the outermost query, not one scoped to a subquery, CTE body,
/// or parenthesized `UNION` branch (`SELECT * FROM (SELECT ... LIMIT 5) t`
/// has no *top-level* limit and would otherwise run unbounded if a naive
/// whole-string scan mistook the inner `LIMIT` for one that caps the outer
/// query). Ignores occurrences inside string literals.
fn has_top_level_limit(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut depth: i32 = 0;
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
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ if b.is_ascii_alphabetic() => {
                    let start = i;
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    if depth <= 0 && sql[start..i].eq_ignore_ascii_case("LIMIT") {
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

/// Builds the SQL to run for `page` (0-indexed) of `sql` at `page_size` rows
/// per page. Returns `None` when `sql` isn't a plain read query, or already
/// carries a top-level `LIMIT` — callers should run it unmodified in that
/// case and not offer pagination controls, since the user's own limit is
/// authoritative.
pub fn paginate(sql: &str, page: usize, page_size: u64) -> Option<String> {
    if !is_paginable_statement(sql) || has_top_level_limit(sql) {
        return None;
    }
    let trimmed = sql.trim_end().trim_end_matches(';');
    let offset = page as u64 * page_size;
    Some(format!("{trimmed} LIMIT {page_size} OFFSET {offset}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paginates_plain_select() {
        assert_eq!(
            paginate("SELECT * FROM widgets", 0, 500),
            Some("SELECT * FROM widgets LIMIT 500 OFFSET 0".to_string())
        );
    }

    #[test]
    fn advances_offset_by_page() {
        assert_eq!(
            paginate("SELECT * FROM widgets", 2, 500),
            Some("SELECT * FROM widgets LIMIT 500 OFFSET 1000".to_string())
        );
    }

    #[test]
    fn respects_explicit_limit() {
        assert_eq!(paginate("SELECT * FROM widgets LIMIT 10", 0, 500), None);
    }

    #[test]
    fn subquery_limit_does_not_count_as_top_level() {
        // The inner LIMIT only bounds the subquery; the outer, unbounded
        // SELECT still needs auto-pagination.
        assert_eq!(
            paginate("SELECT * FROM (SELECT id FROM widgets LIMIT 10) t", 0, 500),
            Some("SELECT * FROM (SELECT id FROM widgets LIMIT 10) t LIMIT 500 OFFSET 0".to_string())
        );
    }

    #[test]
    fn does_not_paginate_non_select_statements() {
        assert_eq!(paginate("UPDATE widgets SET price = 1", 0, 500), None);
        assert_eq!(paginate("SHOW TABLES", 0, 500), None);
    }

    #[test]
    fn strips_trailing_semicolon_before_appending() {
        assert_eq!(
            paginate("SELECT * FROM widgets;", 0, 500),
            Some("SELECT * FROM widgets LIMIT 500 OFFSET 0".to_string())
        );
    }
}
