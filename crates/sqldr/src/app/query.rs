//! Running SQL: the editor's "run" flow (with the `WHERE`-less DML
//! confirmation and auto-pagination), page rebuilding, connection setup,
//! and per-connection query history.

use std::sync::Arc;

use crossterm::event::KeyCode;
use sqldr_core::{is_mutating, needs_where_confirmation, ConnConfig, Driver, History};
use tokio_util::sync::CancellationToken;

use super::results::{PageState, ResultsState};
use super::tasks;
use super::{App, AppEvent, ConnStatus, Overlay, StatusMessage};

/// A pending SQL statement queued in the history picker, ready to load into
/// the editor.
pub struct HistoryPicker {
    /// All entries for the active connection, most recent last (as stored).
    pub items: Vec<String>,
    pub filter: String,
    pub selected: usize,
}

impl HistoryPicker {
    /// Entries matching the current filter, most-recent-first.
    pub fn filtered(&self) -> Vec<&str> {
        let needle = self.filter.to_ascii_lowercase();
        self.items
            .iter()
            .rev()
            .map(String::as_str)
            .filter(|sql| needle.is_empty() || sql.to_ascii_lowercase().contains(&needle))
            .collect()
    }
}

impl App {
    pub(super) fn run_editor_query(&mut self) {
        let sql = self.editor.lines().join("\n");
        if sql.trim().is_empty() {
            return;
        }
        // If the editor still reads exactly like the last query's
        // un-paginated base (i.e. the user deleted the `LIMIT`/`OFFSET` we
        // appended and re-ran), honor that as "run unbounded" instead of
        // silently reinstating the auto-limit they just removed.
        let unbounded = self.last_synced_base_sql.as_deref() == Some(sql.as_str());
        self.maybe_confirm_and_run(sql, None, true, unbounded);
    }

    /// Runs the editor's current SQL through `EXPLAIN` instead of
    /// executing it, showing the plan in the results pane. Bypasses the
    /// mutation guard and `WHERE`-less confirmation entirely — `EXPLAIN`
    /// never touches data, even for an `UPDATE`/`DELETE`, so it's safe on
    /// a read-only connection too.
    pub(super) fn run_editor_explain(&mut self) {
        let sql = self.editor.lines().join("\n");
        if sql.trim().is_empty() {
            self.status = StatusMessage::Error("nothing to explain: the editor is empty".into());
            return;
        }
        let Some(ci) = self.active_conn else {
            self.status = StatusMessage::Error("no active connection: pick one in the sidebar".into());
            return;
        };
        let driver = match &self.conns[ci].status {
            ConnStatus::Connected(driver) => Arc::clone(driver),
            _ => {
                self.status = StatusMessage::Error("connection not ready yet".into());
                return;
            }
        };

        self.results = ResultsState { running: true, ..ResultsState::default() };
        self.status = StatusMessage::Running;

        let tx = self.events.clone();
        tokio::spawn(async move {
            match driver.explain(&sql).await {
                Ok(plan) => tasks::pump_rows(&tx, futures::stream::iter(plan.rows.into_iter().map(Ok))).await,
                Err(e) => {
                    let _ = tx.send(AppEvent::QueryError(e.to_string()));
                }
            }
        });
    }

    /// Runs `sql` immediately, unless it's an `UPDATE`/`DELETE` without a
    /// `WHERE` clause — then it's staged behind a confirmation overlay.
    /// `unbounded` skips auto-pagination entirely, running `sql` as typed.
    fn maybe_confirm_and_run(&mut self, sql: String, source_table: Option<(String, String)>, record_history: bool, unbounded: bool) {
        if needs_where_confirmation(&sql) {
            self.overlay = Some(Overlay::Confirm {
                message: format!("No WHERE clause — run anyway?\n\n{sql}"),
                sql,
                source_table,
                record_history,
            });
            return;
        }
        let pagination_request = if unbounded { None } else { Some((0, sqldr_core::DEFAULT_PAGE_SIZE)) };
        self.run_query(sql, source_table, record_history, pagination_request);
    }

