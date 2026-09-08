//! Modal overlays: history picker (`Ctrl+R`) and the DML-without-`WHERE`
//! confirmation dialog. Rendered on top of everything else, centered.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Overlay};

fn centered(width: u16, height_pct: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(20);
    let [area] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [area] = Layout::vertical([Constraint::Percentage(height_pct)]).flex(Flex::Center).areas(area);
    area
}

pub fn render(frame: &mut Frame, app: &App) {
    let Some(overlay) = &app.overlay else { return };
    match overlay {
        Overlay::History(picker) => render_history(frame, picker),
        Overlay::Confirm { message, .. } => render_confirm(frame, message),
    }
}

fn render_history(frame: &mut Frame, picker: &crate::app::HistoryPicker) {
    let area = centered(90, 70, frame.area());
    frame.render_widget(Clear, area);

    let title = format!("Historial — filtro: {}_", picker.filter);
    let matches = picker.filtered();
    let items: Vec<ListItem> = matches
        .iter()
        .map(|sql| ListItem::new(sql.replace('\n', " ⏎ ")))
        .collect();

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    if !matches.is_empty() {
        state.select(Some(picker.selected.min(matches.len() - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_confirm(frame: &mut Frame, message: &str) {
    let area = centered(70, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Confirmar")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let mut lines: Vec<Line> = message.lines().map(Line::from).collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Enter/y: ejecutar   cualquier otra tecla: cancelar",
        Style::default().add_modifier(Modifier::ITALIC),
    )));

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}
