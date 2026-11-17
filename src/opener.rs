//! Launching files with an external program, either detached from the TUI or in the
//! foreground with the terminal handed over to it.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
  #[error("no opener configured")]
  NoOpener,
  #[error("failed to run `{program}`: {source}")]
  Spawn {
    program: String,
    source: std::io::Error,
  },
}

/// Runs `opener <path>` detached. `opener` is split on whitespace.
pub fn open(opener: &str, path: &Path) -> Result<(), OpenError> {
  let argv: Vec<String> = opener.split_whitespace().map(str::to_string).collect();
  spawn_detached(&argv, path)
}

/// Runs `argv` with `path` appended, in its own process group with null stdio, so it neither
/// draws over the TUI nor receives the terminal's signals. A thread reaps the child when it
/// exits.
pub fn spawn_detached(argv: &[String], path: &Path) -> Result<(), OpenError> {
  let (program, mut cmd) = command(argv, path)?;
  let mut child = cmd
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .process_group(0)
    .spawn()
    .map_err(|source| OpenError::Spawn {
      program: program.to_string(),
      source,
    })?;
  thread::spawn(move || {
    let _ = child.wait();
  });
  Ok(())
}

/// Runs `argv` with `path` appended on the terminal and waits for it. The caller must have
/// released the terminal (cooked mode, main screen) and stopped reading input.
///
/// The child stays in tansu's process group, so it is the terminal's foreground job and can
/// read input. Terminal signals (Ctrl-C, Ctrl-\) then reach both processes, so tansu ignores
/// them until the child exits.
pub fn run_foreground(argv: &[String], path: &Path) -> Result<ExitStatus, OpenError> {
  let (program, mut cmd) = command(argv, path)?;
  let mut child = cmd.spawn().map_err(|source| OpenError::Spawn {
    program: program.to_string(),
    source,
  })?;
  // Installed after the spawn so the child keeps the default dispositions.
  let _signals = IgnoreTerminalSignals::install();
  child.wait().map_err(|source| OpenError::Spawn {
    program: program.to_string(),
    source,
  })
}

fn command<'a>(argv: &'a [String], path: &Path) -> Result<(&'a str, Command), OpenError> {
  let (program, args) = argv.split_first().ok_or(OpenError::NoOpener)?;
  let mut cmd = Command::new(program);
  cmd.args(args).arg(path);
  Ok((program, cmd))
}

/// Ignores SIGINT and SIGQUIT while alive; restores the previous handlers on drop.
struct IgnoreTerminalSignals {
  previous: [(libc::c_int, libc::sighandler_t); 2],
}

impl IgnoreTerminalSignals {
  fn install() -> Self {
    let previous = [libc::SIGINT, libc::SIGQUIT].map(|sig| {
      // SAFETY: installing SIG_IGN has no handler code to run, so it cannot violate
      // async-signal safety.
      (sig, unsafe { libc::signal(sig, libc::SIG_IGN) })
    });
    IgnoreTerminalSignals { previous }
  }
}

impl Drop for IgnoreTerminalSignals {
  fn drop(&mut self) {
    for (sig, handler) in self.previous {
      if handler != libc::SIG_ERR {
        // SAFETY: restores the disposition that was in place before `install`.
        unsafe {
          libc::signal(sig, handler);
        }
      }
    }
  }
}
