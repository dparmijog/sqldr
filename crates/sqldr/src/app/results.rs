//! Results pane input: cell/row navigation, copy actions, and paging
//! through a previously-run query.

use crossterm::event::{KeyCode, KeyEvent};
use sqldr_core::{Driver, ForeignKey};

use super::{App, ConnStatus, StatusMessage, TablesState};

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
        let row_count = self.results.rows.len();
        let col_count = self.results.cols.len();
        match key.code {
            KeyCode::Up => {
                self.results.cursor_row = self.results.cursor_row.saturating_sub(1);
            }
            KeyCode::Down => {
                if row_count > 0 {
                    self.results.cursor_row = (self.results.cursor_row + 1).min(row_count - 1);
                }
            }
            KeyCode::Left => {
                self.results.cursor_col = self.results.cursor_col.saturating_sub(1);
            }
            KeyCode::Right => {
                if col_count > 0 {
                    self.results.cursor_col = (self.results.cursor_col + 1).min(col_count - 1);
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
        let Some(row) = self.results.rows.get(self.results.cursor_row) else {
            self.status = StatusMessage::Error("no row selected".into());
            return;
        };
        let Some(value) = row.values.get(self.results.cursor_col) else { return };
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
        let Some(ci) = self.active_conn else {
            self.status = StatusMessage::Error("no active connection".into());
            return;
        };
        let Some((db, table)) = self.results.source_table.clone() else {
            self.status = StatusMessage::Error(
                "foreign-key navigation needs a table preview (open a table from the sidebar first)".into(),
            );
            return;
        };
        let Some(col_name) = self.results.cols.get(self.results.cursor_col).cloned() else { return };
        let Some(value) = self
            .results
            .rows
            .get(self.results.cursor_row)
            .and_then(|r| r.values.get(self.results.cursor_col))
            .cloned()
        else {
            self.status = StatusMessage::Error("no row selected".into());
            return;
        };

        let Some(fk) = self.foreign_key_for(ci, &db, &table, &col_name) else {
            self.status = StatusMessage::Error(format!("'{col_name}' is not a foreign key on {table}"));
            return;
        };

        let Some(ConnStatus::Connected(driver)) = self.conns.get(ci).map(|c| &c.status) else {
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
        let tab = self.tabs.iter().find(|t| t.conn_idx == ci && t.db_name == db)?;
        let TablesState::Loaded(tables) = &tab.tables else { return None };
        let t = tables.iter().find(|t| t.name == table)?;
        t.foreign_keys.iter().find(|fk| fk.column == column).cloned()
    }

    fn copy_selected_row_as(&mut self, format: RowFormat) {
        let Some(row) = self.results.rows.get(self.results.cursor_row) else {
            self.status = StatusMessage::Error("no row selected".into());
            return;
        };
        let text = match format {
            RowFormat::Json => crate::clipboard::row_json(row),
            RowFormat::Csv => crate::clipboard::row_csv(row),
            RowFormat::Insert => {
                let Some((db, table)) = &self.results.source_table else {
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
        let Some(pagination) = self.results.pagination.clone() else {
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
        let source_table = self.results.source_table.clone();
        // Not a fresh query the user typed — don't record it again.
        self.run_query(pagination.base_sql, source_table, false, Some((new_page, pagination.page_size)));
    }
}
