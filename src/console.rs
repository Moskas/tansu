//! The `:` command line: a single-line editor and the parser for the commands typed into it.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::action::Action;
use crate::fs::SortKey;
use crate::keymap::Key;

/// What a key press did to the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOutcome {
  /// Keep editing.
  Continue,
  /// Close the command line without running anything.
  Cancel,
  /// Run the given input.
  Submit(String),
}

/// Single-line editor. The cursor is a char index into `input`, so multibyte text is safe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineEditor {
  input: String,
  cursor: usize,
}

impl LineEditor {
  pub fn new() -> Self {
    LineEditor::default()
  }

  /// An editor holding `input`, with the cursor at the end.
  pub fn with_input(input: impl Into<String>) -> Self {
    let mut editor = LineEditor::new();
    editor.set_input(input);
    editor
  }

  /// Replaces the input and moves the cursor to the end.
  pub fn set_input(&mut self, input: impl Into<String>) {
    self.input = input.into();
    self.cursor = self.len();
  }

  pub fn input(&self) -> &str {
    &self.input
  }

  /// Cursor position in chars.
  pub fn cursor(&self) -> usize {
    self.cursor
  }

  /// The input before the cursor.
  pub fn before_cursor(&self) -> &str {
    &self.input[..self.byte_at(self.cursor)]
  }

  pub fn handle_key(&mut self, key: Key) -> EditOutcome {
    match Edit::for_key(key) {
      Edit::Cancel => return EditOutcome::Cancel,
      Edit::Submit => return EditOutcome::Submit(self.input.clone()),
      // Like vim, erasing past the start of an empty line leaves the command line.
      Edit::Backspace if self.input.is_empty() => return EditOutcome::Cancel,
      Edit::Backspace => {
        if self.cursor > 0 {
          self.cursor -= 1;
          self.remove_at(self.cursor);
        }
      }
      Edit::Delete => {
        if self.cursor < self.len() {
          self.remove_at(self.cursor);
        }
      }
      Edit::Left => self.cursor = self.cursor.saturating_sub(1),
      Edit::Right => self.cursor = (self.cursor + 1).min(self.len()),
      Edit::Home => self.cursor = 0,
      Edit::End => self.cursor = self.len(),
      Edit::KillToStart => {
        let end = self.byte_at(self.cursor);
        self.input.replace_range(..end, "");
        self.cursor = 0;
      }
      Edit::KillToEnd => {
        let start = self.byte_at(self.cursor);
        self.input.truncate(start);
      }
      Edit::KillWord => self.delete_word_before(),
      Edit::Insert(c) => {
        let at = self.byte_at(self.cursor);
        self.input.insert(at, c);
        self.cursor += 1;
      }
      Edit::Ignore => {}
    }
    EditOutcome::Continue
  }

  fn len(&self) -> usize {
    self.input.chars().count()
  }

  /// Byte offset of the char at index `chars` (or the end of the input).
  fn byte_at(&self, chars: usize) -> usize {
    self
      .input
      .char_indices()
      .nth(chars)
      .map_or(self.input.len(), |(i, _)| i)
  }

  fn remove_at(&mut self, chars: usize) {
    let at = self.byte_at(chars);
    self.input.remove(at);
  }

  /// Deletes the whitespace and then the word before the cursor.
  fn delete_word_before(&mut self) {
    let chars: Vec<char> = self.input.chars().take(self.cursor).collect();
    let mut start = chars.len();
    while start > 0 && chars[start - 1].is_whitespace() {
      start -= 1;
    }
    while start > 0 && !chars[start - 1].is_whitespace() {
      start -= 1;
    }
    let (from, to) = (self.byte_at(start), self.byte_at(self.cursor));
    self.input.replace_range(from..to, "");
    self.cursor = start;
  }
}

/// Line-editing operation for a key, with readline-style control keys.
enum Edit {
  Cancel,
  Submit,
  Backspace,
  Delete,
  Left,
  Right,
  Home,
  End,
  KillToStart,
  KillToEnd,
  KillWord,
  Insert(char),
  Ignore,
}

impl Edit {
  fn for_key(key: Key) -> Edit {
    let ctrl = key.mods.contains(KeyModifiers::CONTROL);
    let alt = key.mods.contains(KeyModifiers::ALT);
    match (key.code, ctrl) {
      (KeyCode::Esc, _) | (KeyCode::Char('c' | 'g'), true) => Edit::Cancel,
      (KeyCode::Enter, _) => Edit::Submit,
      (KeyCode::Backspace, _) | (KeyCode::Char('h'), true) => Edit::Backspace,
      (KeyCode::Delete, _) | (KeyCode::Char('d'), true) => Edit::Delete,
      (KeyCode::Left, _) | (KeyCode::Char('b'), true) => Edit::Left,
      (KeyCode::Right, _) | (KeyCode::Char('f'), true) => Edit::Right,
      (KeyCode::Home, _) | (KeyCode::Char('a'), true) => Edit::Home,
      (KeyCode::End, _) | (KeyCode::Char('e'), true) => Edit::End,
      (KeyCode::Char('u'), true) => Edit::KillToStart,
      (KeyCode::Char('k'), true) => Edit::KillToEnd,
      (KeyCode::Char('w'), true) => Edit::KillWord,
      (KeyCode::Char(c), false) if !alt => Edit::Insert(c),
      _ => Edit::Ignore,
    }
  }
}

