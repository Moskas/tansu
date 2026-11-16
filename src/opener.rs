//! Launching files with an external program, detached from the TUI.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
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

/// Runs `opener <path>` in its own process group with null stdio, so it neither draws over
/// the TUI nor receives the terminal's signals. A thread reaps the child when it exits.
pub fn open(opener: &str, path: &Path) -> Result<(), OpenError> {
  let mut words = opener.split_whitespace();
  let program = words.next().ok_or(OpenError::NoOpener)?;
  let mut child = Command::new(program)
    .args(words)
    .arg(path)
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
