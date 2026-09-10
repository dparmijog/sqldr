//! Modal overlays: history picker (`Ctrl+R`), the connection wizard
//! (`Ctrl+N`), the DML-without-`WHERE` confirmation dialog, and the
//! options dialog (`Ctrl+O`). Rendered on top of everything else, centered.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use sqldr_core::Table;

use crate::app::{App, ConnField, ConnWizard, Engine, Overlay, WizardStep};
use crate::theme::Theme;

fn centered(width: u16, height_pct: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(20);
    let [area] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [area] = Layout::vertical([Constraint::Percentage(height_pct)]).flex(Flex::Center).areas(area);
    area
}

/// Selection highlight shared by every list in the overlays: reversed
/// video tinted with the theme's accent, so it reads clearly regardless of
/// the surrounding border color.
fn highlight_style(theme: Theme) -> Style {
    Style::default().fg(theme.accent).add_modifier(Modifier::REVERSED)
}

pub fn render(frame: &mut Frame, app: &App) {
    let Some(overlay) = &app.overlay else { return };
    let theme = app.theme;
    match overlay {
        Overlay::History(picker) => render_history(frame, theme, picker),
        Overlay::Confirm { message, .. } => render_confirm(frame, theme, message),
        Overlay::AddConnection(form) => render_add_connection(frame, theme, form),
        Overlay::Settings { selected, .. } => render_settings(frame, theme, *selected),
        Overlay::ConfirmDeleteConnection { name, .. } => render_confirm_delete(frame, theme, name),
        Overlay::TableStructure { db_name, table } => render_table_structure(frame, theme, db_name, table),
        Overlay::Autocomplete { candidates, selected, .. } => {
            render_autocomplete(frame, theme, candidates, *selected)
        }
    }
}

