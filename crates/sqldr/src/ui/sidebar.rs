//! Sidebar: connections → databases → tables tree, plus a flat table
//! search mode (`/`).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, ConnStatus, Focus, SidebarNode};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Sidebar;

    let (title, labels): (String, Vec<String>) = match &app.sidebar_filter {
        Some(filter) => {
            let matches = app.sidebar_search_matches();
            let title = format!("Buscar tabla: {filter}_  ({} — Esc: salir)", matches.len());
            let labels = matches.iter().map(|&(ci, di, ti)| app.sidebar_search_label(ci, di, ti)).collect();
            (title, labels)
        }
        None => {
            let nodes = app.sidebar_nodes();
            let labels = nodes.iter().map(|node| label(app, node)).collect();
            ("Conexiones (/ busca tablas)".to_string(), labels)
        }
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
        .border_style(if focused { Style::default().fg(Color::Cyan) } else { Style::default() });

    let list = List::new(items).block(block).highlight_style(
        Style::default().add_modifier(Modifier::REVERSED),
    );

    let mut state = ListState::default();
    if total > 0 {
        let cursor = app.sidebar_cursor.min(total - 1);
        state.select(Some(cursor - start));
    }

    frame.render_stateful_widget(list, area, &mut state);
}

fn label(app: &App, node: &SidebarNode) -> String {
    match *node {
        SidebarNode::Connection(ci) => {
            let conn = &app.conns[ci];
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
                _ => format!("{arrow} {icon} {}{ro}", conn.entry.name),
            }
        }
        SidebarNode::Database(ci, di) => {
            let conn = &app.conns[ci];
            let expanded = conn.db_expanded.get(di).copied().unwrap_or(false);
            let arrow = if expanded { "▾" } else { "▸" };
            let name = conn
                .schema
                .as_ref()
                .and_then(|s| s.databases.get(di))
                .map(|(name, _)| name.as_str())
                .unwrap_or("?");
            format!("  {arrow} {name}")
        }
        SidebarNode::Table(ci, di, ti) => {
            let name = app.conns[ci]
                .schema
                .as_ref()
                .and_then(|s| s.databases.get(di))
                .and_then(|(_, tables)| tables.get(ti))
                .map(|t| t.name.as_str())
                .unwrap_or("?");
            format!("    · {name}")
        }
    }
}
