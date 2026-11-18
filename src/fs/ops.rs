//! Copying and moving entries for paste. Conflicts are checked when the operation runs, so
//! they reflect the filesystem at that moment rather than a possibly stale listing.

use std::ffi::OsString;
use std::fs::{self, FileTimes, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::jobs::Meter;

/// Copies go in chunks this big, each one counted.
const CHUNK: u64 = 4 * 1024 * 1024;

/// Whether a paste copies its sources or moves them (after a cut).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteMode {
  Copy,
  Move,
}

/// What to do when the name a paste wants is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
  /// Replace a file, merge into a directory.
  Overwrite,
  /// Leave the source where it is.
  Skip,
  /// Overwrite when the source was modified later, skip otherwise.
  KeepNewer,
  /// Paste under a free suffixed name (see [`free_name`]).
  Rename,
}

/// How a paste of several entries treats taken names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnConflict {
  /// Look for conflicts first and, when there are any, report them instead of pasting.
  Ask,
  /// Overwrite everything.
  Overwrite,
  /// As answered for each source; sources without an answer are renamed.
  Answered(Vec<(PathBuf, Resolution)>),
}

impl OnConflict {
  pub fn for_source(&self, src: &Path) -> Resolution {
    match self {
      OnConflict::Ask => Resolution::Rename,
      OnConflict::Overwrite => Resolution::Overwrite,
      OnConflict::Answered(answers) => answers
        .iter()
        .find(|(p, _)| p == src)
        .map_or(Resolution::Rename, |(_, r)| *r),
    }
  }
}

/// Size, time and type of one side of a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
  pub dir: bool,
  pub size: u64,
  pub mtime: i64,
}

impl Stat {
  fn of(meta: &Metadata) -> Self {
    Stat {
      dir: meta.is_dir(),
      size: meta.len(),
      mtime: meta.mtime(),
    }
  }
}

/// A source whose name is taken in the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
  pub source: PathBuf,
  /// The name, safe to draw.
  pub name: String,
  pub incoming: Stat,
  pub existing: Stat,
  /// The name renaming would give it, safe to draw.
  pub renamed: String,
  /// The source is the entry already there (a copy into its own directory): it can only be
  /// duplicated under the new name or skipped.
  pub itself: bool,
}

/// The conflict pasting `src` into `dest_dir` would run into, if any. Copying an entry into
/// its own directory is one; moving it there is none, as that leaves it where it is.
pub fn probe(src: &Path, dest_dir: &Path, mode: PasteMode) -> Option<Conflict> {
  let name = src.file_name()?;
  let target = dest_dir.join(name);
  let existing = fs::symlink_metadata(&target).ok()?;
  let incoming = fs::symlink_metadata(src).ok()?;
  let itself = same_inode(&incoming, &existing);
  if itself && mode == PasteMode::Move {
    return None;
  }
  let shown = |p: &Path| {
    p.file_name()
      .map(|n| crate::fs::entry::sanitize(&n.to_string_lossy()))
      .unwrap_or_default()
  };
  Some(Conflict {
    source: src.to_path_buf(),
    name: shown(src),
    incoming: Stat::of(&incoming),
    existing: Stat::of(&existing),
    renamed: shown(&free_name(&target)),
    itself,
  })
}

/// Pastes `src` into the directory `dest_dir` and returns the path it ended up at.
///
/// Without `overwrite` an existing name gets a suffix, as in ranger: `name_`, then `name_0`,
/// `name_1`, and so on. With it, files are replaced and directories merged. Pasting an entry
/// onto itself copies it under a suffixed name without `overwrite`, and does nothing otherwise.
pub fn paste(
  src: &Path,
  dest_dir: &Path,
  mode: PasteMode,
  overwrite: bool,
) -> Result<PathBuf, String> {
  let resolution = if overwrite {
    Resolution::Overwrite
  } else {
    Resolution::Rename
  };
  match paste_with(src, dest_dir, mode, resolution, &mut Meter::silent()) {
    Ok(Some(target)) => Ok(target),
    Ok(None) => Err(format!("paste: {}: skipped", src.display())),
    Err(e) => Err(e),
  }
}

