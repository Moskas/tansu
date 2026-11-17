//! Copying and moving entries for paste. Conflicts are checked when the operation runs, so
//! they reflect the filesystem at that moment rather than a possibly stale listing.

use std::ffi::OsString;
use std::fs::{self, FileTimes, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Whether a paste copies its sources or moves them (after a cut).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteMode {
  Copy,
  Move,
}

/// Pastes `src` into the directory `dest_dir` and returns the path it ended up at.
///
/// Without `overwrite` an existing name gets a suffix, as in ranger: `name_`, then `name_0`,
/// `name_1`, and so on. With it, files are replaced and directories merged. Pasting an entry
/// onto itself copies it under a suffixed name, or does nothing when moving or overwriting.
pub fn paste(
  src: &Path,
  dest_dir: &Path,
  mode: PasteMode,
  overwrite: bool,
) -> Result<PathBuf, String> {
  let Some(name) = src.file_name() else {
    return Err(format!("paste: {}: not a file or directory", src.display()));
  };
  let shown = name.to_string_lossy().into_owned();
  let err = |e: io::Error| format!("paste: {shown}: {e}");
  let meta = fs::symlink_metadata(src).map_err(err)?;
  // Copying a directory into its own subtree would never finish.
  if meta.is_dir() && within(&dest_dir.canonicalize().map_err(err)?, &meta) {
    return Err(format!("paste: cannot paste `{shown}` into itself"));
  }
  let mut target = dest_dir.join(name);
  if let Ok(target_meta) = fs::symlink_metadata(&target) {
    // Compared by inode, since case-insensitive filesystems and bind mounts give one entry
    // several paths.
    if same_inode(&meta, &target_meta) && (overwrite || mode == PasteMode::Move) {
      return Ok(target);
    }
    if overwrite {
      // Removing or merging into the target would destroy the source inside it.
      if target_meta.is_dir() && within(&location(src).map_err(err)?, &target_meta) {
        return Err(format!(
          "paste: cannot overwrite `{shown}` with its own contents"
        ));
      }
      clear_target(meta.is_dir(), &target).map_err(err)?;
    } else {
      target = free_name(&target);
    }
  }
  match mode {
    PasteMode::Copy => copy_tree(src, &target),
    PasteMode::Move => move_tree(src, &target),
  }
  .map_err(err)?;
  Ok(target)
}

/// The first of `path`, `path_`, `path_0`, `path_1`... that does not exist.
pub fn free_name(path: &Path) -> PathBuf {
  let exists = |p: &Path| fs::symlink_metadata(p).is_ok();
  if !exists(path) {
    return path.to_path_buf();
  }
  let mut base = OsString::from(path.as_os_str());
  base.push("_");
  let base = PathBuf::from(base);
  if !exists(&base) {
    return base;
  }
  (0u64..)
    .map(|n| {
      let mut p = OsString::from(base.as_os_str());
      p.push(n.to_string());
      PathBuf::from(p)
    })
    .find(|p| !exists(p))
    .unwrap_or(base)
}

fn same_inode(a: &Metadata, b: &Metadata) -> bool {
  (a.dev(), a.ino()) == (b.dev(), b.ino())
}

/// Whether `dir` is `path` or one of its ancestors. `path` must be free of symlinks.
fn within(path: &Path, dir: &Metadata) -> bool {
  path
    .ancestors()
    .any(|a| fs::metadata(a).is_ok_and(|m| same_inode(&m, dir)))
}

/// `path` with its parent resolved, while a symlink still names the link itself, not what
/// it points to.
fn location(path: &Path) -> io::Result<PathBuf> {
  let parent = match path.parent() {
    Some(p) if !p.as_os_str().is_empty() => p.canonicalize()?,
    _ => std::env::current_dir()?,
  };
  Ok(match path.file_name() {
    Some(name) => parent.join(name),
    None => parent,
  })
}

/// Makes room for an entry at `target`: an existing directory stays for a directory to be
/// merged into, anything else that is not a directory is removed.
fn clear_target(src_is_dir: bool, target: &Path) -> io::Result<()> {
  let Ok(meta) = fs::symlink_metadata(target) else {
    return Ok(());
  };
  match (meta.is_dir(), src_is_dir) {
    (true, true) => Ok(()),
    (true, false) => Err(io::Error::other(
      "cannot overwrite a directory with a non-directory",
    )),
    (false, _) => fs::remove_file(target),
  }
}

/// Copies `src` to `target` recursively. Symlinks are copied as links; fifos, sockets and
/// devices are refused. An existing directory at `target` is merged into.
fn copy_tree(src: &Path, target: &Path) -> io::Result<()> {
  let meta = fs::symlink_metadata(src)?;
  let kind = meta.file_type();
  if kind.is_symlink() {
    std::os::unix::fs::symlink(fs::read_link(src)?, target)?;
    return Ok(());
  }
  if kind.is_file() {
    fs::copy(src, target)?;
  } else if kind.is_dir() {
    match fs::create_dir(target) {
      Err(e) if e.kind() == io::ErrorKind::AlreadyExists && target.is_dir() => {}
      r => r?,
    }
    for child in fs::read_dir(src)? {
      let child = child?;
      let to = target.join(child.file_name());
      clear_target(child.file_type()?.is_dir(), &to)?;
      copy_tree(&child.path(), &to)?;
    }
    // Applied last so a read-only directory can still be filled.
    fs::set_permissions(target, meta.permissions())?;
  } else {
    return Err(io::Error::other(format!(
      "{}: cannot copy a special file",
      src.display()
    )));
  }
  keep_times(target, &meta);
  Ok(())
}

/// Moves `src` to `target`, renaming when possible and copying across filesystems. An
/// existing directory at `target` is merged into.
fn move_tree(src: &Path, target: &Path) -> io::Result<()> {
  if fs::symlink_metadata(target).is_err() {
    match fs::rename(src, target) {
      Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
      r => return r,
    }
    copy_tree(src, target)?;
    return remove_tree(src);
  }
  for child in fs::read_dir(src)? {
    let child = child?;
    let to = target.join(child.file_name());
    clear_target(child.file_type()?.is_dir(), &to)?;
    move_tree(&child.path(), &to)?;
  }
  fs::remove_dir(src)
}

fn remove_tree(path: &Path) -> io::Result<()> {
  if fs::symlink_metadata(path)?.is_dir() {
    fs::remove_dir_all(path)
  } else {
    fs::remove_file(path)
  }
}

/// Carries the access and modification times over, like `cp -p`. Best effort: a copy whose
/// times could not be set is still a good copy.
fn keep_times(target: &Path, meta: &Metadata) {
  let (Ok(accessed), Ok(modified)) = (meta.accessed(), meta.modified()) else {
    return;
  };
  if let Ok(file) = fs::File::open(target) {
    let times = FileTimes::new()
      .set_accessed(accessed)
      .set_modified(modified);
    let _ = file.set_times(times);
  }
}

/// Deletes `path` for good. A directory is removed with everything in it; a symlink is
/// removed itself, never what it points to.
pub fn remove(path: &Path) -> Result<(), String> {
  let shown = path.file_name().map_or_else(
    || path.display().to_string(),
    |n| n.to_string_lossy().into_owned(),
  );
  let meta = fs::symlink_metadata(path).map_err(|e| format!("delete: {shown}: {e}"))?;
  let removed = if meta.is_dir() {
    // Does not follow symlinks inside the tree either.
    fs::remove_dir_all(path)
  } else {
    fs::remove_file(path)
  };
  removed.map_err(|e| format!("delete: {shown}: {e}"))
}

/// Creates a directory (`dir`) or an empty file at `path`, with any missing parent
/// directories, as ranger's `mkdir` and `touch` do. An existing entry is an error.
pub fn create(path: &Path, dir: bool) -> Result<(), String> {
  let command = if dir { "mkdir" } else { "touch" };
  let shown = path.file_name().map_or_else(
    || path.display().to_string(),
    |n| n.to_string_lossy().into_owned(),
  );
  if fs::symlink_metadata(path).is_ok() {
    return Err(format!("{command}: `{shown}` already exists"));
  }
  let err = |e: io::Error| format!("{command}: {shown}: {e}");
  if dir {
    return fs::create_dir_all(path).map_err(err);
  }
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(err)?;
  }
  fs::File::create_new(path).map(drop).map_err(err)
}
