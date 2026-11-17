//! Archives: telling formats apart by name, listing their members and extracting them. zip
//! and tar (plain, or compressed with gzip, bzip2, xz, lzip or zstd) are read in-process;
//! other formats (7z, rar, iso, ...) go through 7-Zip, unrar or bsdtar when one is installed.
//! Free of UI types.

mod builtin;
mod external;
mod extract;

use std::collections::HashSet;
use std::path::Path;

pub use builtin::MAX_SCAN;
pub use extract::{ExtractError, Extracted, extract};

use external::Tool;

/// How the data of a tar archive or a single compressed file is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
  None,
  Gzip,
  Bzip2,
  Xz,
  Lzip,
  Zstd,
}

impl Compression {
  pub fn name(self) -> &'static str {
    match self {
      Compression::None => "uncompressed",
      Compression::Gzip => "gzip",
      Compression::Bzip2 => "bzip2",
      Compression::Xz => "xz",
      Compression::Lzip => "lzip",
      Compression::Zstd => "zstd",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
  Zip,
  Tar(Compression),
  /// A compressed file (`.gz` and the like): a single file, or a tar archive not named as
  /// one, which only its content tells.
  Compressed(Compression),
  /// Read by the first of these programs that is installed.
  External(&'static [Tool]),
}

const RAR: &[Tool] = &[Tool::Unrar, Tool::SevenZip, Tool::Bsdtar];
const ANY: &[Tool] = &[Tool::SevenZip, Tool::Bsdtar];
/// Compressed tars: 7-Zip would only take off the compression.
const TAR: &[Tool] = &[Tool::Bsdtar];

/// Name suffixes and their formats. Longer suffixes come first, so `.tar.gz` wins over `.gz`.
const SUFFIXES: &[(&str, Format)] = &[
  (".tar.gz", Format::Tar(Compression::Gzip)),
  (".tar.bz2", Format::Tar(Compression::Bzip2)),
  (".tar.xz", Format::Tar(Compression::Xz)),
  (".tar.lz", Format::Tar(Compression::Lzip)),
  (".tar.zst", Format::Tar(Compression::Zstd)),
  (".tar.lzma", Format::External(TAR)),
  (".tar.lz4", Format::External(TAR)),
  (".tar.z", Format::External(TAR)),
  (".tgz", Format::Tar(Compression::Gzip)),
  (".tbz2", Format::Tar(Compression::Bzip2)),
  (".tbz", Format::Tar(Compression::Bzip2)),
  (".txz", Format::Tar(Compression::Xz)),
  (".tzst", Format::Tar(Compression::Zstd)),
  (".tar", Format::Tar(Compression::None)),
  (".gz", Format::Compressed(Compression::Gzip)),
  (".bz2", Format::Compressed(Compression::Bzip2)),
  (".xz", Format::Compressed(Compression::Xz)),
  (".lz", Format::Compressed(Compression::Lzip)),
  (".zst", Format::Compressed(Compression::Zstd)),
  (".zip", Format::Zip),
  (".jar", Format::Zip),
  (".war", Format::Zip),
  (".apk", Format::Zip),
  (".cbz", Format::Zip),
  (".epub", Format::Zip),
  (".whl", Format::Zip),
  (".xpi", Format::Zip),
  (".rar", Format::External(RAR)),
  (".cbr", Format::External(RAR)),
  (".7z", Format::External(ANY)),
  (".cb7", Format::External(ANY)),
  (".iso", Format::External(ANY)),
  (".cab", Format::External(ANY)),
  (".cpio", Format::External(ANY)),
  (".deb", Format::External(ANY)),
  (".rpm", Format::External(ANY)),
  (".xar", Format::External(ANY)),
  (".lzh", Format::External(ANY)),
  (".lha", Format::External(ANY)),
  (".arj", Format::External(ANY)),
];

impl Format {
  /// The format a file name says, and the name without its archive suffix (`photos` for
  /// `photos.tar.gz`). `None` for names that are not archives.
  pub fn from_name(name: &str) -> Option<(Format, &str)> {
    let lower = name.to_ascii_lowercase();
    SUFFIXES.iter().find_map(|&(suffix, format)| {
      let stem = &name[..lower.strip_suffix(suffix)?.len()];
      // `.tar` is a hidden file, not an archive.
      (!stem.is_empty()).then_some((format, stem))
    })
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
  File,
  Dir,
  Link,
  /// Devices, fifos and the like, which tar can hold.
  Other,
}

/// One entry of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
  /// The path as stored, `/` separated, without a trailing `/`.
  pub path: String,
  pub kind: MemberKind,
  /// Unpacked size, when known.
  pub size: Option<u64>,
  /// `YYYY-MM-DD HH:MM` (or what an external program printed).
  pub modified: Option<String>,
  /// What a link points to.
  pub link: Option<String>,
  pub encrypted: bool,
}

/// What an archive holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contents {
  /// `zip`, `tar, gzip`, `rar`, ...
  pub format: String,
  /// The members, in archive order, up to the requested number.
  pub members: Vec<Member>,
  /// Counted over every member read, also those past the requested number.
  pub files: usize,
  /// Directories, including those only implied by the paths of other members.
  pub dirs: usize,
  pub links: usize,
  pub encrypted: usize,
  /// Unpacked size of the files.
  pub size: u64,
  /// Every member was read. False when listing stopped early, at the member count or
  /// because decompressing went on for too long.
  pub complete: bool,
  pub comment: Option<String>,
  /// A single compressed file rather than an archive.
  pub single: bool,
  /// Distinct first path components, for telling where extracting puts things.
  top: HashSet<String>,
  seen_dirs: HashSet<String>,
}

impl Contents {
  fn new(format: impl Into<String>) -> Self {
    Contents {
      format: format.into(),
      complete: true,
      ..Contents::default()
    }
  }

