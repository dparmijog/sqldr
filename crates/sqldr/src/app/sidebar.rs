//! Sidebar state: pinned favorite databases, the connection/database tree,
//! "use this database" tabs, and the flat search (`/`) — which searches
//! *databases* while viewing the tree, and *tables* once a database's tab
//! is open, rather than one search mixing both scopes.

use crossterm::event::{KeyCode, KeyEvent};

use crate::favorites::DbRef;

use super::{App, ConnStatus, DbTab, SidebarNode, StatusMessage, TablesState};

impl App {
    /// Flattens the sidebar tree according to current expand state, so
    /// rendering and cursor movement share one source of truth. Pinned
    /// favorite databases always lead, ahead of the connection tree, so a
    /// frequently-used database never needs re-navigating.
    pub fn sidebar_nodes(&self) -> Vec<SidebarNode> {
        let mut nodes = Vec::new();
        for i in 0..self.favorites.favorites.len() {
            nodes.push(SidebarNode::Favorite(i));
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
            KeyCode::Char('e') if !nodes.is_empty() => {
                self.edit_selected_connection();
            }
            KeyCode::Char('d') if !nodes.is_empty() => {
                self.request_delete_selected_connection();
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
            // Favoriting the tab's database doesn't need its tables
            // loaded, unlike navigating/previewing them.
            KeyCode::Char('f') => {
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
            KeyCode::Char('s') if table_count > 0 => {
                let ti = self.sidebar_cursor.min(table_count - 1);
                self.show_table_structure(ci, di, ti);
            }
            _ => {}
        }
    }

    /// Opens a tab for `(ci, di)` — "use this database" — or switches to it
    /// if it's already open, rather than duplicating. Fetching its tables
    /// (see [`TablesState`]) starts here, the first time the tab is
    /// created — never eagerly for every database up front.
    fn open_db_tab(&mut self, ci: usize, di: usize) {
        self.active_conn = Some(ci);
        let db_name = self.conns[ci]
            .schema
            .as_ref()
            .and_then(|s| s.databases.get(di))
            .cloned()
            .unwrap_or_default();

        let pos = self.tabs.iter().position(|t| t.conn_idx == ci && t.db_idx == di);
        self.active_tab = Some(match pos {
            Some(pos) => pos,
            None => {
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
                if let Some(db_ref) = self.favorites.favorites.get(idx).cloned() {
                    self.open_pinned_database(db_ref);
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

    /// Opens a database referenced from the pinned Favorites section.
    /// Unlike `open_db_tab` (index-based, assumes the connection's schema
    /// is already loaded), this resolves the connection by name and
    /// connects/loads its schema first if needed, deferring the actual
    /// tab-open until `AppEvent::SchemaLoaded` arrives (see
    /// `pending_open`).
    fn open_pinned_database(&mut self, db_ref: DbRef) {
        let Some(ci) = self.conns.iter().position(|c| c.entry.name == db_ref.conn) else {
            self.status = StatusMessage::Error(format!("connection '{}' no longer exists", db_ref.conn));
            return;
        };
        self.active_conn = Some(ci);
        self.pending_open = Some(db_ref);
        if self.conns[ci].schema.is_none() {
            self.conns[ci].expanded = true;
            if matches!(self.conns[ci].status, ConnStatus::Idle | ConnStatus::Error(_)) {
                self.connect_and_load_schema(ci);
            }
            self.status = StatusMessage::Info(format!("connecting to '{}'…", self.conns[ci].entry.name));
            return;
        }
        self.open_pinned_database_from_schema(ci);
    }

    /// Resolves `pending_open`'s database by name within `ci`'s (now
    /// loaded) list of database names and opens its tab. Called once the
    /// connection's schema is available, either immediately or after
    /// `AppEvent::SchemaLoaded` arrives.
    pub(super) fn open_pinned_database_from_schema(&mut self, ci: usize) {
        let Some(db_ref) = self.pending_open.take() else { return };
        let Some(schema) = &self.conns[ci].schema else { return };
        let Some(di) = schema.databases.iter().position(|name| *name == db_ref.db) else {
            self.status = StatusMessage::Error(format!(
                "database '{}' not found on '{}'",
                db_ref.db, db_ref.conn
            ));
            return;
        };
        self.open_db_tab(ci, di);
    }

    pub(super) fn preview_table(&mut self, ci: usize, di: usize, ti: usize) {
        let Some(tab) = self.tabs.iter().find(|t| t.conn_idx == ci && t.db_idx == di) else { return };
        let TablesState::Loaded(tables) = &tab.tables else { return };
        let Some(table_name) = tables.get(ti).map(|t| t.name.clone()) else { return };
        let db_name = tab.db_name.clone();
        let Some(ConnStatus::Connected(driver)) = self.conns.get(ci).map(|c| &c.status) else {
            self.status = StatusMessage::Error("connection not ready".into());
            return;
        };
        let dialect = driver.dialect();
        let sql =
            format!("SELECT * FROM {}.{}", dialect.quote_ident(&db_name), dialect.quote_ident(&table_name));
        let source_table = Some((db_name, table_name));
        self.active_conn = Some(ci);
        self.focus = super::Focus::Results;
        // The default page (LIMIT 500) is a convenience, not a deliberate
        // query the user wants to recall later, so it doesn't get recorded.
        self.run_query(sql, source_table, false, Some((0, sqldr_core::DEFAULT_PAGE_SIZE)));
    }

    /// Shows a read-only structure view (columns, indexes, foreign keys)
    /// for a table already loaded in an open tab — no new query needed,
    /// `Driver::tables` already fetched all of this.
    pub(super) fn show_table_structure(&mut self, ci: usize, di: usize, ti: usize) {
        let Some(tab) = self.tabs.iter().find(|t| t.conn_idx == ci && t.db_idx == di) else { return };
        let TablesState::Loaded(tables) = &tab.tables else { return };
        let Some(table) = tables.get(ti).cloned() else { return };
        let db_name = tab.db_name.clone();
        self.overlay = Some(super::Overlay::TableStructure { db_name, table });
    }

    /// Resolves whatever database is currently "selected" in the sidebar,
    /// regardless of mode — a pinned favorite row, a tree `Database` node,
    /// or the database backing an open tab — used by the `f`
    /// favorite-toggle key. Tree mode's `Connection` nodes and search mode
    /// have no single database selected, so they resolve to `None`.
    fn selected_db_ref(&self) -> Option<DbRef> {
        if self.sidebar_filter.is_some() {
            return None;
        }
        if let Some(active) = self.active_tab {
            let tab = &self.tabs[active];
            let conn = self.conns.get(tab.conn_idx)?.entry.name.clone();
            return Some(DbRef { conn, db: tab.db_name.clone() });
        }
        self.node_db_ref(self.sidebar_nodes().get(self.sidebar_cursor)?)
    }

    /// Resolves any sidebar tree node to the database it represents, if
    /// any — shared by `selected_db_ref` (current cursor) and
    /// `toggle_favorite_selected` (relocating the cursor after the list
    /// resizes).
    fn node_db_ref(&self, node: &SidebarNode) -> Option<DbRef> {
        match *node {
            SidebarNode::Favorite(idx) => self.favorites.favorites.get(idx).cloned(),
            SidebarNode::Connection(_) => None,
            SidebarNode::Database(ci, di) => self.db_ref_for(ci, di),
        }
    }

    fn db_ref_for(&self, ci: usize, di: usize) -> Option<DbRef> {
        let conn = self.conns.get(ci)?.entry.name.clone();
        let db = self.conns.get(ci)?.schema.as_ref()?.databases.get(di)?.clone();
        Some(DbRef { conn, db })
    }

    pub(super) fn toggle_favorite_selected(&mut self) {
        // If browsing the tree on a `Database` node, remember its
        // `(ci, di)` so the cursor can return to that exact row after
        // toggling — not just "some node with the same database", which
        // would jump to the newly (un)pinned row instead.
        let tree_node = match self.active_tab.is_none().then(|| self.sidebar_nodes().get(self.sidebar_cursor).cloned()).flatten() {
            Some(SidebarNode::Database(ci, di)) => Some((ci, di)),
            _ => None,
        };
        let Some(db_ref) = self.selected_db_ref() else { return };
        let now_favorite = self.favorites.toggle(db_ref.clone());
        self.persist_favorites();
        self.status = StatusMessage::Info(if now_favorite {
            format!("\u{2605} added to favorites: {}", db_ref.label())
        } else {
            format!("removed from favorites: {}", db_ref.label())
        });
        // Favoriting/unfavoriting inserts or removes a pinned row above
        // the tree, shifting every index below it — follow the cursor to
        // wherever the same logical selection landed instead of leaving
        // it on whatever now occupies the old index.
        if self.active_tab.is_none() {
            let nodes = self.sidebar_nodes();
            let new_idx = match tree_node {
                Some((ci, di)) => {
                    nodes.iter().position(|n| matches!(n, SidebarNode::Database(c, d) if *c == ci && *d == di))
                }
                None => nodes.iter().position(|n| self.node_db_ref(n).as_ref() == Some(&db_ref)),
            };
            if let Some(new_idx) = new_idx {
                self.sidebar_cursor = new_idx;
            }
        }
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
