//! Sidebar state: pinned favorites/recents, the connection/database tree,
//! "use this database" tabs, and the flat search (`/`) — which searches
//! *databases* while viewing the tree, and *tables* once a database's tab
//! is open, rather than one search mixing both scopes.

use crossterm::event::{KeyCode, KeyEvent};
use sqldr_core::Driver;

use crate::recents::TableRef;

use super::{App, ConnStatus, DbTab, SidebarNode, StatusMessage, TablesState};

impl App {
    /// Flattens the sidebar tree according to current expand state, so
    /// rendering and cursor movement share one source of truth. Pinned
    /// favorites/recents always lead, ahead of the connection tree, so
    /// a frequently-used table never needs re-navigating.
    pub fn sidebar_nodes(&self) -> Vec<SidebarNode> {
        let mut nodes = Vec::new();
        for i in 0..self.recents.favorites.len() {
            nodes.push(SidebarNode::Favorite(i));
        }
        for i in 0..self.recents.recent_excluding_favorites().len() {
            nodes.push(SidebarNode::Recent(i));
        }
        for (ci, conn) in self.conns.iter().enumerate() {
            nodes.push(SidebarNode::Connection(ci));
            if !conn.expanded {
                continue;
            }
            if let Some(schema) = &conn.schema {
                for di in 0..schema.databases.len() {
                    nodes.push(SidebarNode::Database(ci, di));
                }
            }
        }
        nodes
    }

    pub(super) fn on_sidebar_key(&mut self, key: KeyEvent) {
        if self.sidebar_filter.is_some() {
            self.on_sidebar_search_key(key);
            return;
        }
        if self.active_tab.is_some() {
            self.on_sidebar_tab_key(key);
            return;
        }

        let nodes = self.sidebar_nodes();
        match key.code {
            KeyCode::Char('/') => {
                self.sidebar_filter = Some(String::new());
                self.sidebar_cursor = 0;
                self.sidebar_scroll_top = 0;
            }
            KeyCode::Char('f') if !nodes.is_empty() => {
                self.toggle_favorite_selected();
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

    /// `/` search, scoped by whatever's currently showing: database names
    /// while browsing the connection tree, or table names once a
    /// database's tab is open. Enter on a database match opens its tab
    /// (which lazily loads its tables); Enter on a table match previews it
    /// directly.
    fn on_sidebar_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.sidebar_filter = None;
                self.sidebar_cursor = 0;
                self.sidebar_scroll_top = 0;
            }
            KeyCode::Enter => {
                if let Some(active) = self.active_tab {
                    let matches = self.sidebar_table_search_matches();
                    if let Some(&ti) = matches.get(self.sidebar_cursor) {
                        let (ci, di) = (self.tabs[active].conn_idx, self.tabs[active].db_idx);
                        self.sidebar_filter = None;
                        self.preview_table(ci, di, ti);
                    }
                } else {
                    let matches = self.sidebar_database_search_matches();
                    if let Some(&(ci, di)) = matches.get(self.sidebar_cursor) {
                        self.sidebar_filter = None;
                        self.open_db_tab(ci, di);
                    }
                }
            }
            KeyCode::Up => {
                self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1);
            }
            KeyCode::Down => {
                let len = if self.active_tab.is_some() {
                    self.sidebar_table_search_matches().len()
                } else {
                    self.sidebar_database_search_matches().len()
                };
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
            // `f` is deliberately not bound here: in search mode every
            // character is filter text the user is typing, not a command.
            KeyCode::Char(c) => {
                if let Some(filter) = self.sidebar_filter.as_mut() {
                    filter.push(c);
                }
                self.sidebar_cursor = 0;
            }
            _ => {}
        }
    }

    /// Table count for the currently active tab: `0` while its tables are
    /// still loading (or failed), so Up/Down/Enter stay inert until
    /// `AppEvent::TablesLoaded` actually arrives.
    pub(super) fn active_tab_table_count(&self) -> usize {
        let Some(active) = self.active_tab else { return 0 };
        match &self.tabs[active].tables {
            TablesState::Loaded(tables) => tables.len(),
            TablesState::Loading | TablesState::Error(_) => 0,
        }
    }

