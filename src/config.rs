//! User configuration: `$XDG_CONFIG_HOME/tansu/config.toml`. Every field is optional; a missing
//! file means defaults.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ratatui::style::Color;
use serde::{Deserialize, Deserializer};

use crate::fs::SortKey;
use crate::preview::{HighlightConfig, PreviewLimits};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  /// Show dotfiles on startup.
  pub show_hidden: bool,
  /// Initial sort order: `name`, `size`, `mtime` or `ctime`. Directories always come first.
  pub sort: SortKey,
  /// Nerd Font icons when true, ASCII markers when false.
  pub icons: bool,
  /// Command used to open files; the path is appended as the last argument.
  pub opener: String,
  /// Width ratios of the parent, current and preview columns. A zero hides that column.
  pub column_ratios: [u16; 3],
  /// Show git status markers next to entries (like ranger's `vcs_aware`).
  pub vcs_aware: bool,
  /// Add each directory entered to the zoxide database (`zoxide add`), like the shell hook.
  pub zoxide_add: bool,
  /// Sizes in base 1024 with `Ki`/`Mi` units instead of base 1000 with `k`/`M`.
  pub binary_size_prefix: bool,
  /// Rows of context kept above and below the cursor while scrolling (like ranger's
  /// `scroll_offset`). When the list is too short for that, the cursor stays centered.
  pub scroll_offset: usize,
  /// Show images in the preview column with the terminal's graphics protocol (like
  /// ranger's `preview_images`).
  pub preview_images: bool,
  /// Show the keys that can follow an unfinished key sequence, like which-key in vim and
  /// emacs.
  pub which_key: bool,
  /// React to clicks and the wheel (like ranger's `mouse_enabled`). Capturing the mouse
  /// takes over the terminal's own text selection, which then needs a modifier (usually
  /// Shift).
  pub mouse_enabled: bool,
  /// Keep the command-line history across sessions in `$XDG_DATA_HOME/tansu/history` (like
  /// ranger's `save_console_history`).
  pub save_console_history: bool,
  pub preview: PreviewLimits,
  /// Syntax highlighting of text previews through `bat`.
  pub highlight: HighlightConfig,
  pub theme: Theme,
  /// Key notation to an action name or a command line, e.g. `"J" = "half_page_down"` or
  /// `"ge" = "shell -w nvim %s"`. `"none"` unbinds.
  pub keys: BTreeMap<String, String>,
  /// Custom commands, by name: command lines run in order, like ranger's `commands.py`.
  pub commands: BTreeMap<String, UserCommand>,
  /// Set the terminal title to the current directory (like ranger's `update_title`).
  pub update_title: bool,
  /// List the visible directories again when other programs add, remove or rename entries
  /// in them. They are checked once a second, by the directory's modification time.
  pub auto_refresh: bool,
  /// How files are opened by type, like ranger's rifle: the first matching rule wins, and
  /// `opener` handles files no rule matches.
  pub open: Vec<OpenRule>,
}

/// An `[[open]]` rule: a command line to open files whose name and MIME type match.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRule {
  /// Globs on the file name (`*.mkv`, `Makefile`); one has to match.
  #[serde(default, deserialize_with = "lines")]
  pub name: Vec<String>,
  /// Globs on the MIME type from `file --mime-type` (`text/*`); one has to match.
  #[serde(default, deserialize_with = "lines")]
  pub mime: Vec<String>,
  /// The command line to run, as in `[keys]`, e.g. `shell emacs -nw %s`.
  pub run: String,
}

impl OpenRule {
  /// Whether the rule needs the file's MIME type to decide.
  pub fn needs_mime(&self) -> bool {
    !self.mime.is_empty()
  }

  /// Whether a file named `name` with MIME type `mime` (`None` when unknown) matches.
  pub fn matches(&self, name: &str, mime: Option<&str>) -> bool {
    let any = |globs: &[String], text: &str| {
      globs
        .iter()
        .any(|g| crate::search::Pattern::glob(g).is_some_and(|p| p.score(text).is_some()))
    };
    (self.name.is_empty() || any(&self.name, name))
      && (self.mime.is_empty() || mime.is_some_and(|m| any(&self.mime, m)))
  }
}

fn lines<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
  Lines::deserialize(d).map(Vec::from)
}

/// A custom command from `[commands]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "UserCommandRepr")]
pub struct UserCommand {
  /// Command lines, run in order. `%a` in them stands for the arguments typed after the
  /// command's name.
  pub run: Vec<String>,
  /// Shown in the help and which-key popups instead of the command line.
  pub description: Option<String>,
}

