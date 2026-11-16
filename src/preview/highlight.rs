//! Syntax highlighting through an external `bat` process.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;

use super::PreviewError;
use super::ansi::{self, StyledLine};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HighlightConfig {
  /// Highlight text previews. Without the command, previews fall back to plain text.
  pub enabled: bool,
  /// The `bat` executable.
  pub command: String,
  /// Passed as `--theme`; empty uses bat's own configuration. `ansi` follows the terminal
  /// palette, like ranger's scope.sh.
  pub theme: String,
}

impl Default for HighlightConfig {
  fn default() -> Self {
    HighlightConfig {
      enabled: true,
      command: "bat".to_string(),
      theme: "ansi".to_string(),
    }
  }
}

pub struct Highlighter {
  config: HighlightConfig,
  /// Set once the command turns out to be missing; no further attempts are made.
  unavailable: AtomicBool,
}

impl Highlighter {
  pub fn new(config: HighlightConfig) -> Self {
    Highlighter {
      config,
      unavailable: AtomicBool::new(false),
    }
  }

  /// Highlights `text`, using `file_name` for syntax detection. `Ok(None)` means the
  /// highlighter is unavailable or failed and the caller should show plain text.
  pub fn highlight(
    &self,
    file_name: &str,
    text: &str,
    cancelled: &dyn Fn() -> bool,
  ) -> Result<Option<Vec<StyledLine>>, PreviewError> {
    if self.unavailable.load(Ordering::Relaxed) {
      return Ok(None);
    }
    let mut cmd = Command::new(&self.config.command);
    cmd.args([
      "--color=always",
      "--style=plain",
      "--paging=never",
      "--wrap=never",
      // Tabs are passed through and expanded by the parser, like plain previews.
      "--tabs=0",
    ]);
    if !self.config.theme.is_empty() {
      cmd.arg(format!("--theme={}", self.config.theme));
    }
    cmd
      .arg(format!("--file-name={file_name}"))
      .arg("-")
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null());
    let mut child = match cmd.spawn() {
      Ok(c) => c,
      Err(e) => {
        if e.kind() == std::io::ErrorKind::NotFound {
          self.unavailable.store(true, Ordering::Relaxed);
        }
        return Ok(None);
      }
    };
    // Written from another thread: bat streams output while reading, so writing everything
    // before reading could fill both pipes and deadlock.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let input = text.to_owned();
    let writer = std::thread::spawn(move || {
      let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take().expect("stdout is piped");
    // Color codes inflate the text; anything far beyond that is not bat output.
    let cap = text.len() * 16 + 64 * 1024;
    let mut out = Vec::new();
    let mut chunk = [0u8; 8 * 1024];
    let mut failed = false;
    loop {
      if cancelled() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = writer.join();
        return Err(PreviewError::Cancelled);
      }
      match stdout.read(&mut chunk) {
        Ok(0) => break,
        Ok(n) => out.extend_from_slice(&chunk[..n]),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(_) => {
          failed = true;
          break;
        }
      }
      if out.len() > cap {
        failed = true;
        break;
      }
    }
    if failed {
      let _ = child.kill();
    }
    let status = child.wait();
    let _ = writer.join();
    if failed || !status.is_ok_and(|s| s.success()) {
      return Ok(None);
    }
    Ok(Some(ansi::parse(&String::from_utf8_lossy(&out))))
  }
}
