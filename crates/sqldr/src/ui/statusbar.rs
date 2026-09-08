//! Bottom status line: active connection, read-only flag, last message.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, ConnStatus, Focus, StatusMessage};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();

    match app.active_conn.and_then(|ci| app.conns.get(ci)) {
        Some(conn) => {
            let state = match &conn.status {
                ConnStatus::Idle => "idle",
                ConnStatus::Connecting => "connecting",
                ConnStatus::Connected(_) => "connected",
                ConnStatus::Error(_) => "error",
            };
            let style = if conn.entry.read_only {
                Style::default().bg(Color::Red).fg(Color::White)
            } else {
                Style::default().fg(Color::Green)
            };
            spans.push(Span::styled(format!(" {} [{state}] ", conn.entry.name), style));
        }
        None => spans.push(Span::raw(" sin conexión ")),
    }

    let focus_name = match app.focus {
        Focus::Sidebar => "sidebar",
        Focus::Editor => "editor",
        Focus::Results => "resultados",
    };
    spans.push(Span::raw(format!(" | foco: {focus_name}")));

    spans.push(Span::raw(" | "));
    match &app.status {
        StatusMessage::Idle => spans.push(Span::raw("listo")),
        StatusMessage::Running => spans.push(Span::styled("ejecutando…", Style::default().fg(Color::Yellow))),
        StatusMessage::Error(e) => spans.push(Span::styled(format!("error: {e}"), Style::default().fg(Color::Red))),
        StatusMessage::Info(m) => spans.push(Span::raw(m.clone())),
    }

    spans.push(Span::raw(
        " | Tab: foco  Ctrl+Enter/F5: ejecutar  Ctrl+C: cancelar  Ctrl+R: historial  Ctrl+N: nueva conexión  Ctrl+E: $EDITOR  q: salir",
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
