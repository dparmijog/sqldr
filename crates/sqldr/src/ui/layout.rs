//! Application layout: `sidebar | editor / resultados`, with a one-line
//! status bar pinned to the bottom. The sidebar width and editor height
//! are adjustable at runtime (mouse-drag resize), so callers pass them in
//! rather than relying on fixed constants.

use ratatui::layout::{Constraint, Layout, Rect};

pub struct Areas {
    pub sidebar: Rect,
    pub editor: Rect,
    pub results: Rect,
    pub status: Rect,
}

/// `sidebar_width_pct` is the sidebar's share of the total width (10-60,
/// clamped by the caller). `editor_height` is an absolute row count.
pub fn split(area: Rect, sidebar_width_pct: u16, editor_height: u16) -> Areas {
    let [main, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

    let [sidebar, right] = Layout::horizontal([
        Constraint::Percentage(sidebar_width_pct),
        Constraint::Percentage(100 - sidebar_width_pct),
    ])
    .areas(main);

    let [editor, results] =
        Layout::vertical([Constraint::Length(editor_height), Constraint::Min(0)]).areas(right);

    Areas { sidebar, editor, results, status }
}
