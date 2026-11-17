use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

use super::Entry;

/// Order of entries within a listing. Directories always come before everything else.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
  /// Natural, case-insensitive name order.
  #[default]
  Name,
  /// Largest first. Directories stay in name order: their own size says nothing useful.
  Size,
  /// Most recently modified first.
  Mtime,
  /// Most recent status change first.
  Ctime,
}

impl SortKey {
  pub const ALL: [SortKey; 4] = [SortKey::Name, SortKey::Size, SortKey::Mtime, SortKey::Ctime];

  pub fn name(self) -> &'static str {
    match self {
      SortKey::Name => "name",
      SortKey::Size => "size",
      SortKey::Mtime => "mtime",
      SortKey::Ctime => "ctime",
    }
  }
}

impl fmt::Display for SortKey {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.name())
  }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown sort type `{0}`")]
pub struct UnknownSortKey(pub String);

impl FromStr for SortKey {
  type Err = UnknownSortKey;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    SortKey::ALL
      .into_iter()
      .find(|k| k.name() == s)
      .ok_or_else(|| UnknownSortKey(s.to_string()))
  }
}

/// Sorts directories first, then by natural, case-insensitive name order.
pub fn sort_entries(entries: &mut [Entry]) {
  sort_entries_by(entries, SortKey::Name);
}

pub fn sort_entries_by(entries: &mut [Entry], key: SortKey) {
  entries.sort_by(|a, b| compare_entries_by(a, b, key));
}

pub fn compare_entries(a: &Entry, b: &Entry) -> Ordering {
  compare_entries_by(a, b, SortKey::Name)
}

/// Directories first, then by `key`, with name order breaking ties.
pub fn compare_entries_by(a: &Entry, b: &Entry, key: SortKey) -> Ordering {
  let by_key = match key {
    SortKey::Name => Ordering::Equal,
    SortKey::Size if a.is_dir() && b.is_dir() => Ordering::Equal,
    SortKey::Size => b.size.cmp(&a.size),
    SortKey::Mtime => b.mtime.cmp(&a.mtime),
    SortKey::Ctime => b.ctime.cmp(&a.ctime),
  };
  b.is_dir()
    .cmp(&a.is_dir())
    .then(by_key)
    .then_with(|| natural_cmp(&a.name, &b.name))
}

/// Natural, case-insensitive comparison: digit runs compare by numeric value, so `file2`
/// sorts before `file10`. Ties fall back to a plain comparison for a stable total order.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
  let mut ai = a.chars().peekable();
  let mut bi = b.chars().peekable();
  loop {
    match (ai.peek().copied(), bi.peek().copied()) {
      (None, None) => return a.cmp(b),
      (None, Some(_)) => return Ordering::Less,
      (Some(_), None) => return Ordering::Greater,
      (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
        let na = take_digits(&mut ai);
        let nb = take_digits(&mut bi);
        let ord = cmp_digit_runs(&na, &nb);
        if ord != Ordering::Equal {
          return ord;
        }
      }
      (Some(ca), Some(cb)) => {
        ai.next();
        bi.next();
        let ord = ca.to_lowercase().cmp(cb.to_lowercase());
        if ord != Ordering::Equal {
          return ord;
        }
      }
    }
  }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
  let mut s = String::new();
  while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
    s.push(c);
    it.next();
  }
  s
}

fn cmp_digit_runs(a: &str, b: &str) -> Ordering {
  let ta = a.trim_start_matches('0');
  let tb = b.trim_start_matches('0');
  ta.len()
    .cmp(&tb.len())
    .then_with(|| ta.cmp(tb))
    // Fewer leading zeros first: `1` < `01`.
    .then_with(|| a.len().cmp(&b.len()))
}
