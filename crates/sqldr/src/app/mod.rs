//! Global TUI state and the event loop that drives it.
//!
//! Implementation is split by concern across sibling modules — `mouse`,
//! `query`, `results`, `settings`, `sidebar`, `wizard` — each holding an
//! `impl App` block for its slice of behavior. This file owns the shared
//! types, the `App` struct itself, and the top-level key/event dispatch
//! that routes into those modules.

mod mouse;
mod query;
mod results;
mod settings;
mod sidebar;
mod wizard;

use std::collections::HashMap;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use sqldr_core::{History, MySqlDriver, Row, Schema, Table};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tui_textarea::TextArea;

use crate::config::{ConnEntry, Config};
use crate::recents::{DbRef, RecentTables};
use crate::theme::Theme;

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
}

impl ConnState {
    fn new(entry: ConnEntry) -> Self {
        ConnState { entry, status: ConnStatus::Idle, expanded: false, schema: None }
    }
}

/// A flattened, renderable row of the sidebar's top-level tree
/// (connections and their databases only — tables live inside a
/// [`DbTab`], opened by selecting a database).
pub enum SidebarNode {
    /// A user-starred table, indexing `App::recents.favorites`.
    Favorite(usize),
    /// A recently-viewed (non-favorited) table, indexing the filtered
    /// list from `RecentTables::recent_excluding_favorites`.
    Recent(usize),
    Connection(usize),
    Database(usize, usize),
}

/// Lazily-loaded table list for an open [`DbTab`]. Kept separate from the
/// connection's [`Schema`] (database names only) so opening one database
/// never pays for walking every table in every other database on the
/// same server.
pub enum TablesState {
    Loading,
    Loaded(Vec<Table>),
    Error(String),
}

/// An open "use this database" tab: selecting a database in the
/// connection tree opens (or switches to) one of these, and the sidebar
/// then shows that database's tables instead of the tree.
pub struct DbTab {
    pub conn_idx: usize,
    pub db_idx: usize,
    pub db_name: String,
    pub tables: TablesState,
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
    /// Options dialog (`Ctrl+O`): arrowing through `Theme::ALL` previews
    /// each theme live (`App::theme` is mutated immediately); `original`
    /// is restored on cancel, `Enter` persists the current preview.
    Settings { selected: usize, original: Theme },
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
    /// Result of lazily loading one database's tables (see
    /// [`TablesState`]). Keyed by `(conn_idx, db_name)` rather than a tab
    /// index, since tabs can be closed/reordered while the request is in
    /// flight.
    TablesLoaded(usize, String, Vec<Table>),
    TablesError(usize, String, String),
    Connected(usize, Arc<MySqlDriver>),
    ConnectError(usize, String),
    /// Result of testing credentials in the "add connection" wizard: the
    /// request id (see [`ConnWizard::request_id`]) and either the server's
    /// database list or an error message.
    WizardTested(u64, Result<Vec<String>, String>),
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
    /// Open "use this database" tabs.
    pub tabs: Vec<DbTab>,
    /// Index into `tabs` currently shown in the sidebar; `None` shows the
    /// connection tree instead.
    pub active_tab: Option<usize>,
    pub editor: TextArea<'static>,
    /// Pre-pagination form of the last query synced into the editor —
    /// deleting our auto-appended `LIMIT`/`OFFSET` back to exactly this
    /// text and rerunning is treated as "run unbounded, on purpose".
    last_synced_base_sql: Option<String>,
    pub results: ResultsState,
    pub status: StatusMessage,
    pub quit: bool,
    pub cancel: Option<CancellationToken>,
    pub overlay: Option<Overlay>,
    /// Recently-opened and favorited databases, pinned atop the sidebar tree.
    pub recents: RecentTables,
    /// A pinned-database open request waiting on its connection's schema
    /// to finish loading (see `sidebar::open_pinned_database`).
    pending_open: Option<DbRef>,
    /// Active color palette; changed live from the options dialog
    /// (`Ctrl+O`) and persisted to `config.toml` on confirm.
    pub theme: Theme,
    /// Sidebar width as a percentage of total width; mouse-drag resizable.
    pub sidebar_width_pct: u16,
    /// Editor pane height in rows; mouse-drag resizable.
    pub editor_height: u16,
    /// Area the UI was last rendered into, used to hit-test mouse events
    /// against the same layout the user is looking at.
    pub last_area: ratatui::layout::Rect,
    drag: Option<mouse::Drag>,
    /// Lazily loaded per-connection query history, keyed by connection name.
    history: HashMap<String, History>,
    pub events: mpsc::UnboundedSender<AppEvent>,
}