/// Like [`paste`], resolving a taken name with `on_conflict` and counting what it copies
/// with `meter`. Returns `None` when the source was skipped. A paste the meter stops leaves
/// nothing behind that was not there before: a partly copied file or new directory is
/// removed, and a move removes its source only once it was copied.
pub fn paste_with(
  src: &Path,
  dest_dir: &Path,
  mode: PasteMode,
  on_conflict: Resolution,
  meter: &mut Meter,
) -> Result<Option<PathBuf>, String> {
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
    // An entry pasted onto itself can only be duplicated (renamed) or left alone.
    let resolution = if same_inode(&meta, &target_meta) {
      match on_conflict {
        _ if mode == PasteMode::Move => return Ok(Some(target)),
        // Nothing to replace: the entry is already there.
        Resolution::Overwrite => return Ok(None),
        Resolution::Rename => Resolution::Rename,
        Resolution::Skip | Resolution::KeepNewer => return Ok(None),
      }
    } else if on_conflict == Resolution::KeepNewer {
      match (meta.modified(), target_meta.modified()) {
        (Ok(incoming), Ok(existing)) if incoming > existing => Resolution::Overwrite,
        _ => Resolution::Skip,
      }
    } else {
      on_conflict
    };
    match resolution {
      Resolution::Skip | Resolution::KeepNewer => return Ok(None),
      Resolution::Overwrite => {
        // Removing or merging into the target would destroy the source inside it.
        if target_meta.is_dir() && within(&location(src).map_err(err)?, &target_meta) {
          return Err(format!(
            "paste: cannot overwrite `{shown}` with its own contents"
          ));
        }
        clear_target(meta.file_type(), &target).map_err(err)?;
      }
      Resolution::Rename => target = free_name(&target),
    }
  }
  let fresh = fs::symlink_metadata(&target).is_err();
  let renamed = mode == PasteMode::Move && same_filesystem(&meta, dest_dir);
  meter.item(&shown).map_err(err)?;
  let pasted = match mode {
    PasteMode::Copy => copy_tree(src, &target, meter),
    PasteMode::Move => move_tree(src, &target, meter),
  };
  if let Err(e) = pasted {
    if meter.stopped() && fresh {
      let _ = remove_tree(&target);
    }
    return Err(err(e));
  }
  // A rename moves everything at once, so it counts as one entry (see `paste_totals`).
  if renamed {
    meter.item_done();
  }
  Ok(Some(target))
}

fn same_filesystem(meta: &Metadata, dir: &Path) -> bool {
  fs::metadata(dir).is_ok_and(|d| d.dev() == meta.dev())
}

