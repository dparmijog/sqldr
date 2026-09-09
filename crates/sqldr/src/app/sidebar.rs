//! Sidebar state: pinned favorites/recents, the connection/database tree,
//! "use this database" tabs, and the flat table search (`/`).

use crossterm::event::{KeyCode, KeyEvent};
use sqldr_core::Driver;

use crate::recents::TableRef;

use super::{App, ConnStatus, DbTab, SidebarNode};

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
                    self.open_db_tab(ci, di);
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

    /// Table list for the currently active tab (empty if none/not loaded).
    pub(super) fn active_tab_table_count(&self) -> usize {
        let Some(active) = self.active_tab else { return 0 };
        let tab = &self.tabs[active];
        self.conns
            .get(tab.conn_idx)
            .and_then(|c| c.schema.as_ref())
            .and_then(|s| s.databases.get(tab.db_idx))
            .map(|(_, tables)| tables.len())
            .unwrap_or(0)
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
    /// if it's already open, rather than duplicating.
    fn open_db_tab(&mut self, ci: usize, di: usize) {
        self.active_conn = Some(ci);
        let pos = self.tabs.iter().position(|t| t.conn_idx == ci && t.db_idx == di);
        self.active_tab = Some(match pos {
            Some(pos) => pos,
            None => {
                self.tabs.push(DbTab { conn_idx: ci, db_idx: di });
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
    /// Unlike `preview_table` (index-based, assumes an already-expanded,
    /// schema-loaded connection), this resolves the connection by name and
    /// connects/loads its schema first if needed, deferring the actual
    /// open until `AppEvent::SchemaLoaded` arrives (see `pending_open`).
    fn open_pinned_table(&mut self, table_ref: TableRef) {
        let Some(ci) = self.conns.iter().position(|c| c.entry.name == table_ref.conn) else {
            self.status = super::StatusMessage::Error(format!("conexión '{}' ya no existe", table_ref.conn));
            return;
        };
        self.active_conn = Some(ci);
        if self.conns[ci].schema.is_none() {
            self.conns[ci].expanded = true;
            if matches!(self.conns[ci].status, ConnStatus::Idle | ConnStatus::Error(_)) {
                self.connect_and_load_schema(ci);
            }
            self.status = super::StatusMessage::Info(format!("conectando a '{}'…", self.conns[ci].entry.name));
            self.pending_open = Some(table_ref);
            return;
        }
        self.open_pinned_table_from_schema(ci, &table_ref);
    }

    /// Resolves `table_ref`'s database/table by name within `ci`'s
    /// (now-loaded) schema and opens/previews it, exactly like clicking it
    /// in the tree would. Called either immediately (schema already
    /// loaded) or once deferred via `pending_open`.
    pub(super) fn open_pinned_table_from_schema(&mut self, ci: usize, table_ref: &TableRef) {
        let Some(schema) = &self.conns[ci].schema else { return };
        let Some(di) = schema.databases.iter().position(|(name, _)| *name == table_ref.db) else {
            self.status = super::StatusMessage::Error(format!(
                "base de datos '{}' no encontrada en '{}'",
                table_ref.db, table_ref.conn
            ));
            return;
        };
        let Some(ti) = schema.databases[di].1.iter().position(|t| t.name == table_ref.table) else {
            self.status = super::StatusMessage::Error(format!("tabla '{}' no encontrada", table_ref.table));
            return;
        };
        self.open_db_tab(ci, di);
        self.preview_table(ci, di, ti);
    }

    pub(super) fn preview_table(&mut self, ci: usize, di: usize, ti: usize) {
        let Some(schema) = &self.conns[ci].schema else { return };
        let Some((db_name, tables)) = schema.databases.get(di) else { return };
        let Some(table) = tables.get(ti) else { return };
        let Some(ConnStatus::Connected(driver)) = self.conns.get(ci).map(|c| &c.status) else {
            self.status = super::StatusMessage::Error("conexión no lista".into());
            return;
        };
        let dialect = driver.dialect();
        let sql = format!(
            "SELECT * FROM {}.{}",
            dialect.quote_ident(db_name),
            dialect.quote_ident(&table.name)
        );
        let source_table = Some((db_name.clone(), table.name.clone()));
        let table_ref =
            TableRef { conn: self.conns[ci].entry.name.clone(), db: db_name.clone(), table: table.name.clone() };
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
        let schema = self.conns.get(ci)?.schema.as_ref()?;
        let (db, tables) = schema.databases.get(di)?;
        let table = tables.get(ti)?.name.clone();
        Some(TableRef { conn, db: db.clone(), table })
    }

    pub(super) fn toggle_favorite_selected(&mut self) {
        let Some(table_ref) = self.selected_table_ref() else { return };
        let now_favorite = self.recents.toggle_favorite(table_ref.clone());
        self.persist_recents();
        self.status = super::StatusMessage::Info(if now_favorite {
            format!("\u{2605} agregado a favoritos: {}", table_ref.label())
        } else {
            format!("quitado de favoritos: {}", table_ref.label())
        });
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
}
