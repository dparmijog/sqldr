//! Results pane input: cell/row navigation, copy actions, and paging
//! through a previously-run query.

use crossterm::event::{KeyCode, KeyEvent};
use sqldr_core::{ForeignKey, Row};

use super::sidebar::TablesState;
use super::{App, ConnStatus, StatusMessage};

/// Result of the most recent query run, shown in the results pane.
pub struct ResultsState {
    pub rows: Vec<Row>,
    pub cols: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    /// Top row currently visible; kept in sync with `cursor_row` by the
    /// results renderer so the selection never scrolls off-screen.
    pub scroll_top: usize,
    pub running: bool,
    /// `(database, table)` this result set was previewed from, if any.
    /// Enables "copy row as INSERT" (needs a concrete target table).
    pub source_table: Option<(String, String)>,
    /// Present when this result set can be paged further/back — absent
    /// when the query wasn't a plain read or already had its own `LIMIT`.
    pub pagination: Option<PageState>,
    /// Per-column display width, grown to fit the widest value seen so
    /// far (header included) as rows stream in — gives the results table
    /// a real grid look instead of one flat `Min` width for every column.
    pub col_widths: Vec<u16>,
}

/// Tracks the un-paginated SQL and current page for a paginated result set,
/// so `PageUp`/`PageDown` can rebuild the query for the next/previous page.
#[derive(Clone)]
pub struct PageState {
    pub base_sql: String,
    pub page: usize,
    pub page_size: u64,
}

impl Default for ResultsState {
    fn default() -> Self {
        ResultsState {
            rows: Vec::new(),
            cols: Vec::new(),
            cursor_row: 0,
            cursor_col: 0,
            scroll_top: 0,
            running: false,
            source_table: None,
            pagination: None,
            col_widths: Vec::new(),
        }
    }
}

enum RowFormat {
    Json,
    Csv,
    Insert,
}

impl RowFormat {
    fn label(&self) -> &'static str {
        match self {
            RowFormat::Json => "row (JSON)",
            RowFormat::Csv => "row (CSV)",
            RowFormat::Insert => "row (INSERT)",
        }
    }
}

impl App {
    pub(super) fn on_results_key(&mut self, key: KeyEvent) {
        let row_count = self.query.results.rows.len();
        let col_count = self.query.results.cols.len();
        match key.code {
            KeyCode::Up => {
                self.query.results.cursor_row = self.query.results.cursor_row.saturating_sub(1);
            }
            KeyCode::Down => {
                if row_count > 0 {
                    self.query.results.cursor_row = (self.query.results.cursor_row + 1).min(row_count - 1);
                }
            }
            KeyCode::Left => {
                self.query.results.cursor_col = self.query.results.cursor_col.saturating_sub(1);
            }
            KeyCode::Right => {
                if col_count > 0 {
                    self.query.results.cursor_col = (self.query.results.cursor_col + 1).min(col_count - 1);
                }
            }
            KeyCode::Char('y') => self.copy_selected_cell(),
            KeyCode::Char('Y') => self.copy_selected_row_as(RowFormat::Json),
            KeyCode::Char('c') => self.copy_selected_row_as(RowFormat::Csv),
            KeyCode::Char('i') => self.copy_selected_row_as(RowFormat::Insert),
            KeyCode::Char('g') => self.follow_foreign_key(),
            KeyCode::PageDown => self.go_to_page(1),
            KeyCode::PageUp => self.go_to_page(-1),
            _ => {}
        }
    }

    fn copy_selected_cell(&mut self) {
        let Some(row) = self.query.results.rows.get(self.query.results.cursor_row) else {
            self.status = StatusMessage::Error("no row selected".into());
            return;
        };
        let Some(value) = row.values.get(self.query.results.cursor_col) else { return };
        let text = crate::clipboard::cell_text(value);
        self.report_copy(crate::clipboard::copy(&text), "cell");
    }

