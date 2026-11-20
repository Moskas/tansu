//! Key notation parsing and key-sequence to [`Action`] resolution.
//!
//! Notation follows vim: printable characters stand for themselves, special keys and modifiers
//! use angle brackets (`<Enter>`, `<C-d>`, `<A-x>`, `<S-Tab>`). Sequences concatenate (`gg`).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::action::Action;
use crate::fs::SortKey;

/// A normalized key press: SHIFT is folded into the character for printable keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
  pub code: KeyCode,
  pub mods: KeyModifiers,
}

impl Key {
  pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
    let mut mods = mods & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
    let code = match code {
      KeyCode::Char(c) => {
        mods.remove(KeyModifiers::SHIFT);
        if mods.contains(KeyModifiers::CONTROL) {
          KeyCode::Char(c.to_ascii_lowercase())
        } else {
          KeyCode::Char(c)
        }
      }
      KeyCode::BackTab => {
        mods.remove(KeyModifiers::SHIFT);
        KeyCode::BackTab
      }
      other => other,
    };
    Key { code, mods }
  }

  pub fn plain(code: KeyCode) -> Self {
    Key::new(code, KeyModifiers::NONE)
  }

  /// Converts a terminal key event, ignoring key releases.
  pub fn from_event(ev: &KeyEvent) -> Option<Self> {
    if ev.kind == KeyEventKind::Release {
      return None;
    }
    Some(Key::new(ev.code, ev.modifiers))
  }
}

impl fmt::Display for Key {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let special = match self.code {
      KeyCode::Char(' ') => Some("Space".to_string()),
      KeyCode::Char('<') => Some("lt".to_string()),
      KeyCode::Char(_) => None,
      KeyCode::Enter => Some("Enter".into()),
      KeyCode::Esc => Some("Esc".into()),
      KeyCode::Tab => Some("Tab".into()),
      KeyCode::BackTab => Some("S-Tab".into()),
      KeyCode::Backspace => Some("BS".into()),
      KeyCode::Up => Some("Up".into()),
      KeyCode::Down => Some("Down".into()),
      KeyCode::Left => Some("Left".into()),
      KeyCode::Right => Some("Right".into()),
      KeyCode::Home => Some("Home".into()),
      KeyCode::End => Some("End".into()),
      KeyCode::PageUp => Some("PageUp".into()),
      KeyCode::PageDown => Some("PageDown".into()),
      KeyCode::Delete => Some("Del".into()),
      KeyCode::Insert => Some("Insert".into()),
      KeyCode::F(n) => Some(format!("F{n}")),
      _ => Some("?".into()),
    };
    let mut prefix = String::new();
    if self.mods.contains(KeyModifiers::CONTROL) {
      prefix.push_str("C-");
    }
    if self.mods.contains(KeyModifiers::ALT) {
      prefix.push_str("A-");
    }
    if self.mods.contains(KeyModifiers::SHIFT) {
      prefix.push_str("S-");
    }
    match (special, self.code) {
      (None, KeyCode::Char(c)) if prefix.is_empty() => write!(f, "{c}"),
      (None, KeyCode::Char(c)) => write!(f, "<{prefix}{c}>"),
      (Some(name), _) => write!(f, "<{prefix}{name}>"),
      (None, _) => Ok(()),
    }
  }
}

