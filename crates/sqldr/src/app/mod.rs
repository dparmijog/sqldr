//! Global TUI state and the event loop that drives it.
//!
//! Implementation is split by concern across sibling modules — `mouse`,
//! `query`, `results`, `settings`, `sidebar`, `wizard` — each holding an
//! `impl App` block for its slice of behavior. This file owns the shared
//! types, the `App` struct itself, and the top-level key/event dispatch
//! that routes into those modules.

mod autocomplete;
mod mouse;
pub(crate) mod query;
pub(crate) mod results;
mod settings;
pub(crate) mod sidebar;
mod tasks;
pub(crate) mod wizard;

use std::collections::HashMap;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use sqldr_core::{Driver, History, Row, Schema, Table};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tui_textarea::TextArea;

use crate::config::{ConnEntry, Config};
use crate::favorites::{DbRef, Favorites};
use crate::theme::Theme;

use query::HistoryPicker;
use results::ResultsState;
use sidebar::{DbTab, TablesState};
use wizard::{ConnWizard, WizardStep};

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
    Connected(Arc<dyn Driver>),
    Error(String),
}

/// One connection entry plus everything the sidebar needs to render its
/// subtree (schema, expand/collapse state).
pub struct ConnState {
    pub entry: ConnEntry,
    pub status: ConnStatus,
    pub expanded: bool,
    pub schema: Option<Schema>,
    /// When the last successful heartbeat `SELECT 1` completed, if any —
    /// shown next to a connected entry in the sidebar as a quick "this is
    /// still alive" signal instead of only finding out on the next query.
    pub last_ping: Option<std::time::Instant>,
    /// Cancelled whenever this connection's underlying driver is replaced
    /// or removed (reconnect, edit, delete), so a stale heartbeat loop
    /// pinging a dropped pool never lingers.
    heartbeat_cancel: Option<CancellationToken>,
}