  /// Counts `member` and keeps it while there are fewer than `max`.
  fn add(&mut self, mut member: Member, max: usize) {
    member.path = member
      .path
      .trim_start_matches("./")
      .trim_end_matches('/')
      .to_string();
    if member.path.is_empty() || member.path == "." {
      return;
    }
    match member.kind {
      MemberKind::Dir => {
        if self.seen_dirs.insert(member.path.clone()) {
          self.dirs += 1;
        }
      }
      MemberKind::Link => self.links += 1,
      MemberKind::File | MemberKind::Other => {
        self.files += 1;
        self.size += member.size.unwrap_or(0);
      }
    }
    self.encrypted += usize::from(member.encrypted);
    // Directories leading to the member count even when the archive has no entry for them.
    let mut parent = member.path.as_str();
    while let Some((dir, _)) = parent.rsplit_once('/') {
      if !self.seen_dirs.insert(dir.to_string()) {
        break;
      }
      self.dirs += 1;
      parent = dir;
    }
    if let Some(first) = member.path.split('/').next()
      && !self.top.contains(first)
    {
      self.top.insert(first.to_string());
    }
    if self.members.len() < max {
      self.members.push(member);
    }
  }

  /// What extracting puts into the directory, given the archive's name without its suffix:
  /// the single top-level entry, or a new directory named after the archive. `None` when
  /// the listing is incomplete, or the archive empty.
  pub fn extracts_to(&self, stem: &str) -> Option<String> {
    if !self.complete {
      return None;
    }
    let mut top = self.top.iter();
    match (top.next(), top.next()) {
      (None, _) => None,
      (Some(only), None) if self.single => Some(only.clone()),
      (Some(only), None) => {
        let dir = self.seen_dirs.contains(only.as_str());
        Some(if dir {
          format!("{only}/")
        } else {
          only.clone()
        })
      }
      _ => Some(format!("{stem}/")),
    }
  }
}

#[derive(Debug, thiserror::Error)]
pub enum ListError {
  #[error("cancelled")]
  Cancelled,
  /// Not an archive by its name, or no program to read it is installed.
  #[error("unsupported archive")]
  Unsupported,
  #[error("{0}")]
  Failed(String),
}

impl From<std::io::Error> for ListError {
  fn from(e: std::io::Error) -> Self {
    ListError::Failed(e.to_string())
  }
}

/// Lists the archive at `path`, keeping at most `max` members (all are counted when that
/// is cheap). Long-running work checks `cancelled` and stops when it returns true.
pub fn list(path: &Path, max: usize, cancelled: &dyn Fn() -> bool) -> Result<Contents, ListError> {
  let name = path
    .file_name()
    .map(|n| n.to_string_lossy())
    .unwrap_or_default();
  let Some((format, _)) = Format::from_name(&name) else {
    return Err(ListError::Unsupported);
  };
  match format {
    Format::Zip => builtin::list_zip(path, max, cancelled),
    Format::Tar(c) => builtin::list_tar(path, c, max, cancelled),
    Format::Compressed(c) => builtin::list_compressed(path, c, &name, max, cancelled),
    Format::External(tools) => {
      let label = name.rsplit_once('.').map_or("archive", |(_, ext)| ext);
      external::list(path, tools, &label.to_ascii_lowercase(), max, cancelled)
    }
  }
}