/// Formats a key sequence in config notation, e.g. `gg` or `<C-d>`.
pub fn format_sequence(seq: &[Key]) -> String {
  seq.iter().map(Key::to_string).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyParseError {
  #[error("empty key sequence")]
  Empty,
  #[error("unterminated `<` in `{0}`")]
  Unterminated(String),
  #[error("unknown key name `{0}`")]
  UnknownKey(String),
  #[error("unknown modifier `{0}`")]
  UnknownModifier(String),
}

/// Parses a key sequence such as `gg`, `<C-d>` or `<lt>`.
pub fn parse_sequence(s: &str) -> Result<Vec<Key>, KeyParseError> {
  let mut keys = Vec::new();
  let mut chars = s.chars().peekable();
  while let Some(c) = chars.next() {
    if c == '<' && s.len() > 1 {
      let mut inner = String::new();
      let mut closed = false;
      for c in chars.by_ref() {
        if c == '>' && !inner.is_empty() {
          closed = true;
          break;
        }
        inner.push(c);
      }
      if !closed {
        return Err(KeyParseError::Unterminated(s.to_string()));
      }
      keys.push(parse_special(&inner)?);
    } else {
      keys.push(Key::plain(KeyCode::Char(c)));
    }
  }
  if keys.is_empty() {
    return Err(KeyParseError::Empty);
  }
  Ok(keys)
}

fn parse_special(inner: &str) -> Result<Key, KeyParseError> {
  let mut mods = KeyModifiers::NONE;
  let mut rest = inner;
  // Modifier prefixes look like `C-`; a lone `-` key name is still allowed.
  while rest.chars().count() > 2 && rest.chars().nth(1) == Some('-') {
    let m = rest.chars().next().unwrap_or_default();
    match m.to_ascii_uppercase() {
      'C' => mods |= KeyModifiers::CONTROL,
      'A' | 'M' => mods |= KeyModifiers::ALT,
      'S' => mods |= KeyModifiers::SHIFT,
      _ => return Err(KeyParseError::UnknownModifier(m.to_string())),
    }
    rest = &rest[m.len_utf8() + 1..];
  }

  let mut chars = rest.chars();
  if let (Some(c), None) = (chars.next(), chars.next()) {
    let c = if mods.contains(KeyModifiers::SHIFT) {
      c.to_ascii_uppercase()
    } else {
      c
    };
    return Ok(Key::new(KeyCode::Char(c), mods));
  }

  let lower = rest.to_ascii_lowercase();
  let code = match lower.as_str() {
    "enter" | "cr" | "return" => KeyCode::Enter,
    "esc" | "escape" => KeyCode::Esc,
    "tab" if mods.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
    "tab" => KeyCode::Tab,
    "bs" | "backspace" => KeyCode::Backspace,
    "space" => KeyCode::Char(' '),
    "lt" => KeyCode::Char('<'),
    "gt" => KeyCode::Char('>'),
    "up" => KeyCode::Up,
    "down" => KeyCode::Down,
    "left" => KeyCode::Left,
    "right" => KeyCode::Right,
    "home" => KeyCode::Home,
    "end" => KeyCode::End,
    "pageup" | "pgup" => KeyCode::PageUp,
    "pagedown" | "pgdn" => KeyCode::PageDown,
    "del" | "delete" => KeyCode::Delete,
    "insert" | "ins" => KeyCode::Insert,
    f if f.starts_with('f') => match f[1..].parse::<u8>() {
      Ok(n @ 1..=24) => KeyCode::F(n),
      _ => return Err(KeyParseError::UnknownKey(rest.to_string())),
    },
    _ => return Err(KeyParseError::UnknownKey(rest.to_string())),
  };
  Ok(Key::new(code, mods))
}

/// What a key sequence is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
  Action(Action),
  /// A command line, run as if typed after `:`.
  Line(Arc<str>),
}

/// Result of feeding one key into the keymap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyResult {
  Action(Action),
  /// Run this command line.
  Line(Arc<str>),
  /// The keys so far are a prefix of a longer binding (e.g. the first `g` of `gg`).
  Pending,
  Unbound,
}

/// What pressing one more key does while a sequence is pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuation {
  /// Completes the sequence and runs this action.
  Action(Action),
  /// Completes the sequence and runs this command line.
  Line(Arc<str>),
  /// Leads to this many longer bindings.
  Group(usize),
}

impl From<Binding> for Continuation {
  fn from(b: Binding) -> Self {
    match b {
      Binding::Action(a) => Continuation::Action(a),
      Binding::Line(l) => Continuation::Line(l),
    }
  }
}

impl From<Binding> for KeyResult {
  fn from(b: Binding) -> Self {
    match b {
      Binding::Action(a) => KeyResult::Action(a),
      Binding::Line(l) => KeyResult::Line(l),
    }
  }
}

/// Binding table plus the state needed to resolve multi-key sequences.
#[derive(Debug, Clone)]
pub struct Keymap {
  bindings: BTreeMap<Vec<KeyId>, (Vec<Key>, Binding)>,
  pending: Vec<Key>,
}

/// Orderable identity of a key, used as the binding table index.
type KeyId = String;

fn seq_id(seq: &[Key]) -> Vec<KeyId> {
  seq.iter().map(Key::to_string).collect()
}