impl ConnState {
    fn new(entry: ConnEntry) -> Self {
        ConnState {
            entry,
            status: ConnStatus::Idle,
            expanded: false,
            schema: None,
            last_ping: None,
            heartbeat_cancel: None,
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
    /// Confirmation for removing a connection entirely (config entry +
    /// stored keyring password), triggered by `d` on a connection node.
    ConfirmDeleteConnection { conn_idx: usize, name: String },
    /// Read-only table structure view (`s` on a table row): columns,
    /// indexes, and foreign keys, from the already-loaded [`Table`].
    /// Any key dismisses it.
    TableStructure { db_name: String, table: Table },
    /// Completion popup (`Ctrl+Space`/`F7` in the editor): candidates
    /// (keywords + table/column names from the open tab) matching the
    /// identifier prefix immediately before the cursor when triggered.
    /// `anchor` is that prefix's `(row, col)` start and `replace_len` its
    /// length, so accepting a candidate knows exactly what to splice out.
    Autocomplete { candidates: Vec<String>, selected: usize, anchor: (usize, usize), replace_len: usize },
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
    Connected(usize, Arc<dyn Driver>),
    ConnectError(usize, String),
    /// Result of testing credentials in the "add connection" wizard: the
    /// request id (see [`ConnWizard::request_id`]) and either the server's
    /// database list or an error message.
    WizardTested(u64, Result<Vec<String>, String>),
    /// A background heartbeat `SELECT 1` against connection `ci` succeeded.
    HeartbeatOk(usize),
    /// A background heartbeat `SELECT 1` against connection `ci` failed —
    /// the connection is treated as dropped (mirrors `ConnectError`).
    HeartbeatError(usize, String),
}

/// Everything about the set of configured connections and the "use this
/// database" tabs opened from them — domain state describing what's
/// connected and loaded, independent of how the sidebar happens to be
/// scrolled or laid out on screen.
pub struct ConnectionsState {
    pub conns: Vec<ConnState>,
    pub active_conn: Option<usize>,
    /// Open "use this database" tabs.
    pub tabs: Vec<DbTab>,
    /// Index into `tabs` currently shown in the sidebar; `None` shows the
    /// connection tree instead.
    pub active_tab: Option<usize>,
    /// Favorited databases, pinned atop the sidebar tree.
    pub favorites: Favorites,
    /// A pinned-database open request waiting on its connection's schema
    /// to finish loading (see `sidebar::open_pinned_database`).
    pending_open: Option<DbRef>,
    /// Lazily loaded per-connection query history, keyed by connection name.
    history: HashMap<String, History>,
}

/// The SQL currently being edited/executed and its results — domain
/// state independent of the editor pane's on-screen geometry.
pub struct QueryState {
    pub editor: TextArea<'static>,
    /// Pre-pagination form of the last query synced into the editor —
    /// deleting our auto-appended `LIMIT`/`OFFSET` back to exactly this
    /// text and rerunning is treated as "run unbounded, on purpose".
    last_synced_base_sql: Option<String>,
    pub results: ResultsState,
    pub cancel: Option<CancellationToken>,
}

pub struct App {
    pub conn: ConnectionsState,
    pub query: QueryState,
    pub focus: Focus,
    pub sidebar_cursor: usize,
    /// Top row currently visible in the sidebar; kept in sync with
    /// `sidebar_cursor` by the sidebar renderer so scrolling only moves the
    /// minimum amount needed, instead of jumping the whole viewport.
    pub sidebar_scroll_top: usize,
    /// Active table search text (`/` in the sidebar). `None` = normal tree
    /// navigation; `Some(text)` = flat search across every loaded table.
    pub sidebar_filter: Option<String>,
    pub status: StatusMessage,
    pub quit: bool,
    pub overlay: Option<Overlay>,
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
    pub events: mpsc::UnboundedSender<AppEvent>,
}

impl App {
    pub fn new(config: Config, favorites: Favorites, events: mpsc::UnboundedSender<AppEvent>) -> Self {
        let mut editor = TextArea::default();
        editor.set_placeholder_text("-- write SQL, Ctrl+Enter to run");
        let theme = Theme::by_name(config.theme.as_deref().unwrap_or(""));
        App {
            conn: ConnectionsState {
                conns: config.connections.into_iter().map(ConnState::new).collect(),
                active_conn: None,
                tabs: Vec::new(),
                active_tab: None,
                favorites,
                pending_open: None,
                history: HashMap::new(),
            },
            query: QueryState {
                editor,
                last_synced_base_sql: None,
                results: ResultsState::default(),
                cancel: None,
            },
            focus: Focus::Sidebar,
            sidebar_cursor: 0,
            sidebar_scroll_top: 0,
            sidebar_filter: None,
            status: StatusMessage::Idle,
            quit: false,
            overlay: None,
            theme,
            sidebar_width_pct: 25,
            editor_height: 7,
            last_area: ratatui::layout::Rect::default(),
            drag: None,
            events,
        }
    }

    /// Snapshots the current connections and theme into a saveable
    /// [`Config`] — used by both the "add connection" wizard and the
    /// options dialog so persisting one setting never drops the other.
    fn to_config(&self) -> Config {
        Config {
            connections: self.conn.conns.iter().map(|c| c.entry.clone()).collect(),
            theme: Some(self.theme.name.to_string()),
        }
    }

    /// Persists `favorites` to disk; failures are non-fatal (surfaced in
    /// the status line) since favorites are a convenience, not the source
    /// of truth for anything else in the app.
    fn persist_favorites(&mut self) {
        let result = crate::config::favorites_path().and_then(|path| self.conn.favorites.save(&path));
        if let Err(e) = result {
            self.status = StatusMessage::Error(format!("could not save favorites: {e}"));
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
            (KeyCode::Char('x'), KeyModifiers::CONTROL) | (KeyCode::F(6), _) => {
                self.run_editor_explain();
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
                if key.code == KeyCode::F(7)
                    || (key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    self.open_autocomplete();
                } else {
                    self.query.editor.input(key);
                }
            }
            Focus::Results => self.on_results_key(key),
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        match self.overlay.take() {
            Some(Overlay::History(picker)) => {
                self.overlay = self.on_history_key(picker, key).map(Overlay::History);
            }
            Some(Overlay::Confirm { sql, source_table, record_history, .. }) => {
                self.on_confirm_key(sql, source_table, record_history, key)
            }
            Some(Overlay::AddConnection(wizard)) => {
                self.overlay = self.on_wizard_key(wizard, key).map(Overlay::AddConnection);
            }
            Some(Overlay::Settings { selected, original }) => {
                self.overlay = self
                    .on_settings_key(selected, original, key)
                    .map(|(selected, original)| Overlay::Settings { selected, original });
            }
            Some(Overlay::ConfirmDeleteConnection { conn_idx, name }) => {
                self.on_confirm_delete_key(conn_idx, name, key)
            }
            Some(Overlay::TableStructure { .. }) => {
                // Read-only info popup: any key dismisses it — already
                // removed from `self.overlay` by `.take()` above.
            }
            Some(Overlay::Autocomplete { candidates, selected, anchor, replace_len }) => {
                self.overlay = self
                    .on_autocomplete_key(candidates, selected, anchor, replace_len, key)
                    .map(|(candidates, selected)| Overlay::Autocomplete { candidates, selected, anchor, replace_len });
            }
            None => {}
        }
    }

    pub fn on_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.on_key(key),
            AppEvent::Mouse(mouse) => self.on_mouse(mouse),
            AppEvent::Resize => {}
            AppEvent::QueryRow(row) => {
                if self.query.results.cols.is_empty() {
                    self.query.results.cols = row.cols.clone();
                    self.query.results.col_widths =
                        self.query.results.cols.iter().map(|c| (c.chars().count() as u16).clamp(4, 40)).collect();
                }
                for (i, v) in row.values.iter().enumerate() {
                    if let Some(w) = self.query.results.col_widths.get_mut(i) {
                        let len = (v.to_string().chars().count() as u16).clamp(4, 40);
                        if len > *w {
                            *w = len;
                        }
                    }
                }
                self.query.results.rows.push(row);
            }
            AppEvent::QueryDone => {
                self.query.results.running = false;
                self.query.cancel = None;
                self.status = StatusMessage::Idle;
            }
            AppEvent::QueryError(e) => {
                self.query.results.running = false;
                self.query.cancel = None;
                self.status = StatusMessage::Error(e);
            }
            AppEvent::Connected(ci, driver) => {
                self.conn.conns[ci].status = ConnStatus::Connected(Arc::clone(&driver));
                self.start_heartbeat(ci, driver);
            }
            AppEvent::SchemaLoaded(ci, schema) => {
                self.conn.conns[ci].schema = Some(*schema);
                if matches!(&self.conn.pending_open, Some(t) if self.conn.conns[ci].entry.name == t.conn) {
                    self.open_pinned_database_from_schema(ci);
                }
            }
            AppEvent::TablesLoaded(ci, db_name, tables) => {
                if let Some(tab) = self.conn.tabs.iter_mut().find(|t| t.conn_idx == ci && t.db_name == db_name) {
                    tab.tables = TablesState::Loaded(tables);
                }
            }
            AppEvent::TablesError(ci, db_name, e) => {
                if let Some(tab) = self.conn.tabs.iter_mut().find(|t| t.conn_idx == ci && t.db_name == db_name) {
                    tab.tables = TablesState::Error(e.clone());
                }
                self.status = StatusMessage::Error(format!("tables for '{db_name}': {e}"));
            }
            AppEvent::SchemaError(ci, e) => {
                self.conn.conns[ci].status = ConnStatus::Error(e.clone());
                self.status = StatusMessage::Error(format!("schema '{}': {e}", self.conn.conns[ci].entry.name));
            }
            AppEvent::ConnectError(ci, e) => {
                self.conn.conns[ci].status = ConnStatus::Error(e.clone());
                self.status = StatusMessage::Error(format!("connecting '{}': {e}", self.conn.conns[ci].entry.name));
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
                        wizard.error = Some(format!("could not connect: {e}"));
                    }
                }
                self.overlay = Some(Overlay::AddConnection(wizard));
            }
            AppEvent::HeartbeatOk(ci) => {
                if let Some(conn) = self.conn.conns.get_mut(ci) {
                    conn.last_ping = Some(std::time::Instant::now());
                }
            }
            AppEvent::HeartbeatError(ci, e) => {
                let name = self.conn.conns.get(ci).map(|c| c.entry.name.clone()).unwrap_or_default();
                if let Some(conn) = self.conn.conns.get_mut(ci) {
                    conn.status = ConnStatus::Error(e.clone());
                    conn.heartbeat_cancel = None;
                }
                self.status = StatusMessage::Error(format!("connection '{name}' heartbeat failed: {e}"));
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
    use sidebar::SidebarNode;
    use wizard::{ConnField, Engine};

    fn test_app() -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        App::new(Config::default(), Favorites::default(), tx)
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
    fn sidebar_nodes_lists_favorites_then_connections() {
        let mut app = test_app();
        app.conn.favorites.favorites.push(DbRef { conn: "a".into(), db: "fav".into() });
        app.conn.conns.push(ConnState::new(ConnEntry { name: "a".into(), url: "mysql://x".into(), read_only: false }));

        let nodes = app.sidebar_nodes();
        assert!(matches!(nodes[0], SidebarNode::Favorite(0)), "favorites come first");
        assert!(matches!(nodes[1], SidebarNode::Connection(0)), "then the connection tree");
    }

    #[test]
    fn favorite_toggle_from_pinned_list_persists_and_removes_entry() {
        let _guard = DATA_ENV_LOCK.lock();
        // `favorites_path` resolves through `SQLDR_DATA_DIR` when set, so
        // this real disk-persistence assertion can never read or clobber
        // the developer's actual favorites.json.
        let dir = std::env::temp_dir().join(format!(
            "sqldr-test-data-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SQLDR_DATA_DIR", &dir);

        let mut app = test_app();
        let db_ref = DbRef { conn: "demo".into(), db: "billing".into() };
        app.conn.favorites.favorites.push(db_ref.clone());
        app.sidebar_cursor = 0;
        assert!(matches!(app.sidebar_nodes().first(), Some(SidebarNode::Favorite(0))));

        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)));

        assert!(!app.conn.favorites.is_favorite(&db_ref), "f must un-favorite the selected pinned entry");
        let saved = Favorites::load(&crate::config::favorites_path().unwrap());
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
        app.conn.conns.push(conn);
        // sidebar_nodes(): [Connection(0), Database(0,0)] — no favorites/recents.
        app.sidebar_cursor = 1;
        app.activate_sidebar_node();

        assert_eq!(app.conn.tabs.len(), 1, "selecting a database must open its tab");
        assert!(
            matches!(app.conn.tabs[0].tables, TablesState::Loading),
            "tables must start Loading, never eagerly populated, when opening a database tab"
        );
        assert_eq!(app.active_tab_table_count(), 0, "table count must be 0 while still loading");
    }

