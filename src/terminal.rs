//! Terminal setup and teardown, including restoration on panic.
//!
//! The UI is drawn on `/dev/tty` rather than stdout, so it still shows when stdout is
//! redirected (`$(tansu --choosefile=/dev/stdout)`). Without a controlling terminal it falls
//! back to stdout. Input needs nothing: crossterm reads `/dev/tty` when stdin is not one.

use std::fs::File;
use std::io::{self, BufWriter, IsTerminal, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::panic;
use std::process::Stdio;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
  EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

pub type Tui = Terminal<CrosstermBackend<BufWriter<TermOut>>>;

/// The controlling terminal, opened once; `None` when there is none.
fn tty() -> Option<&'static File> {
  static TTY: OnceLock<Option<File>> = OnceLock::new();
  TTY
    .get_or_init(|| File::options().read(true).write(true).open("/dev/tty").ok())
    .as_ref()
}

/// Writes to the terminal: `/dev/tty`, or stdout without one. Unbuffered; the UI wraps it in
/// a `BufWriter` so a frame is written in few syscalls.
pub struct TermOut;

impl Write for TermOut {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    match tty() {
      Some(mut tty) => tty.write(buf),
      None => io::stdout().write(buf),
    }
  }

  fn flush(&mut self) -> io::Result<()> {
    match tty() {
      Some(mut tty) => tty.flush(),
      None => io::stdout().flush(),
    }
  }
}

/// Reads a line from the terminal (cooked mode), for prompts while tansu is suspended.
pub fn read_line() {
  let Some(mut tty) = tty().filter(|_| !io::stdin().is_terminal()) else {
    let _ = io::stdin().read_line(&mut String::new());
    return;
  };
  // Byte by byte, so nothing past the line is consumed.
  let mut byte = [0u8];
  while matches!(tty.read(&mut byte), Ok(1)) && byte[0] != b'\n' {}
}

/// Stdin for a program that takes over the terminal: tansu's own when it is the terminal,
/// otherwise the terminal itself.
pub fn child_stdin() -> Stdio {
  child_stdio(io::stdin().is_terminal())
}

/// Stdout for a program that takes over the terminal, like [`child_stdin`]; an editor
/// started from `$(tansu ...)` must not draw into the captured output.
pub fn child_stdout() -> Stdio {
  child_stdio(io::stdout().is_terminal())
}

fn child_stdio(inherited_is_terminal: bool) -> Stdio {
  match tty() {
    Some(tty) if !inherited_is_terminal => tty
      .try_clone()
      .map_or_else(|_| Stdio::inherit(), Stdio::from),
    _ => Stdio::inherit(),
  }
}

/// Runs `f` with the terminal as stdin and stdout, for code that only talks to stdio (the
/// graphics query of ratatui-image). The redirected descriptors are put back afterwards.
///
/// `f` must not start crossterm's input, which picks its descriptor once and would keep
/// reading fd 0 after it is restored.
pub fn with_tty_stdio<T>(f: impl FnOnce() -> T) -> T {
  let Some(tty) = tty() else {
    return f();
  };
  let _ = io::stdout().flush();
  let mut saved: Vec<(i32, OwnedFd)> = Vec::new();
  for (fd, is_terminal) in [
    (libc::STDIN_FILENO, io::stdin().is_terminal()),
    (libc::STDOUT_FILENO, io::stdout().is_terminal()),
  ] {
    if is_terminal {
      continue;
    }
    // SAFETY: stdin and stdout stay open for the life of the process.
    let Ok(copy) = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned() else {
      continue;
    };
    // SAFETY: both descriptors are open; dup2 replaces `fd` atomically.
    if unsafe { libc::dup2(tty.as_fd().as_raw_fd(), fd) } >= 0 {
      saved.push((fd, copy));
    }
  }
  let out = f();
  let _ = io::stdout().flush();
  for (fd, copy) in saved {
    // SAFETY: as above; `copy` is closed when dropped.
    unsafe { libc::dup2(copy.as_raw_fd(), fd) };
  }
  out
}