pub const DEFAULT_BINDINGS: &[(&str, Action)] = &[
  ("j", Action::MoveDown),
  ("<Down>", Action::MoveDown),
  ("k", Action::MoveUp),
  ("<Up>", Action::MoveUp),
  ("h", Action::GoParent),
  ("<Left>", Action::GoParent),
  ("<BS>", Action::GoParent),
  ("l", Action::Open),
  ("<Right>", Action::Open),
  ("<Enter>", Action::Open),
  ("gg", Action::MoveTop),
  ("<Home>", Action::MoveTop),
  ("G", Action::MoveBottom),
  ("<End>", Action::MoveBottom),
  ("<C-d>", Action::HalfPageDown),
  ("<C-u>", Action::HalfPageUp),
  ("<PageDown>", Action::PageDown),
  ("<PageUp>", Action::PageUp),
  ("<C-j>", Action::PreviewDown),
  ("<C-k>", Action::PreviewUp),
  ("~", Action::GoHome),
  ("cd", Action::PromptCd),
  ("cz", Action::PromptZ),
  (".", Action::ToggleHidden),
  ("i", Action::ToggleMetadata),
  ("on", Action::Sort(SortKey::Name)),
  ("os", Action::Sort(SortKey::Size)),
  ("om", Action::Sort(SortKey::Mtime)),
  ("oc", Action::Sort(SortKey::Ctime)),
  ("<Space>", Action::ToggleMark),
  ("v", Action::InvertMarks),
  ("uv", Action::UnmarkAll),
  ("r", Action::Rename),
  ("yy", Action::Copy),
  ("dd", Action::Cut),
  ("pp", Action::Paste),
  ("po", Action::PasteOverwrite),
  ("dD", Action::Delete),
  ("dT", Action::Trash),
  ("md", Action::MakeDir),
  ("mf", Action::Touch),
  ("x", Action::Extract),
  ("w", Action::Tasks),
  ("uy", Action::ClearBuffer),
  ("ud", Action::ClearBuffer),
  ("yp", Action::YankPath),
  ("yd", Action::YankDir),
  ("yn", Action::YankName),
  ("y.", Action::YankNameWithoutExt),
  ("yf", Action::YankFile),
  ("!", Action::Shell),
  ("s", Action::Shell),
  ("/", Action::Search),
  ("n", Action::SearchNext),
  ("N", Action::SearchPrev),
  ("<Esc>", Action::ClearSearch),
  ("ms", Action::MarkMatches),
  ("R", Action::Reload),
  ("?", Action::ToggleHelp),
  (":", Action::CommandLine),
  ("gn", Action::TabNew),
  ("<C-n>", Action::TabNew),
  ("gc", Action::TabClose),
  ("<C-w>", Action::TabClose),
  ("gt", Action::TabNext),
  ("<Tab>", Action::TabNext),
  ("gT", Action::TabPrev),
  ("<S-Tab>", Action::TabPrev),
  ("uq", Action::TabRestore),
  ("<A-1>", Action::TabOpen(1)),
  ("<A-2>", Action::TabOpen(2)),
  ("<A-3>", Action::TabOpen(3)),
  ("<A-4>", Action::TabOpen(4)),
  ("<A-5>", Action::TabOpen(5)),
  ("<A-6>", Action::TabOpen(6)),
  ("<A-7>", Action::TabOpen(7)),
  ("<A-8>", Action::TabOpen(8)),
  ("<A-9>", Action::TabOpen(9)),
  ("q", Action::Quit),
  ("Q", Action::QuitAll),
  ("<C-c>", Action::QuitAll),
];

/// Value used in the config to remove a binding.
pub const UNBIND: &str = "none";

impl Default for Keymap {
  fn default() -> Self {
    let mut km = Keymap::empty();
    for (notation, action) in DEFAULT_BINDINGS {
      let seq = parse_sequence(notation).expect("default bindings are valid");
      km.bind(seq, *action);
    }
    km
  }
}

impl Keymap {
  pub fn empty() -> Self {
    Keymap {
      bindings: BTreeMap::new(),
      pending: Vec::new(),
    }
  }

