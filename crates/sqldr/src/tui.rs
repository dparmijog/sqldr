//! Terminal setup/teardown and the async event loop that drives [`App`].

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use futures::{FutureExt, StreamExt};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tui_textarea::TextArea;

use crate::app::{App, AppEvent};
use crate::config::Config;
use crate::ui;

type Term = Terminal<CrosstermBackend<std::io::Stdout>>;

pub async fn run(config: Config) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    // Kitty keyboard protocol: without it, most terminals send the same
    // bytes for Ctrl+Enter as plain Enter, so Ctrl+Enter can't be told
    // apart from Enter. Only push it on terminals that report support
    // (kitty, wezterm, foot, ghostty, …); tmux/xterm/most others don't,
    // and Ctrl+Enter silently behaves like Enter there instead.
    let enhanced_keys = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced_keys {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, config, enhanced_keys).await;

    if enhanced_keys {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn is_ctrl_e(key: &crossterm::event::KeyEvent) -> bool {
    key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL)
}

async fn run_app(terminal: &mut Term, config: Config, enhanced_keys: bool) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut app = App::new(config, tx);
    let mut term_events = EventStream::new();

    terminal.draw(|f| ui::draw(f, &mut app))?;

    loop {
        tokio::select! {
            maybe_ev = term_events.next().fuse() => {
                match maybe_ev {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        if is_ctrl_e(&key) && app.overlay.is_none() {
                            edit_in_external_editor(terminal, &mut app, enhanced_keys).await?;
                        } else {
                            app.on_app_event(AppEvent::Key(key));
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        app.on_app_event(AppEvent::Resize);
                    }
                    Some(Ok(Event::Mouse(mouse))) => {
                        app.on_app_event(AppEvent::Mouse(mouse));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                }
            }
            Some(event) = rx.recv() => {
                app.on_app_event(event);
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                // Periodic redraw tick so streaming query rows still show up
                // even without a terminal event in between.
            }
        }

        if app.quit {
            break;
        }
        terminal.draw(|f| ui::draw(f, &mut app))?;
    }

    Ok(())
}

/// Suspends the TUI, opens the editor's SQL in `$EDITOR` (falling back to
/// `vi`), and reloads whatever was saved back into the editor pane.
async fn edit_in_external_editor(terminal: &mut Term, app: &mut App, enhanced_keys: bool) -> Result<()> {
    let editor_cmd = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let tmp_path = std::env::temp_dir().join(format!("sqldr-{}.sql", std::process::id()));
    if let Err(e) = std::fs::write(&tmp_path, app.editor.lines().join("\n")) {
        app.status = crate::app::StatusMessage::Error(format!("no se pudo crear archivo temporal: {e}"));
        return Ok(());
    }

    // `$EDITOR` commonly carries arguments (`"code --wait"`, `"vim -u NONE"`);
    // tokenize it shell-style instead of treating the whole value as one
    // program name, or those would fail to spawn entirely.
    let mut parts = match shell_words::split(&editor_cmd) {
        Ok(parts) if !parts.is_empty() => parts,
        _ => vec![editor_cmd.clone()],
    };
    let program = parts.remove(0);

    if enhanced_keys {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    let status = tokio::process::Command::new(&program)
        .args(&parts)
        .arg(&tmp_path)
        .status()
        .await;

    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    if enhanced_keys {
        execute!(
            terminal.backend_mut(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    terminal.clear()?;

    match status {
        Ok(exit) if exit.success() => match std::fs::read_to_string(&tmp_path) {
            Ok(content) => {
                let lines: Vec<String> = content.lines().map(String::from).collect();
                app.editor = TextArea::new(if lines.is_empty() { vec![String::new()] } else { lines });
                app.editor.move_cursor(tui_textarea::CursorMove::Bottom);
                app.editor.move_cursor(tui_textarea::CursorMove::End);
                app.editor.set_placeholder_text("-- escribe SQL, Ctrl+Enter para ejecutar");
                app.status = crate::app::StatusMessage::Info(format!("editado en {editor_cmd}"));
            }
            Err(e) => {
                app.status = crate::app::StatusMessage::Error(format!("no se pudo leer de vuelta: {e}"));
            }
        },
        Ok(_) => {
            app.status = crate::app::StatusMessage::Info(format!("{editor_cmd} salió sin guardar"));
        }
        Err(e) => {
            app.status = crate::app::StatusMessage::Error(format!("no se pudo ejecutar '{editor_cmd}': {e}"));
        }
    }

    let _ = std::fs::remove_file(&tmp_path);
    Ok(())
}