/// A command typed into the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleCommand {
  /// Any action by its config name, e.g. `reload` or `open`.
  Action(Action),
  /// Run `argv` with the selected path appended as the last argument.
  OpenWith { argv: Vec<String>, detach: bool },
  /// Rename the selection to this name, taken verbatim (spaces included).
  Rename(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsoleError {
  #[error("unknown command `{0}`")]
  Unknown(String),
  #[error("{0}: missing program")]
  MissingProgram(&'static str),
  #[error("sort: missing sort type (name, size, mtime, ctime)")]
  MissingSortKey,
  #[error("sort: unknown sort type `{0}`")]
  UnknownSortKey(String),
  #[error("{command}: unexpected argument `{arg}`")]
  UnexpectedArgument { command: String, arg: String },
}

/// Name of the command that runs a program on the selection.
pub const OPEN_WITH: &str = "open_with";

/// Name of the command that changes the sort order, followed by a sort type.
pub const SORT: &str = "sort";

/// Name of the command that renames the selection. Without a name it opens the command
/// line prefilled with the current one.
pub const RENAME: &str = "rename";

/// Flag of `open_with` that detaches the program instead of suspending tansu for it.
pub const DETACH_FLAG: &str = "-d";

/// Parses a command line. Words are separated by whitespace; there is no quoting. Returns
/// `None` for a blank line.
pub fn parse(line: &str) -> Option<Result<ConsoleCommand, ConsoleError>> {
  let mut words = line.split_whitespace();
  let name = words.next()?;
  if name == RENAME {
    // File names may contain spaces, so the rest of the line is the name.
    let new_name = rest_after_command(line).trim();
    return Some(Ok(if new_name.is_empty() {
      ConsoleCommand::Action(Action::Rename)
    } else {
      ConsoleCommand::Rename(new_name.to_string())
    }));
  }
  Some(parse_words(name, words))
}

/// The line after its first word, with the separating whitespace removed.
fn rest_after_command(line: &str) -> &str {
  let line = line.trim_start();
  let end = line.find(char::is_whitespace).unwrap_or(line.len());
  line[end..].trim_start()
}

/// What Tab completion draws on besides the line itself.
pub struct CompletionSource<'a> {
  /// Name of the selected entry, for `rename`.
  pub selected: Option<&'a str>,
  /// Programs on `PATH`, for `open_with`. Only called when a program name is completed.
  pub programs: &'a dyn Fn() -> &'a [String],
}

/// Names offered for the first word. Commands that take an argument end in a space so the
/// next Tab goes on to complete the argument.
fn command_names() -> Vec<String> {
  let mut names: Vec<String> = Action::ALL
    .iter()
    .map(|a| a.name())
    .filter(|n| !n.contains(' ') && *n != RENAME)
    .map(str::to_string)
    .collect();
  names.extend([OPEN_WITH, RENAME, SORT].map(|n| format!("{n} ")));
  names.sort();
  names.dedup();
  names
}

/// Completed lines for Tab, in the order they are cycled through; empty when nothing
/// matches. The first word completes to a command name; after it, `rename` offers the
/// selected name, `sort` the sort types and `open_with [-d]` the programs on `PATH`.
pub fn completions(line: &str, src: &CompletionSource) -> Vec<String> {
  let line = line.trim_start();
  let Some(split) = line.find(char::is_whitespace) else {
    return command_names()
      .into_iter()
      .filter(|n| n.starts_with(line))
      .collect();
  };
  let (command, arg) = (&line[..split], line[split..].trim_start());
  match command {
    RENAME => src
      .selected
      .filter(|name| name.starts_with(arg))
      .map(|name| format!("{RENAME} {name}"))
      .into_iter()
      .collect(),
    SORT => SortKey::ALL
      .iter()
      .map(|k| k.name())
      .filter(|k| k.starts_with(arg))
      .map(|k| format!("{SORT} {k}"))
      .collect(),
    OPEN_WITH => {
      let (head, program) = match arg.strip_prefix(DETACH_FLAG) {
        Some(after) if after.starts_with(char::is_whitespace) => {
          (format!("{OPEN_WITH} {DETACH_FLAG} "), after.trim_start())
        }
        _ => (format!("{OPEN_WITH} "), arg),
      };
      // Once the program is typed, its own arguments are not completed.
      if program.contains(char::is_whitespace) {
        return Vec::new();
      }
      (src.programs)()
        .iter()
        .filter(|p| p.starts_with(program))
        .map(|p| format!("{head}{p}"))
        .collect()
    }
    _ => Vec::new(),
  }
}

fn parse_words<'a>(
  name: &str,
  mut args: impl Iterator<Item = &'a str>,
) -> Result<ConsoleCommand, ConsoleError> {
  if name == OPEN_WITH {
    let mut first = args.next();
    let detach = first == Some(DETACH_FLAG);
    if detach {
      first = args.next();
    }
    let program = first.ok_or(ConsoleError::MissingProgram(OPEN_WITH))?;
    let argv = std::iter::once(program)
      .chain(args)
      .map(str::to_string)
      .collect();
    return Ok(ConsoleCommand::OpenWith { argv, detach });
  }
  let action = match name {
    "q" => Action::Quit,
    SORT => {
      let key = args.next().ok_or(ConsoleError::MissingSortKey)?;
      let key = key
        .parse::<SortKey>()
        .map_err(|_| ConsoleError::UnknownSortKey(key.to_string()))?;
      Action::Sort(key)
    }
    other => other
      .parse::<Action>()
      .map_err(|_| ConsoleError::Unknown(other.to_string()))?,
  };
  if let Some(arg) = args.next() {
    return Err(ConsoleError::UnexpectedArgument {
      command: name.to_string(),
      arg: arg.to_string(),
    });
  }
  Ok(ConsoleCommand::Action(action))
}