    #[test]
    fn database_search_matches_scoped_to_database_names_only() {
        let mut app = test_app();
        app.conn.conns.push(conn_with_databases("acme", vec!["billing".into(), "reporting".into()]));
        app.sidebar_filter = Some("bill".into());

        let matches = app.sidebar_database_search_matches();
        assert_eq!(matches.len(), 1, "must match only the database whose name contains the filter");
        assert_eq!(app.sidebar_database_search_label(matches[0].0, matches[0].1), "acme/billing");
    }

    #[test]
    fn table_search_is_scoped_to_the_open_tab_only() {
        let mut app = test_app();
        app.conn.conns.push(conn_with_databases("acme", vec!["billing".into()]));
        app.conn.tabs.push(DbTab {
            conn_idx: 0,
            db_idx: 0,
            db_name: "billing".into(),
            tables: TablesState::Loaded(vec![
                Table { name: "invoices".into(), columns: vec![], indexes: vec![], foreign_keys: vec![] },
                Table { name: "customers".into(), columns: vec![], indexes: vec![], foreign_keys: vec![] },
            ]),
        });
        app.conn.active_tab = Some(0);
        app.sidebar_filter = Some("inv".into());

        let matches = app.sidebar_table_search_matches();
        let TablesState::Loaded(tables) = &app.conn.tabs[0].tables else { unreachable!() };
        assert_eq!(matches.len(), 1);
        assert_eq!(tables[matches[0]].name, "invoices");
    }

