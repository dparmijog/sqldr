//! Running SQL: the editor's "run" flow (with the `WHERE`-less DML
//! confirmation and auto-pagination), page rebuilding, connection setup,
//! and per-connection query history.

use std::sync::Arc;

use crossterm::event::KeyCode;
use sqldr_core::{is_mutating, needs_where_confirmation, ConnConfig, Driver, History, MySqlDriver};
use tokio_util::sync::CancellationToken;

use super::{App, AppEvent, ConnStatus, HistoryPicker, Overlay, PageState, ResultsState, StatusMessage};

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

    /// Runs `sql` immediately, unless it's an `UPDATE`/`DELETE` without a
    /// `WHERE` clause — then it's staged behind a confirmation overlay.
    /// `unbounded` skips auto-pagination entirely, running `sql` as typed.
    fn maybe_confirm_and_run(&mut self, sql: String, source_table: Option<(String, String)>, record_history: bool, unbounded: bool) {
        if needs_where_confirmation(&sql) {
            self.overlay = Some(Overlay::Confirm {
                message: format!("Sin WHERE — ¿ejecutar de todas formas?\n\n{sql}"),
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
                self.status = StatusMessage::Info("cancelado".into());
            }
        }
    }

    pub(super) fn on_history_key(&mut self, mut picker: HistoryPicker, key: crossterm::event::KeyEvent) {
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                if let Some(sql) = picker.filtered().get(picker.selected).map(|s| s.to_string()) {
                    self.set_editor_sql(&sql);
                    self.focus = super::Focus::Editor;
                }
            }
            KeyCode::Up => {
                picker.selected = picker.selected.saturating_sub(1);
                self.overlay = Some(Overlay::History(picker));
            }
            KeyCode::Down => {
                let len = picker.filtered().len();
                if len > 0 {
                    picker.selected = (picker.selected + 1).min(len - 1);
                }
                self.overlay = Some(Overlay::History(picker));
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.selected = 0;
                self.overlay = Some(Overlay::History(picker));
            }
            KeyCode::Char(c) => {
                picker.filter.push(c);
                picker.selected = 0;
                self.overlay = Some(Overlay::History(picker));
            }
            _ => {
                self.overlay = Some(Overlay::History(picker));
            }
        }
    }

    pub(super) fn open_history_picker(&mut self) {
        let Some(ci) = self.active_conn else {
            self.status = StatusMessage::Error("sin conexión activa".into());
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
        self.editor.set_placeholder_text("-- escribe SQL, Ctrl+Enter para ejecutar");
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
            self.status = StatusMessage::Error("sin conexión activa: elige una en el sidebar".into());
            return;
        };
        let (read_only, name) = {
            let entry = &self.conns[ci].entry;
            (entry.read_only, entry.name.clone())
        };
        if read_only && is_mutating(&sql) {
            self.status = StatusMessage::Error(format!(
                "'{name}' es read-only; la sentencia parece una escritura"
            ));
            return;
        }
        let driver = match &self.conns[ci].status {
            ConnStatus::Connected(driver) => Arc::clone(driver),
            _ => {
                self.status = StatusMessage::Error("conexión aún no lista".into());
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
            use futures::StreamExt;
            let mut stream = driver.query(&exec_sql, cancel);
            let mut sent_err = false;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(row) => {
                        if tx.send(AppEvent::QueryRow(row)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::QueryError(e.to_string()));
                        sent_err = true;
                        break;
                    }
                }
            }
            if !sent_err {
                let _ = tx.send(AppEvent::QueryDone);
            }
        });
    }

    pub(super) fn cancel_running_query(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
            self.status = StatusMessage::Info("query cancelada".into());
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
        let tx = self.events.clone();
        tokio::spawn(async move {
            match driver.tables(&db_name).await {
                Ok(tables) => {
                    let _ = tx.send(AppEvent::TablesLoaded(ci, db_name, tables));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::TablesError(ci, db_name, e.to_string()));
                }
            }
        });
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
            match MySqlDriver::connect(&cfg).await {
                Ok(driver) => {
                    let driver = Arc::new(driver);
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
}
