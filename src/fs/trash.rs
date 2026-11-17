//! Moving entries to the home trash, following the freedesktop.org trash specification
//! (`$XDG_DATA_HOME/Trash`), so file managers and `trash-restore` can find and restore them.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Moves `path` into the trash at `root` (holding `files/` and `info/`) and returns where it
/// ended up. The entry is renamed, never copied: one on another filesystem than the trash is
/// refused, so nothing is ever deleted as a fallback.
pub fn trash(path: &Path, root: &Path) -> Result<PathBuf, String> {
  let Some(name) = path.file_name() else {
    return Err(format!(
      "trash: {}: not a file or directory",
      path.display()
    ));
  };
  let shown = name.to_string_lossy().into_owned();
  let err = |e: io::Error| format!("trash: {shown}: {e}");
  let path = std::path::absolute(path).map_err(err)?;
  fs::symlink_metadata(&path).map_err(err)?;
  if let Ok(root) = root.canonicalize()
    && path.starts_with(&root)
  {
    return Err(format!("trash: {shown}: already in the trash"));
  }
  let files = root.join("files");
  let info = root.join("info");
  // The trash holds other people's deleted files as far as the system knows: private.
  for dir in [root, &files, &info] {
    DirBuilder::new()
      .recursive(true)
      .mode(0o700)
      .create(dir)
      .map_err(err)?;
  }
  let info_text = format!(
    "[Trash Info]\nPath={}\nDeletionDate={}\n",
    percent_encode(path.as_os_str().as_bytes()),
    deletion_date()
  );
  // Creating the info file exclusively reserves the name, also against other programs.
  for n in 1u64.. {
    let mut trashed = name.to_os_string();
    if n > 1 {
      trashed.push(format!(".{n}"));
    }
    let target = files.join(&trashed);
    let mut info_name = trashed.clone();
    info_name.push(".trashinfo");
    let info_path = info.join(info_name);
    if fs::symlink_metadata(&target).is_ok() {
      continue;
    }
    let mut file = match OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&info_path)
    {
      Ok(f) => f,
      Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
      Err(e) => return Err(err(e)),
    };
    let written = file
      .write_all(info_text.as_bytes())
      .and_then(|()| file.sync_all());
    let moved = written.and_then(|()| fs::rename(&path, &target));
    return match moved {
      Ok(()) => Ok(target),
      Err(e) => {
        let _ = fs::remove_file(&info_path);
        if e.raw_os_error() == Some(libc::EXDEV) {
          Err(format!(
            "trash: {shown}: on another filesystem than the trash; use dD to delete"
          ))
        } else {
          Err(err(e))
        }
      }
    };
  }
  unreachable!("the name counter does not run out")
}

/// Percent-encodes a path for the `Path=` key, keeping `/` and unreserved characters.
fn percent_encode(bytes: &[u8]) -> String {
  let mut out = String::with_capacity(bytes.len());
  for &b in bytes {
    if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
      out.push(b as char);
    } else {
      out.push_str(&format!("%{b:02X}"));
    }
  }
  out
}

/// Local time now, as `YYYY-MM-DDThh:mm:ss`.
fn deletion_date() -> String {
  let secs = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |d| d.as_secs() as i64);
  super::meta::format_datetime(secs)
}