    /// Follows a foreign key from the cell under the cursor: looks up
    /// whether the current column is a FK on the table this result set
    /// was previewed from, then runs `SELECT * FROM ref_table WHERE
    /// ref_col = <value>` — a "go to the referenced row" jump. Only
    /// available for a table preview (needs `source_table`), and only
    /// while that table's tab is still open (its `foreign_keys` metadata
    /// lives there, already loaded — no extra query needed to look it up).
    fn follow_foreign_key(&mut self) {
        let Some(ci) = self.conn.active_conn else {
            self.status = StatusMessage::Error("no active connection".into());
            return;
        };
        let Some((db, table)) = self.query.results.source_table.clone() else {
            self.status = StatusMessage::Error(
                "foreign-key navigation needs a table preview (open a table from the sidebar first)".into(),
            );
            return;
        };
        let Some(col_name) = self.query.results.cols.get(self.query.results.cursor_col).cloned() else { return };
        let Some(value) = self.query.results
            .rows
            .get(self.query.results.cursor_row)
            .and_then(|r| r.values.get(self.query.results.cursor_col))
            .cloned()
        else {
            self.status = StatusMessage::Error("no row selected".into());
            return;
        };

        let Some(fk) = self.foreign_key_for(ci, &db, &table, &col_name) else {
            self.status = StatusMessage::Error(format!("'{col_name}' is not a foreign key on {table}"));
            return;
        };

        let Some(ConnStatus::Connected(driver)) = self.conn.conns.get(ci).map(|c| &c.status) else {
            self.status = StatusMessage::Error("connection not ready".into());
            return;
        };
        let dialect = driver.dialect();
        let sql = format!(
            "SELECT * FROM {}.{} WHERE {} = {}",
            dialect.quote_ident(&db),
            dialect.quote_ident(&fk.ref_table),
            dialect.quote_ident(&fk.ref_column),
            crate::clipboard::sql_literal(&value),
        );
        let source_table = Some((db, fk.ref_table));
        // A drill-down the user triggered by navigating, not a query they
        // typed — don't record it again in history.
        self.run_query(sql, source_table, false, Some((0, sqldr_core::DEFAULT_PAGE_SIZE)));
    }

    fn foreign_key_for(&self, ci: usize, db: &str, table: &str, column: &str) -> Option<ForeignKey> {
        let tab = self.conn.tabs.iter().find(|t| t.conn_idx == ci && t.db_name == db)?;
        let TablesState::Loaded(tables) = &tab.tables else { return None };
        let t = tables.iter().find(|t| t.name == table)?;
        t.foreign_keys.iter().find(|fk| fk.column == column).cloned()
    }

    fn copy_selected_row_as(&mut self, format: RowFormat) {
        let Some(row) = self.query.results.rows.get(self.query.results.cursor_row) else {
            self.status = StatusMessage::Error("no row selected".into());
            return;
        };
        let text = match format {
            RowFormat::Json => crate::clipboard::row_json(row),
            RowFormat::Csv => crate::clipboard::row_csv(row),
            RowFormat::Insert => {
                let Some((db, table)) = &self.query.results.source_table else {
                    self.status = StatusMessage::Error(
                        "INSERT not available: this query didn't come from a table".into(),
                    );
                    return;
                };
                crate::clipboard::row_insert(row, &format!("{db}.{table}"))
            }
        };
        self.report_copy(crate::clipboard::copy(&text), format.label());
    }

    fn report_copy(&mut self, result: std::io::Result<()>, what: &str) {
        self.status = match result {
            Ok(()) => StatusMessage::Info(format!("{what} copied (OSC 52)")),
            Err(e) => StatusMessage::Error(format!("copying {what}: {e}")),
        };
    }

    /// Re-runs a paginated result set's base query at a different page.
    fn go_to_page(&mut self, delta: i64) {
        let Some(pagination) = self.query.results.pagination.clone() else {
            self.status = StatusMessage::Error("pagination not available for this query".into());
            return;
        };
        let new_page = if delta < 0 {
            match pagination.page.checked_sub((-delta) as usize) {
                Some(p) => p,
                None => return, // already on the first page
            }
        } else {
            pagination.page + delta as usize
        };
        let source_table = self.query.results.source_table.clone();
        // Not a fresh query the user typed — don't record it again.
        self.run_query(pagination.base_sql, source_table, false, Some((new_page, pagination.page_size)));
    }
}