impl App {
    pub fn new(config: Config, recents: RecentTables, events: mpsc::UnboundedSender<AppEvent>) -> Self {
        let mut editor = TextArea::default();
        editor.set_placeholder_text("-- escribe SQL, Ctrl+Enter para ejecutar");
        let theme = Theme::by_name(config.theme.as_deref().unwrap_or(""));
        App {
            conns: config.connections.into_iter().map(ConnState::new).collect(),
            active_conn: None,
            focus: Focus::Sidebar,
            sidebar_cursor: 0,
            sidebar_scroll_top: 0,
            sidebar_filter: None,
            tabs: Vec::new(),
            active_tab: None,
            editor,
            last_synced_base_sql: None,
            results: ResultsState::default(),
            status: StatusMessage::Idle,
            quit: false,
            cancel: None,
            overlay: None,
            recents,
            pending_open: None,
            theme,
            sidebar_width_pct: 25,
            editor_height: 7,
            last_area: ratatui::layout::Rect::default(),
            drag: None,
            history: HashMap::new(),
            events,
        }
    }

    /// Snapshots the current connections and theme into a saveable
    /// [`Config`] — used by both the "add connection" wizard and the
    /// options dialog so persisting one setting never drops the other.
    fn to_config(&self) -> Config {
        Config {
            connections: self.conns.iter().map(|c| c.entry.clone()).collect(),
            theme: Some(self.theme.name.to_string()),
        }
    }

    /// Persists `recents` to disk; failures are non-fatal (surfaced in the
    /// status line) since favorites/recents are a convenience, not the
    /// source of truth for anything else in the app.
    fn persist_recents(&mut self) {
        let result = crate::config::recent_tables_path().and_then(|path| self.recents.save(&path));
        if let Err(e) = result {
            self.status = StatusMessage::Error(format!("no se pudo guardar recientes: {e}"));
        }
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
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                let selected = Theme::ALL.iter().position(|t| *t == self.theme).unwrap_or(0);
                self.overlay = Some(Overlay::Settings { selected, original: self.theme });
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
            Some(Overlay::History(picker)) => self.on_history_key(picker, key),
            Some(Overlay::Confirm { sql, source_table, record_history, .. }) => {
                self.on_confirm_key(sql, source_table, record_history, key)
            }
            Some(Overlay::AddConnection(wizard)) => self.on_wizard_key(wizard, key),
            Some(Overlay::Settings { selected, original }) => self.on_settings_key(selected, original, key),
            None => {}
        }
    }

    pub fn on_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.on_key(key),
            AppEvent::Mouse(mouse) => self.on_mouse(mouse),
            AppEvent::Resize => {}
            AppEvent::QueryRow(row) => {
                if self.results.cols.is_empty() {
                    self.results.cols = row.cols.clone();
                    self.results.col_widths =
                        self.results.cols.iter().map(|c| (c.chars().count() as u16).clamp(4, 40)).collect();
                }
                for (i, v) in row.values.iter().enumerate() {
                    if let Some(w) = self.results.col_widths.get_mut(i) {
                        let len = (v.to_string().chars().count() as u16).clamp(4, 40);
                        if len > *w {
                            *w = len;
                        }
                    }
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
                self.conns[ci].schema = Some(*schema);
                if matches!(&self.pending_open, Some(t) if self.conns[ci].entry.name == t.conn) {
                    self.open_pinned_database_from_schema(ci);
                }
            }
            AppEvent::TablesLoaded(ci, db_name, tables) => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.conn_idx == ci && t.db_name == db_name) {
                    tab.tables = TablesState::Loaded(tables);
                }
            }
            AppEvent::TablesError(ci, db_name, e) => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.conn_idx == ci && t.db_name == db_name) {
                    tab.tables = TablesState::Error(e.clone());
                }
                self.status = StatusMessage::Error(format!("tablas de '{db_name}': {e}"));
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

