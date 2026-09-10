//! Mouse handling: click-to-focus/select panes and drag-to-resize the
//! sidebar/editor borders.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::{point_in, App, Focus};

/// Which border is currently being mouse-dragged to resize a pane.
pub(super) enum Drag {
    SidebarBorder,
    EditorBorder,
}

impl App {
    pub(super) fn on_mouse(&mut self, mouse: MouseEvent) {
        // Modals own all input while open; clicking through them onto the
        // pane underneath would be confusing.
        if self.overlay.is_some() {
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(mouse.column, mouse.row),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(mouse.column, mouse.row),
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            _ => {}
        }
    }

    fn mouse_down(&mut self, x: u16, y: u16) {
        let areas = crate::ui::layout::split(self.last_area, self.sidebar_width_pct, self.editor_height);

        // Resize handles: a 2-cell-wide band straddling each border, wide
        // enough to grab without needing pixel-perfect clicks.
        let sidebar_border = areas.sidebar.x + areas.sidebar.width;
        let near_sidebar_border = x + 1 >= sidebar_border
            && x <= sidebar_border + 1
            && y >= areas.sidebar.y
            && y < areas.sidebar.y + areas.sidebar.height;
        if near_sidebar_border {
            self.drag = Some(Drag::SidebarBorder);
            return;
        }

        let editor_border = areas.editor.y + areas.editor.height;
        let near_editor_border = y + 1 >= editor_border
            && y <= editor_border + 1
            && x >= areas.editor.x
            && x < areas.editor.x + areas.editor.width;
        if near_editor_border {
            self.drag = Some(Drag::EditorBorder);
            return;
        }

        if point_in(areas.sidebar, x, y) {
            self.focus = Focus::Sidebar;
            if self.sidebar_filter.is_none() {
                if let Some(active) = self.conn.active_tab {
                    let (ci, di) = {
                        let tab = &self.conn.tabs[active];
                        (tab.conn_idx, tab.db_idx)
                    };
                    let table_count = self.active_tab_table_count();
                    if table_count > 0 {
                        let clicked =
                            self.sidebar_scroll_top + y.saturating_sub(areas.sidebar.y + 1) as usize;
                        let ti = clicked.min(table_count - 1);
                        self.preview_table(ci, di, ti);
                    }
                } else {
                    let nodes = self.sidebar_nodes();
                    if !nodes.is_empty() {
                        // Row 0 of the pane's content area is the border; row
                        // 1 is the first list item.
                        let clicked =
                            self.sidebar_scroll_top + y.saturating_sub(areas.sidebar.y + 1) as usize;
                        self.sidebar_cursor = clicked.min(nodes.len() - 1);
                        self.activate_sidebar_node();
                    }
                }
            }
            return;
        }

        if point_in(areas.editor, x, y) {
            self.focus = Focus::Editor;
            return;
        }

        if point_in(areas.results, x, y) {
            self.focus = Focus::Results;
            if !self.query.results.rows.is_empty() {
                // Border + header row precede the data rows.
                let clicked = y.saturating_sub(areas.results.y + 2) as usize;
                self.query.results.cursor_row =
                    (self.query.results.scroll_top + clicked).min(self.query.results.rows.len() - 1);
            }
        }
    }

    fn mouse_drag(&mut self, x: u16, y: u16) {
        let areas = crate::ui::layout::split(self.last_area, self.sidebar_width_pct, self.editor_height);
        match self.drag {
            Some(Drag::SidebarBorder) => {
                if self.last_area.width > 0 {
                    let pct = (x.saturating_sub(self.last_area.x) as u32 * 100
                        / self.last_area.width as u32) as u16;
                    self.sidebar_width_pct = pct.clamp(10, 60);
                }
            }
            Some(Drag::EditorBorder) => {
                let height = y.saturating_sub(areas.editor.y).max(3);
                self.editor_height = height.min(self.last_area.height.saturating_sub(6));
            }
            None => {}
        }
    }
}
