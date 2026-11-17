//! Turning typed paths (`:cd ~/src/../docs`) into absolute ones, and listing directories for
//! completion.

use std::collections::HashSet;
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

/// The ancestors of `dir`, itself included, that are symlinks. One `lstat` per component.
pub fn symlinked_ancestors(dir: &Path) -> HashSet<PathBuf> {
  dir
    .ancestors()
    .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
    .map(Path::to_path_buf)
    .collect()
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn finds_symlinked_ancestors() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(real.join("sub")).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let found = symlinked_ancestors(&link.join("sub"));
    assert_eq!(found, HashSet::from([link]));
    assert!(symlinked_ancestors(&real.join("sub")).is_empty());
  }
}
