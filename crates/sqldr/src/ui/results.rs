//! Results pane: a scrollable data grid of the most recent query's rows,
//! with a cell cursor for `y`/`Y`/`c`/`i` copy actions. Columns auto-size
//! to their widest value (see `App::results.col_widths`) and are divided
//! by a themed rule, alternating rows are dimmed for readability.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Row as UiRow, Table};
use ratatui::Frame;

use crate::app::{App, Focus};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Results;
    let page_info = app.results.pagination.as_ref().map(|p| format!(" — page {} (PgUp/PgDn)", p.page + 1));
    let title = if app.results.running {
        format!("Results ({} rows, running…){}", app.results.rows.len(), page_info.unwrap_or_default())
    } else {
        format!(
            "Results ({} rows){} — y/Y/c/i: copy cell/row JSON/CSV/INSERT",
            app.results.rows.len(),
            page_info.unwrap_or_default()
        )
    };
    let theme = app.theme;
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { theme.accent } else { theme.muted }));

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

    let sep_style = Style::default().fg(theme.muted);
    let header = UiRow::new(
        app.results
            .cols
            .iter()
            .enumerate()
            .map(|(i, c)| grid_cell(i, c.clone(), Style::default().add_modifier(Modifier::BOLD), sep_style))
            .collect::<Vec<_>>(),
    );

    let start = app.results.scroll_top.min(app.results.rows.len());
    let end = (start + visible_height.max(1)).min(app.results.rows.len());

    let cursor_row = app.results.cursor_row;
    let cursor_col = app.results.cursor_col;
    let rows = app.results.rows[start..end].iter().enumerate().map(|(offset, row)| {
        let absolute = start + offset;
        // Alternating rows are subtly dimmed — a real grid look without
        // needing a dedicated "stripe" color per theme.
        let zebra = absolute % 2 == 1;
        UiRow::new(
            row.values
                .iter()
                .enumerate()
                .map(|(col_idx, v)| {
                    let text = v.to_string();
                    let style = if focused && absolute == cursor_row && col_idx == cursor_col {
                        Style::default().fg(theme.accent).add_modifier(Modifier::REVERSED)
                    } else if zebra {
                        Style::default().add_modifier(Modifier::DIM)
                    } else {
                        Style::default()
                    };
                    grid_cell(col_idx, text, style, sep_style)
                })
                .collect::<Vec<_>>(),
        )
    });

    let widths: Vec<Constraint> = app
        .results
        .col_widths
        .iter()
        .enumerate()
        .map(|(i, w)| Constraint::Length(if i == 0 { *w } else { w + 2 }))
        .collect();

    let table = Table::new(rows, widths).header(header).block(block);
    frame.render_widget(table, area);
}

/// Builds one grid cell: every column past the first is prefixed with a
/// themed "│ " divider so adjacent columns read as a real table instead of
/// loosely-aligned text.
fn grid_cell(col_idx: usize, text: String, value_style: Style, sep_style: Style) -> Cell<'static> {
    if col_idx == 0 {
        Cell::from(Span::styled(text, value_style))
    } else {
        Cell::from(Line::from(vec![Span::styled("│ ", sep_style), Span::styled(text, value_style)]))
    }
}
