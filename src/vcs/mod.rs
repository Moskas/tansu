//! Version-control status for entries, modeled on ranger's VCS integration. Only git is
//! supported. Free of UI types: the UI maps [`Mark`]s to colors.

mod git;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use git::{GitError, find_root, is_repo_root, parse_head, parse_status, query_repo};

/// Status of a path inside a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileStatus {
  Conflict,
  Untracked,
  Deleted,
  Changed,
  Staged,
  Ignored,
  Sync,
  Unknown,
}

impl FileStatus {
  /// Order in which a directory takes the status of its contents (ranger's `DIRSTATUSES`).
  /// Ignored contents never mark a directory.
  const DIR_PRIORITY: [FileStatus; 7] = [
    FileStatus::Conflict,
    FileStatus::Untracked,
    FileStatus::Deleted,
    FileStatus::Changed,
    FileStatus::Staged,
    FileStatus::Sync,
    FileStatus::Unknown,
  ];

  fn rank(self) -> usize {
    Self::DIR_PRIORITY
      .iter()
      .position(|&s| s == self)
      .unwrap_or(usize::MAX)
  }

  pub fn symbol(self) -> char {
    match self {
      FileStatus::Conflict => 'X',
      FileStatus::Untracked => '?',
      FileStatus::Deleted => '-',
      FileStatus::Changed => '+',
      FileStatus::Staged => '*',
      FileStatus::Ignored => '·',
      FileStatus::Sync => '✓',
      FileStatus::Unknown => '!',
    }
  }

  /// Translates a porcelain `XY` code (`.` or space meaning unmodified).
  pub fn from_xy(x: u8, y: u8) -> Self {
    let norm = |c: u8| if c == b'.' { b' ' } else { c };
    let (x, y) = (norm(x), norm(y));
    let any = |set: &[u8], c: u8| set.contains(&c);
    let table: [(&[u8], &[u8], FileStatus); 8] = [
      (b"MADRC", b" ", FileStatus::Staged),
      (b" MADRC", b"M", FileStatus::Changed),
      (b" MARC", b"D", FileStatus::Deleted),
      (b"D", b"DU", FileStatus::Conflict),
      (b"A", b"AU", FileStatus::Conflict),
      (b"U", b"ADU", FileStatus::Conflict),
      (b"?", b"?", FileStatus::Untracked),
      (b"!", b"!", FileStatus::Ignored),
    ];
    table
      .iter()
      .find(|(xs, ys, _)| any(xs, x) && any(ys, y))
      .map_or(FileStatus::Unknown, |(_, _, s)| *s)
  }
}

/// Relation of the checked-out branch to its upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteStatus {
  Sync,
  Ahead,
  Behind,
  Diverged,
  /// No upstream configured (or detached HEAD).
  None,
}

impl RemoteStatus {
  pub fn symbol(self) -> char {
    match self {
      RemoteStatus::Sync => '=',
      RemoteStatus::Ahead => '>',
      RemoteStatus::Behind => '<',
      RemoteStatus::Diverged => 'Y',
      RemoteStatus::None => '⌂',
    }
  }

  pub fn from_ahead_behind(ahead: u64, behind: u64) -> Self {
    match (ahead > 0, behind > 0) {
      (true, true) => RemoteStatus::Diverged,
      (true, false) => RemoteStatus::Ahead,
      (false, true) => RemoteStatus::Behind,
      (false, false) => RemoteStatus::Sync,
    }
  }
}

/// One character in the VCS column of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
  File(FileStatus),
  Remote(RemoteStatus),
  /// Placeholder keeping columns aligned.
  Blank,
}

impl Mark {
  pub fn symbol(self) -> char {
    match self {
      Mark::File(s) => s.symbol(),
      Mark::Remote(r) => r.symbol(),
      Mark::Blank => ' ',
    }
  }
}

/// The commit HEAD points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
  /// Commit time in seconds since the Unix epoch.
  pub time: i64,
  /// First line of the message, control characters replaced.
  pub summary: String,
}

/// Snapshot of one repository's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStatus {
  pub root: PathBuf,
  pub remote: RemoteStatus,
  /// Checked-out branch; `None` when detached.
  pub branch: Option<String>,
  /// Last commit on HEAD; `None` in a repository without commits.
  pub head: Option<Commit>,
  /// Status shown for the repository root itself.
  pub summary: FileStatus,
  /// Paths (relative to the root) with a non-sync status. Whole untracked or ignored
  /// directories are stored without their trailing slash.
  paths: HashMap<PathBuf, FileStatus>,
  /// Aggregated status for directories that contain changed paths.
  dirs: HashMap<PathBuf, FileStatus>,
}

impl RepoStatus {
  pub fn new(root: PathBuf, remote: RemoteStatus, paths: HashMap<PathBuf, FileStatus>) -> Self {
    let mut dirs: HashMap<PathBuf, FileStatus> = HashMap::new();
    let mut summary = FileStatus::Sync;
    for (path, &status) in &paths {
      if status == FileStatus::Ignored {
        continue;
      }
      if status.rank() < summary.rank() {
        summary = status;
      }
      for dir in path.ancestors().skip(1) {
        if dir.as_os_str().is_empty() {
          break;
        }
        let slot = dirs.entry(dir.to_path_buf()).or_insert(status);
        if status.rank() < slot.rank() {
          *slot = status;
        }
      }
    }
    RepoStatus {
      root,
      remote,
      branch: None,
      head: None,
      summary,
      paths,
      dirs,
    }
  }

  /// Status of `path` (absolute, inside this repository).
  pub fn status_of(&self, path: &Path, is_dir: bool) -> FileStatus {
    let Ok(rel) = path.strip_prefix(&self.root) else {
      return FileStatus::Unknown;
    };
    if rel.as_os_str().is_empty() {
      return self.summary;
    }
    // The path itself, or a whole directory containing it, has a status.
    for p in rel.ancestors() {
      if p.as_os_str().is_empty() {
        break;
      }
      if let Some(&s) = self.paths.get(p) {
        return s;
      }
    }
    if is_dir && let Some(&s) = self.dirs.get(rel) {
      return s;
    }
    FileStatus::Sync
  }
}

/// What the background scan found for one listed directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirVcs {
  /// Repository containing the directory, if any.
  pub root: Option<PathBuf>,
  /// True when at least one subdirectory is itself a repository root.
  pub has_repo_child: bool,
}

/// Request to compute VCS status for the entries of one listed directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcsRequest {
  pub dir: PathBuf,
  /// Subdirectories of `dir`, checked for being repository roots themselves.
  pub subdirs: Vec<PathBuf>,
  /// Bypass the status cache (used by reload).
  pub force: bool,
}