/// Owns the terminal; restores it when dropped (normal exit and error paths).
pub struct TerminalGuard {
  pub terminal: Tui,
  /// Mouse reporting is on, and must be turned on again after a suspension.
  mouse: bool,
  /// The last title set, so an unchanged one is not sent again.
  title: Option<String>,
  /// The terminal's own title was saved on its title stack and is restored on exit.
  title_pushed: bool,
}

/// xterm's window operations that save the title on the terminal's stack and restore it.
const PUSH_TITLE: &str = "\x1b[22;0t";
const POP_TITLE: &str = "\x1b[23;0t";

impl TerminalGuard {
  pub fn new() -> Result<Self> {
    install_panic_hook();
    enable_raw_mode().context("enabling raw mode")?;
    if let Err(e) = execute!(TermOut, EnterAlternateScreen) {
      restore();
      return Err(e).context("entering alternate screen");
    }
    let mut terminal = match Terminal::new(CrosstermBackend::new(BufWriter::with_capacity(
      1 << 16,
      TermOut,
    ))) {
      Ok(t) => t,
      Err(e) => {
        restore();
        return Err(e).context("creating terminal");
      }
    };
    terminal.hide_cursor().ok();
    clear(&mut terminal).ok();
    Ok(TerminalGuard {
      terminal,
      mouse: false,
      title: None,
      title_pushed: false,
    })
  }
}

impl TerminalGuard {
  /// Turns on mouse reporting. Called after terminal queries are done, so mouse reports
  /// cannot get mixed into their replies.
  pub fn enable_mouse(&mut self) -> Result<()> {
    execute!(TermOut, EnableMouseCapture).context("enabling mouse capture")?;
    self.mouse = true;
    Ok(())
  }

  /// Hands the terminal back to the shell state (cooked mode, main screen, visible cursor,
  /// no mouse reporting) while `f` runs, then takes it over again and forces a full redraw.
  pub fn suspended<T>(&mut self, f: impl FnOnce() -> T) -> Result<T> {
    // The program gets the shell's title back and may set its own, so the next frame sets
    // tansu's again.
    self.pop_title();
    self.title = None;
    restore();
    let out = f();
    enable_raw_mode().context("enabling raw mode")?;
    execute!(TermOut, EnterAlternateScreen).context("entering alternate screen")?;
    if self.mouse {
      self.enable_mouse()?;
    }
    self.terminal.hide_cursor().ok();
    clear(&mut self.terminal).context("clearing terminal")?;
    Ok(out)
  }

  /// Sets the terminal's window title, saving the terminal's own title the first time so it
  /// comes back on exit. Does nothing when `title` is already shown.
  pub fn set_title(&mut self, title: &str) {
    if self.title.as_deref() == Some(title) {
      return;
    }
    if !self.title_pushed {
      let _ = TermOut.write_all(PUSH_TITLE.as_bytes());
      self.title_pushed = true;
    }
    let _ = execute!(TermOut, crossterm::terminal::SetTitle(title));
    self.title = Some(title.to_string());
  }

  /// Restores the title saved by `set_title`, once.
  fn pop_title(&mut self) {
    if std::mem::take(&mut self.title_pushed) {
      let _ = TermOut.write_all(POP_TITLE.as_bytes());
      let _ = TermOut.flush();
    }
  }
}

/// Clears the screen and forces a full redraw on the next frame, at the current size.
///
/// `Terminal::clear` also saves and restores the cursor position, which asks the terminal
/// for it (`ESC [ 6 n`) and fails after two seconds on one that does not answer. A
/// fullscreen UI has no cursor position to keep, and for a fullscreen viewport `resize`
/// clears the screen and resets the back buffer without the query.
fn clear(terminal: &mut Tui) -> io::Result<()> {
  let size = terminal.size()?;
  terminal.resize(Rect::new(0, 0, size.width, size.height))
}

impl Drop for TerminalGuard {
  fn drop(&mut self) {
    self.pop_title();
    restore();
    self.terminal.show_cursor().ok();
  }
}

/// Best-effort terminal restoration; safe to call more than once.
pub fn restore() {
  let _ = disable_raw_mode();
  let _ = execute!(
    TermOut,
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
