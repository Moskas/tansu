use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use crossterm::event::{self as ct, KeyEvent};

use crate::fs::{DirCount, DirListing};
use crate::preview::Preview;

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
  },
  Counts(Vec<(PathBuf, DirCount)>),
}

/// Spawns a thread that forwards terminal input events until the receiver goes away.
pub fn spawn_input_thread(tx: Sender<Event>) {
  thread::spawn(move || {
    while let Ok(ev) = ct::read() {
      let ev = match ev {
        ct::Event::Key(k) => Event::Key(k),
        ct::Event::Resize(w, h) => Event::Resize(w, h),
        _ => continue,
      };
      if tx.send(ev).is_err() {
        break;
      }
    }
  });
}
