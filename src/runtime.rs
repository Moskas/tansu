//! Executes the side effects the app requests ([`Command`]s) and feeds results back as
//! [`Event`]s. The async [`Runtime`] is used by the binary; [`execute_sync`] by tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::Command;
use crate::event::Event;
use crate::fs::{DirCount, Entry, count_entries, list_dir};
use crate::preview::{PreviewContext, PreviewError, PreviewLimits, PreviewRegistry};
use crate::vcs::{self, DirVcs, GitError, RepoStatus, VcsRequest};
use crate::worker::{CancelToken, LatestWorker};

/// Upper bound on entries read for a navigable directory.
pub const LIST_MAX_ENTRIES: usize = 200_000;
/// Item counts stop at this many entries per subdirectory.
pub const COUNT_CAP: usize = 100_000;
const COUNT_FLUSH: Duration = Duration::from_millis(100);
/// Repository status younger than this is reused instead of running git again.
const VCS_TTL: Duration = Duration::from_secs(3);
/// At most this many subdirectories are checked for being repository roots.
const VCS_MAX_SUBDIRS: usize = 1000;

pub fn load_dir(path: &Path) -> Event {
  let result = list_dir(path, LIST_MAX_ENTRIES, &|| false).map_err(|e| e.to_string());
  Event::DirLoaded {
    path: path.to_path_buf(),
    result,
  }
}

/// Runs the registry for one entry. Returns `None` if the job was cancelled.
pub fn run_preview(
  registry: &PreviewRegistry,
  limits: PreviewLimits,
  id: u64,
  entry: &Entry,
  token: &CancelToken,
) -> Option<Event> {
  let cancelled = || token.is_cancelled();
  let ctx = PreviewContext {
    limits,
    cancelled: &cancelled,
  };
  let result = match registry.preview(entry, &ctx) {
    Err(PreviewError::Cancelled) => return None,
    r => r.map_err(|e| e.to_string()),
  };
  Some(Event::PreviewReady {
    id,
    path: entry.path.clone(),
    result,
  })
}

/// Counts each directory's entries, emitting batches so counts appear progressively.
pub fn run_counts(dirs: &[PathBuf], token: &CancelToken, emit: &mut dyn FnMut(Event)) {
  let mut batch: Vec<(PathBuf, DirCount)> = Vec::new();
  let mut last = Instant::now();
  for dir in dirs {
    if token.is_cancelled() {
      return;
    }
    if let Some(count) = count_entries(dir, COUNT_CAP) {
      batch.push((dir.clone(), count));
    }
    if last.elapsed() >= COUNT_FLUSH && !batch.is_empty() {
      emit(Event::Counts(std::mem::take(&mut batch)));
      last = Instant::now();
    }
  }
  if !batch.is_empty() {
    emit(Event::Counts(batch));
  }
}

/// Runs git for VCS requests, caching per repository so the parent, current and preview
/// columns of one repository cost a single `git status`.
#[derive(Default)]
pub struct VcsScanner {
  cache: HashMap<PathBuf, (Instant, Arc<RepoStatus>)>,
  /// Set once git turns out to be missing; no further attempts are made.
  disabled: bool,
}

impl VcsScanner {
  pub fn scan(&mut self, req: &VcsRequest) -> Option<Event> {
    if self.disabled {
      return None;
    }
    let root = vcs::find_root(&req.dir);
    // Repositories nested inside a tracked directory (submodules) are not looked for.
    let child_roots: Vec<&PathBuf> = if root.is_none() {
      req
        .subdirs
        .iter()
        .take(VCS_MAX_SUBDIRS)
        .filter(|d| vcs::is_repo_root(d))
        .collect()
    } else {
      Vec::new()
    };
    let mut repos = Vec::new();
    for r in root.iter().chain(child_roots.iter().copied()) {
      match self.status(r, req.force) {
        Ok(s) => repos.push(s),
        Err(GitError::NotInstalled) => {
          self.disabled = true;
          return None;
        }
        // Not a usable repository (corrupt, permission denied): no markers.
        Err(_) => {}
      }
    }
    Some(Event::Vcs {
      dir: req.dir.clone(),
      info: DirVcs {
        root,
        has_repo_child: !child_roots.is_empty(),
      },
      repos,
    })
  }

