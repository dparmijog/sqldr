//! The "add connection" wizard (`Ctrl+N`): engine → credentials → live
//! test → pick database.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sqldr_core::ConnConfig;

use super::sidebar::SidebarNode;
use super::{App, AppEvent, ConnState, Focus, Overlay, StatusMessage};

/// Database engine offered by the "add connection" wizard. Only MySQL is
/// implemented today; the roadmap adds Postgres and SQLite as more
/// `Driver` impls land, at which point they join `Engine::ALL`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    MySql,
}

impl Engine {
    pub const ALL: [Engine; 1] = [Engine::MySql];

    pub fn label(self) -> &'static str {
        match self {
            Engine::MySql => "MySQL",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Engine::MySql => 3306,
        }
    }
}

/// Which field of the connection-details form currently has input focus.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnField {
    Name,
    Host,
    Port,
    User,
    Password,
    ReadOnly,
}

impl ConnField {
    const ORDER: [ConnField; 6] = [
        ConnField::Name,
        ConnField::Host,
        ConnField::Port,
        ConnField::User,
        ConnField::Password,
        ConnField::ReadOnly,
    ];

    fn next(self) -> Self {
        let idx = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(idx + 1) % Self::ORDER.len()]
    }

    fn prev(self) -> Self {
        let idx = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(idx + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// Where the "add connection" wizard currently is. Mirrors the flow the
/// user asked for: pick an engine, fill in host/credentials, test them
/// live against the server, then pick a database from what's actually
/// there — rather than typing a database name blind.
#[derive(Clone)]
pub enum WizardStep {
    SelectEngine { selected: usize },
    Details,
    Testing,
    SelectDatabase { databases: Vec<String>, selected: usize },
}

/// Form state for the "add connection" modal (`Ctrl+N`).
pub struct ConnWizard {
    pub step: WizardStep,
    pub engine: Engine,
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub read_only: bool,
    pub field: ConnField,
    pub error: Option<String>,
    /// Identifies which background connection test this wizard is waiting
    /// on, so a stale result (e.g. after the user cancelled and reopened
    /// the wizard) is silently dropped instead of clobbering fresh state.
    pub(super) request_id: u64,
    /// `Some(idx)` when this wizard is editing the connection at `conns[idx]`
    /// (opened via `e` on a connection node) rather than adding a new one;
    /// `finalize_connection` replaces that entry in place instead of
    /// pushing a new one.
    edit_target: Option<usize>,
}

impl ConnWizard {
    pub(super) fn new() -> Self {
        let engine = Engine::ALL[0];
        ConnWizard {
            step: WizardStep::SelectEngine { selected: 0 },
            engine,
            name: String::new(),
            host: "127.0.0.1".to_string(),
            port: engine.default_port().to_string(),
            user: String::new(),
            password: String::new(),
            read_only: false,
            field: ConnField::Name,
            error: None,
            request_id: 0,
            edit_target: None,
        }
    }

    /// Pre-fills the details step from an existing connection's URL
    /// (host/port/user; the password field is left blank — leaving it
    /// blank on save keeps whatever is already stored in the keyring).
    /// Skips the engine picker since only one engine exists to pick from.
    fn for_edit(idx: usize, entry: &crate::config::ConnEntry) -> Self {
        let engine = Engine::ALL[0];
        let parsed = url::Url::parse(&entry.url).ok();
        let host = parsed.as_ref().and_then(|u| u.host_str()).unwrap_or("127.0.0.1").to_string();
        let port = parsed.as_ref().and_then(|u| u.port()).unwrap_or(engine.default_port()).to_string();
        let user = parsed.as_ref().map(|u| u.username().to_string()).unwrap_or_default();
        ConnWizard {
            step: WizardStep::Details,
            engine,
            name: entry.name.clone(),
            host,
            port,
            user,
            password: String::new(),
            read_only: entry.read_only,
            field: ConnField::Name,
            error: None,
            request_id: 0,
            edit_target: Some(idx),
        }
    }

    fn field_mut(&mut self, field: ConnField) -> Option<&mut String> {
        match field {
            ConnField::Name => Some(&mut self.name),
            ConnField::Host => Some(&mut self.host),
            ConnField::Port => Some(&mut self.port),
            ConnField::User => Some(&mut self.user),
            ConnField::Password => Some(&mut self.password),
            ConnField::ReadOnly => None,
        }
    }
}

static NEXT_WIZARD_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_wizard_request_id() -> u64 {
    NEXT_WIZARD_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl App {
    /// Returns the (possibly updated) wizard to keep the overlay open
    /// with, or `None` to close it — `start_connection_test`/
    /// `finalize_connection` follow the same contract.
    pub(super) fn on_wizard_key(&mut self, mut wizard: ConnWizard, key: KeyEvent) -> Option<ConnWizard> {
        let step = wizard.step.clone();
        match step {
            WizardStep::SelectEngine { selected } => match key.code {
                KeyCode::Esc => {
                    self.status = StatusMessage::Info("cancelled".into());
                    None
                }
                KeyCode::Up => {
                    wizard.step = WizardStep::SelectEngine { selected: selected.saturating_sub(1) };
                    Some(wizard)
                }
                KeyCode::Down => {
                    let selected = (selected + 1).min(Engine::ALL.len() - 1);
                    wizard.step = WizardStep::SelectEngine { selected };
                    Some(wizard)
                }
                KeyCode::Enter => {
                    wizard.engine = Engine::ALL[selected];
                    wizard.port = wizard.engine.default_port().to_string();
                    wizard.step = WizardStep::Details;
                    Some(wizard)
                }
                _ => Some(wizard),
            },
            WizardStep::Details => match (key.code, key.modifiers) {
                (KeyCode::Esc, _) => {
                    wizard.error = None;
                    wizard.step = WizardStep::SelectEngine { selected: 0 };
                    Some(wizard)
                }
                (KeyCode::Char('s'), KeyModifiers::CONTROL) => self.start_connection_test(wizard),
                (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Down, _) => {
                    wizard.field = wizard.field.next();
                    Some(wizard)
                }
                (KeyCode::BackTab, _) | (KeyCode::Up, _) => {
                    wizard.field = wizard.field.prev();
                    Some(wizard)
                }
                (KeyCode::Char(' '), _) | (KeyCode::Enter, _) if wizard.field == ConnField::ReadOnly => {
                    wizard.read_only = !wizard.read_only;
                    Some(wizard)
                }
                (KeyCode::Enter, _) => {
                    wizard.field = wizard.field.next();
                    Some(wizard)
                }
                (KeyCode::Backspace, _) => {
                    let field = wizard.field;
                    if let Some(s) = wizard.field_mut(field) {
                        s.pop();
                    }
                    Some(wizard)
                }
                (KeyCode::Char(c), _) => {
                    let field = wizard.field;
                    if let Some(s) = wizard.field_mut(field) {
                        s.push(c);
                    }
                    Some(wizard)
                }
                _ => Some(wizard),
            },
            WizardStep::Testing => {
                if key.code == KeyCode::Esc {
                    wizard.step = WizardStep::Details;
                }
                Some(wizard)
            }
            WizardStep::SelectDatabase { databases, selected } => match key.code {
                KeyCode::Esc => {
                    wizard.step = WizardStep::Details;
                    Some(wizard)
                }
                KeyCode::Up => {
                    wizard.step =
                        WizardStep::SelectDatabase { databases, selected: selected.saturating_sub(1) };
                    Some(wizard)
                }
                KeyCode::Down => {
                    // Index 0 is the synthetic "no database" option.
                    let selected = (selected + 1).min(databases.len());
                    wizard.step = WizardStep::SelectDatabase { databases, selected };
                    Some(wizard)
                }
                KeyCode::Enter => {
                    let database = if selected == 0 { None } else { databases.get(selected - 1).cloned() };
                    self.finalize_connection(wizard, database);
                    None
                }
                _ => {
                    wizard.step = WizardStep::SelectDatabase { databases, selected };
                    Some(wizard)
                }
            },
        }
    }

    /// Validates the connection-details step, then tests the credentials
    /// against the real server in the background (connecting without a
    /// default database) so the next step can offer a live list of
    /// databases to pick from.
    fn start_connection_test(&mut self, mut wizard: ConnWizard) -> Option<ConnWizard> {
        let name = wizard.name.trim().to_string();
        if name.is_empty() {
            wizard.error = Some("name is required".into());
            return Some(wizard);
        }
        if self.conn.conns.iter().enumerate().any(|(i, c)| c.entry.name == name && Some(i) != wizard.edit_target) {
            wizard.error = Some(format!("a connection named '{name}' already exists"));
            return Some(wizard);
        }
        let host = if wizard.host.trim().is_empty() { "127.0.0.1" } else { wizard.host.trim() }.to_string();
        let port_str = if wizard.port.trim().is_empty() {
            wizard.engine.default_port().to_string()
        } else {
            wizard.port.trim().to_string()
        };
        let Ok(port) = port_str.parse::<u16>() else {
            wizard.error = Some(format!("invalid port: '{port_str}'"));
            return Some(wizard);
        };
        let user = wizard.user.trim().to_string();

        let mut url = match url::Url::parse(&format!("mysql://{host}:{port}")) {
            Ok(u) => u,
            Err(e) => {
                wizard.error = Some(format!("invalid host/port: {e}"));
                return Some(wizard);
            }
        };
        if !user.is_empty() {
            let _ = url.set_username(&user);
        }
        // Editing with the password field left blank means "keep what's
        // already stored" — but the live test still needs a real password
        // to authenticate with, so fall back to whatever's in the keyring
        // under the connection's current (pre-edit) name.
        let password_for_test = if !wizard.password.is_empty() {
            Some(wizard.password.clone())
        } else {
            wizard
                .edit_target
                .and_then(|idx| self.conn.conns.get(idx))
                .and_then(|c| crate::config::get_password(&c.entry.name).ok().flatten())
        };
        if let Some(pw) = &password_for_test {
            let _ = url.set_password(Some(pw));
        }

        wizard.error = None;
        wizard.step = WizardStep::Testing;
        let request_id = next_wizard_request_id();
        wizard.request_id = request_id;

        let cfg = ConnConfig { name, url: url.to_string(), read_only: wizard.read_only };
        self.spawn_into_event(
            async move {
                match sqldr_core::connect(&cfg).await {
                    Ok(driver) => driver.list_databases().await.map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                }
            },
            move |result| AppEvent::WizardTested(request_id, result),
        );
        Some(wizard)
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

        match wizard.edit_target {
            Some(idx) => {
                let old_name = self.conn.conns[idx].entry.name.clone();
                if let Some(cancel) = self.conn.conns[idx].heartbeat_cancel.take() {
                    cancel.cancel();
                }
                self.conn.conns[idx].entry = entry;
                // The old driver/schema no longer match these credentials
                // (host/user/password/database may all have changed) —
                // reset so the next expand reconnects from scratch.
                self.conn.conns[idx].status = super::ConnStatus::Idle;
                self.conn.conns[idx].schema = None;
                self.conn.conns[idx].expanded = false;

                // Renamed with no fresh password typed: migrate whatever
                // was already stored, or `resolve_url` would look it up
                // under the new name and find nothing.
                if old_name != name && wizard.password.is_empty() {
                    if let Ok(Some(pw)) = crate::config::get_password(&old_name) {
                        let _ = crate::config::set_password(&name, &pw);
                    }
                }
            }
            None => {
                self.conn.conns.push(ConnState::new(entry));
            }
        }

        let cfg = self.to_config();
        if let Err(e) = crate::config::save(&cfg) {
            self.status = StatusMessage::Error(format!("connection saved but config.toml could not be saved: {e}"));
            return;
        }

        if !wizard.password.is_empty() {
            if let Err(e) = crate::config::set_password(&name, &wizard.password) {
                self.status = StatusMessage::Error(format!("connection saved, but the password could not be saved: {e}"));
                return;
            }
        }

        self.status = StatusMessage::Info(format!(
            "connection '{name}' {}",
            if wizard.edit_target.is_some() { "updated" } else { "added" }
        ));
        self.focus = Focus::Sidebar;
    }

    /// Opens the wizard pre-filled for the connection currently under the
    /// sidebar cursor (tree mode only). No-op on any other node — editing
    /// a favorite/database row doesn't make sense here.
    pub(super) fn edit_selected_connection(&mut self) {
        let nodes = self.sidebar_nodes();
        let Some(SidebarNode::Connection(ci)) = nodes.get(self.sidebar_cursor).copied() else {
            return;
        };
        let entry = self.conn.conns[ci].entry.clone();
        self.overlay = Some(Overlay::AddConnection(ConnWizard::for_edit(ci, &entry)));
    }

    /// Stages a confirmation for removing the connection under the
    /// sidebar cursor. No-op on any other node.
    pub(super) fn request_delete_selected_connection(&mut self) {
        let nodes = self.sidebar_nodes();
        let Some(SidebarNode::Connection(ci)) = nodes.get(self.sidebar_cursor).copied() else {
            return;
        };
        let name = self.conn.conns[ci].entry.name.clone();
        self.overlay = Some(Overlay::ConfirmDeleteConnection { conn_idx: ci, name });
    }

    pub(super) fn on_confirm_delete_key(&mut self, conn_idx: usize, name: String, key: KeyEvent) {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.delete_connection(conn_idx, &name);
            }
            _ => {
                self.status = StatusMessage::Info("cancelled".into());
            }
        }
    }

    /// Removes a connection entirely: config entry, stored keyring
    /// password, any open tabs backed by it, and its background
    /// heartbeat loop.
    fn delete_connection(&mut self, idx: usize, name: &str) {
        if idx >= self.conn.conns.len() {
            return;
        }
        if let Some(cancel) = self.conn.conns[idx].heartbeat_cancel.take() {
            cancel.cancel();
        }
        self.conn.conns.remove(idx);

        // Every connection after `idx` just shifted down one slot, but
        // its heartbeat task (if any) already captured its *old* index
        // by value at spawn time — left alone, it would keep firing
        // Heartbeat* events tagged with an index that now names a
        // different connection. Respawn each one so its events carry the
        // corrected index (`start_heartbeat` cancels the stale token).
        for i in idx..self.conn.conns.len() {
            if let super::ConnStatus::Connected(driver) = &self.conn.conns[i].status {
                let driver = std::sync::Arc::clone(driver);
                self.start_heartbeat(i, driver);
            }
        }

        // Reindexing every open tab precisely across a removed connection
        // is more complexity than this buys; falling back to the
        // connection tree is always safe and the tabs reopen in a click.
        self.conn.active_tab = None;
        self.conn.tabs.retain(|t| t.conn_idx != idx);
        for t in self.conn.tabs.iter_mut() {
            if t.conn_idx > idx {
                t.conn_idx -= 1;
            }
        }
        self.conn.active_conn = match self.conn.active_conn {
            Some(c) if c == idx => None,
            Some(c) if c > idx => Some(c - 1),
            other => other,
        };
        self.sidebar_cursor = 0;
        self.sidebar_scroll_top = 0;

        if let Err(e) = crate::config::delete_password(name) {
            self.status =
                StatusMessage::Error(format!("connection '{name}' removed, but the stored password could not be deleted: {e}"));
        }

        let cfg = self.to_config();
        if let Err(e) = crate::config::save(&cfg) {
            self.status = StatusMessage::Error(format!("connection '{name}' removed, but config.toml could not be saved: {e}"));
            return;
        }
        self.status = StatusMessage::Info(format!("connection '{name}' removed"));
    }
}