fn render_history(frame: &mut Frame, theme: Theme, picker: &crate::app::HistoryPicker) {
    let area = centered(90, 70, frame.area());
    frame.render_widget(Clear, area);

    let title = format!("History — filter: {}_", picker.filter);
    let matches = picker.filtered();
    let items: Vec<ListItem> = matches
        .iter()
        .map(|sql| ListItem::new(sql.replace('\n', " ⏎ ")))
        .collect();

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning));

    let list = List::new(items).block(block).highlight_style(highlight_style(theme));

    let mut state = ListState::default();
    if !matches.is_empty() {
        state.select(Some(picker.selected.min(matches.len() - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_confirm(frame: &mut Frame, theme: Theme, message: &str) {
    let area = centered(70, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Confirm")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error));

    let mut lines: Vec<Line> = message.lines().map(Line::from).collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Enter/y: run   any other key: cancel",
        Style::default().add_modifier(Modifier::ITALIC),
    )));

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_add_connection(frame: &mut Frame, theme: Theme, wizard: &ConnWizard) {
    match &wizard.step {
        WizardStep::SelectEngine { selected } => render_select_engine(frame, theme, *selected),
        WizardStep::Details => render_connection_details(frame, theme, wizard),
        WizardStep::Testing => render_testing(frame, theme, wizard),
        WizardStep::SelectDatabase { databases, selected } => {
            render_select_database(frame, theme, databases, *selected)
        }
    }
}

fn render_select_engine(frame: &mut Frame, theme: Theme, selected: usize) {
    let area = centered(50, 40, frame.area());
    frame.render_widget(Clear, area);

    let items: Vec<ListItem> = Engine::ALL.iter().map(|e| ListItem::new(e.label())).collect();
    let block = Block::default()
        .title("New connection — pick the engine (Esc: cancel)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let list = List::new(items).block(block).highlight_style(highlight_style(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(Engine::ALL.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_connection_details(frame: &mut Frame, theme: Theme, wizard: &ConnWizard) {
    let area = centered(64, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(
            "New connection: {} (Ctrl+S: test and continue, Esc: back)",
            wizard.engine.label()
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let field_style = |field: ConnField| {
        if wizard.field == field { highlight_style(theme) } else { Style::default() }
    };
    let text_line = |label: &str, value: &str, field: ConnField, mask: bool| {
        let shown = if mask { "*".repeat(value.chars().count()) } else { value.to_string() };
        Line::from(vec![
            Span::raw(format!("{label:<10}")),
            Span::styled(format!("{shown}_"), field_style(field)),
        ])
    };

    let mut lines = vec![
        text_line("Name", &wizard.name, ConnField::Name, false),
        text_line("Host", &wizard.host, ConnField::Host, false),
        text_line("Port", &wizard.port, ConnField::Port, false),
        text_line("User", &wizard.user, ConnField::User, false),
        text_line("Password", &wizard.password, ConnField::Password, true),
        Line::from(vec![
            Span::raw(format!("{:<10}", "Read-only")),
            Span::styled(
                if wizard.read_only { "[x] (space to toggle)" } else { "[ ] (space to toggle)" },
                field_style(ConnField::ReadOnly),
            ),
        ]),
        Line::default(),
        Line::from("Tab/↓: next field   Shift+Tab/↑: previous"),
        Line::from("The database is chosen after testing the connection."),
    ];
    if let Some(err) = &wizard.error {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(err.as_str(), Style::default().fg(theme.error))));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_testing(frame: &mut Frame, theme: Theme, wizard: &ConnWizard) {
    let area = centered(60, 30, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Testing connection (Esc: cancel)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning));

    let host = if wizard.host.trim().is_empty() { "127.0.0.1" } else { wizard.host.trim() };
    let message = format!(
        "Connecting to {}@{}:{} ({})…",
        if wizard.user.trim().is_empty() { "(no user)" } else { wizard.user.trim() },
        host,
        wizard.port,
        wizard.engine.label()
    );
    let paragraph = Paragraph::new(message).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_select_database(frame: &mut Frame, theme: Theme, databases: &[String], selected: usize) {
    let area = centered(60, 60, frame.area());
    frame.render_widget(Clear, area);

    let mut items = vec![ListItem::new("(no default database)")];
    items.extend(databases.iter().map(|db| ListItem::new(db.as_str())));

    let block = Block::default()
        .title("Pick a database (Enter: save, Esc: back)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let list = List::new(items).block(block).highlight_style(highlight_style(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(databases.len())));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The `Ctrl+O` options dialog: a theme picker. Arrowing through the list
/// previews each theme immediately (the border/highlight colors you're
/// looking at right now belong to the currently-selected entry).
fn render_settings(frame: &mut Frame, theme: Theme, selected: usize) {
    let area = centered(46, 45, frame.area());
    frame.render_widget(Clear, area);

    let items: Vec<ListItem> = Theme::ALL.iter().map(|t| ListItem::new(t.name)).collect();
    let block = Block::default()
        .title("Options — Theme (↑/↓: preview, Enter: save, Esc: cancel)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let list = List::new(items).block(block).highlight_style(highlight_style(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(Theme::ALL.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_confirm_delete(frame: &mut Frame, theme: Theme, name: &str) {
    let area = centered(60, 35, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Delete connection")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error));

    let lines = vec![
        Line::from(format!("Remove '{name}'? This clears it from config.toml")),
        Line::from("and deletes its stored password from the keyring."),
        Line::default(),
        Line::from(Span::styled(
            "Enter/y: delete   any other key: cancel",
            Style::default().add_modifier(Modifier::ITALIC),
        )),
    ];
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Read-only table structure view (`s` on a table row): columns (with
/// type/nullability/key), indexes, and foreign keys — all already
/// present on the loaded [`Table`], so this needs no query of its own.
fn render_table_structure(frame: &mut Frame, theme: Theme, db_name: &str, table: &Table) {
    let area = centered(96, 80, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!("{db_name}.{} — structure (any key: close)", table.name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    let mut lines = vec![Line::from(Span::styled("Columns", Style::default().add_modifier(Modifier::BOLD)))];
    for col in &table.columns {
        let key = col.key.as_deref().unwrap_or("");
        let nullable = if col.nullable { "NULL" } else { "NOT NULL" };
        lines.push(Line::from(format!("  {:<24} {:<20} {nullable:<9} {key}", col.name, col.ty)));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Indexes", Style::default().add_modifier(Modifier::BOLD))));
    if table.indexes.is_empty() {
        lines.push(Line::from("  (none)"));
    } else {
        for idx in &table.indexes {
            lines.push(Line::from(format!("  {idx}")));
        }
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Foreign keys", Style::default().add_modifier(Modifier::BOLD))));
    if table.foreign_keys.is_empty() {
        lines.push(Line::from("  (none)"));
    } else {
        for fk in &table.foreign_keys {
            lines.push(Line::from(format!("  {} -> {}.{}", fk.column, fk.ref_table, fk.ref_column)));
        }
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Completion popup (`Ctrl+Space`/`F7`): a plain scrollable list of
/// matching keywords/table/column names — Up/Down to pick, Enter/Tab to
/// insert, Esc to cancel.
fn render_autocomplete(frame: &mut Frame, theme: Theme, candidates: &[String], selected: usize) {
    let area = centered(50, 60, frame.area());
    frame.render_widget(Clear, area);

    let items: Vec<ListItem> = candidates.iter().map(|c| ListItem::new(c.as_str())).collect();
    let block = Block::default()
        .title("Complete (↑/↓, Enter/Tab: insert, Esc: cancel)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let list = List::new(items).block(block).highlight_style(highlight_style(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(candidates.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut state);
}