fn point_in(rect: ratatui::layout::Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        App::new(Config::default(), RecentTables::default(), tx)
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

    /// Guards `SQLDR_CONFIG_DIR` (a process-wide env var) so this is the
    /// only test allowed to touch it, and never races another thread.
    static CONFIG_ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn options_dialog_previews_theme_live_and_persists_on_confirm() {
        let _guard = CONFIG_ENV_LOCK.lock();
        // `config::save`/`load` resolve through `SQLDR_CONFIG_DIR` when
        // set, specifically so this real disk-persistence assertion can
        // never read or clobber the developer's actual config.toml.
        let dir = std::env::temp_dir().join(format!(
            "sqldr-test-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SQLDR_CONFIG_DIR", &dir);

        let mut app = test_app();
        let original = app.theme;
        app.overlay = Some(Overlay::Settings { selected: 0, original });

        // Arrow to the next theme: should apply immediately (live preview).
        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        assert_ne!(app.theme, original, "arrowing in the options dialog must preview the theme live");
        assert!(app.overlay.is_some(), "dialog stays open while browsing");

        let previewed = app.theme;
        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(app.theme, previewed, "Enter keeps the previewed theme");
        assert!(app.overlay.is_none(), "Enter closes the dialog");

        let saved = crate::config::load().expect("saved config must be readable");
        assert_eq!(
            saved.theme.as_deref(),
            Some(previewed.name),
            "Enter must persist the previewed theme to disk"
        );

        std::env::remove_var("SQLDR_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_dialog_reverts_theme_on_escape() {
        let mut app = test_app();
        let original = app.theme;
        app.overlay = Some(Overlay::Settings { selected: 0, original });

        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        assert_ne!(app.theme, original);

        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.theme, original, "Esc must restore the theme active before opening the dialog");
        assert!(app.overlay.is_none());
    }

    /// Guards against silently reintroducing a hardcoded border color:
    /// renders a real frame and checks the focused sidebar border pixel
    /// actually carries the active theme's `accent`, not some fixed
    /// `Color::Cyan`. Uses `TestBackend`, which stores `Style`s directly
    /// in its buffer — no ANSI is written, so this is unaffected by
    /// terminal capability or `NO_COLOR`.
    #[test]
    fn focused_border_color_tracks_the_active_theme() {
        let mut app = test_app();
        assert_eq!(app.focus, Focus::Sidebar);
        for &theme in Theme::ALL {
            app.theme = theme;
            let backend = ratatui::backend::TestBackend::new(80, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let corner = &terminal.backend().buffer()[(0, 0)];
            assert_eq!(
                corner.style().fg,
                Some(theme.accent),
                "focused sidebar border must use theme '{}' accent color",
                theme.name
            );
        }
    }

    /// Guards `SQLDR_DATA_DIR` (a process-wide env var) so this is the
    /// only test allowed to touch it, and never races another thread.
    static DATA_ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn sidebar_nodes_lists_favorites_then_recents_then_connections() {
        let mut app = test_app();
        app.recents.favorites.push(DbRef { conn: "a".into(), db: "fav".into() });
        app.recents.touch(DbRef { conn: "a".into(), db: "rec".into() });
        // Also touched, but already favorited — must not appear twice.
        app.recents.touch(DbRef { conn: "a".into(), db: "fav".into() });

        let nodes = app.sidebar_nodes();
        assert!(matches!(nodes[0], SidebarNode::Favorite(0)), "favorites come first");
        assert!(matches!(nodes[1], SidebarNode::Recent(0)), "then non-favorited recents");
        assert_eq!(
            nodes.iter().filter(|n| matches!(n, SidebarNode::Recent(_))).count(),
            1,
            "a favorited database must not also show up under Recent"
        );
    }

    #[test]
    fn favorite_toggle_from_pinned_list_persists_and_removes_entry() {
        let _guard = DATA_ENV_LOCK.lock();
        // `recent_tables_path` resolves through `SQLDR_DATA_DIR` when set,
        // so this real disk-persistence assertion can never read or
        // clobber the developer's actual recent_tables.json.
        let dir = std::env::temp_dir().join(format!(
            "sqldr-test-data-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SQLDR_DATA_DIR", &dir);

        let mut app = test_app();
        let db_ref = DbRef { conn: "demo".into(), db: "billing".into() };
        app.recents.favorites.push(db_ref.clone());
        app.sidebar_cursor = 0;
        assert!(matches!(app.sidebar_nodes().first(), Some(SidebarNode::Favorite(0))));

        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)));

        assert!(!app.recents.is_favorite(&db_ref), "f must un-favorite the selected pinned entry");
        let saved = RecentTables::load(&crate::config::recent_tables_path().unwrap());
        assert!(!saved.is_favorite(&db_ref), "toggling favorite must persist to disk");

        std::env::remove_var("SQLDR_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn conn_with_databases(name: &str, databases: Vec<String>) -> ConnState {
        let mut conn = ConnState::new(ConnEntry { name: name.into(), url: "mysql://x".into(), read_only: false });
        conn.schema = Some(Schema { databases });
        conn
    }

    #[test]
    fn opening_a_database_tab_starts_table_loading_lazily_not_eagerly() {
        let mut app = test_app();
        let mut conn = conn_with_databases("acme", vec!["billing".into()]);
        conn.expanded = true;
        app.conns.push(conn);
        // sidebar_nodes(): [Connection(0), Database(0,0)] — no favorites/recents.
        app.sidebar_cursor = 1;
        app.activate_sidebar_node();

        assert_eq!(app.tabs.len(), 1, "selecting a database must open its tab");
        assert!(
            matches!(app.tabs[0].tables, TablesState::Loading),
            "tables must start Loading, never eagerly populated, when opening a database tab"
        );
        assert_eq!(app.active_tab_table_count(), 0, "table count must be 0 while still loading");
    }

    #[test]
    fn database_search_matches_scoped_to_database_names_only() {
        let mut app = test_app();
        app.conns.push(conn_with_databases("acme", vec!["billing".into(), "reporting".into()]));
        app.sidebar_filter = Some("bill".into());

        let matches = app.sidebar_database_search_matches();
        assert_eq!(matches.len(), 1, "must match only the database whose name contains the filter");
        assert_eq!(app.sidebar_database_search_label(matches[0].0, matches[0].1), "acme/billing");
    }

    #[test]
    fn table_search_is_scoped_to_the_open_tab_only() {
        let mut app = test_app();
        app.conns.push(conn_with_databases("acme", vec!["billing".into()]));
        app.tabs.push(DbTab {
            conn_idx: 0,
            db_idx: 0,
            db_name: "billing".into(),
            tables: TablesState::Loaded(vec![
                Table { name: "invoices".into(), columns: vec![], indexes: vec![] },
                Table { name: "customers".into(), columns: vec![], indexes: vec![] },
            ]),
        });
        app.active_tab = Some(0);
        app.sidebar_filter = Some("inv".into());

        let matches = app.sidebar_table_search_matches();
        let TablesState::Loaded(tables) = &app.tabs[0].tables else { unreachable!() };
        assert_eq!(matches.len(), 1);
        assert_eq!(tables[matches[0]].name, "invoices");
    }

    #[test]
    fn pending_open_resolves_once_schema_loads() {
        let mut app = test_app();
        app.conns.push(ConnState::new(ConnEntry { name: "acme".into(), url: "mysql://x".into(), read_only: false }));
        // Simulate opening a favorite database before its connection has
        // ever been expanded.
        app.pending_open = Some(DbRef { conn: "acme".into(), db: "billing".into() });

        app.on_app_event(AppEvent::SchemaLoaded(0, Box::new(Schema { databases: vec!["billing".into()] })));

        assert_eq!(app.tabs.len(), 1, "resolving pending_open must open the matching db tab");
        assert_eq!(app.tabs[0].db_name, "billing");
        assert!(app.pending_open.is_none(), "pending_open must clear once the database tab is opened");
    }
}
