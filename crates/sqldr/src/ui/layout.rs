//! Fixed application layout: `sidebar | editor / resultados`, with a
//! one-line status bar pinned to the bottom.

use ratatui::layout::{Constraint, Layout, Rect};

pub struct Areas {
    pub sidebar: Rect,
    pub editor: Rect,
    pub results: Rect,
    pub status: Rect,
}

pub fn split(area: Rect) -> Areas {
    let [main, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

    let [sidebar, right] =
        Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)]).areas(main);

    let [editor, results] =
        Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).areas(right);

    Areas { sidebar, editor, results, status }
}
