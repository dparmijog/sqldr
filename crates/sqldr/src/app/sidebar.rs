//! Sidebar state: the connection/database tree, "use this database" tabs,
//! and the flat table search (`/`).

use crossterm::event::{KeyCode, KeyEvent};
use sqldr_core::Driver;

use super::{App, ConnStatus, DbTab, SidebarNode};

impl App {
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
        self.active_conn = Some(ci);
        self.focus = super::Focus::Results;
        // The default page (LIMIT 500) is a convenience, not a deliberate
        // query the user wants to recall later, so it doesn't get recorded.
        self.run_query(sql, source_table, false, Some((0, sqldr_core::DEFAULT_PAGE_SIZE)));
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
