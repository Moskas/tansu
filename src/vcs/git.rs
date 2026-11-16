//! Git backend: runs `git status --porcelain=v2` and parses its NUL-separated output.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{Commit, FileStatus, RemoteStatus, RepoStatus};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
  #[error("git is not installed")]
  NotInstalled,
  #[error("running git: {0}")]
  Io(std::io::Error),
  #[error("git status failed in {}", .0.display())]
  Failed(PathBuf),
}

/// True if `dir` contains a `.git` entry (directory, or file for worktrees and submodules).
pub fn is_repo_root(dir: &Path) -> bool {
  std::fs::symlink_metadata(dir.join(".git")).is_ok()
}

/// Nearest ancestor of `dir` (inclusive) that is a repository root. Walks the given path
/// rather than asking git, so the result shares a prefix with listed entry paths even when
/// the directory was reached through a symlink.
pub fn find_root(dir: &Path) -> Option<PathBuf> {
  dir
    .ancestors()
    .find(|a| is_repo_root(a))
    .map(Path::to_path_buf)
}

/// Runs git in `root` and parses the result.
pub fn query_repo(root: &Path) -> Result<RepoStatus, GitError> {
  let out = run_git(
    root,
    &[
      "status",
      "--porcelain=v2",
      "--branch",
      "-z",
      "--ignored",
      "--untracked-files=normal",
    ],
  )?;
  let mut status = parse_status(root, &out);
  // Fails in a repository without commits; that just means no HEAD info.
  status.head = run_git(root, &["log", "-1", "--format=%ct%x00%s"])
    .ok()
    .and_then(|out| parse_head(&out));
  Ok(status)
}

fn run_git(root: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
  let out = Command::new("git")
    // Without this, `git status` may rewrite the index and collide with the user's git.
    .arg("--no-optional-locks")
    .arg("-C")
    .arg(root)
    .args(args)
    .stdin(Stdio::null())
    .stderr(Stdio::null())
    .output()
    .map_err(|e| match e.kind() {
      std::io::ErrorKind::NotFound => GitError::NotInstalled,
      _ => GitError::Io(e),
    })?;
  if !out.status.success() {
    return Err(GitError::Failed(root.to_path_buf()));
  }
  Ok(out.stdout)
}

/// Parses `git log -1 --format=%ct%x00%s`.
pub fn parse_head(out: &[u8]) -> Option<Commit> {
  let text = String::from_utf8_lossy(out);
  let (time, summary) = text.trim_end_matches('\n').split_once('\0')?;
  Some(Commit {
    time: time.trim().parse().ok()?,
    summary: crate::fs::entry::sanitize(summary),
  })
}

/// Parses `git status --porcelain=v2 --branch -z` output. Paths are relative to `root`.
pub fn parse_status(root: &Path, data: &[u8]) -> RepoStatus {
  let mut paths = HashMap::new();
  let mut has_upstream = false;
  let mut ahead_behind = None;
  let mut branch = None;
  let mut records = data.split(|&b| b == 0).filter(|r| !r.is_empty());
  while let Some(rec) = records.next() {
    match rec.first() {
      Some(b'#') => {
        if let Some(head) = rec.strip_prefix(b"# branch.head ") {
          if head != b"(detached)" {
            branch = Some(String::from_utf8_lossy(head).into_owned());
          }
        } else if rec.starts_with(b"# branch.upstream ") {
          has_upstream = true;
        } else if let Some(ab) = rec.strip_prefix(b"# branch.ab ") {
          ahead_behind = parse_ab(ab);
        }
      }
      Some(b'1') => {
        if let Some((xy, path)) = fields(rec, 9) {
          paths.insert(rel(path), FileStatus::from_xy(xy[0], xy[1]));
        }
      }
      Some(b'2') => {
        if let Some((xy, path)) = fields(rec, 10) {
          paths.insert(rel(path), FileStatus::from_xy(xy[0], xy[1]));
        }
        records.next(); // original path of the rename/copy
      }
      Some(b'u') => {
        if let Some((_, path)) = fields(rec, 11) {
          paths.insert(rel(path), FileStatus::Conflict);
        }
      }
      Some(b'?') => {
        if let Some(path) = rec.get(2..) {
          paths.insert(rel(path), FileStatus::Untracked);
        }
      }
      Some(b'!') => {
        if let Some(path) = rec.get(2..) {
          paths.insert(rel(path), FileStatus::Ignored);
        }
      }
      _ => {}
    }
  }
  let remote = match (has_upstream, ahead_behind) {
    (true, Some((a, b))) => RemoteStatus::from_ahead_behind(a, b),
    _ => RemoteStatus::None,
  };
  let mut status = RepoStatus::new(root.to_path_buf(), remote, paths);
  status.branch = branch;
  status
}

/// Splits a record into `n` space-separated fields; returns the XY code and the last field
/// (the path, which may itself contain spaces).
fn fields(rec: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
  let parts: Vec<&[u8]> = rec.splitn(n, |&b| b == b' ').collect();
  if parts.len() != n || parts[1].len() != 2 {
    return None;
  }
  Some((parts[1], parts[n - 1]))
}

fn rel(path: &[u8]) -> PathBuf {
  let path = path.strip_suffix(b"/").unwrap_or(path);
  PathBuf::from(OsStr::from_bytes(path))
}

fn parse_ab(ab: &[u8]) -> Option<(u64, u64)> {
  let text = std::str::from_utf8(ab).ok()?;
  let mut it = text.split(' ');
  let ahead = it.next()?.strip_prefix('+')?.parse().ok()?;
  let behind = it.next()?.strip_prefix('-')?.parse().ok()?;
  Some((ahead, behind))
}
