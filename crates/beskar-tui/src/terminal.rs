//! Terminal runtime: TTY guard, raw mode, alt-screen, and the event loop
//! that wires crossterm input and effect fulfillment to the pure reducer.

use std::io::{self, Stdout};
use std::time::Duration;

use beskar_core::error::{Error, Result};
use crossterm::event::{self, Event as CrosstermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::tty::IsTty;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use crate::effect::Effect;
use crate::event::Event;
use crate::input::key_of;
use crate::reduce::reduce;
use crate::service::{self, Services};
use crate::view;

/// Runs the TUI until the user quits. Fails closed with a typed error when
/// stdin/stdout are not interactive terminals (§4: a clear message instead
/// of misrendered output; the CLI maps this to a non-zero exit, §94).
pub fn run(app: &mut App, services: &Services) -> Result<()> {
    if !io::stdout().is_tty() || !io::stdin().is_tty() {
        return Err(Error::unsupported_state(
            "beskar tui requires an interactive terminal; stdout/stdin are \
             not TTYs — run it inside a terminal emulator",
        ));
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Errors during setup still restore the terminal state best-effort.
    if let Err(err) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(Error::Io(err));
    }
    let backend = CrosstermBackend::new(stdout);
    let result = match Terminal::new(backend) {
        Ok(mut terminal) => {
            let outcome = event_loop(&mut terminal, app, services);
            let _ = terminal.show_cursor();
            outcome
        }
        Err(err) => Err(Error::Io(err)),
    };

    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    result
}

/// The main loop: draw, fulfill queued effects against the core services,
/// reduce their result events, poll keyboard input. All rendering flows
/// through the pure view; all state changes flow through the reducer.
fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    services: &Services,
) -> Result<()> {
    let mut effects: Vec<Effect> = vec![Effect::Refresh];
    loop {
        let mut events: Vec<Event> = Vec::new();
        if !effects.is_empty() {
            // Fulfill effects one by one, drawing the busy indicator so the
            // user sees progress during (potentially slow) git operations.
            for effect in effects.drain(..) {
                app.busy = Some(effect.label());
                draw(terminal, app)?;
                app.busy = None;
                if let Some(result) = service::fulfill(services, effect) {
                    events.push(result);
                }
            }
        } else if event::poll(Duration::from_millis(250))?
            && let CrosstermEvent::Key(key) = event::read()?
            && let Some(key) = key_of(key)
        {
            events.push(Event::Key(key));
            // Resize/paste/focus events need no state change; the loop
            // redraws every iteration.
        }
        for event in events {
            effects.extend(reduce(app, event));
        }
        draw(terminal, app)?;
        if app.quit {
            return Ok(());
        }
    }
}

fn draw(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &App) -> Result<()> {
    terminal.draw(|frame| view::render(app, frame))?;
    Ok(())
}
