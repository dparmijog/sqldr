//! Results pane: a scrollable table of the most recent query's rows.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Row as UiRow, Table};
use ratatui::Frame;

use crate::app::{App, Focus};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Results;
    let title = if app.results.running {
        format!("Resultados ({} filas, ejecutando…)", app.results.rows.len())
    } else {
        format!("Resultados ({} filas)", app.results.rows.len())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if focused { Style::default().fg(Color::Cyan) } else { Style::default() });

    if app.results.cols.is_empty() {
        frame.render_widget(block, area);
        return;
    }

    let header = UiRow::new(app.results.cols.clone()).style(Style::default().add_modifier(Modifier::BOLD));

    let visible_height = area.height.saturating_sub(3) as usize; // borders + header
    let start = app.results.scroll.min(app.results.rows.len().saturating_sub(1));
    let end = (start + visible_height.max(1)).min(app.results.rows.len());

    let rows = app.results.rows[start..end]
        .iter()
        .map(|row| UiRow::new(row.values.iter().map(|v| v.to_string()).collect::<Vec<_>>()));

    let widths: Vec<Constraint> =
        app.results.cols.iter().map(|_| Constraint::Min(8)).collect();

    let table = Table::new(rows, widths).header(header).block(block);
    frame.render_widget(table, area);
}
