//! Terminal setup/teardown and the async event loop that drives [`App`].

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    Event, EventStream, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
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

use crate::app::{App, AppEvent};
use crate::config::Config;
use crate::ui;

pub async fn run(config: Config) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

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

    let result = run_app(&mut terminal, config).await;

    if enhanced_keys {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    config: Config,
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut app = App::new(config, tx);
    let mut term_events = EventStream::new();

    terminal.draw(|f| ui::draw(f, &app))?;

    loop {
        tokio::select! {
            maybe_ev = term_events.next().fuse() => {
                match maybe_ev {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        app.on_app_event(AppEvent::Key(key));
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        app.on_app_event(AppEvent::Resize);
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
        terminal.draw(|f| ui::draw(f, &app))?;
    }

    Ok(())
}
