//! SQL editor pane: thin wrapper over `tui-textarea`, with dialect-aware
//! syntax highlighting applied as a post-render color pass over the
//! buffer cells tui-textarea already drew — cursor, selection, and
//! placeholder rendering are all untouched, only foreground colors
//! change. Highlighting only lights up once connected: the keyword
//! vocabulary comes from the active connection's `Dialect`, so it's
//! inherently per-engine (MySQL today; a future Postgres/SQLite
//! connection highlights with its own vocabulary automatically).

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;
use sqldr_core::Driver;

use crate::app::{App, ConnStatus, Focus};
use crate::sql_highlight;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Editor;
    let border_color = if focused { app.theme.accent } else { app.theme.muted };
    let mut editor = app.editor.clone();
    editor.set_block(
        Block::default()
            .title("Editor (Ctrl+Enter to run)")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    );
    if !focused {
        editor.set_cursor_style(Style::default());
    }
    let inner = Block::default().borders(Borders::ALL).inner(area);
    frame.render_widget(&editor, area);

    if app.editor.is_empty() || inner.width == 0 || inner.height == 0 {
        // Placeholder text is rendered instead of real content; nothing
        // to tokenize.
        return;
    }

    let keywords = app
        .active_conn
        .and_then(|ci| app.conns.get(ci))
        .and_then(|c| match &c.status {
            ConnStatus::Connected(driver) => Some(driver.dialect().keywords()),
            _ => None,
        })
        .unwrap_or(&[]);
    if keywords.is_empty() {
        // No live connection yet — no dialect vocabulary to highlight
        // against. Highlighting lights up the moment one connects.
        return;
    }

    // `sqldr` renders a fresh `app.editor.clone()` every frame (see
    // above), and `TextArea::clone()` copies `Viewport`'s *current*
    // stored offset — but nothing ever writes that back to `app.editor`
    // itself (only the throwaway clone's copy gets updated, and it's
    // dropped at the end of this function). So `app.editor`'s viewport
    // never advances past its `Viewport::default()` baseline of `(0, 0)`,
    // and the real widget recomputes scroll from that frozen zero every
    // single render. Match that exactly — a persisted offset here would
    // silently drift from what's actually on screen.
    let (cursor_row, cursor_col) = app.editor.cursor();
    let height = inner.height;
    let width = inner.width;
    let top_row = sql_highlight::next_scroll_top(0, cursor_row as u16, height);
    let top_col = sql_highlight::next_scroll_top(0, cursor_col as u16, width);

    let theme = app.theme;
    let lines = app.editor.lines();
    let buf = frame.buffer_mut();
    for screen_row in 0..height {
        let Some(line) = lines.get((top_row + screen_row) as usize) else { break };
        let tokens = sql_highlight::tokenize(line, keywords);
        let mut col: u16 = 0;
        for tok in tokens {
            let tok_len = tok.text.chars().count() as u16;
            let start = col;
            let end = col + tok_len;
            col = end;

            let Some(style) = sql_highlight::style_for(tok.kind, theme) else { continue };
            let vis_start = start.max(top_col);
            let vis_end = end.min(top_col + width);
            if vis_start >= vis_end {
                continue;
            }
            for x in vis_start..vis_end {
                let screen_x = inner.x + (x - top_col);
                let screen_y = inner.y + screen_row;
                if let Some(cell) = buf.cell_mut((screen_x, screen_y)) {
                    cell.set_style(style);
                }
            }
        }
    }
}