/// What pasting `src` into `dest_dir` counts: its files (anything but directories) and
/// their bytes, or one entry for a move within a filesystem, which is a rename. Links are
/// not followed.
pub fn paste_totals(src: &Path, dest_dir: &Path, mode: PasteMode, meter: &mut Meter) -> (u64, u64) {
  let Ok(meta) = fs::symlink_metadata(src) else {
    return (0, 0);
  };
  if mode == PasteMode::Move && same_filesystem(&meta, dest_dir) {
    return (1, 0);
  }
  let (mut files, mut bytes) = (0, 0);
  let mut stack = vec![(src.to_path_buf(), meta)];
  while let Some((path, meta)) = stack.pop() {
    if !meta.is_dir() {
      files += 1;
      if meta.is_file() {
        bytes += meta.len();
      }
      continue;
    }
    if meter.check().is_err() {
      break;
    }
    let Ok(entries) = fs::read_dir(&path) else {
      continue;
    };
    for entry in entries.flatten() {
      if let Ok(meta) = entry.metadata() {
        stack.push((entry.path(), meta));
      }
    }
  }
  (files, bytes)
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

/// Makes room for an entry of type `src` at `target`: an existing directory stays for a
/// directory to be merged into, and anything else that is not a directory is removed. A
/// regular file is not removed for a regular file, which replaces it in one step once
/// complete (see [`copy_file`]), so a copy that fails or is stopped never loses it.
fn clear_target(src: fs::FileType, target: &Path) -> io::Result<()> {
  let Ok(meta) = fs::symlink_metadata(target) else {
    return Ok(());
  };
  match (meta.is_dir(), src.is_dir()) {
    (true, true) => Ok(()),
    (true, false) => Err(io::Error::other(
      "cannot overwrite a directory with a non-directory",
    )),
    (false, _) if src.is_file() && meta.is_file() => Ok(()),
    (false, _) => fs::remove_file(target),
  }
}

/// Copies `src` to `target` recursively. Symlinks are copied as links; fifos, sockets and
/// devices are refused. An existing directory at `target` is merged into.
fn copy_tree(src: &Path, target: &Path, meter: &mut Meter) -> io::Result<()> {
  meter.check()?;
  let meta = fs::symlink_metadata(src)?;
  let kind = meta.file_type();
  if kind.is_symlink() {
    std::os::unix::fs::symlink(fs::read_link(src)?, target)?;
    meter.item_done();
    return Ok(());
  }
  if kind.is_file() {
    copy_file(src, target, &meta, meter)?;
    meter.item_done();
  } else if kind.is_dir() {
    match fs::create_dir(target) {
      Err(e) if e.kind() == io::ErrorKind::AlreadyExists && target.is_dir() => {}
      r => r?,
    }
    for child in fs::read_dir(src)? {
      let child = child?;
      let to = target.join(child.file_name());
      clear_target(child.file_type()?, &to)?;
      copy_tree(&child.path(), &to, meter)?;
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

/// Copies a file's contents and permissions a chunk at a time, counting each chunk. The
/// kernel still does the copying (`copy_file_range`), so filesystems that can share extents
/// still do. The copy is written next to `target` under a hidden name and renamed over it
/// when complete: until then a file it replaces is untouched, and a copy that fails or is
/// stopped is removed.
fn copy_file(src: &Path, target: &Path, meta: &Metadata, meter: &mut Meter) -> io::Result<()> {
  use std::io::Read;
  let name = target.file_name().unwrap_or_default().to_string_lossy();
  if let Some(src_name) = src.file_name() {
    meter.item(&src_name.to_string_lossy())?;
  }
  let partial =
    free_name(&target.with_file_name(format!(".{name}.tansu-part-{}", std::process::id())));
  let mut from = fs::File::open(src)?;
  let mut to = fs::File::create_new(&partial)?;
  let mut copy = || -> io::Result<()> {
    loop {
      let n = io::copy(&mut (&mut from).take(CHUNK), &mut to)?;
      if n == 0 {
        break;
      }
      meter.add_bytes(n)?;
    }
    to.set_permissions(meta.permissions())?;
    fs::rename(&partial, target)
  };
  let copied = copy();
  if copied.is_err() {
    drop(to);
    let _ = fs::remove_file(&partial);
  }
  copied
}

/// Moves `src` to `target`, renaming when possible and copying across filesystems. An
/// existing directory at `target` is merged into; anything else there is replaced.
fn move_tree(src: &Path, target: &Path, meter: &mut Meter) -> io::Result<()> {
  meter.check()?;
  if !fs::symlink_metadata(target).is_ok_and(|m| m.is_dir()) {
    match fs::rename(src, target) {
      Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
      r => return r,
    }
    copy_tree(src, target, meter)?;
    return remove_tree(src);
  }
  for child in fs::read_dir(src)? {
    let child = child?;
    let to = target.join(child.file_name());
    clear_target(child.file_type()?, &to)?;
    move_tree(&child.path(), &to, meter)?;
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
