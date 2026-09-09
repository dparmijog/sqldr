//! SQL editor pane: thin wrapper over `tui-textarea`.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::app::{App, Focus};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Editor;
    let border_color = if focused { app.theme.accent } else { app.theme.muted };
    let mut editor = app.editor.clone();
    editor.set_block(
        Block::default()
            .title("Editor (Ctrl+Enter ejecuta)")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    );
    if !focused {
        editor.set_cursor_style(Style::default());
    }
    frame.render_widget(&editor, area);
}