  fn status(&mut self, root: &Path, force: bool) -> Result<Arc<RepoStatus>, GitError> {
    if !force
      && let Some((at, s)) = self.cache.get(root)
      && at.elapsed() < VCS_TTL
    {
      return Ok(Arc::clone(s));
    }
    let s = Arc::new(vcs::query_repo(root)?);
    self
      .cache
      .insert(root.to_path_buf(), (Instant::now(), Arc::clone(&s)));
    Ok(s)
  }
}

/// Starts the VCS thread. Requests are handled in order; queued requests for the same
/// directory are merged so fast navigation does not pile up git runs.
fn spawn_vcs_worker(tx: Sender<Event>) -> Sender<VcsRequest> {
  let (req_tx, req_rx) = mpsc::channel::<VcsRequest>();
  let spawned = thread::Builder::new().name("vcs".into()).spawn(move || {
    let mut scanner = VcsScanner::default();
    while let Ok(first) = req_rx.recv() {
      let mut merged: Vec<VcsRequest> = Vec::new();
      for req in std::iter::once(first).chain(req_rx.try_iter()) {
        match merged.iter_mut().find(|m| m.dir == req.dir) {
          Some(m) => {
            m.force |= req.force;
            m.subdirs = req.subdirs;
          }
          None => merged.push(req),
        }
      }
      for req in merged {
        if let Some(ev) = scanner.scan(&req)
          && tx.send(ev).is_err()
        {
          return;
        }
      }
    }
  });
  if let Err(e) = spawned {
    eprintln!("tansu: could not start vcs worker: {e}");
  }
  req_tx
}

/// Background execution: one thread per directory listing, latest-wins workers for
/// previews and item counts so stale work is dropped as the cursor moves, and an ordered
/// worker for VCS status.
pub struct Runtime {
  tx: Sender<Event>,
  previews: LatestWorker<(u64, Entry)>,
  counts: LatestWorker<Vec<PathBuf>>,
  vcs: Sender<VcsRequest>,
  opener: String,
}

impl Runtime {
  pub fn new(
    tx: Sender<Event>,
    registry: Arc<PreviewRegistry>,
    limits: PreviewLimits,
    opener: String,
  ) -> Self {
    let ptx = tx.clone();
    let previews = LatestWorker::spawn("preview", move |(id, entry): (u64, Entry), token| {
      if let Some(ev) = run_preview(&registry, limits, id, &entry, token) {
        let _ = ptx.send(ev);
      }
    });
    let ctx = tx.clone();
    let counts = LatestWorker::spawn("counts", move |dirs: Vec<PathBuf>, token| {
      run_counts(&dirs, token, &mut |ev| {
        let _ = ctx.send(ev);
      });
    });
    let vcs = spawn_vcs_worker(tx.clone());
    Runtime {
      tx,
      previews,
      counts,
      vcs,
      opener,
    }
  }

  /// Starts a command. Returns an error message for failures that happen synchronously.
  pub fn execute(&mut self, cmd: Command) -> Option<String> {
    match cmd {
      Command::LoadDir(path) => {
        let tx = self.tx.clone();
        let spawned = thread::Builder::new().name("list".into()).spawn(move || {
          let _ = tx.send(load_dir(&path));
        });
        if let Err(e) = spawned {
          return Some(format!("could not start listing: {e}"));
        }
      }
      Command::Preview { id, entry } => self.previews.submit((id, entry)),
      Command::CancelPreview => self.previews.cancel(),
      Command::CountDirs(dirs) => self.counts.submit(dirs),
      Command::VcsScan(req) => {
        let _ = self.vcs.send(req);
      }
      Command::Open(path) => {
        if let Err(e) = crate::opener::open(&self.opener, &path) {
          return Some(e.to_string());
        }
      }
    }
    None
  }
}

/// Runs a command to completion on the current thread. `Open` is ignored.
pub fn execute_sync(cmd: Command, registry: &PreviewRegistry, limits: PreviewLimits) -> Vec<Event> {
  let token = CancelToken::never();
  match cmd {
    Command::LoadDir(path) => vec![load_dir(&path)],
    Command::Preview { id, entry } => run_preview(registry, limits, id, &entry, &token)
      .into_iter()
      .collect(),
    Command::CountDirs(dirs) => {
      let mut out = Vec::new();
      run_counts(&dirs, &token, &mut |ev| out.push(ev));
      out
    }
    Command::VcsScan(req) => VcsScanner::default().scan(&req).into_iter().collect(),
    Command::Open(_) | Command::CancelPreview => Vec::new(),
  }
}
