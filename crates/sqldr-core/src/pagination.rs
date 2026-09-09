//! Automatic `LIMIT`/`OFFSET` pagination for interactive query execution.
//!
//! Running an unbounded `SELECT` in a TUI can hang the connection and flood
//! memory pulling millions of rows. Every read query gets a default page
//! size automatically, with `OFFSET` bumped to page forward/backward —
//! unless the user already wrote their own `LIMIT`, which is always
//! respected as-is (no pagination controls offered for it).

use crate::guard::contains_keyword;

pub const DEFAULT_PAGE_SIZE: u64 = 500;

/// True if `sql`'s leading keyword indicates a row-returning read query
/// (`SELECT`, or a `WITH` CTE feeding one) — the only statements it makes
/// sense to auto-paginate.
fn is_paginable_statement(sql: &str) -> bool {
    let head = sql.trim_start().to_ascii_uppercase();
    head.starts_with("SELECT") || head.starts_with("WITH")
}

/// Builds the SQL to run for `page` (0-indexed) of `sql` at `page_size` rows
/// per page. Returns `None` when `sql` isn't a plain read query, or already
/// carries an explicit `LIMIT` — callers should run it unmodified in that
/// case and not offer pagination controls, since the user's own limit is
/// authoritative.
pub fn paginate(sql: &str, page: usize, page_size: u64) -> Option<String> {
    if !is_paginable_statement(sql) || contains_keyword(sql, "LIMIT") {
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