  /// Default bindings with config overrides applied. Each override maps key notation to an
  /// action name, a command line or `"none"`. A command line must start with a built-in
  /// command or a custom one (`is_user_command`). Invalid entries are skipped and reported.
  pub fn with_overrides<'a>(
    overrides: impl IntoIterator<Item = (&'a String, &'a String)>,
    is_user_command: impl Fn(&str) -> bool,
  ) -> (Self, Vec<String>) {
    let mut km = Keymap::default();
    let mut warnings = Vec::new();
    for (keys, value) in overrides {
      let seq = match parse_sequence(keys) {
        Ok(seq) => seq,
        Err(e) => {
          warnings.push(format!("keys: `{keys}`: {e}"));
          continue;
        }
      };
      if value == UNBIND {
        km.unbind(&seq);
        continue;
      }
      if let Ok(a) = value.parse::<Action>() {
        km.bind(seq, a);
        continue;
      }
      let (name, _) = crate::console::split_command(value);
      if is_user_command(name) {
        km.bind_line(seq, value);
        continue;
      }
      match crate::console::parse(value) {
        Some(Ok(_)) => km.bind_line(seq, value),
        Some(Err(e)) => warnings.push(format!("keys: `{keys}`: {e}")),
        None => warnings.push(format!("keys: `{keys}`: empty command")),
      }
    }
    (km, warnings)
  }

  pub fn bind(&mut self, seq: Vec<Key>, action: Action) {
    self
      .bindings
      .insert(seq_id(&seq), (seq, Binding::Action(action)));
  }

  /// Binds `seq` to run a command line.
  pub fn bind_line(&mut self, seq: Vec<Key>, line: &str) {
    let line = Binding::Line(Arc::from(line.trim()));
    self.bindings.insert(seq_id(&seq), (seq, line));
  }

  pub fn unbind(&mut self, seq: &[Key]) {
    self.bindings.remove(&seq_id(seq));
  }

  pub fn lookup(&self, seq: &[Key]) -> Option<Binding> {
    self.bindings.get(&seq_id(seq)).map(|(_, b)| b.clone())
  }

  /// All bindings for `action`, in notation form.
  pub fn keys_for(&self, action: Action) -> Vec<String> {
    self
      .bindings
      .values()
      .filter(|(_, b)| *b == Binding::Action(action))
      .map(|(seq, _)| format_sequence(seq))
      .collect()
  }

  /// Bindings to command lines, as (keys in notation form, line), sorted by keys.
  pub fn line_bindings(&self) -> Vec<(String, Arc<str>)> {
    self
      .bindings
      .values()
      .filter_map(|(seq, b)| match b {
        Binding::Line(line) => Some((format_sequence(seq), Arc::clone(line))),
        Binding::Action(_) => None,
      })
      .collect()
  }

  pub fn is_pending(&self) -> bool {
    !self.pending.is_empty()
  }

  /// The keys typed so far of an unfinished sequence, in notation (`g`, `<C-x>`).
  pub fn pending_notation(&self) -> String {
    format_sequence(&self.pending)
  }

  /// What can follow the pending keys, one entry per next key, sorted by key.
  pub fn continuations(&self) -> Vec<(String, Continuation)> {
    let depth = self.pending.len();
    let mut next: BTreeMap<String, Continuation> = BTreeMap::new();
    for (seq, binding) in self.bindings.values() {
      if seq.len() <= depth || seq[..depth] != self.pending[..] {
        continue;
      }
      let key = seq[depth].to_string();
      if seq.len() == depth + 1 {
        next.insert(key, binding.clone().into());
      } else if let Continuation::Group(n) = next.entry(key).or_insert(Continuation::Group(0)) {
        *n += 1;
      }
    }
    next.into_iter().collect()
  }

  pub fn reset(&mut self) {
    self.pending.clear();
  }

  pub fn feed(&mut self, key: Key) -> KeyResult {
    self.pending.push(key);
    match self.resolve() {
      KeyResult::Unbound if self.pending.len() > 1 => {
        // The sequence broke off; start over with just the latest key.
        self.pending.clear();
        self.pending.push(key);
        let res = self.resolve();
        if res != KeyResult::Pending {
          self.pending.clear();
        }
        res
      }
      KeyResult::Pending => KeyResult::Pending,
      res => {
        self.pending.clear();
        res
      }
    }
  }

  fn resolve(&self) -> KeyResult {
    let id = seq_id(&self.pending);
    if let Some((_, binding)) = self.bindings.get(&id) {
      return binding.clone().into();
    }
    let is_prefix = self
      .bindings
      .range(id.clone()..)
      .next()
      .is_some_and(|(k, _)| k.starts_with(&id));
    if is_prefix {
      KeyResult::Pending
    } else {
      KeyResult::Unbound
    }
  }
}
