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
  /// Change to this path, taken verbatim; empty means home and `-` the previous directory.
  Cd(String),
  /// Jump to the best zoxide match for these keywords, like the shell's `z`.
  Zoxide(Vec<String>),
  /// Run `command` through the shell. `flags` are the letters after a leading `-`.
  Shell { flags: String, command: String },
  /// Search the current directory for this pattern, taken verbatim (spaces included).
  Search(String),
  /// Create a directory (`dir`) or an empty file at this path, relative to the current
  /// directory and taken verbatim.
  Create { name: String, dir: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsoleError {
  #[error("unknown command `{0}`")]
  Unknown(String),
  #[error("{0}: missing program")]
  MissingProgram(&'static str),
  #[error("shell: missing command")]
  MissingCommand,
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

/// Name of the command that changes directory.
pub const CD: &str = "cd";

/// Name of the command that jumps to a directory through zoxide.
pub const Z: &str = "z";

/// Name of the command that runs a shell command line. Without one it opens the command
/// line as `shell `.
pub const SHELL: &str = "shell";

/// Name of the command that searches the current directory. Without a pattern it opens the
/// command line as `search `.
pub const SEARCH: &str = "search";

/// Names of the commands that create a directory and an empty file. Without a name they open
/// the command line as `mkdir ` / `touch `.
pub const MKDIR: &str = "mkdir";
pub const TOUCH: &str = "touch";

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
  if name == MKDIR || name == TOUCH {
    // Names may contain spaces, so the rest of the line is the name.
    let dir = name == MKDIR;
    let new = rest_after_command(line).trim_end();
    return Some(Ok(match (new.is_empty(), dir) {
      (true, true) => ConsoleCommand::Action(Action::MakeDir),
      (true, false) => ConsoleCommand::Action(Action::Touch),
      (false, _) => ConsoleCommand::Create {
        name: new.to_string(),
        dir,
      },
    }));
  }
  if name == SEARCH {
    // Names may contain spaces, so the rest of the line is the pattern.
    let pattern = rest_after_command(line).trim_end();
    return Some(Ok(if pattern.is_empty() {
      ConsoleCommand::Action(Action::Search)
    } else {
      ConsoleCommand::Search(pattern.to_string())
    }));
  }
  if name == SHELL {
    let rest = rest_after_command(line);
    if rest.trim().is_empty() {
      return Some(Ok(ConsoleCommand::Action(Action::Shell)));
    }
    // The command goes to the shell as typed, quotes and all.
    let (flags, command) = match rest.strip_prefix('-') {
      Some(after) => {
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        (&after[..end], after[end..].trim_start())
      }
      None => ("", rest),
    };
    if command.trim().is_empty() {
      return Some(Err(ConsoleError::MissingCommand));
    }
    return Some(Ok(ConsoleCommand::Shell {
      flags: flags.to_string(),
      command: command.to_string(),
    }));
  }
  match name {
    // Paths may contain spaces, so the rest of the line is the path.
    CD => Some(Ok(ConsoleCommand::Cd(
      rest_after_command(line).trim().to_string(),
    ))),
    Z => Some(Ok(ConsoleCommand::Zoxide(
      words.map(str::to_string).collect(),
    ))),
    _ => Some(parse_words(name, words)),
  }
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
  /// Directory names inside a typed directory (`""`, `src/`, `~/`), for `cd`.
  pub subdirs: &'a dyn Fn(&str) -> Vec<String>,
  /// Names of the listed entries in the current directory, for `shell` arguments.
  pub files: &'a dyn Fn() -> Vec<String>,
}

/// Names offered for the first word. Commands that take an argument end in a space so the
/// next Tab goes on to complete the argument.
fn command_names() -> Vec<String> {
  let mut names: Vec<String> = Action::ALL
    .iter()
    .map(|a| a.name())
    .filter(|n| !n.contains(' ') && ![MKDIR, RENAME, SEARCH, SHELL, TOUCH].contains(n))
    .map(str::to_string)
    .collect();
  let with_argument = [CD, MKDIR, OPEN_WITH, RENAME, SEARCH, SHELL, SORT, TOUCH, Z];
  names.extend(with_argument.map(|n| format!("{n} ")));
  names.sort();
  names.dedup();
  names
}

/// Completed lines for Tab, in the order they are cycled through; empty when nothing
/// matches. The first word completes to a command name; after it, `rename` offers the
/// selected name, `sort` the sort types, `open_with [-d]` the programs on `PATH` and `cd`
/// the directories at the typed path. Hidden directories are offered once a `.` is typed.
/// `shell` completes like ranger's: see [`complete_shell`].
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
    CD if arg == "~" => vec![format!("{CD} ~/")],
    CD => {
      let (dir, prefix) = arg.split_at(arg.rfind('/').map_or(0, |i| i + 1));
      let hidden = prefix.starts_with('.');
      (src.subdirs)(dir)
        .into_iter()
        .filter(|n| n.starts_with(prefix) && (hidden || !n.starts_with('.')))
        .map(|n| format!("{CD} {dir}{n}/"))
        .collect()
    }
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
    SHELL => complete_shell(line, arg, src),
    _ => Vec::new(),
  }
}

/// Completion for `shell [-flags] command`, as in ranger: the first word of the command
/// completes to a program on `PATH`; after a space the selected name is inserted; a partial
/// word completes to the names in the current directory. Names are backslash-escaped.
fn complete_shell(line: &str, arg: &str, src: &CompletionSource) -> Vec<String> {
  let command = match arg.strip_prefix('-') {
    Some(after) => match after.find(char::is_whitespace) {
      Some(end) => after[end..].trim_start(),
      // Still typing the flags.
      None => return Vec::new(),
    },
    None => arg,
  };
  let head = &line[..line.len() - command.len()];
  if !command.contains(' ') {
    return (src.programs)()
      .iter()
      .filter(|p| p.starts_with(command))
      .map(|p| format!("{head}{p} "))
      .collect();
  }
  if command.ends_with(' ') {
    return vec![match src.selected {
      Some(name) => format!("{line}{} ", crate::shell::shell_escape(name)),
      None => format!("{line}%s "),
    }];
  }
  let Some((before, word)) = line.rsplit_once(' ') else {
    return Vec::new();
  };
  (src.files)()
    .iter()
    .map(|n| crate::shell::shell_escape(n))
    .filter(|n| n.starts_with(word))
    .map(|n| format!("{before} {n}"))
    .collect()
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
