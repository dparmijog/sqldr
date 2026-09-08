//! Global TUI state and the event loop that drives it.

use std::collections::HashMap;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sqldr_core::{is_mutating, needs_where_confirmation, Driver, History, MySqlDriver, Row, Schema};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tui_textarea::TextArea;

use crate::config::{ConnEntry, Config};

/// Which pane currently receives key input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Editor,
    Results,
}

impl Focus {
    pub fn next(self) -> Focus {
        match self {
            Focus::Sidebar => Focus::Editor,
            Focus::Editor => Focus::Results,
            Focus::Results => Focus::Sidebar,
        }
    }

    pub fn prev(self) -> Focus {
        match self {
            Focus::Sidebar => Focus::Results,
            Focus::Editor => Focus::Sidebar,
            Focus::Results => Focus::Editor,
        }
    }
}

/// Connection lifecycle as tracked by the sidebar.
pub enum ConnStatus {
    Idle,
    Connecting,
    Connected(Arc<MySqlDriver>),
    Error(String),
}

/// One connection entry plus everything the sidebar needs to render its
/// subtree (schema, expand/collapse state).
pub struct ConnState {
    pub entry: ConnEntry,
    pub status: ConnStatus,
    pub expanded: bool,
    pub schema: Option<Schema>,
    /// Expand state per database, indexed like `schema.databases`.
    pub db_expanded: Vec<bool>,
}

impl ConnState {
    fn new(entry: ConnEntry) -> Self {
        ConnState {
            entry,
            status: ConnStatus::Idle,
            expanded: false,
            schema: None,
            db_expanded: Vec::new(),
        }
    }
}

/// A flattened, renderable row of the sidebar tree.
pub enum SidebarNode {
    Connection(usize),
    Database(usize, usize),
    Table(usize, usize, usize),
}

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
        }
    }
}

/// Status-bar line: active connection, running/error state, read-only flag.
pub enum StatusMessage {
    Idle,
    Running,
    Error(String),
    Info(String),
}

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

/// A modal that intercepts all key input until resolved.
pub enum Overlay {
    History(HistoryPicker),
    /// Confirmation for a `DML` statement without `WHERE`. Carries what to
    /// do if the user accepts.
    Confirm {
        sql: String,
        message: String,
        source_table: Option<(String, String)>,
        record_history: bool,
    },
}

/// Events fed into the main select loop, whatever their origin (terminal,
/// a running query, or a background schema load).
pub enum AppEvent {
    Key(KeyEvent),
    Resize,
    QueryRow(Row),
    QueryDone,
    QueryError(String),
    SchemaLoaded(usize, Box<Schema>),
    SchemaError(usize, String),
    Connected(usize, Arc<MySqlDriver>),
    ConnectError(usize, String),
}

pub struct App {
    pub conns: Vec<ConnState>,
    pub active_conn: Option<usize>,
    pub focus: Focus,
    pub sidebar_cursor: usize,
    pub editor: TextArea<'static>,
    pub results: ResultsState,
    pub status: StatusMessage,
    pub quit: bool,
    pub cancel: Option<CancellationToken>,
    pub overlay: Option<Overlay>,
    /// Lazily loaded per-connection query history, keyed by connection name.
    history: HashMap<String, History>,
    pub events: mpsc::UnboundedSender<AppEvent>,
}

impl App {
    pub fn new(config: Config, events: mpsc::UnboundedSender<AppEvent>) -> Self {
        let mut editor = TextArea::default();
        editor.set_placeholder_text("-- escribe SQL, Ctrl+Enter para ejecutar");
        App {
            conns: config.connections.into_iter().map(ConnState::new).collect(),
            active_conn: None,
            focus: Focus::Sidebar,
            sidebar_cursor: 0,
            editor,
            results: ResultsState::default(),
            status: StatusMessage::Idle,
            quit: false,
            cancel: None,
            overlay: None,
            history: HashMap::new(),
            events,
        }
    }

