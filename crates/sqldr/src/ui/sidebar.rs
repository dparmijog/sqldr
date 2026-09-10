//! Sidebar: three modes sharing one list widget — pinned favorite
//! databases plus the connection/database tree, an open database's table
//! list ("tab"), and a `/` search scoped to whichever of those is showing
//! (database names in tree mode, table names in tab mode).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::sidebar::{SidebarNode, TablesState};
use crate::app::{App, ConnStatus, Focus};
use crate::favorites::DbRef;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Sidebar;

    let (title, labels): (String, Vec<String>) = if let Some(filter) = &app.sidebar_filter {
        if let Some(active) = app.conn.active_tab {
            let matches = app.sidebar_table_search_matches();
            let title = format!("Search table: {filter}_  ({} — Esc: exit)", matches.len());
            let empty_tables = Vec::new();
            let tables = match &app.conn.tabs[active].tables {
                TablesState::Loaded(tables) => tables,
                _ => &empty_tables,
            };
            let labels = matches.iter().filter_map(|&ti| tables.get(ti)).map(|t| t.name.clone()).collect();
            (title, labels)
        } else {
            let matches = app.sidebar_database_search_matches();
            let title = format!("Search database: {filter}_  ({} — Esc: exit)", matches.len());
            let labels =
                matches.iter().map(|&(ci, di)| app.sidebar_database_search_label(ci, di)).collect();
            (title, labels)
        }
    } else if let Some(active) = app.conn.active_tab {
        (tab_title(app, active), tab_table_labels(app, active))
    } else {
        let nodes = app.sidebar_nodes();
        let labels = nodes.iter().map(|node| label(app, node)).collect();
        ("Connections (/ search databases)".to_string(), labels)
    };

    // Keep the cursor inside the visible window, scrolling the minimum
    // amount necessary rather than resetting to the top every frame (a
    // freshly built `ListState` can't remember the previous offset).
    let visible_height = area.height.saturating_sub(2) as usize; // top+bottom border
    let total = labels.len();
    if total == 0 {
        app.sidebar_scroll_top = 0;
    } else {
        let cursor = app.sidebar_cursor.min(total - 1);
        if cursor < app.sidebar_scroll_top {
            app.sidebar_scroll_top = cursor;
        } else if visible_height > 0 && cursor >= app.sidebar_scroll_top + visible_height {
            app.sidebar_scroll_top = cursor + 1 - visible_height;
        }
    }
    let start = app.sidebar_scroll_top.min(total);
    let end = (start + visible_height.max(1)).min(total);

    let items: Vec<ListItem> = labels[start..end].iter().map(|l| ListItem::new(l.as_str())).collect();

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { app.theme.accent } else { app.theme.muted }));

    let list = List::new(items).block(block).highlight_style(
        Style::default().fg(app.theme.accent).add_modifier(Modifier::REVERSED),
    );

    let mut state = ListState::default();
    if total > 0 {
        let cursor = app.sidebar_cursor.min(total - 1);
        state.select(Some(cursor - start));
    }

    frame.render_stateful_widget(list, area, &mut state);
}

/// Breadcrumb title for tab mode: every open tab, the active one bracketed
/// and starred if favorited, plus the key hints for switching/closing tabs.
fn tab_title(app: &App, active: usize) -> String {
    let crumbs: Vec<String> = app.conn.tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let conn_name = app.conn.conns.get(tab.conn_idx).map(|c| c.entry.name.as_str()).unwrap_or("?");
            let db_ref = DbRef { conn: conn_name.to_string(), db: tab.db_name.clone() };
            let star = if app.conn.favorites.is_favorite(&db_ref) { "\u{2605} " } else { "" };
            let name = format!("{star}{conn_name}/{}", tab.db_name);
            if i == active { format!("[{name}]") } else { name }
        })
        .collect();
    format!("{}  (←/→: tab  x: close  Esc: connections)", crumbs.join("  "))
}

fn tab_table_labels(app: &App, active: usize) -> Vec<String> {
    match &app.conn.tabs[active].tables {
        TablesState::Loading => vec!["loading tables…".to_string()],
        TablesState::Error(e) => vec![format!("error loading tables: {e}")],
        TablesState::Loaded(tables) => tables.iter().map(|t| format!("\u{00b7} {}", t.name)).collect(),
    }
}

fn label(app: &App, node: &SidebarNode) -> String {
    match *node {
        SidebarNode::Favorite(idx) => app.conn.favorites
            .favorites
            .get(idx)
            .map(|db| format!("\u{2605} {}", db.label()))
            .unwrap_or_else(|| "?".to_string()),
        SidebarNode::Connection(ci) => {
            let conn = &app.conn.conns[ci];
            let icon = match &conn.status {
                ConnStatus::Idle => "○",
                ConnStatus::Connecting => "◐",
                ConnStatus::Connected(_) => "●",
                ConnStatus::Error(_) => "✗",
            };
            let arrow = if conn.expanded { "▾" } else { "▸" };
            let ro = if conn.entry.read_only { " [ro]" } else { "" };
            match &conn.status {
                ConnStatus::Error(e) => {
                    format!("{arrow} {icon} {}{ro} — {e}", conn.entry.name)
                }
                ConnStatus::Connected(_) => {
                    let ping = conn
                        .last_ping
                        .map(|t| format!(" ({}s)", t.elapsed().as_secs()))
                        .unwrap_or_default();
                    format!("{arrow} {icon} {}{ro}{ping}", conn.entry.name)
                }
                _ => format!("{arrow} {icon} {}{ro}", conn.entry.name),
            }
        }
        SidebarNode::Database(ci, di) => {
            let conn = &app.conn.conns[ci];
            let name = conn.schema.as_ref().and_then(|s| s.databases.get(di)).map(|name| name.as_str()).unwrap_or("?");
            let db_ref = DbRef { conn: conn.entry.name.clone(), db: name.to_string() };
            let star = if app.conn.favorites.is_favorite(&db_ref) { "\u{2605} " } else { "" };
            format!("  ▸ {star}{name}")
        }
    }
}
