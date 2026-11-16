use super::devicons;
use crate::fs::{Entry, FileClass};

/// Type icon for an entry: a Nerd Font glyph chosen by name and extension (as the
/// ranger_devicons plugin does), or a one-character ASCII marker.
pub fn icon(entry: &Entry, nerd: bool) -> &'static str {
  if nerd {
    nerd_icon(entry)
  } else {
    ascii_icon(entry)
  }
}

fn nerd_icon(entry: &Entry) -> &'static str {
  if entry.is_dir() {
    return lookup(devicons::DIR_NAMES, &entry.name).unwrap_or(devicons::DEFAULT_DIR);
  }
  lookup(devicons::FILE_NAMES, &entry.name)
    .or_else(|| {
      // Like ranger, everything after the last dot counts, so `.bashrc` has `bashrc`.
      let (_, ext) = entry.name.rsplit_once('.')?;
      lookup(devicons::EXTENSIONS, &ext.to_lowercase())
    })
    .unwrap_or(devicons::DEFAULT_FILE)
}

fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
  table
    .binary_search_by(|(k, _)| (*k).cmp(key))
    .ok()
    .map(|i| table[i].1)
}

fn ascii_icon(entry: &Entry) -> &'static str {
  match entry.class() {
    FileClass::Dir => "/",
    FileClass::Executable => "*",
    FileClass::BrokenLink => "!",
    FileClass::Special => "=",
    _ if entry.is_symlink() => "@",
    FileClass::Archive => "#",
    FileClass::Image | FileClass::Video | FileClass::Audio => "&",
    _ => "-",
  }
}

#[cfg(test)]
mod tests {
  use super::devicons;

  /// Binary search requires the tables to be sorted by byte order.
  #[test]
  fn tables_are_sorted() {
    for table in [
      devicons::EXTENSIONS,
      devicons::FILE_NAMES,
      devicons::DIR_NAMES,
    ] {
      assert!(table.windows(2).all(|w| w[0].0 < w[1].0));
    }
  }
}