    /// Flattens the sidebar tree according to current expand state, so
    /// rendering and cursor movement share one source of truth.
    pub fn sidebar_nodes(&self) -> Vec<SidebarNode> {
        let mut nodes = Vec::new();
        for (ci, conn) in self.conns.iter().enumerate() {
            nodes.push(SidebarNode::Connection(ci));
            if !conn.expanded {
                continue;
            }
            if let Some(schema) = &conn.schema {
                for (di, (_, tables)) in schema.databases.iter().enumerate() {
                    nodes.push(SidebarNode::Database(ci, di));
                    let expanded = conn.db_expanded.get(di).copied().unwrap_or(false);
                    if !expanded {
                        continue;
                    }
                    for ti in 0..tables.len() {
                        nodes.push(SidebarNode::Table(ci, di, ti));
                    }
                }
            }
        }
        nodes
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.overlay.is_some() {
            self.on_overlay_key(key);
            return;
        }

        // Global keys work regardless of focus.
        match (key.code, key.modifiers) {
            (KeyCode::Tab, _) => {
                self.focus = self.focus.next();
                return;
            }
            (KeyCode::BackTab, _) => {
                self.focus = self.focus.prev();
                return;
            }
            (KeyCode::Enter, KeyModifiers::CONTROL) | (KeyCode::F(5), _) => {
                self.run_editor_query();
                return;
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.cancel_running_query();
                return;
            }
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                self.open_history_picker();
                return;
            }
            // Ctrl+E is handled by the terminal event loop (it needs to
            // suspend/resume the terminal to shell out to `$EDITOR`).
            (KeyCode::Char('q'), KeyModifiers::NONE) if self.focus != Focus::Editor => {
                self.quit = true;
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::Sidebar => self.on_sidebar_key(key),
            Focus::Editor => {
                self.editor.input(key);
            }
            Focus::Results => self.on_results_key(key),
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        match self.overlay.take() {
            Some(Overlay::History(mut picker)) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some(sql) = picker.filtered().get(picker.selected).map(|s| s.to_string()) {
                        self.editor = TextArea::new(sql.lines().map(String::from).collect());
                        self.editor.move_cursor(tui_textarea::CursorMove::Bottom);
                        self.editor.move_cursor(tui_textarea::CursorMove::End);
                        self.editor.set_placeholder_text("-- escribe SQL, Ctrl+Enter para ejecutar");
                        self.focus = Focus::Editor;
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
            },
            Some(Overlay::Confirm { sql, source_table, record_history, .. }) => {
                match key.code {
                    KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                        self.run_query(sql, source_table, record_history);
                    }
                    _ => {
                        self.status = StatusMessage::Info("cancelado".into());
                    }
                }
            }
            None => {}
        }
    }

    fn open_history_picker(&mut self) {
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

    fn on_sidebar_key(&mut self, key: KeyEvent) {
        let nodes = self.sidebar_nodes();
        if nodes.is_empty() {
            return;
        }
        match key.code {
            KeyCode::Up => {
                self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1);
            }
            KeyCode::Down => {
                self.sidebar_cursor = (self.sidebar_cursor + 1).min(nodes.len() - 1);
            }
            KeyCode::Enter => {
                self.activate_sidebar_node();
            }
            _ => {}
        }
    }

    fn activate_sidebar_node(&mut self) {
        let nodes = self.sidebar_nodes();
        let Some(node) = nodes.get(self.sidebar_cursor) else { return };
        match *node {
            SidebarNode::Connection(ci) => {
                self.active_conn = Some(ci);
                let conn = &mut self.conns[ci];
                conn.expanded = !conn.expanded;
                if conn.expanded && conn.schema.is_none() && matches!(conn.status, ConnStatus::Idle | ConnStatus::Error(_)) {
                    self.connect_and_load_schema(ci);
                }
            }
            SidebarNode::Database(ci, di) => {
                let conn = &mut self.conns[ci];
                if conn.db_expanded.len() <= di {
                    conn.db_expanded.resize(di + 1, false);
                }
                conn.db_expanded[di] = !conn.db_expanded[di];
            }
            SidebarNode::Table(ci, di, ti) => {
                self.preview_table(ci, di, ti);
            }
        }
    }

    fn preview_table(&mut self, ci: usize, di: usize, ti: usize) {
        let Some(schema) = &self.conns[ci].schema else { return };
        let Some((db_name, tables)) = schema.databases.get(di) else { return };
        let Some(table) = tables.get(ti) else { return };
        let Some(ConnStatus::Connected(driver)) = self.conns.get(ci).map(|c| &c.status) else {
            self.status = StatusMessage::Error("conexión no lista".into());
            return;
        };
        let dialect = driver.dialect();
        let sql = dialect.limit(
            &format!(
                "SELECT * FROM {}.{}",
                dialect.quote_ident(db_name),
                dialect.quote_ident(&table.name)
            ),
            200,
        );
        let source_table = Some((db_name.clone(), table.name.clone()));
        self.active_conn = Some(ci);
        self.focus = Focus::Results;
        // A LIMIT-200 preview is a convenience, not a deliberate query the
        // user wants to recall later, so it doesn't get recorded.
        self.run_query(sql, source_table, false);
    }

