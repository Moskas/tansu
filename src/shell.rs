//! `:shell`, as in ranger: a command line run through the user's shell, with `-flags` that
//! choose how it runs and `%` macros that expand to the selection.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

/// How a shell command runs, derived from its flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellMode {
  /// On the terminal with tansu suspended; `wait_enter` prompts before returning (`-w`).
  Foreground { wait_enter: bool },
  /// Output (stdout and stderr) shown in `less` (`-p`).
  Pager,
  /// In the background with output discarded; the exit status is reported (`-s`).
  Silent,
  /// Detached, with output discarded and nothing reported (`-f`).
  Detached,
  /// Detached in a new terminal window (`-t`).
  Terminal,
}

/// The parsed flags of a `:shell` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellFlags {
  pub mode: ShellMode,
  /// Run through `sudo` (`-r`).
  pub sudo: bool,
  /// Pass `-b` to `sudo` so it backgrounds the command (`-rf`).
  pub sudo_background: bool,
}

impl ShellFlags {
  /// Interprets ranger's flags. An uppercase flag cancels its lowercase one; unknown flags
  /// and `c` (only meaningful with multiple selected files) are ignored, as in ranger.
  pub fn parse(flags: &str) -> ShellFlags {
    let mut set: Vec<char> = flags.chars().collect();
    for flag in flags.chars().filter(char::is_ascii_uppercase) {
      let lower = flag.to_ascii_lowercase();
      set.retain(|&c| c != flag && c != lower);
    }
    let has = |c: char| set.contains(&c);
    // The same decisions as ranger's runner, in the same order.
    let mut toggle_ui = true;
    let mut wait = true;
    if has('p') {
      toggle_ui = false;
      wait = false;
    }
    if has('s') {
      toggle_ui = false;
    }
    if has('f') {
      toggle_ui = false;
      wait = false;
    }
    let wait_enter = has('w') && !has('p') && wait;
    let sudo = has('r');
    let mode = if has('t') {
      ShellMode::Terminal
    } else if has('p') {
      ShellMode::Pager
    } else if sudo {
      // sudo may prompt for a password, so it always gets the terminal.
      ShellMode::Foreground { wait_enter }
    } else if has('f') {
      ShellMode::Detached
    } else if !toggle_ui {
      ShellMode::Silent
    } else {
      ShellMode::Foreground { wait_enter }
    };
    ShellFlags {
      mode,
      sudo,
      sudo_background: sudo && has('f'),
    }
  }

  /// Prefixes `script` with `sudo` when asked to, as ranger does.
  pub fn apply_sudo(&self, script: OsString) -> OsString {
    if !self.sudo {
      return script;
    }
    let mut out = OsString::from(if self.sudo_background {
      "sudo -b "
    } else {
      "sudo "
    });
    out.push(script);
    out
  }
}

/// What the macros expand to.
pub struct MacroContext<'a> {
  /// The current directory.
  pub dir: &'a Path,
  /// The selected entry's full path.
  pub selected: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MacroError {
  #[error("%{0}: nothing selected")]
  NothingSelected(String),
}

/// Expands ranger's macros in `command`, shell-quoting every value:
///
/// - `%f`: the selected file's name
/// - `%d`: the current directory
/// - `%s`: the names of the selected files (the selected file)
/// - `%p`: the full paths of the selected files
/// - `%t`, `%c`: tagged files and the copy buffer, which tansu does not have, so empty
/// - `%D`, `%F`, `%S`, `%P`: the same for the next tab, which is this one; `%F` and `%S` are
///   full paths
/// - `%%`: a literal `%`
///
/// A macro name is the longest run of letters, digits and `_` after the `%`, or a name in
/// braces (`%{f}`). Anything that is not a known macro is kept as written.
pub fn expand_macros(command: &str, ctx: &MacroContext) -> Result<OsString, MacroError> {
  let mut out: Vec<u8> = Vec::with_capacity(command.len());
  let mut rest = command;
  while let Some(at) = rest.find('%') {
    out.extend_from_slice(&rest.as_bytes()[..at]);
    let after = &rest[at + 1..];
    if let Some(after) = after.strip_prefix('%') {
      out.push(b'%');
      rest = after;
      continue;
    }
    let (name, consumed) = match after.strip_prefix('{') {
      Some(inner) => match inner.find('}') {
        Some(end) if is_macro_name(&inner[..end]) => (&inner[..end], end + 2),
        _ => ("", 0),
      },
      None => {
        let end = after
          .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
          .unwrap_or(after.len());
        (&after[..end], end)
      }
    };
    match macro_value(name, ctx)? {
      Some(value) => out.extend_from_slice(&value),
      None => out.extend_from_slice(&rest.as_bytes()[at..at + 1 + consumed]),
    }
    rest = &after[consumed..];
  }
  out.extend_from_slice(rest.as_bytes());
  Ok(OsString::from_vec(out))
}

fn is_macro_name(s: &str) -> bool {
  !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The quoted value of a macro, or `None` if `name` is not one.
fn macro_value(name: &str, ctx: &MacroContext) -> Result<Option<Vec<u8>>, MacroError> {
  let file_name = || ctx.selected.and_then(Path::file_name);
  let value = match name {
    "f" => {
      let name = file_name().ok_or_else(|| MacroError::NothingSelected(name.to_string()))?;
      shell_quote(name.as_bytes())
    }
    "d" => shell_quote(ctx.dir.as_os_str().as_bytes()),
    "s" => file_name().map_or_else(Vec::new, |n| shell_quote(n.as_bytes())),
    "p" => ctx
      .selected
      .map_or_else(Vec::new, |p| shell_quote(p.as_os_str().as_bytes())),
    "t" | "c" => Vec::new(),
    // The next tab's directory, file and selection. Tansu has a single tab, which ranger
    // then treats as its own next tab; the files are given as full paths.
    "D" => shell_quote(ctx.dir.as_os_str().as_bytes()),
    "F" => {
      let path = ctx
        .selected
        .ok_or_else(|| MacroError::NothingSelected(name.to_string()))?;
      shell_quote(path.as_os_str().as_bytes())
    }
    "S" | "P" => ctx
      .selected
      .map_or_else(Vec::new, |p| shell_quote(p.as_os_str().as_bytes())),
    _ => return Ok(None),
  };
  Ok(Some(value))
}

/// Quotes `s` for a POSIX shell: `'...'`, with each `'` written as `'\''`.
pub fn shell_quote(s: &[u8]) -> Vec<u8> {
  let mut out = Vec::with_capacity(s.len() + 2);
  out.push(b'\'');
  for &b in s {
    if b == b'\'' {
      out.extend_from_slice(b"'\\''");
    } else {
      out.push(b);
    }
  }
  out.push(b'\'');
  out
}

/// Escapes a file name with backslashes for Tab completion, as ranger does; names with
/// control characters are quoted instead.
pub fn shell_escape(name: &str) -> String {
  const META: &[char] = &[
    ' ', '\'', '"', '`', '&', '|', ';', '#', '$', '!', '(', ')', '[', ']', '<', '>', '\t',
  ];
  if name
    .chars()
    .any(|c| (c.is_control() && c != '\t') || ('\u{7f}'..='\u{ff}').contains(&c))
  {
    return String::from_utf8_lossy(&shell_quote(name.as_bytes())).into_owned();
  }
  let mut out = String::with_capacity(name.len());
  for c in name.chars() {
    if c == '\\' || META.contains(&c) {
      out.push('\\');
    }
    out.push(c);
  }
  out
}
