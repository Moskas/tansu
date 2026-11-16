use std::fs::{self, Metadata};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// What an entry is, after following symlinks where possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
  Dir,
  File,
  Fifo,
  Socket,
  CharDevice,
  BlockDevice,
  /// A symlink whose target does not exist, or anything else we could not stat.
  Unknown,
}

/// Coarse content class used for colors and icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
  Dir,
  Executable,
  Archive,
  Image,
  Video,
  Audio,
  Document,
  Code,
  Text,
  BrokenLink,
  Special,
  Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
  /// File name with control characters replaced, safe to print.
  pub name: String,
  pub path: PathBuf,
  pub kind: EntryKind,
  /// Symlink target, if the entry itself is a symlink.
  pub link_target: Option<PathBuf>,
  /// Size of the target for symlinks, of the entry otherwise.
  pub size: u64,
  /// Mode bits of the entry itself (not the symlink target).
  pub mode: u32,
  pub nlink: u64,
  pub uid: u32,
  pub gid: u32,
  pub owner: String,
  pub group: String,
  /// Modification time in seconds since the Unix epoch.
  pub mtime: i64,
}

/// Number of entries inside a directory, both including and excluding hidden ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirCount {
  pub all: usize,
  pub visible: usize,
}

impl DirCount {
  pub fn get(&self, show_hidden: bool) -> usize {
    if show_hidden { self.all } else { self.visible }
  }
}

impl Entry {
  /// Builds an entry from the path, following symlinks for kind and size.
  pub fn from_path(path: &Path, names: &mut impl FnMut(u32, u32) -> (String, String)) -> Self {
    let raw_name = path
      .file_name()
      .map(|n| n.to_string_lossy().into_owned())
      .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let lmeta = fs::symlink_metadata(path).ok();
    let is_link = lmeta.as_ref().is_some_and(|m| m.file_type().is_symlink());
    let link_target = if is_link {
      fs::read_link(path).ok()
    } else {
      None
    };
    let target_meta = if is_link {
      fs::metadata(path).ok()
    } else {
      lmeta.clone()
    };
    let kind = target_meta.as_ref().map_or(EntryKind::Unknown, kind_of);
    let size = target_meta
      .as_ref()
      .or(lmeta.as_ref())
      .map_or(0, Metadata::len);
    let (mode, nlink, uid, gid, mtime) = lmeta.as_ref().map_or((0, 0, 0, 0, 0), |m| {
      let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(m.mtime(), |d| d.as_secs() as i64);
      (m.mode(), m.nlink(), m.uid(), m.gid(), mtime)
    });
    let (owner, group) = names(uid, gid);
    Entry {
      name: sanitize(&raw_name),
      path: path.to_path_buf(),
      kind,
      link_target,
      size,
      mode,
      nlink,
      uid,
      gid,
      owner,
      group,
      mtime,
    }
  }

  pub fn is_dir(&self) -> bool {
    self.kind == EntryKind::Dir
  }

  pub fn is_symlink(&self) -> bool {
    self.link_target.is_some()
  }

  pub fn is_hidden(&self) -> bool {
    self.name.starts_with('.')
  }

  pub fn is_executable(&self) -> bool {
    self.kind == EntryKind::File && self.mode & 0o111 != 0
  }

  pub fn extension(&self) -> Option<String> {
    let (stem, ext) = self.name.rsplit_once('.')?;
    if stem.is_empty() {
      return None;
    }
    Some(ext.to_ascii_lowercase())
  }

  pub fn class(&self) -> FileClass {
    match self.kind {
      EntryKind::Dir => return FileClass::Dir,
      EntryKind::Unknown if self.is_symlink() => return FileClass::BrokenLink,
      EntryKind::Fifo | EntryKind::Socket | EntryKind::CharDevice | EntryKind::BlockDevice => {
        return FileClass::Special;
      }
      _ => {}
    }
    if let Some(class) = self.extension().as_deref().and_then(class_for_extension) {
      return class;
    }
    if self.is_executable() {
      FileClass::Executable
    } else {
      FileClass::Other
    }
  }
}

fn kind_of(m: &Metadata) -> EntryKind {
  let ft = m.file_type();
  if ft.is_dir() {
    EntryKind::Dir
  } else if ft.is_file() {
    EntryKind::File
  } else if ft.is_fifo() {
    EntryKind::Fifo
  } else if ft.is_socket() {
    EntryKind::Socket
  } else if ft.is_char_device() {
    EntryKind::CharDevice
  } else if ft.is_block_device() {
    EntryKind::BlockDevice
  } else {
    EntryKind::Unknown
  }
}

/// Replaces control characters so names cannot corrupt the terminal.
pub fn sanitize(name: &str) -> String {
  name
    .chars()
    .map(|c| if c.is_control() { '?' } else { c })
    .collect()
}

pub fn class_for_extension(ext: &str) -> Option<FileClass> {
  let class = match ext {
    "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "txz" | "zst" | "7z" | "rar" | "lz" | "lzma"
    | "lz4" | "cab" | "deb" | "rpm" | "iso" | "jar" | "apk" => FileClass::Archive,
    "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff" | "avif"
    | "heic" | "jxl" | "xcf" | "psd" => FileClass::Image,
    "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg" => {
      FileClass::Video
    }
    "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "wma" | "alac" | "ape" | "cue" => {
      FileClass::Audio
    }
    "pdf" | "epub" | "djvu" | "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx"
    | "odp" | "rtf" => FileClass::Document,
    "rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "py" | "js" | "ts" | "tsx" | "jsx" | "go"
    | "java" | "kt" | "rb" | "lua" | "sh" | "bash" | "zsh" | "fish" | "nix" | "hs" | "ml"
    | "zig" | "swift" | "cs" | "php" | "pl" | "html" | "css" | "scss" | "vue" | "sql" => {
      FileClass::Code
    }
    "txt" | "md" | "rst" | "org" | "log" | "toml" | "yaml" | "yml" | "json" | "ini" | "cfg"
    | "conf" | "csv" | "xml" | "lock" | "diff" | "patch" => FileClass::Text,
    _ => return None,
  };
  Some(class)
}
