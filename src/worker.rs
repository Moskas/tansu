//! A background worker where only the most recent request matters.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;

/// Reports whether the job it was handed to has been superseded.
#[derive(Debug, Clone)]
pub struct CancelToken {
  id: u64,
  latest: Arc<AtomicU64>,
}

impl CancelToken {
  /// A token that is never cancelled, for synchronous use.
  pub fn never() -> Self {
    CancelToken {
      id: 0,
      latest: Arc::new(AtomicU64::new(0)),
    }
  }

  pub fn is_cancelled(&self) -> bool {
    self.latest.load(Ordering::Relaxed) != self.id
  }
}

/// Runs jobs on one thread. Submitting a job cancels the running one (cooperatively, via its
/// token) and drops any queued ones.
pub struct LatestWorker<R> {
  tx: Sender<(u64, R)>,
  latest: Arc<AtomicU64>,
  next_id: u64,
}

impl<R: Send + 'static> LatestWorker<R> {
  pub fn spawn(name: &str, mut job: impl FnMut(R, &CancelToken) + Send + 'static) -> Self {
    let (tx, rx) = mpsc::channel::<(u64, R)>();
    let latest = Arc::new(AtomicU64::new(0));
    let shared = Arc::clone(&latest);
    let spawned = thread::Builder::new()
      .name(name.to_string())
      .spawn(move || {
        while let Ok(mut req) = rx.recv() {
          while let Ok(newer) = rx.try_recv() {
            req = newer;
          }
          let token = CancelToken {
            id: req.0,
            latest: Arc::clone(&shared),
          };
          if !token.is_cancelled() {
            job(req.1, &token);
          }
        }
      });
    if let Err(e) = spawned {
      // Without a worker, jobs are silently dropped; the UI keeps working.
      eprintln!("tansu: could not start {name} worker: {e}");
    }
    LatestWorker {
      tx,
      latest,
      next_id: 0,
    }
  }

  pub fn submit(&mut self, req: R) {
    self.next_id += 1;
    self.latest.store(self.next_id, Ordering::Relaxed);
    let _ = self.tx.send((self.next_id, req));
  }

  /// Cancels the running job (if any) without starting a new one.
  pub fn cancel(&mut self) {
    self.next_id += 1;
    self.latest.store(self.next_id, Ordering::Relaxed);
  }
}
