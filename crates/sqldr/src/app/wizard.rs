//! The "add connection" wizard (`Ctrl+N`): engine → credentials → live
//! test → pick database.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sqldr_core::{ConnConfig, Driver, MySqlDriver};

use super::{App, AppEvent, ConnField, ConnState, ConnWizard, Engine, Focus, Overlay, StatusMessage, WizardStep};

static NEXT_WIZARD_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_wizard_request_id() -> u64 {
    NEXT_WIZARD_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl App {
    pub(super) fn on_wizard_key(&mut self, mut wizard: ConnWizard, key: KeyEvent) {
        let step = wizard.step.clone();
        match step {
            WizardStep::SelectEngine { selected } => match key.code {
                KeyCode::Esc => {
                    self.status = StatusMessage::Info("cancelled".into());
                }
                KeyCode::Up => {
                    wizard.step = WizardStep::SelectEngine { selected: selected.saturating_sub(1) };
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                KeyCode::Down => {
                    let selected = (selected + 1).min(Engine::ALL.len() - 1);
                    wizard.step = WizardStep::SelectEngine { selected };
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                KeyCode::Enter => {
                    wizard.engine = Engine::ALL[selected];
                    wizard.port = wizard.engine.default_port().to_string();
                    wizard.step = WizardStep::Details;
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                _ => {
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
            },
            WizardStep::Details => match (key.code, key.modifiers) {
                (KeyCode::Esc, _) => {
                    wizard.error = None;
                    wizard.step = WizardStep::SelectEngine { selected: 0 };
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                    self.start_connection_test(wizard);
                }
                (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Down, _) => {
                    wizard.field = wizard.field.next();
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                (KeyCode::BackTab, _) | (KeyCode::Up, _) => {
                    wizard.field = wizard.field.prev();
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                (KeyCode::Char(' '), _) | (KeyCode::Enter, _) if wizard.field == ConnField::ReadOnly => {
                    wizard.read_only = !wizard.read_only;
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                (KeyCode::Enter, _) => {
                    wizard.field = wizard.field.next();
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                (KeyCode::Backspace, _) => {
                    let field = wizard.field;
                    if let Some(s) = wizard.field_mut(field) {
                        s.pop();
                    }
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                (KeyCode::Char(c), _) => {
                    let field = wizard.field;
                    if let Some(s) = wizard.field_mut(field) {
                        s.push(c);
                    }
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                _ => {
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
            },
            WizardStep::Testing => {
                if key.code == KeyCode::Esc {
                    wizard.step = WizardStep::Details;
                }
                self.overlay = Some(Overlay::AddConnection(wizard));
            }
            WizardStep::SelectDatabase { databases, selected } => match key.code {
                KeyCode::Esc => {
                    wizard.step = WizardStep::Details;
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                KeyCode::Up => {
                    wizard.step =
                        WizardStep::SelectDatabase { databases, selected: selected.saturating_sub(1) };
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                KeyCode::Down => {
                    // Index 0 is the synthetic "no database" option.
                    let selected = (selected + 1).min(databases.len());
                    wizard.step = WizardStep::SelectDatabase { databases, selected };
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
                KeyCode::Enter => {
                    let database = if selected == 0 { None } else { databases.get(selected - 1).cloned() };
                    self.finalize_connection(wizard, database);
                }
                _ => {
                    wizard.step = WizardStep::SelectDatabase { databases, selected };
                    self.overlay = Some(Overlay::AddConnection(wizard));
                }
            },
        }
    }

    /// Validates the connection-details step, then tests the credentials
    /// against the real server in the background (connecting without a
    /// default database) so the next step can offer a live list of
    /// databases to pick from.
    fn start_connection_test(&mut self, mut wizard: ConnWizard) {
        let name = wizard.name.trim().to_string();
        if name.is_empty() {
            wizard.error = Some("name is required".into());
            self.overlay = Some(Overlay::AddConnection(wizard));
            return;
        }
        if self.conns.iter().any(|c| c.entry.name == name) {
            wizard.error = Some(format!("a connection named '{name}' already exists"));
            self.overlay = Some(Overlay::AddConnection(wizard));
            return;
        }
        let host = if wizard.host.trim().is_empty() { "127.0.0.1" } else { wizard.host.trim() }.to_string();
        let port_str = if wizard.port.trim().is_empty() {
            wizard.engine.default_port().to_string()
        } else {
            wizard.port.trim().to_string()
        };
        let Ok(port) = port_str.parse::<u16>() else {
            wizard.error = Some(format!("invalid port: '{port_str}'"));
            self.overlay = Some(Overlay::AddConnection(wizard));
            return;
        };
        let user = wizard.user.trim().to_string();

        let mut url = match url::Url::parse(&format!("mysql://{host}:{port}")) {
            Ok(u) => u,
            Err(e) => {
                wizard.error = Some(format!("invalid host/port: {e}"));
                self.overlay = Some(Overlay::AddConnection(wizard));
                return;
            }
        };
        if !user.is_empty() {
            let _ = url.set_username(&user);
        }
        if !wizard.password.is_empty() {
            let _ = url.set_password(Some(&wizard.password));
        }

        wizard.error = None;
        wizard.step = WizardStep::Testing;
        let request_id = next_wizard_request_id();
        wizard.request_id = request_id;

        let cfg = ConnConfig { name, url: url.to_string(), read_only: wizard.read_only };
        let tx = self.events.clone();
        tokio::spawn(async move {
            let result = match MySqlDriver::connect(&cfg).await {
                Ok(driver) => driver.list_databases().await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppEvent::WizardTested(request_id, result));
        });
        self.overlay = Some(Overlay::AddConnection(wizard));
    }

    /// Builds the final connection URL (host/port/user/database — no
    /// password), appends it to the config, persists it, and stores the
    /// password in the keyring, mirroring how every other connection here
    /// is set up.
    fn finalize_connection(&mut self, wizard: ConnWizard, database: Option<String>) {
        let name = wizard.name.trim().to_string();
        let host = if wizard.host.trim().is_empty() { "127.0.0.1" } else { wizard.host.trim() };
        let port_str = if wizard.port.trim().is_empty() {
            wizard.engine.default_port().to_string()
        } else {
            wizard.port.trim().to_string()
        };
        let user = wizard.user.trim();

        let mut url = match url::Url::parse(&format!("mysql://{host}:{port_str}")) {
            Ok(u) => u,
            Err(e) => {
                self.status = StatusMessage::Error(format!("invalid URL: {e}"));
                return;
            }
        };
        if !user.is_empty() {
            let _ = url.set_username(user);
        }
        if let Some(db) = &database {
            url.set_path(db);
        }

        let entry = crate::config::ConnEntry { name: name.clone(), url: url.to_string(), read_only: wizard.read_only };
        self.conns.push(ConnState::new(entry.clone()));

        let cfg = self.to_config();
        if let Err(e) = crate::config::save(&cfg) {
            self.status = StatusMessage::Error(format!("connection added but config.toml could not be saved: {e}"));
            return;
        }

        if !wizard.password.is_empty() {
            if let Err(e) = crate::config::set_password(&name, &wizard.password) {
                self.status = StatusMessage::Error(format!("connection saved, but the password could not be saved: {e}"));
                return;
            }
        }

        self.status = StatusMessage::Info(format!("connection '{name}' added"));
        self.focus = Focus::Sidebar;
    }
}
