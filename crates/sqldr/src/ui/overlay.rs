//! Modal overlays: history picker (`Ctrl+R`), the connection wizard
//! (`Ctrl+N`), the DML-without-`WHERE` confirmation dialog, and the
//! options dialog (`Ctrl+O`). Rendered on top of everything else, centered.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

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
    }
}

fn render_history(frame: &mut Frame, theme: Theme, picker: &crate::app::HistoryPicker) {
    let area = centered(90, 70, frame.area());
    frame.render_widget(Clear, area);

    let title = format!("Historial — filtro: {}_", picker.filter);
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
        .title("Confirmar")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error));

    let mut lines: Vec<Line> = message.lines().map(Line::from).collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Enter/y: ejecutar   cualquier otra tecla: cancelar",
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
        .title("Nueva conexión — elegí el motor (Esc: cancelar)")
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
            "Nueva conexión: {} (Ctrl+S: probar y continuar, Esc: atrás)",
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
        text_line("Nombre", &wizard.name, ConnField::Name, false),
        text_line("Host", &wizard.host, ConnField::Host, false),
        text_line("Puerto", &wizard.port, ConnField::Port, false),
        text_line("Usuario", &wizard.user, ConnField::User, false),
        text_line("Password", &wizard.password, ConnField::Password, true),
        Line::from(vec![
            Span::raw(format!("{:<10}", "Read-only")),
            Span::styled(
                if wizard.read_only { "[x] (espacio para cambiar)" } else { "[ ] (espacio para cambiar)" },
                field_style(ConnField::ReadOnly),
            ),
        ]),
        Line::default(),
        Line::from("Tab/↓: siguiente campo   Shift+Tab/↑: anterior"),
        Line::from("La base de datos se elige después de probar la conexión."),
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
        .title("Probando conexión (Esc: cancelar)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning));

    let host = if wizard.host.trim().is_empty() { "127.0.0.1" } else { wizard.host.trim() };
    let message = format!(
        "Conectando a {}@{}:{} ({})…",
        if wizard.user.trim().is_empty() { "(sin usuario)" } else { wizard.user.trim() },
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

    let mut items = vec![ListItem::new("(sin base de datos por defecto)")];
    items.extend(databases.iter().map(|db| ListItem::new(db.as_str())));

    let block = Block::default()
        .title("Elegí una base de datos (Enter: guardar, Esc: atrás)")
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
        .title("Opciones — Tema (↑/↓: previsualizar, Enter: guardar, Esc: cancelar)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let list = List::new(items).block(block).highlight_style(highlight_style(theme));

    let mut state = ListState::default();
    state.select(Some(selected.min(Theme::ALL.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut state);
}
