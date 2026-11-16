//! Executes the side effects the app requests ([`Command`]s) and feeds results back as
//! [`Event`]s. The async [`Runtime`] is used by the binary; [`execute_sync`] by tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use crate::app::Command;
use crate::event::Event;
use crate::fs::{DirCount, Entry, count_entries, list_dir};
use crate::preview::{PreviewContext, PreviewError, PreviewLimits, PreviewRegistry};
use crate::worker::{CancelToken, LatestWorker};

/// Upper bound on entries read for a navigable directory.
pub const LIST_MAX_ENTRIES: usize = 200_000;
/// Item counts stop at this many entries per subdirectory.
pub const COUNT_CAP: usize = 100_000;
const COUNT_FLUSH: Duration = Duration::from_millis(100);

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

/// Background execution: one thread per directory listing, plus latest-wins workers for
/// previews and item counts so stale work is dropped as the cursor moves.
pub struct Runtime {
  tx: Sender<Event>,
  previews: LatestWorker<(u64, Entry)>,
  counts: LatestWorker<Vec<PathBuf>>,
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
    Runtime {
      tx,
      previews,
      counts,
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
    Command::Open(_) | Command::CancelPreview => Vec::new(),
  }
}
