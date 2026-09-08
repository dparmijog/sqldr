//! Sidebar: connections → databases → tables tree.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, ConnStatus, Focus, SidebarNode};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let nodes = app.sidebar_nodes();
    let items: Vec<ListItem> = nodes
        .iter()
        .map(|node| ListItem::new(label(app, node)))
        .collect();

    let focused = app.focus == Focus::Sidebar;
    let block = Block::default()
        .title("Conexiones")
        .borders(Borders::ALL)
        .border_style(if focused { Style::default().fg(Color::Cyan) } else { Style::default() });

    let list = List::new(items).block(block).highlight_style(
        Style::default().add_modifier(Modifier::REVERSED),
    );

    let mut state = ListState::default();
    if !nodes.is_empty() {
        state.select(Some(app.sidebar_cursor.min(nodes.len() - 1)));
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