    fn on_results_key(&mut self, key: KeyEvent) {
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
            _ => {}
        }
    }

    fn copy_selected_cell(&mut self) {
        let Some(row) = self.results.rows.get(self.results.cursor_row) else {
            self.status = StatusMessage::Error("sin fila seleccionada".into());
            return;
        };
        let Some(value) = row.values.get(self.results.cursor_col) else { return };
        let text = crate::clipboard::cell_text(value);
        self.report_copy(crate::clipboard::copy(&text), "celda");
    }

    fn copy_selected_row_as(&mut self, format: RowFormat) {
        let Some(row) = self.results.rows.get(self.results.cursor_row) else {
            self.status = StatusMessage::Error("sin fila seleccionada".into());
            return;
        };
        let text = match format {
            RowFormat::Json => crate::clipboard::row_json(row),
            RowFormat::Csv => crate::clipboard::row_csv(row),
            RowFormat::Insert => {
                let Some((db, table)) = &self.results.source_table else {
                    self.status = StatusMessage::Error(
                        "INSERT no disponible: esta consulta no viene de una tabla".into(),
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
            Ok(()) => StatusMessage::Info(format!("{what} copiada (OSC 52)")),
            Err(e) => StatusMessage::Error(format!("copiando {what}: {e}")),
        };
    }

    fn run_editor_query(&mut self) {
        let sql = self.editor.lines().join("\n");
        if sql.trim().is_empty() {
            return;
        }
        self.maybe_confirm_and_run(sql, None, true);
    }

    /// Runs `sql` immediately, unless it's an `UPDATE`/`DELETE` without a
    /// `WHERE` clause — then it's staged behind a confirmation overlay.
    fn maybe_confirm_and_run(&mut self, sql: String, source_table: Option<(String, String)>, record_history: bool) {
        if needs_where_confirmation(&sql) {
            self.overlay = Some(Overlay::Confirm {
                message: format!("Sin WHERE — ¿ejecutar de todas formas?\n\n{sql}"),
                sql,
                source_table,
                record_history,
            });
            return;
        }
        self.run_query(sql, source_table, record_history);
    }

    fn run_query(&mut self, sql: String, source_table: Option<(String, String)>, record_history: bool) {
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

        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.results = ResultsState { running: true, source_table, ..ResultsState::default() };
        self.status = StatusMessage::Running;

        let tx = self.events.clone();
        tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = driver.query(&sql, cancel);
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

    fn cancel_running_query(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
            self.status = StatusMessage::Info("query cancelada".into());
            self.results.running = false;
        }
    }

    fn connect_and_load_schema(&mut self, ci: usize) {
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
            let cfg = sqldr_core::ConnConfig { name: entry.name.clone(), url, read_only: entry.read_only };
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

    pub fn on_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.on_key(key),
            AppEvent::Resize => {}
            AppEvent::QueryRow(row) => {
                if self.results.cols.is_empty() {
                    self.results.cols = row.cols.clone();
                }
                self.results.rows.push(row);
            }
            AppEvent::QueryDone => {
                self.results.running = false;
                self.cancel = None;
                self.status = StatusMessage::Idle;
            }
            AppEvent::QueryError(e) => {
                self.results.running = false;
                self.cancel = None;
                self.status = StatusMessage::Error(e);
            }
            AppEvent::Connected(ci, driver) => {
                self.conns[ci].status = ConnStatus::Connected(driver);
            }
            AppEvent::SchemaLoaded(ci, schema) => {
                self.conns[ci].db_expanded = vec![false; schema.databases.len()];
                self.conns[ci].schema = Some(*schema);
            }
            AppEvent::SchemaError(ci, e) => {
                self.conns[ci].status = ConnStatus::Error(e.clone());
                self.status = StatusMessage::Error(format!("schema '{}': {e}", self.conns[ci].entry.name));
            }
            AppEvent::ConnectError(ci, e) => {
                self.conns[ci].status = ConnStatus::Error(e.clone());
                self.status = StatusMessage::Error(format!("conectando '{}': {e}", self.conns[ci].entry.name));
            }
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
            RowFormat::Json => "fila (JSON)",
            RowFormat::Csv => "fila (CSV)",
            RowFormat::Insert => "fila (INSERT)",
        }
    }
}