    #[test]
    fn pending_open_resolves_once_schema_loads() {
        let mut app = test_app();
        app.conn.conns.push(ConnState::new(ConnEntry { name: "acme".into(), url: "mysql://x".into(), read_only: false }));
        // Simulate opening a favorite database before its connection has
        // ever been expanded.
        app.conn.pending_open = Some(DbRef { conn: "acme".into(), db: "billing".into() });

        app.on_app_event(AppEvent::SchemaLoaded(0, Box::new(Schema { databases: vec!["billing".into()] })));

        assert_eq!(app.conn.tabs.len(), 1, "resolving pending_open must open the matching db tab");
        assert_eq!(app.conn.tabs[0].db_name, "billing");
        assert!(app.conn.pending_open.is_none(), "pending_open must clear once the database tab is opened");
    }

    #[test]
    fn favoriting_a_tree_database_keeps_the_cursor_on_it_despite_the_list_shift() {
        let mut app = test_app();
        let mut conn = conn_with_databases("acme", vec!["billing".into(), "reporting".into()]);
        conn.expanded = true;
        app.conn.conns.push(conn);
        // sidebar_nodes(): [Connection(0), Database(0,0)=billing, Database(0,1)=reporting].
        app.sidebar_cursor = 1; // billing

        app.toggle_favorite_selected();

        // Favoriting "billing" inserts a new Favorite(0) row above the
        // tree, shifting Database(0,0) from index 1 to index 2 — the
        // cursor must follow it there, not silently land on whatever now
        // occupies index 1 (Connection(0), a completely different node).
        assert_eq!(app.sidebar_cursor, 2, "cursor must follow the favorited database to its new index");
        let nodes = app.sidebar_nodes();
        assert!(matches!(nodes[app.sidebar_cursor], SidebarNode::Database(0, 0)));

        // Unfavoriting it removes that pinned row again — the cursor must
        // follow it back.
        app.toggle_favorite_selected();
        assert_eq!(app.sidebar_cursor, 1, "cursor must follow the database back after un-favoriting");
        let nodes = app.sidebar_nodes();
        assert!(matches!(nodes[app.sidebar_cursor], SidebarNode::Database(0, 0)));
    }

