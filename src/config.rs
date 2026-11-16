//! User configuration: `$XDG_CONFIG_HOME/tansu/config.toml`. Every field is optional; a missing
//! file means defaults.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ratatui::style::Color;
use serde::{Deserialize, Deserializer};

use crate::preview::{HighlightConfig, PreviewLimits};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  /// Show dotfiles on startup.
  pub show_hidden: bool,
  /// Nerd Font icons when true, ASCII markers when false.
  pub icons: bool,
  /// Command used to open files; the path is appended as the last argument.
  pub opener: String,
  /// Width ratios of the parent, current and preview columns. A zero hides that column.
  pub column_ratios: [u16; 3],
  /// Show git status markers next to entries (like ranger's `vcs_aware`).
  pub vcs_aware: bool,
  /// Sizes in base 1024 with `Ki`/`Mi` units instead of base 1000 with `k`/`M`.
  pub binary_size_prefix: bool,
  /// Show images in the preview column with the terminal's graphics protocol (like
  /// ranger's `preview_images`).
  pub preview_images: bool,
  pub preview: PreviewLimits,
  /// Syntax highlighting of text previews through `bat`.
  pub highlight: HighlightConfig,
  pub theme: Theme,
  /// Key notation to action name, e.g. `"J" = "half_page_down"`. `"none"` unbinds.
  pub keys: BTreeMap<String, String>,
}

impl Default for Config {
  fn default() -> Self {
    Config {
      show_hidden: false,
      icons: true,
      opener: "xdg-open".to_string(),
      column_ratios: [1, 3, 4],
      vcs_aware: true,
      binary_size_prefix: false,
      preview_images: true,
      preview: PreviewLimits::default(),
      highlight: HighlightConfig::default(),
      theme: Theme::default(),
      keys: BTreeMap::new(),
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

const fn hex(rgb: u32) -> ThemeColor {
  ThemeColor(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}

/// All colors used by the UI. Defaults to a Gruvbox-dark palette.
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
  /// Selected-row colors. Unset, the row is drawn in reverse video like ranger: the entry's
  /// color becomes the background. Setting either one overrides that side only.
  pub cursor_fg: Option<ThemeColor>,
  pub cursor_bg: Option<ThemeColor>,
  pub status_fg: ThemeColor,
  pub status_perms: ThemeColor,
  /// Permissions of entries owned by another user.
  pub status_perms_other: ThemeColor,
  pub error: ThemeColor,
  pub help_key: ThemeColor,
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
      fg: hex(0xebdbb2),
      dim: hex(0x928374),
      border: hex(0xd5c4a1),
      header_user: hex(0xb8bb26),
      header_path: hex(0xebdbb2),
      // The terminal palette's bright blue, the color ranger uses for directories.
      directory: ThemeColor(Color::LightBlue),
      file: hex(0xebdbb2),
      executable: hex(0xb8bb26),
      symlink: hex(0x8ec07c),
      broken_link: hex(0xd3869b),
      archive: hex(0xfb4934),
      image: hex(0xfabd2f),
      video: hex(0xd3869b),
      audio: hex(0xd3869b),
      document: hex(0xebdbb2),
      code: hex(0xebdbb2),
      special: hex(0xd79921),
      cursor_fg: None,
      cursor_bg: None,
      status_fg: hex(0xd5c4a1),
      status_perms: hex(0x8ec07c),
      status_perms_other: hex(0xd3869b),
      error: hex(0xfb4934),
      help_key: hex(0xfabd2f),
      vcs_sync: hex(0xb8bb26),
      vcs_changed: hex(0xfb4934),
      vcs_staged: hex(0xb8bb26),
      vcs_untracked: hex(0x8ec07c),
      vcs_conflict: hex(0xd3869b),
      vcs_ignored: hex(0xebdbb2),
      vcs_ahead: hex(0x83a598),
      vcs_behind: hex(0xfb4934),
      vcs_diverged: hex(0xd3869b),
      vcs_info: hex(0x83a598),
      vcs_date: hex(0x8ec07c),
      vcs_commit: hex(0xfabd2f),
    }
  }
}
