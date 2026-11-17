use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

use super::text::{clip, display_width, truncate};
use crate::app::App;
use crate::fs::entry::sanitize;

/// `user@host /current/dir/selected`, like ranger's title bar: the directory part in the
/// path color (symlinked components in the symlink color), the selected entry's name in the
/// default color. When it does not fit, directory
/// names are cut down to their first character from the left (`/h/m/Downloads/...`).
pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
  let theme = &app.config.theme;
  let user = Style::new().fg(theme.header_user.0).bold();
  let path = Style::new().fg(theme.header_path.0).bold();
  let link = Style::new().fg(theme.symlink.0).bold();
  let name = Style::new().fg(theme.fg.0).bold();

  let mut parts = vec![
    Part::fixed(app.user_host.clone(), user),
    Part::fixed(" ".into(), user),
  ];
  parts.extend(dir_parts(&app.cwd, &app.cwd_symlinks, path, link));
  if let Some(entry) = app.selected() {
    parts.push(Part::shrinkable(sanitize(&entry.name), name));
  }

  let width = usize::from(area.width);
  let line = if shrink_from_left(&mut parts, width) {
    Line::from(
      parts
        .into_iter()
        .map(|p| Span::styled(p.text, p.style))
        .collect::<Vec<_>>(),
    )
  } else {
    // Even the fixed parts do not fit: cut the whole line.
    let text: String = parts.iter().map(|p| p.text.as_str()).collect();
    Line::styled(truncate(&text, width), user)
  };
  line.render(area, buf);
}

struct Part {
  text: String,
  style: Style,
  /// Fixed parts (`user@host`, separators) are never shortened.
  fixed: bool,
}

impl Part {
  fn fixed(text: String, style: Style) -> Self {
    Part {
      text,
      style,
      fixed: true,
    }
  }

  fn shrinkable(text: String, style: Style) -> Self {
    Part {
      text,
      style,
      fixed: false,
    }
  }

  /// Width a shrinkable part may be cut down to: its first character.
  fn min_width(&self) -> usize {
    self
      .text
      .chars()
      .next()
      .map_or(0, |c| UnicodeWidthChar::width(c).unwrap_or(0))
  }
}

/// `/`, then every directory name followed by a fixed `/`, so cut names keep their separators.
/// Components listed in `symlinks` (as paths up to and including them) get `link_style`.
fn dir_parts(
  dir: &Path,
  symlinks: &HashSet<PathBuf>,
  style: Style,
  link_style: Style,
) -> Vec<Part> {
  let mut parts = Vec::new();
  let mut prefix = PathBuf::new();
  for component in dir.components() {
    prefix.push(component);
    let text = sanitize(&component.as_os_str().to_string_lossy());
    match component {
      Component::RootDir => parts.push(Part::fixed("/".into(), style)),
      Component::Prefix(_) => parts.push(Part::fixed(text, style)),
      _ => {
        let style = if symlinks.contains(&prefix) {
          link_style
        } else {
          style
        };
        parts.push(Part::shrinkable(text, style));
        parts.push(Part::fixed("/".into(), style));
      }
    }
  }
  parts
}

/// Shortens shrinkable parts left to right, each down to at most its first character, until
/// the line fits in `width`. Returns false if it cannot fit even then.
fn shrink_from_left(parts: &mut [Part], width: usize) -> bool {
  let total: usize = parts.iter().map(|p| display_width(&p.text)).sum();
  let mut over = total.saturating_sub(width);
  for part in parts.iter_mut().filter(|p| !p.fixed) {
    if over == 0 {
      break;
    }
    let len = display_width(&part.text);
    let target = len.saturating_sub(over).max(part.min_width());
    // `clip` never splits a wide character, so it may free one cell more than asked.
    part.text = clip(&part.text, target).to_string();
    over = over.saturating_sub(len - display_width(&part.text));
  }
  over == 0
}

#[cfg(test)]
mod tests {
  use super::*;

  fn render_to_string(dir: &str, name: Option<&str>, width: usize) -> String {
    let mut parts = vec![
      Part::fixed("u@h".into(), Style::new()),
      Part::fixed(" ".into(), Style::new()),
    ];
    parts.extend(dir_parts(
      Path::new(dir),
      &HashSet::new(),
      Style::new(),
      Style::new(),
    ));
    if let Some(name) = name {
      parts.push(Part::shrinkable(name.into(), Style::new()));
    }
    if shrink_from_left(&mut parts, width) {
      parts.iter().map(|p| p.text.as_str()).collect()
    } else {
      String::from("<overflow>")
    }
  }

  #[test]
  fn fits_unchanged() {
    assert_eq!(
      render_to_string("/home/me", Some("a.rs"), 80),
      "u@h /home/me/a.rs"
    );
    assert_eq!(render_to_string("/", Some("etc"), 80), "u@h /etc");
    assert_eq!(render_to_string("/", None, 80), "u@h /");
  }

  #[test]
  fn shrinks_leading_directories_first() {
    // Full: "u@h /home/moskas/Downloads/src/rofi.rs" (38 cells).
    let full = "/home/moskas/Downloads/src";
    assert_eq!(
      render_to_string(full, Some("rofi.rs"), 34),
      "u@h /h/moska/Downloads/src/rofi.rs"
    );
    assert_eq!(
      render_to_string(full, Some("rofi.rs"), 30),
      "u@h /h/m/Downloads/src/rofi.rs"
    );
    // The cut stops partway through a name when that is enough.
    assert_eq!(
      render_to_string(full, Some("rofi.rs"), 27),
      "u@h /h/m/Downlo/src/rofi.rs"
    );
  }

  #[test]
  fn shrinks_selected_name_last() {
    assert_eq!(
      render_to_string("/home/me", Some("long-name.rs"), 14),
      "u@h /h/m/long-"
    );
  }

  #[test]
  fn reports_overflow_when_fixed_parts_do_not_fit() {
    assert_eq!(render_to_string("/home/me", Some("x"), 8), "<overflow>");
  }

  #[test]
  fn wide_characters_are_not_split() {
    // "日本" is 4 cells; it can only shrink to its first character (2 cells).
    assert_eq!(render_to_string("/日本/x", Some("f"), 11), "u@h /日/x/f");
  }

  #[test]
  fn symlinked_components_use_link_style() {
    let link = Style::new().italic();
    let symlinks = HashSet::from([PathBuf::from("/home/me")]);
    let parts = dir_parts(Path::new("/home/me/src"), &symlinks, Style::new(), link);
    let styled: Vec<(&str, bool)> = parts
      .iter()
      .map(|p| (p.text.as_str(), p.style == link))
      .collect();
    assert_eq!(
      styled,
      [
        ("/", false),
        ("home", false),
        ("/", false),
        ("me", true),
        ("/", true),
        ("src", false),
        ("/", false),
      ]
    );
  }
}
