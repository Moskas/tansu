//! Terminal setup and teardown, including restoration on panic.

use std::io::{self, Stdout};
use std::panic;

use anyhow::{Context, Result};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
  EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Owns the terminal; restores it when dropped (normal exit and error paths).
pub struct TerminalGuard {
  pub terminal: Tui,
  /// Mouse reporting is on, and must be turned on again after a suspension.
  mouse: bool,
}

impl TerminalGuard {
  pub fn new() -> Result<Self> {
    install_panic_hook();
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen) {
      restore();
      return Err(e).context("entering alternate screen");
    }
    let mut terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
      Ok(t) => t,
      Err(e) => {
        restore();
        return Err(e).context("creating terminal");
      }
    };
    terminal.hide_cursor().ok();
    terminal.clear().ok();
    Ok(TerminalGuard {
      terminal,
      mouse: false,
    })
  }
}

impl TerminalGuard {
  /// Turns on mouse reporting. Called after terminal queries are done, so mouse reports
  /// cannot get mixed into their replies.
  pub fn enable_mouse(&mut self) -> Result<()> {
    execute!(io::stdout(), EnableMouseCapture).context("enabling mouse capture")?;
    self.mouse = true;
    Ok(())
  }

  /// Hands the terminal back to the shell state (cooked mode, main screen, visible cursor,
  /// no mouse reporting) while `f` runs, then takes it over again and forces a full redraw.
  pub fn suspended<T>(&mut self, f: impl FnOnce() -> T) -> Result<T> {
    restore();
    let out = f();
    enable_raw_mode().context("enabling raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen).context("entering alternate screen")?;
    if self.mouse {
      self.enable_mouse()?;
    }
    self.terminal.hide_cursor().ok();
    self.terminal.clear().context("clearing terminal")?;
    Ok(out)
  }
}

impl Drop for TerminalGuard {
  fn drop(&mut self) {
    restore();
    self.terminal.show_cursor().ok();
  }
}

/// Best-effort terminal restoration; safe to call more than once.
pub fn restore() {
  let _ = disable_raw_mode();
  let _ = execute!(
    io::stdout(),
    DisableMouseCapture,
    LeaveAlternateScreen,
    crossterm::cursor::Show
  );
}

fn install_panic_hook() {
  let previous = panic::take_hook();
  panic::set_hook(Box::new(move |info| {
    restore();
    previous(info);
  }));
}
