//! Global TUI state and the event loop that drives it.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sqldr_core::{is_mutating, Driver, MySqlDriver, Row, Schema};
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
    pub scroll: usize,
    pub running: bool,
}

impl Default for ResultsState {
    fn default() -> Self {
        ResultsState { rows: Vec::new(), cols: Vec::new(), scroll: 0, running: false }
    }
}

/// Status-bar line: active connection, running/error state, read-only flag.
pub enum StatusMessage {
    Idle,
    Running,
    Error(String),
    Info(String),
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
        self.active_conn = Some(ci);
        self.focus = Focus::Results;
        self.run_query(sql);
    }

    fn on_results_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.results.scroll = self.results.scroll.saturating_sub(1),
            KeyCode::Down => {
                self.results.scroll = self.results.scroll.saturating_add(1);
            }
            _ => {}
        }
    }

    fn run_editor_query(&mut self) {
        let sql = self.editor.lines().join("\n");
        if sql.trim().is_empty() {
            return;
        }
        self.run_query(sql);
    }

    fn run_query(&mut self, sql: String) {
        let Some(ci) = self.active_conn else {
            self.status = StatusMessage::Error("sin conexión activa: elige una en el sidebar".into());
            return;
        };
        let ConnStatus::Connected(driver) = &self.conns[ci].status else {
            self.status = StatusMessage::Error("conexión aún no lista".into());
            return;
        };
        let entry = &self.conns[ci].entry;
        if entry.read_only && is_mutating(&sql) {
            self.status = StatusMessage::Error(format!(
                "'{}' es read-only; la sentencia parece una escritura",
                entry.name
            ));
            return;
        }

        let driver = Arc::clone(driver);
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.results = ResultsState { running: true, ..ResultsState::default() };
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
