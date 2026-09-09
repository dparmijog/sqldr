//! Bottom status line: active connection, read-only flag, last message.

use ratatui::layout::Rect;
use ratatui::style::Style;
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
                Style::default().bg(app.theme.error).fg(ratatui::style::Color::White)
            } else {
                Style::default().fg(app.theme.success)
            };
            spans.push(Span::styled(format!(" {} [{state}] ", conn.entry.name), style));
        }
        None => spans.push(Span::raw(" no connection ")),
    }

    let focus_name = match app.focus {
        Focus::Sidebar => "sidebar",
        Focus::Editor => "editor",
        Focus::Results => "results",
    };
    spans.push(Span::raw(format!(" | focus: {focus_name}")));

    spans.push(Span::raw(" | "));
    match &app.status {
        StatusMessage::Idle => spans.push(Span::raw("ready")),
        StatusMessage::Running => spans.push(Span::styled("running…", Style::default().fg(app.theme.warning))),
        StatusMessage::Error(e) => spans.push(Span::styled(format!("error: {e}"), Style::default().fg(app.theme.error))),
        StatusMessage::Info(m) => spans.push(Span::raw(m.clone())),
    }

    spans.push(Span::raw(
        " | Tab: focus  f: favorite  e/d: edit/delete conn  s: structure  g: follow FK  Ctrl+Enter/F5: run  Ctrl+X/F6: explain  Ctrl+C: cancel  Ctrl+R: history  Ctrl+N: new connection  Ctrl+O: options  Ctrl+E: $EDITOR  q: quit",
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