    #[test]
    fn editing_a_connection_updates_it_in_place() {
        let _guard = CONFIG_ENV_LOCK.lock();
        let dir = std::env::temp_dir().join(format!(
            "sqldr-test-config-edit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SQLDR_CONFIG_DIR", &dir);

        let mut app = test_app();
        app.conn.conns.push(ConnState::new(ConnEntry {
            name: "acme".into(),
            url: "mysql://olduser@127.0.0.1:3306/db1".into(),
            read_only: false,
        }));
        app.sidebar_cursor = 0;

        app.edit_selected_connection();
        let Some(Overlay::AddConnection(mut wizard)) = app.overlay.take() else {
            panic!("expected the wizard to open pre-filled for editing");
        };
        assert_eq!(wizard.name, "acme", "editing must pre-fill the existing name");
        assert_eq!(wizard.host, "127.0.0.1", "editing must pre-fill the existing host");
        assert_eq!(wizard.user, "olduser", "editing must pre-fill the existing user");
        assert!(matches!(wizard.step, WizardStep::Details), "editing must skip the engine picker");

        // Drive straight to the post-test step (the live credential test
        // itself is a background network call, exercised interactively,
        // not here) — same shortcut `wizard_test_result_updates_matching_wizard` uses.
        wizard.read_only = true;
        wizard.step = WizardStep::SelectDatabase { databases: vec!["db2".into()], selected: 1 };
        app.overlay = Some(Overlay::AddConnection(wizard));
        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

        assert_eq!(app.conn.conns.len(), 1, "editing must replace the entry in place, not add a second one");
        assert!(app.conn.conns[0].entry.read_only, "edited fields must be saved");
        assert!(app.conn.conns[0].entry.url.ends_with("/db2"), "the newly picked database must be saved");
        assert!(app.overlay.is_none(), "finalizing must close the wizard");

        std::env::remove_var("SQLDR_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_connection_removes_entry_closes_tabs_and_cancels_heartbeat() {
        let _guard = CONFIG_ENV_LOCK.lock();
        let dir = std::env::temp_dir().join(format!(
            "sqldr-test-config-delete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SQLDR_CONFIG_DIR", &dir);

        let mut app = test_app();
        app.conn.conns.push(conn_with_databases("acme", vec!["billing".into()]));
        let cancel = tokio_util::sync::CancellationToken::new();
        app.conn.conns[0].heartbeat_cancel = Some(cancel.clone());
        app.conn.tabs.push(DbTab {
            conn_idx: 0,
            db_idx: 0,
            db_name: "billing".into(),
            tables: TablesState::Loading,
        });
        app.conn.active_tab = Some(0);
        app.conn.active_conn = Some(0);
        app.sidebar_cursor = 0;

        app.request_delete_selected_connection();
        match &app.overlay {
            Some(Overlay::ConfirmDeleteConnection { conn_idx, name }) => {
                assert_eq!(*conn_idx, 0);
                assert_eq!(name, "acme");
            }
            _ => panic!("expected a delete confirmation overlay"),
        }

        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)));

        assert!(app.conn.conns.is_empty(), "connection must be removed");
        assert!(app.conn.tabs.is_empty(), "tabs backed by the removed connection must close");
        assert!(app.conn.active_tab.is_none());
        assert!(app.conn.active_conn.is_none());
        assert!(cancel.is_cancelled(), "deleting a connection must cancel its background heartbeat loop");

        std::env::remove_var("SQLDR_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn foreign_key_navigation_requires_a_column_that_is_actually_a_foreign_key() {
        use sqldr_core::{ForeignKey, Value};

        let mut app = test_app();
        app.conn.conns.push(conn_with_databases("acme", vec!["billing".into()]));
        app.conn.tabs.push(DbTab {
            conn_idx: 0,
            db_idx: 0,
            db_name: "billing".into(),
            tables: TablesState::Loaded(vec![Table {
                name: "invoices".into(),
                columns: vec![],
                indexes: vec![],
                foreign_keys: vec![ForeignKey {
                    column: "customer_id".into(),
                    ref_table: "customers".into(),
                    ref_column: "id".into(),
                }],
            }]),
        });
        app.conn.active_conn = Some(0);
        app.query.results.cols = vec!["id".into(), "customer_id".into()];
        app.query.results.rows =
            vec![Row { cols: app.query.results.cols.clone(), values: vec![Value::Int(1), Value::Int(42)] }];
        app.query.results.source_table = Some(("billing".into(), "invoices".into()));
        app.focus = Focus::Results;

        // Cursor on `id` (not a FK): must be rejected with a specific error.
        app.query.results.cursor_col = 0;
        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)));
        assert!(
            matches!(&app.status, StatusMessage::Error(e) if e.contains("is not a foreign key")),
            "a non-FK column must be rejected"
        );

        // Cursor on `customer_id` (a real FK): must resolve past FK
        // lookup, only failing later on "connection not ready" since this
        // test has no live driver to actually run the follow-up query.
        app.query.results.cursor_col = 1;
        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)));
        assert!(
            matches!(&app.status, StatusMessage::Error(e) if e.contains("connection not ready")),
            "a real FK column must resolve past FK lookup, failing only on the missing live connection"
        );
    }

    #[test]
    fn table_structure_view_opens_from_a_loaded_table_and_any_key_closes_it() {
        use sqldr_core::Column;

        let mut app = test_app();
        app.conn.conns.push(conn_with_databases("acme", vec!["billing".into()]));
        app.conn.tabs.push(DbTab {
            conn_idx: 0,
            db_idx: 0,
            db_name: "billing".into(),
            tables: TablesState::Loaded(vec![Table {
                name: "invoices".into(),
                columns: vec![Column { name: "id".into(), ty: "int".into(), nullable: false, key: Some("PRI".into()) }],
                indexes: vec!["PRIMARY".into()],
                foreign_keys: vec![],
            }]),
        });
        app.conn.active_tab = Some(0);
        app.sidebar_cursor = 0;

        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)));
        match &app.overlay {
            Some(Overlay::TableStructure { db_name, table }) => {
                assert_eq!(db_name, "billing");
                assert_eq!(table.name, "invoices");
            }
            _ => panic!("expected a TableStructure overlay to open"),
        }

        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert!(app.overlay.is_none(), "any key must dismiss the read-only structure popup");
    }

    #[test]
    fn autocomplete_requires_an_active_connection() {
        let mut app = test_app();
        app.focus = Focus::Editor;
        app.set_editor_sql("SEL");

        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::F(7), KeyModifiers::NONE)));

        assert!(
            matches!(&app.status, StatusMessage::Error(e) if e.contains("no active connection")),
            "F7 with no active connection must report it, not silently do nothing"
        );
        assert!(app.overlay.is_none());
    }

    #[test]
    fn autocomplete_requires_a_ready_connection() {
        let mut app = test_app();
        app.conn.conns.push(ConnState::new(ConnEntry { name: "acme".into(), url: "mysql://x".into(), read_only: false }));
        app.conn.active_conn = Some(0);
        app.focus = Focus::Editor;
        app.set_editor_sql("SEL");

        // Ctrl+Space is the other trigger, alongside F7.
        app.on_app_event(AppEvent::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)));

        assert!(
            matches!(&app.status, StatusMessage::Error(e) if e.contains("connection not ready")),
            "triggering autocomplete on an Idle connection (never connected) must report it"
        );
        assert!(app.overlay.is_none());
    }
}
