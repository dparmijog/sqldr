mod editor;
mod layout;
mod overlay;
mod results;
mod sidebar;
mod statusbar;

use ratatui::Frame;

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let areas = layout::split(frame.area());
    sidebar::render(frame, app, areas.sidebar);
    editor::render(frame, app, areas.editor);
    results::render(frame, app, areas.results);
    statusbar::render(frame, app, areas.status);
    overlay::render(frame, app);
}
