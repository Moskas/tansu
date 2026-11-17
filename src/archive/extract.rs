//! Extracting an archive next to it, without ever touching what is already there.
//!
//! Everything is unpacked into a new hidden directory first. If the archive holds a single
//! entry (one directory, or one file), that entry is moved into the destination; otherwise
//! the hidden directory itself becomes a directory named after the archive. Names that are
//! taken get a suffix, as in paste.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Format, builtin, external};
use crate::fs::ops::free_name;

/// What an extraction created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
  pub path: PathBuf,
  /// Damaged members left out, when asked to.
  pub skipped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtractError {
  /// These members are damaged (their data fails its checksum). Nothing was extracted;
  /// extracting with `skip_damaged` leaves them out.
  #[error("extract: {archive}: damaged: {}", members.join(", "))]
  Damaged {
    archive: String,
    members: Vec<String>,
  },
  #[error("{0}")]
  Failed(String),
}

/// Extracts `archive` into the directory `dest` and returns what was created. Damaged
/// members (zip only; other formats fail as a whole) are an error unless `skip_damaged`
/// is set, in which case the rest is extracted.
pub fn extract(archive: &Path, dest: &Path, skip_damaged: bool) -> Result<Extracted, ExtractError> {
  let name = archive
    .file_name()
    .map(|n| n.to_string_lossy().into_owned())
    .unwrap_or_default();
  let shown = crate::fs::entry::sanitize(&name);
  let err = |e: io::Error| ExtractError::Failed(format!("extract: {shown}: {e}"));
  let Some((format, stem)) = Format::from_name(&name) else {
    return Err(ExtractError::Failed(format!(
      "extract: {shown}: not an archive"
    )));
  };
  if !archive.is_file() {
    return Err(ExtractError::Failed(format!(
      "extract: {shown}: not a file"
    )));
  }
  let tmp = make_temp_dir(dest, &name).map_err(err)?;
  let result = match unpack(archive, format, stem, &tmp) {
    Err(e) => Err(err(e)),
    Ok(damaged) if !damaged.is_empty() && !skip_damaged => Err(ExtractError::Damaged {
      archive: shown.clone(),
      members: damaged,
    }),
    Ok(skipped) => place(&tmp, dest, stem)
      .map(|path| Extracted { path, skipped })
      .map_err(err),
  };
  // Gone after a successful `place` that moved it; otherwise whatever was unpacked.
  if fs::symlink_metadata(&tmp).is_ok() {
    let _ = make_removable(&tmp);
    let _ = fs::remove_dir_all(&tmp);
  }
  result
}

/// A new, empty, hidden directory in `dest` to unpack into. Being in `dest` keeps it on
/// the same filesystem, so its contents can be renamed into place.
fn make_temp_dir(dest: &Path, name: &str) -> io::Result<PathBuf> {
  static NEXT: AtomicU64 = AtomicU64::new(0);
  loop {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = dest.join(format!(".{name}.tansu-{}-{n}", std::process::id()));
    match fs::create_dir(&dir) {
      Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
      r => return r.map(|()| dir),
    }
  }
}

/// Unpacks `archive` into `dir` and returns the damaged members it left out.
fn unpack(archive: &Path, format: Format, stem: &str, dir: &Path) -> io::Result<Vec<String>> {
  let mut damaged = Vec::new();
  match format {
    // Members compressed with something other than deflate need a program.
    Format::Zip if !builtin::zip_supported(archive)? => external::extract(
      archive,
      &[external::Tool::SevenZip, external::Tool::Bsdtar],
      dir,
    )?,
    Format::Zip => damaged = builtin::extract_zip(archive, dir)?,
    Format::Tar(c) => builtin::extract_tar(archive, c, dir)?,
    Format::Compressed(c) => builtin::extract_compressed(archive, c, dir, &dir.join(stem))?,
    Format::External(tools) => external::extract(archive, tools, dir)?,
  }
  drop_special_bits(dir);
  Ok(damaged)
}

/// Moves what was unpacked into `tmp` to its place in `dest`.
fn place(tmp: &Path, dest: &Path, stem: &str) -> io::Result<PathBuf> {
  let mut entries = fs::read_dir(tmp)?;
  let first = match entries.next() {
    None => return Err(io::Error::other("the archive is empty")),
    Some(first) => first?,
  };
  if entries.next().is_none() {
    let target = free_name(&dest.join(first.file_name()));
    move_entry(&first.path(), &target)?;
    fs::remove_dir(tmp)?;
    return Ok(target);
  }
  let target = free_name(&dest.join(stem));
  // A directory made by create_dir, so with the usual permissions.
  fs::rename(tmp, &target)?;
  Ok(target)
}

/// Renames `from` to `to`. Moving a directory to another parent rewrites its `..` entry,
/// which needs write permission on it, so a read-only directory (as tarballs of Nix store
/// paths hold) is made writable for the move and given its mode back after.
fn move_entry(from: &Path, to: &Path) -> io::Result<()> {
  let meta = fs::symlink_metadata(from)?;
  let mode = meta.permissions().mode();
  if !meta.is_dir() || mode & 0o200 != 0 {
    return fs::rename(from, to);
  }
  fs::set_permissions(from, fs::Permissions::from_mode(mode | 0o200))?;
  fs::rename(from, to)?;
  fs::set_permissions(to, fs::Permissions::from_mode(mode))
}

/// Clears setuid and setgid bits throughout `dir`: an archive from elsewhere should not
/// hand out privileges. Best effort.
fn drop_special_bits(dir: &Path) {
  let Ok(entries) = fs::read_dir(dir) else {
    return;
  };
  for entry in entries.flatten() {
    let path = entry.path();
    let Ok(meta) = fs::symlink_metadata(&path) else {
      continue;
    };
    if meta.is_symlink() {
      continue;
    }
    let mode = meta.permissions().mode();
    if mode & 0o6000 != 0 {
      let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode & !0o6000));
    }
    if meta.is_dir() {
      drop_special_bits(&path);
    }
  }
}

/// Makes the directories under `dir` writable, so a failed extraction of read-only
/// directories can still be cleaned up.
fn make_removable(dir: &Path) -> io::Result<()> {
  let meta = fs::symlink_metadata(dir)?;
  if !meta.is_dir() {
    return Ok(());
  }
  let mode = meta.permissions().mode();
  if mode & 0o700 != 0o700 {
    fs::set_permissions(dir, fs::Permissions::from_mode(mode | 0o700))?;
  }
  for entry in fs::read_dir(dir)?.flatten() {
    let _ = make_removable(&entry.path());
  }
  Ok(())
}
