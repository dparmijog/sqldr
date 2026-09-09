//! Options dialog (`Ctrl+O`): pick a color theme, previewed live while
//! browsing and persisted to `config.toml` on confirm.

use crossterm::event::{KeyCode, KeyEvent};

use crate::theme::Theme;

use super::{App, Overlay, StatusMessage};

impl App {
    pub(super) fn on_settings_key(&mut self, mut selected: usize, original: Theme, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.theme = original;
                self.status = StatusMessage::Info("cancelado".into());
            }
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.theme = Theme::ALL[selected];
                self.overlay = Some(Overlay::Settings { selected, original });
            }
            KeyCode::Down => {
                selected = (selected + 1).min(Theme::ALL.len() - 1);
                self.theme = Theme::ALL[selected];
                self.overlay = Some(Overlay::Settings { selected, original });
            }
            KeyCode::Enter => {
                let cfg = self.to_config();
                self.status = match crate::config::save(&cfg) {
                    Ok(()) => StatusMessage::Info(format!("tema '{}' guardado", self.theme.name)),
                    Err(e) => StatusMessage::Error(format!("no se pudo guardar el tema: {e}")),
                };
            }
            _ => {
                self.overlay = Some(Overlay::Settings { selected, original });
            }
        }
    }
}