    pub(super) fn on_confirm_key(
        &mut self,
        sql: String,
        source_table: Option<(String, String)>,
        record_history: bool,
        key: crossterm::event::KeyEvent,
    ) {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.run_query(sql, source_table, record_history, Some((0, sqldr_core::DEFAULT_PAGE_SIZE)));
            }
            _ => {
                self.status = StatusMessage::Info("cancelled".into());
            }
        }
    }

    /// Returns the (possibly updated) picker to keep the overlay open
    /// with, or `None` to close it.
    pub(super) fn on_history_key(
        &mut self,
        mut picker: HistoryPicker,
        key: crossterm::event::KeyEvent,
    ) -> Option<HistoryPicker> {
        match key.code {
            KeyCode::Esc => None,
            KeyCode::Enter => {
                if let Some(sql) = picker.filtered().get(picker.selected).map(|s| s.to_string()) {
                    self.set_editor_sql(&sql);
                    self.focus = super::Focus::Editor;
                }
                None
            }
            KeyCode::Up => {
                picker.selected = picker.selected.saturating_sub(1);
                Some(picker)
            }
            KeyCode::Down => {
                let len = picker.filtered().len();
                if len > 0 {
                    picker.selected = (picker.selected + 1).min(len - 1);
                }
                Some(picker)
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.selected = 0;
                Some(picker)
            }
            KeyCode::Char(c) => {
                picker.filter.push(c);
                picker.selected = 0;
                Some(picker)
            }
            _ => Some(picker),
        }
    }

    pub(super) fn open_history_picker(&mut self) {
        let Some(ci) = self.active_conn else {
            self.status = StatusMessage::Error("no active connection".into());
            return;
        };
        let name = self.conns[ci].entry.name.clone();
        let items = self.history_for(&name).entries().iter().map(|e| e.sql.clone()).collect();
        self.overlay = Some(Overlay::History(HistoryPicker { items, filter: String::new(), selected: 0 }));
    }

    fn history_for(&mut self, conn_name: &str) -> &mut History {
        self.history.entry(conn_name.to_string()).or_insert_with(|| {
            let path = crate::config::history_path(conn_name).unwrap_or_else(|_| {
                std::env::temp_dir().join(format!("sqldr-history-{conn_name}.jsonl"))
            });
            History::load(path).unwrap_or_else(|_| {
                // Fall back to an empty, non-persisted history rather than
                // failing the whole picker over a disk error.
                History::load(std::env::temp_dir().join("sqldr-history-fallback.jsonl"))
                    .expect("temp dir is writable")
            })
        })
    }

    /// Replaces the editor's content, placing the cursor at the end so
    /// typing continues naturally from there.
    pub(crate) fn set_editor_sql(&mut self, sql: &str) {
        let lines: Vec<String> = sql.lines().map(String::from).collect();
        self.editor = tui_textarea::TextArea::new(if lines.is_empty() { vec![String::new()] } else { lines });
        self.editor.move_cursor(tui_textarea::CursorMove::Bottom);
        self.editor.move_cursor(tui_textarea::CursorMove::End);
        self.editor.set_placeholder_text("-- write SQL, Ctrl+Enter to run");
    }

    /// `pagination_request` is `Some((page, page_size))` to auto-paginate
    /// (skipped if `sql` already has its own top-level `LIMIT`), or `None`
    /// to run `sql` completely unmodified — e.g. the user explicitly
    /// stripped a previously auto-applied `LIMIT` and wants it gone.
    pub(super) fn run_query(
        &mut self,
        sql: String,
        source_table: Option<(String, String)>,
        record_history: bool,
        pagination_request: Option<(usize, u64)>,
    ) {
        let Some(ci) = self.active_conn else {
            self.status = StatusMessage::Error("no active connection: pick one in the sidebar".into());
            return;
        };
        let (read_only, name) = {
            let entry = &self.conns[ci].entry;
            (entry.read_only, entry.name.clone())
        };
        if read_only && is_mutating(&sql) {
            self.status = StatusMessage::Error(format!(
                "'{name}' is read-only; that statement looks like a write"
            ));
            return;
        }
        let driver = match &self.conns[ci].status {
            ConnStatus::Connected(driver) => Arc::clone(driver),
            _ => {
                self.status = StatusMessage::Error("connection not ready yet".into());
                return;
            }
        };

        if record_history {
            let _ = self.history_for(&name).push(&sql);
        }

        let (exec_sql, pagination) = match pagination_request {
            Some((page, page_size)) => match sqldr_core::paginate(&sql, page, page_size) {
                Some(paged) => (paged, Some(PageState { base_sql: sql.clone(), page, page_size })),
                None => (sql.clone(), None),
            },
            None => (sql.clone(), None),
        };
        // Show exactly what's about to run — including any auto-applied
        // LIMIT/OFFSET — so the user can see it and freely edit/rerun (and
        // remember the pre-pagination form, so deleting the LIMIT back to
        // it and rerunning is recognized as "run unbounded").
        self.last_synced_base_sql = Some(sql);
        self.set_editor_sql(&exec_sql);

        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.results = ResultsState { running: true, source_table, pagination, ..ResultsState::default() };
        self.status = StatusMessage::Running;

        let tx = self.events.clone();
        tokio::spawn(async move {
            let stream = driver.query(&exec_sql, cancel);
            tasks::pump_rows(&tx, stream).await;
        });
    }

    pub(super) fn cancel_running_query(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
            self.status = StatusMessage::Info("query cancelled".into());
            self.results.running = false;
        }
    }

    /// Fetches `db_name`'s tables in the background and reports the
    /// result via `AppEvent::TablesLoaded`/`TablesError`, keyed by
    /// `(ci, db_name)` so a closed/reopened tab can't be clobbered by a
    /// stale in-flight request.
    pub(super) fn load_tables_for(&mut self, ci: usize, db_name: String) {
        let ConnStatus::Connected(driver) = &self.conns[ci].status else { return };
        let driver = Arc::clone(driver);
        let db_name_for_fetch = db_name.clone();
        self.spawn_into_event(
            async move { driver.tables(&db_name_for_fetch).await.map_err(|e| e.to_string()) },
            move |result| match result {
                Ok(tables) => AppEvent::TablesLoaded(ci, db_name, tables),
                Err(e) => AppEvent::TablesError(ci, db_name, e),
            },
        );
    }

    pub(super) fn connect_and_load_schema(&mut self, ci: usize) {
        self.conns[ci].status = ConnStatus::Connecting;
        let entry = self.conns[ci].entry.clone();
        let tx = self.events.clone();
        tokio::spawn(async move {
            let url = match crate::config::resolve_url(&entry) {
                Ok(u) => u,
                Err(e) => {
                    let _ = tx.send(AppEvent::ConnectError(ci, e.to_string()));
                    return;
                }
            };
            let cfg = ConnConfig { name: entry.name.clone(), url, read_only: entry.read_only };
            match sqldr_core::connect(&cfg).await {
                Ok(driver) => {
                    let _ = tx.send(AppEvent::Connected(ci, Arc::clone(&driver)));
                    match driver.schema().await {
                        Ok(schema) => {
                            let _ = tx.send(AppEvent::SchemaLoaded(ci, Box::new(schema)));
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::SchemaError(ci, e.to_string()));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::ConnectError(ci, e.to_string()));
                }
            }
        });
    }

    /// Starts a periodic `SELECT 1` heartbeat for connection `ci`, so a
    /// silently dropped connection (server-side timeout, network blip)
    /// flips to `ConnStatus::Error` on its own instead of only being
    /// noticed the next time the user tries to run a query. Cancels
    /// whatever heartbeat loop was previously running for `ci` first —
    /// reconnecting/editing a connection must never leave two loops
    /// pinging in parallel.
    pub(super) fn start_heartbeat(&mut self, ci: usize, driver: Arc<dyn Driver>) {
        if let Some(prev) = self.conns[ci].heartbeat_cancel.take() {
            prev.cancel();
        }
        let cancel = CancellationToken::new();
        self.conns[ci].heartbeat_cancel = Some(cancel.clone());
        let tx = self.events.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(20)) => {}
                }
                match driver.execute("SELECT 1").await {
                    Ok(_) => {
                        if tx.send(AppEvent::HeartbeatOk(ci)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        // The connection is dead; stop pinging it. The
                        // user reconnects the normal way (Enter on the
                        // now-`Error` node), which starts a fresh
                        // heartbeat loop with a new cancellation token.
                        let _ = tx.send(AppEvent::HeartbeatError(ci, e.to_string()));
                        return;
                    }
                }
            }
        });
    }
}