    fn on_sidebar_tab_key(&mut self, key: KeyEvent) {
        let Some(active) = self.active_tab else { return };
        let (ci, di) = {
            let tab = &self.tabs[active];
            (tab.conn_idx, tab.db_idx)
        };
        let table_count = self.active_tab_table_count();

        match key.code {
            KeyCode::Char('/') => {
                self.sidebar_filter = Some(String::new());
                self.sidebar_cursor = 0;
                self.sidebar_scroll_top = 0;
            }
            KeyCode::Char('f') if table_count > 0 => {
                self.toggle_favorite_selected();
            }
            KeyCode::Esc => {
                self.active_tab = None;
                self.sidebar_cursor = 0;
                self.sidebar_scroll_top = 0;
            }
            KeyCode::Left => self.switch_tab(-1),
            KeyCode::Right => self.switch_tab(1),
            KeyCode::Char('x') => self.close_active_tab(),
            KeyCode::Up if table_count > 0 => {
                self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1);
            }
            KeyCode::Down if table_count > 0 => {
                self.sidebar_cursor = (self.sidebar_cursor + 1).min(table_count - 1);
            }
            KeyCode::Enter if table_count > 0 => {
                let ti = self.sidebar_cursor.min(table_count - 1);
                self.preview_table(ci, di, ti);
            }
            _ => {}
        }
    }

    /// Opens a tab for `(ci, di)` — "use this database" — or switches to it
    /// if it's already open, rather than duplicating. Fetching that
    /// database's tables (see [`TablesState`]) starts here, the first time
    /// the tab is created — never eagerly for every database up front.
    fn open_db_tab(&mut self, ci: usize, di: usize) {
        self.active_conn = Some(ci);
        let pos = self.tabs.iter().position(|t| t.conn_idx == ci && t.db_idx == di);
        self.active_tab = Some(match pos {
            Some(pos) => pos,
            None => {
                let db_name = self.conns[ci]
                    .schema
                    .as_ref()
                    .and_then(|s| s.databases.get(di))
                    .cloned()
                    .unwrap_or_default();
                self.tabs.push(DbTab { conn_idx: ci, db_idx: di, db_name: db_name.clone(), tables: TablesState::Loading });
                self.load_tables_for(ci, db_name);
                self.tabs.len() - 1
            }
        });
        self.sidebar_cursor = 0;
        self.sidebar_scroll_top = 0;
    }

    fn close_active_tab(&mut self) {
        let Some(idx) = self.active_tab else { return };
        self.tabs.remove(idx);
        self.active_tab = if self.tabs.is_empty() { None } else { Some(idx.min(self.tabs.len() - 1)) };
        self.sidebar_cursor = 0;
        self.sidebar_scroll_top = 0;
    }

    fn switch_tab(&mut self, delta: i64) {
        let Some(idx) = self.active_tab else { return };
        if self.tabs.is_empty() {
            return;
        }
        let len = self.tabs.len() as i64;
        let new_idx = (idx as i64 + delta).rem_euclid(len) as usize;
        self.active_tab = Some(new_idx);
        self.sidebar_cursor = 0;
        self.sidebar_scroll_top = 0;
    }

    pub(super) fn activate_sidebar_node(&mut self) {
        let nodes = self.sidebar_nodes();
        let Some(node) = nodes.get(self.sidebar_cursor) else { return };
        match *node {
            SidebarNode::Favorite(idx) => {
                if let Some(table_ref) = self.recents.favorites.get(idx).cloned() {
                    self.open_pinned_table(table_ref);
                }
            }
            SidebarNode::Recent(idx) => {
                let table_ref = self.recents.recent_excluding_favorites().get(idx).map(|t| (*t).clone());
                if let Some(table_ref) = table_ref {
                    self.open_pinned_table(table_ref);
                }
            }
            SidebarNode::Connection(ci) => {
                self.active_conn = Some(ci);
                let conn = &mut self.conns[ci];
                conn.expanded = !conn.expanded;
                if conn.expanded && conn.schema.is_none() && matches!(conn.status, ConnStatus::Idle | ConnStatus::Error(_)) {
                    self.connect_and_load_schema(ci);
                }
            }
            SidebarNode::Database(ci, di) => {
                self.open_db_tab(ci, di);
            }
        }
    }

    /// Opens a table referenced from the pinned Favorites/Recent sections.
    /// Unlike `preview_table` (index-based, assumes an already-open tab
    /// with loaded tables), this resolves everything by name and drives
    /// two connect/load stages if needed — connecting (schema: database
    /// names) then opening the tab (tables) — deferring the actual preview
    /// via `pending_open` until whichever stage is still missing finishes.
    fn open_pinned_table(&mut self, table_ref: TableRef) {
        let Some(ci) = self.conns.iter().position(|c| c.entry.name == table_ref.conn) else {
            self.status = StatusMessage::Error(format!("conexión '{}' ya no existe", table_ref.conn));
            return;
        };
        self.active_conn = Some(ci);
        self.pending_open = Some(table_ref);
        if self.conns[ci].schema.is_none() {
            self.conns[ci].expanded = true;
            if matches!(self.conns[ci].status, ConnStatus::Idle | ConnStatus::Error(_)) {
                self.connect_and_load_schema(ci);
            }
            self.status = StatusMessage::Info(format!("conectando a '{}'…", self.conns[ci].entry.name));
            return;
        }
        self.open_pinned_table_from_schema(ci);
    }

    /// Resolves `pending_open`'s database by name within `ci`'s (now
    /// loaded) list of database names and opens its tab — which starts
    /// loading tables if it isn't already open. Called once the
    /// connection's schema is available, either immediately or after
    /// `AppEvent::SchemaLoaded` arrives.
    pub(super) fn open_pinned_table_from_schema(&mut self, ci: usize) {
        let Some(table_ref) = self.pending_open.clone() else { return };
        let Some(schema) = &self.conns[ci].schema else { return };
        let Some(di) = schema.databases.iter().position(|name| *name == table_ref.db) else {
            self.status = StatusMessage::Error(format!(
                "base de datos '{}' no encontrada en '{}'",
                table_ref.db, table_ref.conn
            ));
            self.pending_open = None;
            return;
        };
        self.open_db_tab(ci, di);
        self.try_resolve_pending_open();
    }

    /// Finishes a pinned-table open once its tab's tables are loaded:
    /// resolves the table by name and previews it. Called after opening
    /// the tab (tables may already be cached from a prior visit) and
    /// again from `AppEvent::TablesLoaded`/`TablesError`. A no-op if
    /// nothing is pending, or the tab is still loading.
    pub(super) fn try_resolve_pending_open(&mut self) {
        let Some(table_ref) = self.pending_open.clone() else { return };
        let Some(ci) = self.conns.iter().position(|c| c.entry.name == table_ref.conn) else {
            self.pending_open = None;
            return;
        };
        let Some(tab_idx) = self.tabs.iter().position(|t| t.conn_idx == ci && t.db_name == table_ref.db) else {
            return; // Tab not open yet — still connecting/loading the schema.
        };
        match &self.tabs[tab_idx].tables {
            TablesState::Loading => {} // keep waiting
            TablesState::Error(e) => {
                self.status = StatusMessage::Error(format!("tablas de '{}': {e}", table_ref.db));
                self.pending_open = None;
            }
            TablesState::Loaded(tables) => {
                let Some(ti) = tables.iter().position(|t| t.name == table_ref.table) else {
                    self.status = StatusMessage::Error(format!("tabla '{}' no encontrada", table_ref.table));
                    self.pending_open = None;
                    return;
                };
                let di = self.tabs[tab_idx].db_idx;
                self.pending_open = None;
                self.preview_table(ci, di, ti);
            }
        }
    }

    pub(super) fn preview_table(&mut self, ci: usize, di: usize, ti: usize) {
        let Some(tab) = self.tabs.iter().find(|t| t.conn_idx == ci && t.db_idx == di) else { return };
        let TablesState::Loaded(tables) = &tab.tables else { return };
        let Some(table_name) = tables.get(ti).map(|t| t.name.clone()) else { return };
        let db_name = tab.db_name.clone();
        let Some(ConnStatus::Connected(driver)) = self.conns.get(ci).map(|c| &c.status) else {
            self.status = StatusMessage::Error("conexión no lista".into());
            return;
        };
        let dialect = driver.dialect();
        let sql =
            format!("SELECT * FROM {}.{}", dialect.quote_ident(&db_name), dialect.quote_ident(&table_name));
        let source_table = Some((db_name.clone(), table_name.clone()));
        let table_ref = TableRef { conn: self.conns[ci].entry.name.clone(), db: db_name, table: table_name };
        self.recents.touch(table_ref);
        self.persist_recents();
        self.active_conn = Some(ci);
        self.focus = super::Focus::Results;
        // The default page (LIMIT 500) is a convenience, not a deliberate
        // query the user wants to recall later, so it doesn't get recorded.
        self.run_query(sql, source_table, false, Some((0, sqldr_core::DEFAULT_PAGE_SIZE)));
    }

    /// Resolves whatever table is currently selected in the sidebar,
    /// regardless of mode — a pinned favorite/recent row or an open tab's
    /// table list — used by the `f` favorite-toggle key. Tree mode
    /// (Connection/Database nodes) and search mode have no single table
    /// selected, so they resolve to `None`.
    fn selected_table_ref(&self) -> Option<TableRef> {
        if self.sidebar_filter.is_some() {
            return None;
        }
        if let Some(active) = self.active_tab {
            let (ci, di) = {
                let tab = &self.tabs[active];
                (tab.conn_idx, tab.db_idx)
            };
            return self.table_ref_for(ci, di, self.sidebar_cursor);
        }
        match self.sidebar_nodes().get(self.sidebar_cursor)? {
            SidebarNode::Favorite(idx) => self.recents.favorites.get(*idx).cloned(),
            SidebarNode::Recent(idx) => self.recents.recent_excluding_favorites().get(*idx).map(|t| (*t).clone()),
            SidebarNode::Connection(_) | SidebarNode::Database(_, _) => None,
        }
    }

    fn table_ref_for(&self, ci: usize, di: usize, ti: usize) -> Option<TableRef> {
        let conn = self.conns.get(ci)?.entry.name.clone();
        let tab = self.tabs.iter().find(|t| t.conn_idx == ci && t.db_idx == di)?;
        let TablesState::Loaded(tables) = &tab.tables else { return None };
        let table = tables.get(ti)?.name.clone();
        Some(TableRef { conn, db: tab.db_name.clone(), table })
    }

    pub(super) fn toggle_favorite_selected(&mut self) {
        let Some(table_ref) = self.selected_table_ref() else { return };
        let now_favorite = self.recents.toggle_favorite(table_ref.clone());
        self.persist_recents();
        self.status = StatusMessage::Info(if now_favorite {
            format!("\u{2605} agregado a favoritos: {}", table_ref.label())
        } else {
            format!("quitado de favoritos: {}", table_ref.label())
        });
    }

    /// Every `(connection, database)` pair currently loaded, ignoring
    /// expand state — the search index for `/` while browsing the tree
    /// (no tab open). Search here only ever matches database *names*: it
    /// deliberately never walks into tables, which is exactly the "don't
    /// search tables while I'm looking at databases" behavior wanted.
    pub fn sidebar_database_search_nodes(&self) -> Vec<(usize, usize)> {
        let mut nodes = Vec::new();
        for (ci, conn) in self.conns.iter().enumerate() {
            if let Some(schema) = &conn.schema {
                for di in 0..schema.databases.len() {
                    nodes.push((ci, di));
                }
            }
        }
        nodes
    }

    /// Database search index filtered by the active `sidebar_filter`,
    /// case-insensitive substring match against `connection/database`.
    pub fn sidebar_database_search_matches(&self) -> Vec<(usize, usize)> {
        let Some(filter) = &self.sidebar_filter else { return Vec::new() };
        let needle = filter.to_ascii_lowercase();
        self.sidebar_database_search_nodes()
            .into_iter()
            .filter(|&(ci, di)| {
                needle.is_empty() || self.sidebar_database_search_label(ci, di).to_ascii_lowercase().contains(&needle)
            })
            .collect()
    }

    pub fn sidebar_database_search_label(&self, ci: usize, di: usize) -> String {
        let conn_name = self.conns.get(ci).map(|c| c.entry.name.as_str()).unwrap_or("?");
        let db_name = self
            .conns
            .get(ci)
            .and_then(|c| c.schema.as_ref())
            .and_then(|s| s.databases.get(di))
            .map(|s| s.as_str())
            .unwrap_or("?");
        format!("{conn_name}/{db_name}")
    }

    /// Table indices within the *currently open tab's* database, filtered
    /// by `sidebar_filter` — scoped to one already-selected database, per
    /// the "once I've picked a database, sure, search its tables" request.
    /// Empty while that tab's tables are still loading.
    pub fn sidebar_table_search_matches(&self) -> Vec<usize> {
        let Some(active) = self.active_tab else { return Vec::new() };
        let TablesState::Loaded(tables) = &self.tabs[active].tables else { return Vec::new() };
        let Some(filter) = &self.sidebar_filter else { return Vec::new() };
        let needle = filter.to_ascii_lowercase();
        (0..tables.len())
            .filter(|&ti| needle.is_empty() || tables[ti].name.to_ascii_lowercase().contains(&needle))
            .collect()
    }
}
