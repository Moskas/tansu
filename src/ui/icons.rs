use crate::fs::{Entry, FileClass};

/// Type icon for an entry: a Nerd Font glyph, or a one-character ASCII marker.
pub fn icon(entry: &Entry, nerd: bool) -> &'static str {
  let class = entry.class();
  if !nerd {
    return match class {
      FileClass::Dir => "/",
      FileClass::Executable => "*",
      FileClass::BrokenLink => "!",
      FileClass::Special => "=",
      _ if entry.is_symlink() => "@",
      FileClass::Archive => "#",
      FileClass::Image | FileClass::Video | FileClass::Audio => "&",
      _ => "-",
    };
  }
  match class {
    FileClass::Dir => special_dir(&entry.name).unwrap_or("\u{f07b}"),
    FileClass::Executable => "\u{f489}",
    FileClass::Archive => "\u{f1c6}",
    FileClass::Image => "\u{f1c5}",
    FileClass::Video => "\u{f1c8}",
    FileClass::Audio => "\u{f1c7}",
    FileClass::Document if entry.extension().as_deref() == Some("pdf") => "\u{f1c1}",
    FileClass::Document => "\u{f02d}",
    FileClass::Code => "\u{f1c9}",
    FileClass::Text => "\u{f15c}",
    FileClass::BrokenLink => "\u{f127}",
    FileClass::Special => "\u{f0e7}",
    FileClass::Other if entry.is_symlink() => "\u{f0c1}",
    FileClass::Other => "\u{f15b}",
  }
}

fn special_dir(name: &str) -> Option<&'static str> {
  Some(match name {
    "Downloads" => "\u{f019}",
    "Music" => "\u{f001}",
    "Pictures" => "\u{f03e}",
    "Videos" => "\u{f03d}",
    "Documents" => "\u{f02d}",
    "Desktop" => "\u{f108}",
    ".git" => "\u{f1d3}",
    _ => return None,
  })
}
