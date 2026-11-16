//! Display-width aware string helpers.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const TRUNCATION_MARK: char = '~';

pub fn display_width(s: &str) -> usize {
  UnicodeWidthStr::width(s)
}

/// Truncates `s` to at most `max` terminal cells. If it does not fit, the result ends with
/// `~`. Wide characters are never split, so the result may be one cell shorter than `max`.
pub fn truncate(s: &str, max: usize) -> String {
  if display_width(s) <= max {
    return s.to_string();
  }
  if max == 0 {
    return String::new();
  }
  let budget = max - 1; // room for the mark
  let mut out = String::new();
  let mut width = 0;
  for c in s.chars() {
    let w = UnicodeWidthChar::width(c).unwrap_or(0);
    if width + w > budget {
      break;
    }
    out.push(c);
    width += w;
  }
  out.push(TRUNCATION_MARK);
  out
}

/// Cuts `s` to at most `max` cells without adding a mark (used for preview text).
pub fn clip(s: &str, max: usize) -> &str {
  let mut width = 0;
  for (i, c) in s.char_indices() {
    let w = UnicodeWidthChar::width(c).unwrap_or(0);
    if width + w > max {
      return &s[..i];
    }
    width += w;
  }
  s
}
