//! Results pane: a scrollable table of the most recent query's rows, with
//! a cell cursor for `y`/`Y`/`c`/`i` copy actions.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row as UiRow, Table};
use ratatui::Frame;

use crate::app::{App, Focus};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Results;
    let title = if app.results.running {
        format!("Resultados ({} filas, ejecutando…)", app.results.rows.len())
    } else {
        format!("Resultados ({} filas) — y/Y/c/i: copiar celda/fila JSON/CSV/INSERT", app.results.rows.len())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if focused { Style::default().fg(Color::Cyan) } else { Style::default() });

    if app.results.cols.is_empty() {
        frame.render_widget(block, area);
        return;
    }

    // Keep the cursor row inside the visible window, scrolling the minimum
    // amount necessary rather than re-centering every move.
    let visible_height = area.height.saturating_sub(3) as usize; // borders + header
    if !app.results.rows.is_empty() {
        let cursor = app.results.cursor_row.min(app.results.rows.len() - 1);
        if cursor < app.results.scroll_top {
            app.results.scroll_top = cursor;
        } else if visible_height > 0 && cursor >= app.results.scroll_top + visible_height {
            app.results.scroll_top = cursor + 1 - visible_height;
        }
    }

    let header = UiRow::new(app.results.cols.clone()).style(Style::default().add_modifier(Modifier::BOLD));

    let start = app.results.scroll_top.min(app.results.rows.len());
    let end = (start + visible_height.max(1)).min(app.results.rows.len());

    let cursor_row = app.results.cursor_row;
    let cursor_col = app.results.cursor_col;
    let rows = app.results.rows[start..end].iter().enumerate().map(|(offset, row)| {
        let absolute = start + offset;
        UiRow::new(row.values.iter().enumerate().map(|(col_idx, v)| {
            let text = v.to_string();
            if focused && absolute == cursor_row && col_idx == cursor_col {
                Cell::from(text).style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                Cell::from(text)
            }
        }))
    });

    let widths: Vec<Constraint> = app.results.cols.iter().map(|_| Constraint::Min(8)).collect();

    let table = Table::new(rows, widths).header(header).block(block);
    frame.render_widget(table, area);
}