/// The forms a custom command can be written in: one line, a list of lines, or a table with
/// `run` and `description`.
#[derive(Deserialize)]
#[serde(untagged)]
enum UserCommandRepr {
  Lines(Lines),
  Table(UserCommandTable),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserCommandTable {
  run: Lines,
  description: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Lines {
  One(String),
  Many(Vec<String>),
}

impl From<Lines> for Vec<String> {
  fn from(lines: Lines) -> Self {
    match lines {
      Lines::One(line) => vec![line],
      Lines::Many(lines) => lines,
    }
  }
}

impl From<UserCommandRepr> for UserCommand {
  fn from(repr: UserCommandRepr) -> Self {
    match repr {
      UserCommandRepr::Lines(lines) => UserCommand {
        run: lines.into(),
        description: None,
      },
      UserCommandRepr::Table(t) => UserCommand {
        run: t.run.into(),
        description: t.description,
      },
    }
  }
}

impl Default for Config {
  fn default() -> Self {
    Config {
      show_hidden: false,
      sort: SortKey::Name,
      icons: true,
      opener: "xdg-open".to_string(),
      column_ratios: [1, 3, 4],
      vcs_aware: true,
      zoxide_add: true,
      binary_size_prefix: false,
      scroll_offset: 8,
      preview_images: true,
      which_key: true,
      mouse_enabled: true,
      save_console_history: true,
      preview: PreviewLimits::default(),
      highlight: HighlightConfig::default(),
      theme: Theme::default(),
      keys: BTreeMap::new(),
      commands: BTreeMap::new(),
      update_title: false,
      auto_refresh: true,
      open: Vec::new(),
    }
  }
}

impl Config {
  /// `$XDG_CONFIG_HOME/tansu/config.toml` (or the platform equivalent).
  pub fn default_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "tansu").map(|d| d.config_dir().join("config.toml"))
  }

  pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(text)
  }

  /// Loads the config at `path`, falling back to defaults. Problems are returned as warnings
  /// instead of errors so a broken config never prevents startup.
  pub fn load(path: Option<&Path>) -> (Config, Vec<String>) {
    let Some(path) = path else {
      return (Config::default(), Vec::new());
    };
    let text = match std::fs::read_to_string(path) {
      Ok(t) => t,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
        return (Config::default(), Vec::new());
      }
      Err(e) => {
        return (
          Config::default(),
          vec![format!("config: {}: {e}", path.display())],
        );
      }
    };
    match Config::parse(&text) {
      Ok(mut cfg) => {
        let warnings = cfg.validate();
        (cfg, warnings)
      }
      Err(e) => {
        let msg = e.message().to_string();
        (
          Config::default(),
          vec![format!("config: {}: {msg}", path.display())],
        )
      }
    }
  }

  /// Fixes out-of-range values in place and reports what was changed.
  pub fn validate(&mut self) -> Vec<String> {
    let mut warnings = Vec::new();
    if self.column_ratios[1] == 0 {
      warnings.push("config: column_ratios: the middle column cannot be 0".to_string());
      self.column_ratios = Config::default().column_ratios;
    }
    if self.opener.split_whitespace().next().is_none() {
      warnings.push("config: opener is empty, using xdg-open".to_string());
      self.opener = Config::default().opener;
    }
    if self.highlight.enabled && self.highlight.command.trim().is_empty() {
      warnings.push("config: highlight.command is empty, using bat".to_string());
      self.highlight.command = HighlightConfig::default().command;
    }
    let p = &mut self.preview;
    p.max_bytes = p.max_bytes.clamp(1024, 16 * 1024 * 1024);
    p.max_lines = p.max_lines.max(1);
    p.max_entries = p.max_entries.max(1);
    self.commands.retain(|name, cmd| {
      let problem = if name.is_empty() || name.contains(char::is_whitespace) {
        "a name cannot be empty or contain spaces"
      } else if crate::console::is_builtin(name) {
        "a built-in command has this name"
      } else if cmd.run.iter().all(|l| l.trim().is_empty()) {
        "nothing to run"
      } else {
        return true;
      };
      warnings.push(format!("config: commands: `{name}`: {problem}"));
      false
    });
    let mut index = 0;
    self.open.retain(|rule| {
      index += 1;
      let problem = if rule.name.is_empty() && rule.mime.is_empty() {
        "needs `name` or `mime`"
      } else if rule.run.trim().is_empty() {
        "nothing to run"
      } else {
        return true;
      };
      warnings.push(format!("config: open: rule {index}: {problem}"));
      false
    });
    warnings
  }
}

/// A color parsed from `#rrggbb`, a named color (`red`, `lightblue`), an index (`208`) or
/// `reset` for the terminal default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColor(pub Color);

impl<'de> Deserialize<'de> for ThemeColor {
  fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
    let s = String::deserialize(d)?;
    Color::from_str(&s)
      .map(ThemeColor)
      .map_err(|_| serde::de::Error::custom(format!("invalid color `{s}`")))
  }
}

impl From<ThemeColor> for Color {
  fn from(c: ThemeColor) -> Self {
    c.0
  }
}

