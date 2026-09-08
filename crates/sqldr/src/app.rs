//! Global TUI state and the event loop that drives it.

use std::collections::HashMap;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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

/// Database engine offered by the "add connection" wizard. Only MySQL is
/// implemented today; the roadmap adds Postgres and SQLite as more
/// `Driver` impls land, at which point they join `Engine::ALL`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    MySql,
}

impl Engine {
    pub const ALL: [Engine; 1] = [Engine::MySql];

    pub fn label(self) -> &'static str {
        match self {
            Engine::MySql => "MySQL",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Engine::MySql => 3306,
        }
    }
}

/// Which field of the connection-details form currently has input focus.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnField {
    Name,
    Host,
    Port,
    User,
    Password,
    ReadOnly,
}

impl ConnField {
    const ORDER: [ConnField; 6] = [
        ConnField::Name,
        ConnField::Host,
        ConnField::Port,
        ConnField::User,
        ConnField::Password,
        ConnField::ReadOnly,
    ];

    fn next(self) -> Self {
        let idx = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(idx + 1) % Self::ORDER.len()]
    }

    fn prev(self) -> Self {
        let idx = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(idx + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// Where the "add connection" wizard currently is. Mirrors the flow the
/// user asked for: pick an engine, fill in host/credentials, test them
/// live against the server, then pick a database from what's actually
/// there — rather than typing a database name blind.
#[derive(Clone)]
pub enum WizardStep {
    SelectEngine { selected: usize },
    Details,
    Testing,
    SelectDatabase { databases: Vec<String>, selected: usize },
}

/// Form state for the "nueva conexión" modal (`Ctrl+N`).
pub struct ConnWizard {
    pub step: WizardStep,
    pub engine: Engine,
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub read_only: bool,
    pub field: ConnField,
    pub error: Option<String>,
    /// Identifies which background connection test this wizard is waiting
    /// on, so a stale result (e.g. after the user cancelled and reopened
    /// the wizard) is silently dropped instead of clobbering fresh state.
    request_id: u64,
}

impl ConnWizard {
    fn new() -> Self {
        let engine = Engine::ALL[0];
        ConnWizard {
            step: WizardStep::SelectEngine { selected: 0 },
            engine,
            name: String::new(),
            host: "127.0.0.1".to_string(),
            port: engine.default_port().to_string(),
            user: String::new(),
            password: String::new(),
            read_only: false,
            field: ConnField::Name,
            error: None,
            request_id: 0,
        }
    }

    fn field_mut(&mut self, field: ConnField) -> Option<&mut String> {
        match field {
            ConnField::Name => Some(&mut self.name),
            ConnField::Host => Some(&mut self.host),
            ConnField::Port => Some(&mut self.port),
            ConnField::User => Some(&mut self.user),
            ConnField::Password => Some(&mut self.password),
            ConnField::ReadOnly => None,
        }
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
    AddConnection(ConnWizard),
}

/// Events fed into the main select loop, whatever their origin (terminal,
/// a running query, or a background schema load).
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize,
    QueryRow(Row),
    QueryDone,
    QueryError(String),
    SchemaLoaded(usize, Box<Schema>),
    SchemaError(usize, String),
    Connected(usize, Arc<MySqlDriver>),
    ConnectError(usize, String),
    /// Result of testing credentials in the "add connection" wizard: the
    /// request id (see [`ConnWizard::request_id`]) and either the server's
    /// database list or an error message.
    WizardTested(u64, Result<Vec<String>, String>),
}

/// Which border is currently being mouse-dragged to resize a pane.
enum Drag {
    SidebarBorder,
    EditorBorder,
}

pub struct App {
    pub conns: Vec<ConnState>,
    pub active_conn: Option<usize>,
    pub focus: Focus,
    pub sidebar_cursor: usize,
    /// Top row currently visible in the sidebar; kept in sync with
    /// `sidebar_cursor` by the sidebar renderer so scrolling only moves the
    /// minimum amount needed, instead of jumping the whole viewport.
    pub sidebar_scroll_top: usize,
    /// Active table search text (`/` in the sidebar). `None` = normal tree
    /// navigation; `Some(text)` = flat search across every loaded table.
    pub sidebar_filter: Option<String>,
    pub editor: TextArea<'static>,
    pub results: ResultsState,
    pub status: StatusMessage,
    pub quit: bool,
    pub cancel: Option<CancellationToken>,
    pub overlay: Option<Overlay>,
    /// Sidebar width as a percentage of total width; mouse-drag resizable.
    pub sidebar_width_pct: u16,
    /// Editor pane height in rows; mouse-drag resizable.
    pub editor_height: u16,
    /// Area the UI was last rendered into, used to hit-test mouse events
    /// against the same layout the user is looking at.
    pub last_area: ratatui::layout::Rect,
    drag: Option<Drag>,
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
            sidebar_scroll_top: 0,
            sidebar_filter: None,
            editor,
            results: ResultsState::default(),
            status: StatusMessage::Idle,
            quit: false,
            cancel: None,
            overlay: None,
            sidebar_width_pct: 25,
            editor_height: 7,
            last_area: ratatui::layout::Rect::default(),
            drag: None,
            history: HashMap::new(),
            events,
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        // Modals own all input while open; clicking through them onto the
        // pane underneath would be confusing.
        if self.overlay.is_some() {
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(mouse.column, mouse.row),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(mouse.column, mouse.row),
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            _ => {}
        }
    }

    fn mouse_down(&mut self, x: u16, y: u16) {
        let areas = crate::ui::layout::split(self.last_area, self.sidebar_width_pct, self.editor_height);

        // Resize handles: a 2-cell-wide band straddling each border, wide
        // enough to grab without needing pixel-perfect clicks.
        let sidebar_border = areas.sidebar.x + areas.sidebar.width;
        let near_sidebar_border = x + 1 >= sidebar_border
            && x <= sidebar_border + 1
            && y >= areas.sidebar.y
            && y < areas.sidebar.y + areas.sidebar.height;
        if near_sidebar_border {
            self.drag = Some(Drag::SidebarBorder);
            return;
        }

        let editor_border = areas.editor.y + areas.editor.height;
        let near_editor_border = y + 1 >= editor_border
            && y <= editor_border + 1
            && x >= areas.editor.x
            && x < areas.editor.x + areas.editor.width;
        if near_editor_border {
            self.drag = Some(Drag::EditorBorder);
            return;
        }

        if point_in(areas.sidebar, x, y) {
            self.focus = Focus::Sidebar;
            if self.sidebar_filter.is_none() {
                let nodes = self.sidebar_nodes();
                if !nodes.is_empty() {
                    // Row 0 of the pane's content area is the border; row 1 is
                    // the first list item.
                    let clicked =
                        self.sidebar_scroll_top + y.saturating_sub(areas.sidebar.y + 1) as usize;
                    self.sidebar_cursor = clicked.min(nodes.len() - 1);
                    self.activate_sidebar_node();
                }
            }
            return;
        }

        if point_in(areas.editor, x, y) {
            self.focus = Focus::Editor;
            return;
        }

        if point_in(areas.results, x, y) {
            self.focus = Focus::Results;
            if !self.results.rows.is_empty() {
                // Border + header row precede the data rows.
                let clicked = y.saturating_sub(areas.results.y + 2) as usize;
                self.results.cursor_row =
                    (self.results.scroll_top + clicked).min(self.results.rows.len() - 1);
            }
        }
    }

    fn mouse_drag(&mut self, x: u16, y: u16) {
        let areas = crate::ui::layout::split(self.last_area, self.sidebar_width_pct, self.editor_height);
        match self.drag {
            Some(Drag::SidebarBorder) => {
                if self.last_area.width > 0 {
                    let pct = (x.saturating_sub(self.last_area.x) as u32 * 100
                        / self.last_area.width as u32) as u16;
                    self.sidebar_width_pct = pct.clamp(10, 60);
                }
            }
            Some(Drag::EditorBorder) => {
                let height = y.saturating_sub(areas.editor.y).max(3);
                self.editor_height = height.min(self.last_area.height.saturating_sub(6));
            }
            None => {}
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
            (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.overlay = Some(Overlay::AddConnection(ConnWizard::new()));
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
            Some(Overlay::AddConnection(mut wizard)) => {
                let step = wizard.step.clone();
                match step {
                    WizardStep::SelectEngine { selected } => match key.code {
                        KeyCode::Esc => {
                            self.status = StatusMessage::Info("cancelado".into());
                        }
                        KeyCode::Up => {
                            wizard.step = WizardStep::SelectEngine { selected: selected.saturating_sub(1) };
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        KeyCode::Down => {
                            let selected = (selected + 1).min(Engine::ALL.len() - 1);
                            wizard.step = WizardStep::SelectEngine { selected };
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        KeyCode::Enter => {
                            wizard.engine = Engine::ALL[selected];
                            wizard.port = wizard.engine.default_port().to_string();
                            wizard.step = WizardStep::Details;
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        _ => {
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                    },
                    WizardStep::Details => match (key.code, key.modifiers) {
                        (KeyCode::Esc, _) => {
                            wizard.error = None;
                            wizard.step = WizardStep::SelectEngine { selected: 0 };
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                            self.start_connection_test(wizard);
                        }
                        (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Down, _) => {
                            wizard.field = wizard.field.next();
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        (KeyCode::BackTab, _) | (KeyCode::Up, _) => {
                            wizard.field = wizard.field.prev();
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        (KeyCode::Char(' '), _) | (KeyCode::Enter, _) if wizard.field == ConnField::ReadOnly => {
                            wizard.read_only = !wizard.read_only;
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        (KeyCode::Enter, _) => {
                            wizard.field = wizard.field.next();
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        (KeyCode::Backspace, _) => {
                            let field = wizard.field;
                            if let Some(s) = wizard.field_mut(field) {
                                s.pop();
                            }
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        (KeyCode::Char(c), _) => {
                            let field = wizard.field;
                            if let Some(s) = wizard.field_mut(field) {
                                s.push(c);
                            }
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        _ => {
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                    },
                    WizardStep::Testing => {
                        if key.code == KeyCode::Esc {
                            wizard.step = WizardStep::Details;
                        }
                        self.overlay = Some(Overlay::AddConnection(wizard));
                    }
                    WizardStep::SelectDatabase { databases, selected } => match key.code {
                        KeyCode::Esc => {
                            wizard.step = WizardStep::Details;
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        KeyCode::Up => {
                            wizard.step =
                                WizardStep::SelectDatabase { databases, selected: selected.saturating_sub(1) };
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        KeyCode::Down => {
                            // Index 0 is the synthetic "no database" option.
                            let selected = (selected + 1).min(databases.len());
                            wizard.step = WizardStep::SelectDatabase { databases, selected };
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                        KeyCode::Enter => {
                            let database = if selected == 0 { None } else { databases.get(selected - 1).cloned() };
                            self.finalize_connection(wizard, database);
                        }
                        _ => {
                            wizard.step = WizardStep::SelectDatabase { databases, selected };
                            self.overlay = Some(Overlay::AddConnection(wizard));
                        }
                    },
                }
            }
            None => {}
        }
    }

    /// Validates the connection-details step, then tests the credentials
    /// against the real server in the background (connecting without a
    /// default database) so the next step can offer a live list of
    /// databases to pick from.
    fn start_connection_test(&mut self, mut wizard: ConnWizard) {
        let name = wizard.name.trim().to_string();
        if name.is_empty() {
            wizard.error = Some("el nombre es obligatorio".into());
            self.overlay = Some(Overlay::AddConnection(wizard));
            return;
        }
        if self.conns.iter().any(|c| c.entry.name == name) {
            wizard.error = Some(format!("ya existe una conexión llamada '{name}'"));
            self.overlay = Some(Overlay::AddConnection(wizard));
            return;
        }
        let host = if wizard.host.trim().is_empty() { "127.0.0.1" } else { wizard.host.trim() }.to_string();
        let port_str = if wizard.port.trim().is_empty() {
            wizard.engine.default_port().to_string()
        } else {
            wizard.port.trim().to_string()
        };
        let Ok(port) = port_str.parse::<u16>() else {
            wizard.error = Some(format!("puerto inválido: '{port_str}'"));
            self.overlay = Some(Overlay::AddConnection(wizard));
            return;
        };
        let user = wizard.user.trim().to_string();

        let mut url = match url::Url::parse(&format!("mysql://{host}:{port}")) {
            Ok(u) => u,
            Err(e) => {
                wizard.error = Some(format!("host/puerto inválido: {e}"));
                self.overlay = Some(Overlay::AddConnection(wizard));
                return;
            }
        };
        if !user.is_empty() {
            let _ = url.set_username(&user);
        }
        if !wizard.password.is_empty() {
            let _ = url.set_password(Some(&wizard.password));
        }

        wizard.error = None;
        wizard.step = WizardStep::Testing;
        let request_id = next_wizard_request_id();
        wizard.request_id = request_id;

        let cfg = sqldr_core::ConnConfig { name, url: url.to_string(), read_only: wizard.read_only };
        let tx = self.events.clone();
        tokio::spawn(async move {
            let result = match MySqlDriver::connect(&cfg).await {
                Ok(driver) => driver.list_databases().await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppEvent::WizardTested(request_id, result));
        });
        self.overlay = Some(Overlay::AddConnection(wizard));
    }

    /// Builds the final connection URL (host/port/user/database — no
    /// password), appends it to the config, persists it, and stores the
    /// password in the keyring, mirroring how every other connection here
    /// is set up.
    fn finalize_connection(&mut self, wizard: ConnWizard, database: Option<String>) {
        let name = wizard.name.trim().to_string();
        let host = if wizard.host.trim().is_empty() { "127.0.0.1" } else { wizard.host.trim() };
        let port_str = if wizard.port.trim().is_empty() {
            wizard.engine.default_port().to_string()
        } else {
            wizard.port.trim().to_string()
        };
        let user = wizard.user.trim();

        let mut url = match url::Url::parse(&format!("mysql://{host}:{port_str}")) {
            Ok(u) => u,
            Err(e) => {
                self.status = StatusMessage::Error(format!("URL inválida: {e}"));
                return;
            }
        };
        if !user.is_empty() {
            let _ = url.set_username(user);
        }
        if let Some(db) = &database {
            url.set_path(db);
        }

        let entry = crate::config::ConnEntry { name: name.clone(), url: url.to_string(), read_only: wizard.read_only };
        self.conns.push(ConnState::new(entry.clone()));

        let cfg = crate::config::Config { connections: self.conns.iter().map(|c| c.entry.clone()).collect() };
        if let Err(e) = crate::config::save(&cfg) {
            self.status = StatusMessage::Error(format!("conexión agregada pero no se pudo guardar config.toml: {e}"));
            return;
        }

        if !wizard.password.is_empty() {
            if let Err(e) = crate::config::set_password(&name, &wizard.password) {
                self.status = StatusMessage::Error(format!("conexión guardada, pero falló guardar la contraseña: {e}"));
                return;
            }
        }

        self.status = StatusMessage::Info(format!("conexión '{name}' agregada"));
        self.focus = Focus::Sidebar;
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
        if self.sidebar_filter.is_some() {
            self.on_sidebar_search_key(key);
            return;
        }

        let nodes = self.sidebar_nodes();
        match key.code {
            KeyCode::Char('/') => {
                self.sidebar_filter = Some(String::new());
                self.sidebar_cursor = 0;
                self.sidebar_scroll_top = 0;
            }
            KeyCode::Up if !nodes.is_empty() => {
                self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1);
            }
            KeyCode::Down if !nodes.is_empty() => {
                self.sidebar_cursor = (self.sidebar_cursor + 1).min(nodes.len() - 1);
            }
            KeyCode::Enter if !nodes.is_empty() => {
                self.activate_sidebar_node();
            }
            _ => {}
        }
    }

    fn on_sidebar_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.sidebar_filter = None;
                self.sidebar_cursor = 0;
                self.sidebar_scroll_top = 0;
            }
            KeyCode::Enter => {
                let matches = self.sidebar_search_matches();
                if let Some(&(ci, di, ti)) = matches.get(self.sidebar_cursor) {
                    self.sidebar_filter = None;
                    self.sidebar_cursor = 0;
                    self.sidebar_scroll_top = 0;
                    // Expand the tree down to the match so, after the jump,
                    // the sidebar shows where the table actually lives.
                    self.conns[ci].expanded = true;
                    if self.conns[ci].db_expanded.len() <= di {
                        self.conns[ci].db_expanded.resize(di + 1, false);
                    }
                    self.conns[ci].db_expanded[di] = true;
                    self.preview_table(ci, di, ti);
                }
            }
            KeyCode::Up => {
                self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1);
            }
            KeyCode::Down => {
                let len = self.sidebar_search_matches().len();
                if len > 0 {
                    self.sidebar_cursor = (self.sidebar_cursor + 1).min(len - 1);
                }
            }
            KeyCode::Backspace => {
                if let Some(filter) = self.sidebar_filter.as_mut() {
                    filter.pop();
                }
                self.sidebar_cursor = 0;
            }
            KeyCode::Char(c) => {
                if let Some(filter) = self.sidebar_filter.as_mut() {
                    filter.push(c);
                }
                self.sidebar_cursor = 0;
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

    /// Every loaded table across every connection, flattened and ignoring
    /// expand state — the search index for `/` in the sidebar.
    pub fn sidebar_search_nodes(&self) -> Vec<(usize, usize, usize)> {
        let mut nodes = Vec::new();
        for (ci, conn) in self.conns.iter().enumerate() {
            if let Some(schema) = &conn.schema {
                for (di, (_, tables)) in schema.databases.iter().enumerate() {
                    for ti in 0..tables.len() {
                        nodes.push((ci, di, ti));
                    }
                }
            }
        }
        nodes
    }

    /// Search index filtered by the active `sidebar_filter`, case-insensitive
    /// substring match against the qualified `connection/database/table`
    /// label. Empty filter (just opened search) matches everything.
    pub fn sidebar_search_matches(&self) -> Vec<(usize, usize, usize)> {
        let Some(filter) = &self.sidebar_filter else { return Vec::new() };
        let needle = filter.to_ascii_lowercase();
        self.sidebar_search_nodes()
            .into_iter()
            .filter(|&(ci, di, ti)| needle.is_empty() || self.sidebar_search_label(ci, di, ti).to_ascii_lowercase().contains(&needle))
            .collect()
    }

    pub fn sidebar_search_label(&self, ci: usize, di: usize, ti: usize) -> String {
        let conn_name = self.conns.get(ci).map(|c| c.entry.name.as_str()).unwrap_or("?");
        let schema = self.conns.get(ci).and_then(|c| c.schema.as_ref());
        let db_name = schema.and_then(|s| s.databases.get(di)).map(|(name, _)| name.as_str()).unwrap_or("?");
        let table_name = schema
            .and_then(|s| s.databases.get(di))
            .and_then(|(_, tables)| tables.get(ti))
            .map(|t| t.name.as_str())
            .unwrap_or("?");
        format!("{conn_name}/{db_name}/{table_name}")
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
            AppEvent::Mouse(mouse) => self.on_mouse(mouse),
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
            AppEvent::WizardTested(id, result) => {
                let is_current = matches!(
                    &self.overlay,
                    Some(Overlay::AddConnection(wizard)) if wizard.request_id == id
                );
                if !is_current {
                    // Stale result (wizard cancelled/reopened) or a
                    // different overlay is open now — leave it alone.
                    return;
                }
                let Some(Overlay::AddConnection(mut wizard)) = self.overlay.take() else {
                    return;
                };
                match result {
                    Ok(databases) => {
                        wizard.step = WizardStep::SelectDatabase { databases, selected: 0 };
                    }
                    Err(e) => {
                        wizard.step = WizardStep::Details;
                        wizard.error = Some(format!("no se pudo conectar: {e}"));
                    }
                }
                self.overlay = Some(Overlay::AddConnection(wizard));
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

fn point_in(rect: ratatui::layout::Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

static NEXT_WIZARD_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_wizard_request_id() -> u64 {
    NEXT_WIZARD_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        App::new(Config::default(), tx)
    }

    #[test]
    fn stale_wizard_test_result_does_not_clobber_a_different_overlay() {
        let mut app = test_app();
        app.overlay = Some(Overlay::History(HistoryPicker {
            items: vec!["SELECT 1".to_string()],
            filter: String::new(),
            selected: 0,
        }));

        // A WizardTested event arrives (e.g. a stale background connection
        // test) while a completely different overlay is open.
        app.on_app_event(AppEvent::WizardTested(999, Ok(vec!["some_db".to_string()])));

        assert!(
            matches!(app.overlay, Some(Overlay::History(_))),
            "an unrelated overlay must survive a WizardTested event for a different request"
        );
    }

    #[test]
    fn wizard_test_result_updates_matching_wizard() {
        let mut app = test_app();
        let mut wizard = ConnWizard::new();
        wizard.step = WizardStep::Testing;
        wizard.request_id = 42;
        app.overlay = Some(Overlay::AddConnection(wizard));

        app.on_app_event(AppEvent::WizardTested(42, Ok(vec!["some_db".to_string()])));

        match &app.overlay {
            Some(Overlay::AddConnection(w)) => match &w.step {
                WizardStep::SelectDatabase { databases, .. } => {
                    assert_eq!(databases, &vec!["some_db".to_string()]);
                }
                _ => panic!("expected SelectDatabase step after a successful test"),
            },
            _ => panic!("expected AddConnection overlay to remain"),
        }
    }

    #[test]
    fn stale_wizard_test_result_is_ignored_for_a_newer_wizard() {
        let mut app = test_app();
        let mut wizard = ConnWizard::new();
        wizard.step = WizardStep::Testing;
        wizard.request_id = 2; // newer than the stale result below
        app.overlay = Some(Overlay::AddConnection(wizard));

        app.on_app_event(AppEvent::WizardTested(1, Ok(vec!["some_db".to_string()])));

        match &app.overlay {
            Some(Overlay::AddConnection(w)) => {
                assert!(matches!(w.step, WizardStep::Testing), "stale result must not advance a newer wizard");
            }
            _ => panic!("expected AddConnection overlay to remain"),
        }
    }
}
