use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::meta::{NameCache, free_space};
use super::sort::sort_entries;
use super::{DirCount, Entry};

#[derive(Debug, thiserror::Error)]
pub enum ListError {
  #[error("permission denied: {}", .0.display())]
  PermissionDenied(PathBuf),
  #[error("not found: {}", .0.display())]
  NotFound(PathBuf),
  #[error("{}: {source}", path.display())]
  Io { path: PathBuf, source: io::Error },
  #[error("cancelled")]
  Cancelled,
}

impl ListError {
  pub fn from_io(path: &Path, e: io::Error) -> Self {
    match e.kind() {
      io::ErrorKind::PermissionDenied => ListError::PermissionDenied(path.to_path_buf()),
      io::ErrorKind::NotFound => ListError::NotFound(path.to_path_buf()),
      _ => ListError::Io {
        path: path.to_path_buf(),
        source: e,
      },
    }
  }
}

/// Sorted contents of one directory (hidden entries included; filtering happens in views).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
  pub path: PathBuf,
  pub entries: Vec<Entry>,
  /// True when the entry cap was hit and `entries` is incomplete.
  pub truncated: bool,
  /// Free bytes on the containing filesystem, if known.
  pub free: Option<u64>,
  /// The directory's own modification time when listing began, to notice later changes.
  pub mtime: Option<SystemTime>,
}

impl DirListing {
  pub fn count(&self) -> DirCount {
    DirCount {
      all: self.entries.len(),
      visible: self.entries.iter().filter(|e| !e.is_hidden()).count(),
    }
  }

  pub fn position(&self, name: &str) -> Option<usize> {
    self.entries.iter().position(|e| e.name == name)
  }
}

/// Lists and sorts `path`, reading at most `max_entries` entries. `cancelled` is polled
/// periodically so a stale request can stop early.
pub fn list_dir(
  path: &Path,
  max_entries: usize,
  cancelled: &dyn Fn() -> bool,
) -> Result<DirListing, ListError> {
  // Taken before reading, so a change made during the listing still looks newer later.
  let mtime = dir_mtime(path);
  let rd = fs::read_dir(path).map_err(|e| ListError::from_io(path, e))?;
  let mut names = NameCache::default();
  let mut resolve = |uid, gid| names.resolve(uid, gid);
  let mut entries = Vec::new();
  let mut truncated = false;
  for (i, item) in rd.enumerate() {
    if i % 256 == 0 && cancelled() {
      return Err(ListError::Cancelled);
    }
    if entries.len() >= max_entries {
      truncated = true;
      break;
    }
    // Entries that vanish or fail mid-listing are skipped rather than failing the directory.
    let Ok(item) = item else { continue };
    entries.push(Entry::from_path(&item.path(), &mut resolve));
  }
  sort_entries(&mut entries);
  Ok(DirListing {
    path: path.to_path_buf(),
    entries,
    truncated,
    free: free_space(path),
    mtime,
  })
}

/// Modification time of the directory at `path`, following symlinks.
pub fn dir_mtime(path: &Path) -> Option<SystemTime> {
  fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Counts entries in `path`, stopping at `cap`. Returns `None` if the directory is unreadable.
pub fn count_entries(path: &Path, cap: usize) -> Option<DirCount> {
  let rd = fs::read_dir(path).ok()?;
  let mut count = DirCount::default();
  for item in rd.take(cap) {
    let Ok(item) = item else { continue };
    count.all += 1;
    if !item.file_name().as_encoded_bytes().starts_with(b".") {
      count.visible += 1;
    }
  }
  Some(count)
}
