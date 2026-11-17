use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::event::{self as ct, KeyEvent};

use crate::fs::{DirCount, DirListing};
use crate::graphics::EncodedImage;
use crate::preview::Preview;
use crate::vcs::{DirVcs, RepoStatus};

/// Everything the main loop reacts to: terminal input, timer ticks, and results from
/// background jobs.
#[derive(Debug)]
pub enum Event {
  Key(KeyEvent),
  Resize(u16, u16),
  /// Emitted when no other event arrived within the tick interval.
  Tick,
  DirLoaded {
    path: PathBuf,
    result: Result<DirListing, String>,
  },
  PreviewReady {
    id: u64,
    path: PathBuf,
    result: Result<Preview, String>,
    /// The image of an image preview, encoded for the preview column.
    image: Option<EncodedImage>,
  },
  Counts(Vec<(PathBuf, DirCount)>),
  /// The directory zoxide picked for a `:z` query.
  Zoxide(Result<PathBuf, String>),
  /// Result of renaming `from` to `to`.
  Renamed {
    from: PathBuf,
    to: PathBuf,
    result: Result<(), String>,
  },
  /// VCS status for the entries of `dir`, plus the repositories involved.
  Vcs {
    dir: PathBuf,
    info: DirVcs,
    repos: Vec<Arc<RepoStatus>>,
  },
}

/// How long the input thread waits for input before checking whether it should pause.
const INPUT_POLL: Duration = Duration::from_millis(50);

#[derive(Default)]
struct InputState {
  pause_requested: bool,
  /// The thread is parked (or gone) and not reading the terminal.
  parked: bool,
}

/// Handle to the input thread, used to stop it reading while another program owns the
/// terminal.
#[derive(Clone)]
pub struct InputHandle {
  state: Arc<(Mutex<InputState>, Condvar)>,
}

impl InputHandle {
  /// Blocks until the input thread has stopped reading. crossterm reads pending bytes into
  /// its own buffer even when only polling, so the thread must be parked, not just flagged.
  pub fn pause(&self) {
    let (lock, cv) = &*self.state;
    let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
    s.pause_requested = true;
    while !s.parked {
      s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
    }
  }

  pub fn resume(&self) {
    let (lock, cv) = &*self.state;
    let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
    s.pause_requested = false;
    cv.notify_all();
  }
}

/// Marks the thread as parked when it exits, so `pause` never waits for a dead thread.
struct ParkOnExit(Arc<(Mutex<InputState>, Condvar)>);

impl Drop for ParkOnExit {
  fn drop(&mut self) {
    let (lock, cv) = &*self.0;
    lock.lock().unwrap_or_else(|e| e.into_inner()).parked = true;
    cv.notify_all();
  }
}

/// Spawns a thread that forwards terminal input events until the receiver goes away.
pub fn spawn_input_thread(tx: Sender<Event>) -> InputHandle {
  let state = Arc::new((Mutex::new(InputState::default()), Condvar::new()));
  let handle = InputHandle {
    state: Arc::clone(&state),
  };
  thread::spawn(move || {
    let _exit = ParkOnExit(Arc::clone(&state));
    loop {
      wait_while_paused(&state);
      match ct::poll(INPUT_POLL) {
        Ok(true) => {}
        Ok(false) => continue,
        Err(_) => return,
      }
      let ev = match ct::read() {
        Ok(ct::Event::Key(k)) => Event::Key(k),
        Ok(ct::Event::Resize(w, h)) => Event::Resize(w, h),
        Ok(_) => continue,
        Err(_) => return,
      };
      if tx.send(ev).is_err() {
        return;
      }
    }
  });
  handle
}

fn wait_while_paused(state: &(Mutex<InputState>, Condvar)) {
  let (lock, cv) = state;
  let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
  if !s.pause_requested {
    return;
  }
  s.parked = true;
  cv.notify_all();
  while s.pause_requested {
    s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
  }
  s.parked = false;
}
