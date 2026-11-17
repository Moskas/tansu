//! Turning typed paths (`:cd ~/src/../docs`) into absolute ones, and listing directories for
//! completion.

use std::path::{Component, Path, PathBuf};

use super::sort::natural_cmp;

/// Expands a leading `~`, makes `input` absolute against `cwd` and removes `.` and `..`
/// lexically, the way a shell's `cd` does, so symlinks in the path are kept as typed.
pub fn resolve(input: &str, cwd: &Path, home: Option<&Path>) -> PathBuf {
  let expanded = match (input.strip_prefix('~'), home) {
    (Some(""), Some(home)) => home.to_path_buf(),
    (Some(rest), Some(home)) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
    _ => PathBuf::from(input),
  };
  normalize(&cwd.join(expanded))
}

/// Removes `.` and `..` components without touching the filesystem. `..` at the root stays
/// at the root.
pub fn normalize(path: &Path) -> PathBuf {
  let mut out = PathBuf::new();
  for part in path.components() {
    match part {
      Component::CurDir => {}
      Component::ParentDir => {
        out.pop();
      }
      other => out.push(other),
    }
  }
  out
}

/// Names of the directories (symlinks to directories included) inside `dir`, in natural
/// order. An unreadable directory has none.
pub fn subdirectories(dir: &Path) -> Vec<String> {
  let Ok(rd) = std::fs::read_dir(dir) else {
    return Vec::new();
  };
  let mut names: Vec<String> = rd
    .flatten()
    .filter(|e| std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()))
    .filter_map(|e| e.file_name().into_string().ok())
    .collect();
  names.sort_by(|a, b| natural_cmp(a, b));
  names
}
