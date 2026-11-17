use std::fmt;
use std::str::FromStr;

use crate::fs::SortKey;

/// Everything the user can ask the application to do. Keys are mapped to actions through the
/// keymap; the app only ever reacts to actions, never to raw keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
  MoveUp,
  MoveDown,
  MoveTop,
  MoveBottom,
  HalfPageUp,
  HalfPageDown,
  PageUp,
  PageDown,
  GoParent,
  Open,
  GoHome,
  ToggleHidden,
  ToggleMetadata,
  Sort(SortKey),
  Rename,
  Copy,
  Cut,
  Paste,
  PasteOverwrite,
  ClearBuffer,
  YankPath,
  YankDir,
  YankName,
  YankNameWithoutExt,
  YankFile,
  Shell,
  Reload,
  ToggleHelp,
  CommandLine,
  Quit,
}

impl Action {
  pub const ALL: [Action; 33] = [
    Action::MoveUp,
    Action::MoveDown,
    Action::MoveTop,
    Action::MoveBottom,
    Action::HalfPageUp,
    Action::HalfPageDown,
    Action::PageUp,
    Action::PageDown,
    Action::GoParent,
    Action::Open,
    Action::GoHome,
    Action::ToggleHidden,
    Action::ToggleMetadata,
    Action::Sort(SortKey::Name),
    Action::Sort(SortKey::Size),
    Action::Sort(SortKey::Mtime),
    Action::Sort(SortKey::Ctime),
    Action::Rename,
    Action::Copy,
    Action::Cut,
    Action::Paste,
    Action::PasteOverwrite,
    Action::ClearBuffer,
    Action::YankPath,
    Action::YankDir,
    Action::YankName,
    Action::YankNameWithoutExt,
    Action::YankFile,
    Action::Shell,
    Action::Reload,
    Action::ToggleHelp,
    Action::CommandLine,
    Action::Quit,
  ];

  /// Stable identifier used in the config file. It is also a valid command line.
  pub fn name(self) -> &'static str {
    match self {
      Action::MoveUp => "move_up",
      Action::MoveDown => "move_down",
      Action::MoveTop => "move_top",
      Action::MoveBottom => "move_bottom",
      Action::HalfPageUp => "half_page_up",
      Action::HalfPageDown => "half_page_down",
      Action::PageUp => "page_up",
      Action::PageDown => "page_down",
      Action::GoParent => "go_parent",
      Action::Open => "open",
      Action::GoHome => "go_home",
      Action::ToggleHidden => "toggle_hidden",
      Action::ToggleMetadata => "toggle_metadata",
      Action::Sort(SortKey::Name) => "sort name",
      Action::Sort(SortKey::Size) => "sort size",
      Action::Sort(SortKey::Mtime) => "sort mtime",
      Action::Sort(SortKey::Ctime) => "sort ctime",
      Action::Rename => "rename",
      Action::Copy => "copy",
      Action::Cut => "cut",
      Action::Paste => "paste",
      Action::PasteOverwrite => "paste_overwrite",
      Action::ClearBuffer => "clear_buffer",
      Action::YankPath => "yank_path",
      Action::YankDir => "yank_dir",
      Action::YankName => "yank_name",
      Action::YankNameWithoutExt => "yank_name_without_ext",
      Action::YankFile => "yank_file",
      Action::Shell => "shell",
      Action::Reload => "reload",
      Action::ToggleHelp => "toggle_help",
      Action::CommandLine => "command_line",
      Action::Quit => "quit",
    }
  }

  pub fn description(self) -> &'static str {
    match self {
      Action::MoveUp => "Move cursor up",
      Action::MoveDown => "Move cursor down",
      Action::MoveTop => "Jump to first entry",
      Action::MoveBottom => "Jump to last entry",
      Action::HalfPageUp => "Half page up",
      Action::HalfPageDown => "Half page down",
      Action::PageUp => "Page up",
      Action::PageDown => "Page down",
      Action::GoParent => "Go to parent directory",
      Action::Open => "Enter directory / open file",
      Action::GoHome => "Go to home directory",
      Action::ToggleHidden => "Toggle hidden files",
      Action::ToggleMetadata => "Toggle file metadata / preview",
      Action::Sort(SortKey::Name) => "Sort by name",
      Action::Sort(SortKey::Size) => "Sort by size, largest first",
      Action::Sort(SortKey::Mtime) => "Sort by modification time, newest first",
      Action::Sort(SortKey::Ctime) => "Sort by change time, newest first",
      Action::Rename => "Rename the selection",
      Action::Copy => "Copy the selection into the buffer",
      Action::Cut => "Cut the selection into the buffer",
      Action::Paste => "Paste the buffer here, never overwriting",
      Action::PasteOverwrite => "Paste the buffer here, overwriting",
      Action::ClearBuffer => "Clear the copy/cut buffer",
      Action::YankPath => "Yank the selection's path",
      Action::YankDir => "Yank the current directory's path",
      Action::YankName => "Yank the selection's name",
      Action::YankNameWithoutExt => "Yank the selection's name without extension",
      Action::YankFile => "Yank the selection as a file (text/uri-list)",
      Action::Shell => "Run a shell command",
      Action::Reload => "Reload directory",
      Action::ToggleHelp => "Toggle this help",
      Action::CommandLine => "Open the command line",
      Action::Quit => "Quit",
    }
  }
}

impl fmt::Display for Action {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.name())
  }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown action `{0}`")]
pub struct UnknownAction(pub String);

impl FromStr for Action {
  type Err = UnknownAction;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    Action::ALL
      .into_iter()
      .find(|a| a.name() == s)
      .ok_or_else(|| UnknownAction(s.to_string()))
  }
}
