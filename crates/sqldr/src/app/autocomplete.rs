//! Completion popup (`Ctrl+Space`/`F7` in the editor): a static,
//! prefix-filtered shortlist of the dialect's keywords plus the open
//! tab's table/column names — no live re-filtering while the popup is
//! open, matching the wizard's other static-list pickers.

use crossterm::event::{KeyCode, KeyEvent};
use tui_textarea::CursorMove;

use super::{App, ConnStatus, Overlay, StatusMessage, TablesState};

impl App {
    pub(super) fn open_autocomplete(&mut self) {
        let Some(ci) = self.active_conn else {
            self.status = StatusMessage::Error("no active connection: pick one in the sidebar".into());
            return;
        };
        let keywords: Vec<String> = match self.conns.get(ci).map(|c| &c.status) {
            Some(ConnStatus::Connected(driver)) => {
                driver.dialect().keywords().iter().map(|s| s.to_string()).collect()
            }
            _ => {
                self.status = StatusMessage::Error("connection not ready yet".into());
                return;
            }
        };

        let (row, col) = self.editor.cursor();
        let chars: Vec<char> = self.editor.lines()[row].chars().collect();
        let col = col.min(chars.len());
        let mut start = col;
        while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }
        let prefix: String = chars[start..col].iter().collect();
        let needle = prefix.to_ascii_lowercase();

        let mut candidates = keywords;
        if let Some(active) = self.active_tab {
            if let TablesState::Loaded(tables) = &self.tabs[active].tables {
                for table in tables {
                    candidates.push(table.name.clone());
                    for column in &table.columns {
                        candidates.push(column.name.clone());
                    }
                }
            }
        }
        candidates.sort();
        candidates.dedup();

        let mut matches: Vec<String> = candidates
            .into_iter()
            .filter(|c| needle.is_empty() || c.to_ascii_lowercase().starts_with(&needle))
            .collect();
        matches.sort_by_key(|c| c.to_ascii_lowercase());

        if matches.is_empty() {
            self.status = StatusMessage::Info(format!("no completions for '{prefix}'"));
            return;
        }

        self.overlay = Some(Overlay::Autocomplete {
            candidates: matches,
            selected: 0,
            anchor: (row, start),
            replace_len: col - start,
        });
    }

    /// Returns the `(candidates, selected)` to keep the popup open with,
    /// or `None` to close it — `anchor`/`replace_len` never change after
    /// opening, so the caller (`on_overlay_key`) keeps holding those.
    pub(super) fn on_autocomplete_key(
        &mut self,
        candidates: Vec<String>,
        mut selected: usize,
        anchor: (usize, usize),
        replace_len: usize,
        key: KeyEvent,
    ) -> Option<(Vec<String>, usize)> {
        match key.code {
            KeyCode::Esc => None,
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
                Some((candidates, selected))
            }
            KeyCode::Down => {
                selected = (selected + 1).min(candidates.len().saturating_sub(1));
                Some((candidates, selected))
            }
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(choice) = candidates.get(selected) {
                    self.editor.move_cursor(CursorMove::Jump(anchor.0 as u16, anchor.1 as u16));
                    self.editor.delete_str(replace_len);
                    self.editor.insert_str(choice);
                }
                None
            }
            _ => Some((candidates, selected)),
        }
    }
}