// Default colors are the terminal's own: its default foreground and the 16 palette colors a
// base16 scheme sets, so tansu follows the terminal's theme. The bright variants (9-14) are
// used, as ranger does; base16 makes them the same as the normal ones.
/// The terminal's default foreground.
const DEFAULT: ThemeColor = ThemeColor(Color::Reset);
/// Color 7 (base05, the foreground in base16).
const FOREGROUND: ThemeColor = ThemeColor(Color::Gray);
/// Color 8 (base03, comments in base16).
const COMMENT: ThemeColor = ThemeColor(Color::DarkGray);
const RED: ThemeColor = ThemeColor(Color::LightRed);
const GREEN: ThemeColor = ThemeColor(Color::LightGreen);
const YELLOW: ThemeColor = ThemeColor(Color::LightYellow);
/// Color 3. base16 has no orange among the 16, so warnings and special files use the
/// normal yellow, which other palettes draw darker than color 11.
const DARK_YELLOW: ThemeColor = ThemeColor(Color::Yellow);
const BLUE: ThemeColor = ThemeColor(Color::LightBlue);
const MAGENTA: ThemeColor = ThemeColor(Color::LightMagenta);
const CYAN: ThemeColor = ThemeColor(Color::LightCyan);

/// All colors used by the UI. Defaults to the terminal's palette (see [`DEFAULT`]).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
  pub fg: ThemeColor,
  pub dim: ThemeColor,
  pub border: ThemeColor,
  pub header_user: ThemeColor,
  pub header_path: ThemeColor,
  pub directory: ThemeColor,
  pub file: ThemeColor,
  pub executable: ThemeColor,
  pub symlink: ThemeColor,
  pub broken_link: ThemeColor,
  pub archive: ThemeColor,
  pub image: ThemeColor,
  pub video: ThemeColor,
  pub audio: ThemeColor,
  pub document: ThemeColor,
  pub code: ThemeColor,
  pub special: ThemeColor,
  /// Marked entries, drawn bold in this color like ranger's yellow.
  pub marked: ThemeColor,
  /// The count of marked entries on a tab's label in the header.
  pub tab_marks: ThemeColor,
  /// Selected-row colors. Unset, the row is drawn in reverse video like ranger: the entry's
  /// color becomes the background. Setting either one overrides that side only.
  pub cursor_fg: Option<ThemeColor>,
  pub cursor_bg: Option<ThemeColor>,
  pub status_fg: ThemeColor,
  pub status_perms: ThemeColor,
  /// Permissions of entries owned by another user.
  pub status_perms_other: ThemeColor,
  pub error: ThemeColor,
  /// Task progress bars and the keys in the task popup.
  pub progress: ThemeColor,
  /// The empty part of a progress bar.
  pub progress_track: ThemeColor,
  /// Finished tasks.
  pub success: ThemeColor,
  /// Questions that need an answer before a task goes on (a paste conflict).
  pub warning: ThemeColor,
  pub help_key: ThemeColor,
  /// Prefixes of longer sequences in the which-key popup.
  pub help_group: ThemeColor,
  pub vcs_sync: ThemeColor,
  pub vcs_changed: ThemeColor,
  pub vcs_staged: ThemeColor,
  pub vcs_untracked: ThemeColor,
  pub vcs_conflict: ThemeColor,
  pub vcs_ignored: ThemeColor,
  pub vcs_ahead: ThemeColor,
  pub vcs_behind: ThemeColor,
  pub vcs_diverged: ThemeColor,
  pub vcs_info: ThemeColor,
  pub vcs_date: ThemeColor,
  pub vcs_commit: ThemeColor,
}

impl Default for Theme {
  fn default() -> Self {
    Theme {
      fg: DEFAULT,
      dim: COMMENT,
      border: FOREGROUND,
      header_user: GREEN,
      header_path: BLUE,
      // Bright blue, the color ranger uses for directories.
      directory: BLUE,
      file: DEFAULT,
      executable: GREEN,
      symlink: CYAN,
      broken_link: MAGENTA,
      archive: RED,
      image: YELLOW,
      video: MAGENTA,
      audio: MAGENTA,
      document: DEFAULT,
      code: DEFAULT,
      special: DARK_YELLOW,
      marked: YELLOW,
      tab_marks: MAGENTA,
      cursor_fg: None,
      cursor_bg: None,
      status_fg: DEFAULT,
      status_perms: CYAN,
      status_perms_other: MAGENTA,
      error: RED,
      progress: YELLOW,
      progress_track: COMMENT,
      success: GREEN,
      warning: DARK_YELLOW,
      help_key: YELLOW,
      help_group: RED,
      vcs_sync: GREEN,
      vcs_changed: RED,
      vcs_staged: GREEN,
      vcs_untracked: CYAN,
      vcs_conflict: MAGENTA,
      vcs_ignored: DEFAULT,
      vcs_ahead: BLUE,
      vcs_behind: RED,
      vcs_diverged: MAGENTA,
      vcs_info: BLUE,
      vcs_date: CYAN,
      vcs_commit: YELLOW,
    }
  }
}
