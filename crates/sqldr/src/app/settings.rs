//! Options dialog (`Ctrl+O`): pick a color theme, previewed live while
//! browsing and persisted to `config.toml` on confirm.

use crossterm::event::{KeyCode, KeyEvent};

use crate::theme::Theme;

use super::{App, StatusMessage};

impl App {
    /// Returns the (possibly updated) `(selected, original)` state to
    /// keep the dialog open with, or `None` to close it — the caller
    /// (`on_overlay_key`) reinserts the overlay, so there's no way to
    /// forget to and silently drop the dialog mid-browse.
    pub(super) fn on_settings_key(
        &mut self,
        mut selected: usize,
        original: Theme,
        key: KeyEvent,
    ) -> Option<(usize, Theme)> {
        match key.code {
            KeyCode::Esc => {
                self.theme = original;
                self.status = StatusMessage::Info("cancelled".into());
                None
            }
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.theme = Theme::ALL[selected];
                Some((selected, original))
            }
            KeyCode::Down => {
                selected = (selected + 1).min(Theme::ALL.len() - 1);
                self.theme = Theme::ALL[selected];
                Some((selected, original))
            }
            KeyCode::Enter => {
                let cfg = self.to_config();
                self.status = match crate::config::save(&cfg) {
                    Ok(()) => StatusMessage::Info(format!("theme '{}' saved", self.theme.name)),
                    Err(e) => StatusMessage::Error(format!("could not save theme: {e}")),
                };
                None
            }
            _ => Some((selected, original)),
        }
    }
}
