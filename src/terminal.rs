//! Terminal setup and teardown, including restoration on panic.

use std::io::{self, Stdout};
use std::panic;

use anyhow::{Context, Result};
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
    Ok(TerminalGuard { terminal })
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
  let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
}

fn install_panic_hook() {
  let previous = panic::take_hook();
  panic::set_hook(Box::new(move |info| {
    restore();
    previous(info);
  }));
}
