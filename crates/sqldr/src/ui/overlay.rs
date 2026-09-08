//! Modal overlays: history picker (`Ctrl+R`) and the DML-without-`WHERE`
//! confirmation dialog. Rendered on top of everything else, centered.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ConnField, ConnWizard, Engine, Overlay, WizardStep};

fn centered(width: u16, height_pct: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(20);
    let [area] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [area] = Layout::vertical([Constraint::Percentage(height_pct)]).flex(Flex::Center).areas(area);
    area
}

pub fn render(frame: &mut Frame, app: &App) {
    let Some(overlay) = &app.overlay else { return };
    match overlay {
        Overlay::History(picker) => render_history(frame, picker),
        Overlay::Confirm { message, .. } => render_confirm(frame, message),
        Overlay::AddConnection(form) => render_add_connection(frame, form),
    }
}

fn render_history(frame: &mut Frame, picker: &crate::app::HistoryPicker) {
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
        .border_style(Style::default().fg(Color::Yellow));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    if !matches.is_empty() {
        state.select(Some(picker.selected.min(matches.len() - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_confirm(frame: &mut Frame, message: &str) {
    let area = centered(70, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Confirmar")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let mut lines: Vec<Line> = message.lines().map(Line::from).collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Enter/y: ejecutar   cualquier otra tecla: cancelar",
        Style::default().add_modifier(Modifier::ITALIC),
    )));

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_add_connection(frame: &mut Frame, wizard: &ConnWizard) {
    match &wizard.step {
        WizardStep::SelectEngine { selected } => render_select_engine(frame, *selected),
        WizardStep::Details => render_connection_details(frame, wizard),
        WizardStep::Testing => render_testing(frame, wizard),
        WizardStep::SelectDatabase { databases, selected } => {
            render_select_database(frame, databases, *selected)
        }
    }
}

fn render_select_engine(frame: &mut Frame, selected: usize) {
    let area = centered(50, 40, frame.area());
    frame.render_widget(Clear, area);

    let items: Vec<ListItem> = Engine::ALL.iter().map(|e| ListItem::new(e.label())).collect();
    let block = Block::default()
        .title("Nueva conexión — elegí el motor (Esc: cancelar)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    state.select(Some(selected.min(Engine::ALL.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_connection_details(frame: &mut Frame, wizard: &ConnWizard) {
    let area = centered(64, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(
            "Nueva conexión: {} (Ctrl+S: probar y continuar, Esc: atrás)",
            wizard.engine.label()
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let field_style = |field: ConnField| {
        if wizard.field == field {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
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
        lines.push(Line::from(Span::styled(err.as_str(), Style::default().fg(Color::Red))));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_testing(frame: &mut Frame, wizard: &ConnWizard) {
    let area = centered(60, 30, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Probando conexión (Esc: cancelar)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

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

fn render_select_database(frame: &mut Frame, databases: &[String], selected: usize) {
    let area = centered(60, 60, frame.area());
    frame.render_widget(Clear, area);

    let mut items = vec![ListItem::new("(sin base de datos por defecto)")];
    items.extend(databases.iter().map(|db| ListItem::new(db.as_str())));

    let block = Block::default()
        .title("Elegí una base de datos (Enter: guardar, Esc: atrás)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    state.select(Some(selected.min(databases.len())));
    frame.render_stateful_widget(list, area, &mut state);
}
