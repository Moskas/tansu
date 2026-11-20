//! Long file operations (copy, move, delete, extract) as tasks: how a worker reports their
//! progress and is paused or cancelled, and the app's view of them for the task popup.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::fs::PasteMode;
use crate::fs::ops::{Conflict, Resolution};

pub type JobId = u64;

/// A task is shown this long after it was started, so quick ones never flash a popup.
pub const SHOW_AFTER: Duration = Duration::from_millis(400);
/// Progress is reported at most this often.
const REPORT_EVERY: Duration = Duration::from_millis(100);
/// Speed is measured over at least this long.
const RATE_WINDOW: Duration = Duration::from_millis(500);
/// How long a toast stays: a success, and a failure.
const TOAST_OK: Duration = Duration::from_secs(5);
const TOAST_FAILED: Duration = Duration::from_secs(10);
/// Toasts shown at once; older ones go first.
const MAX_TOASTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
  Copy,
  Move,
  Delete,
  Trash,
  Extract,
  /// A shell command run with `:shell -b`.
  Shell,
}

impl JobKind {
  /// `Copying`, the popup's title.
  pub fn verb(self) -> &'static str {
    match self {
      JobKind::Copy => "Copying",
      JobKind::Move => "Moving",
      JobKind::Delete => "Deleting",
      JobKind::Trash => "Trashing",
      JobKind::Extract => "Extracting",
      JobKind::Shell => "Running",
    }
  }

  /// `Copy`, in the task list and the status bar.
  pub fn name(self) -> &'static str {
    match self {
      JobKind::Copy => "Copy",
      JobKind::Move => "Move",
      JobKind::Delete => "Delete",
      JobKind::Trash => "Trash",
      JobKind::Extract => "Extract",
      JobKind::Shell => "Shell",
    }
  }

  /// `Copied`, for a finished task.
  pub fn past(self) -> &'static str {
    match self {
      JobKind::Copy => "Copied",
      JobKind::Move => "Moved",
      JobKind::Delete => "Deleted",
      JobKind::Trash => "Trashed",
      JobKind::Extract => "Extracted",
      JobKind::Shell => "Finished",
    }
  }

  /// What its counts count: copies and extractions go file by file, the others entry by
  /// entry.
  pub fn unit(self) -> &'static str {
    match self {
      JobKind::Copy | JobKind::Extract => "files",
      JobKind::Move | JobKind::Delete | JobKind::Trash | JobKind::Shell => "items",
    }
  }

  pub fn from_paste(mode: PasteMode) -> Self {
    match mode {
      PasteMode::Copy => JobKind::Copy,
      PasteMode::Move => JobKind::Move,
    }
  }
}

/// How far a task got. Totals are unknown while they are being counted, and for archives
/// unpacked by another program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
  pub files_done: u64,
  pub files_total: Option<u64>,
  pub bytes_done: u64,
  pub bytes_total: Option<u64>,
  /// The name being worked on, safe to draw.
  pub current: String,
}

impl Progress {
  /// The part done, from 0 to 1, by bytes when they are known and by files otherwise.
  pub fn fraction(&self) -> Option<f64> {
    let part = |done: u64, total: u64| (done as f64 / total as f64).min(1.0);
    match (self.bytes_total, self.files_total) {
      (Some(total), _) if total > 0 => Some(part(self.bytes_done, total)),
      (_, Some(total)) if total > 0 => Some(part(self.files_done, total)),
      _ => None,
    }
  }
}

/// Shared by whoever started a task and the worker running it.
#[derive(Debug, Default)]
pub struct Control {
  cancelled: AtomicBool,
  paused: Mutex<bool>,
  resumed: Condvar,
}

impl Control {
  pub fn cancel(&self) {
    self.cancelled.store(true, Ordering::Relaxed);
    self.set_paused(false);
  }

  pub fn set_paused(&self, paused: bool) {
    *self.paused.lock().unwrap_or_else(|e| e.into_inner()) = paused;
    self.resumed.notify_all();
  }

  pub fn is_cancelled(&self) -> bool {
    self.cancelled.load(Ordering::Relaxed)
  }

  pub fn is_paused(&self) -> bool {
    *self.paused.lock().unwrap_or_else(|e| e.into_inner())
  }

  /// Waits while the task is paused. True when it should stop.
  pub fn checkpoint(&self) -> bool {
    let mut paused = self.paused.lock().unwrap_or_else(|e| e.into_inner());
    while *paused && !self.is_cancelled() {
      paused = self.resumed.wait(paused).unwrap_or_else(|e| e.into_inner());
    }
    self.is_cancelled()
  }
}

/// The error a [`Meter`] stops a task with.
#[derive(Debug)]
struct Stopped;

impl fmt::Display for Stopped {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("cancelled")
  }
}

impl std::error::Error for Stopped {}

static NEVER: fn() -> bool = || false;

/// Counts a task's progress as it goes, passes it on now and then, and stops the task once
/// `stop` says so: every count fails from then on, and [`Meter::stopped`] tells that
/// failure apart from others (which may wrap it).
pub struct Meter<'a> {
  pub progress: Progress,
  /// Waits while paused; true when the task should stop.
  stop: &'a dyn Fn() -> bool,
  report: Option<&'a mut dyn FnMut(&Progress)>,
  last: Option<Instant>,
  stopped: bool,
}

impl<'a> Meter<'a> {
  pub fn new(stop: &'a dyn Fn() -> bool, report: &'a mut dyn FnMut(&Progress)) -> Self {
    Meter {
      progress: Progress::default(),
      stop,
      report: Some(report),
      last: None,
      stopped: false,
    }
  }

  /// A meter that never stops and reports nothing.
  pub fn silent() -> Meter<'static> {
    Meter {
      progress: Progress::default(),
      stop: &NEVER,
      report: None,
      last: None,
      stopped: false,
    }
  }

  pub fn stopped(&self) -> bool {
    self.stopped
  }

  /// Fails once the task is to stop; waits while it is paused.
  pub fn check(&mut self) -> io::Result<()> {
    if !self.stopped && (self.stop)() {
      self.stopped = true;
    }
    if self.stopped {
      return Err(io::Error::other(Stopped));
    }
    Ok(())
  }

  /// Starts on the entry `name`.
  pub fn item(&mut self, name: &str) -> io::Result<()> {
    self.progress.current = crate::fs::entry::sanitize(name);
    self.tick()
  }

  /// Finishes an entry.
  pub fn item_done(&mut self) {
    self.progress.files_done += 1;
  }

  pub fn add_bytes(&mut self, n: u64) -> io::Result<()> {
    self.progress.bytes_done += n;
    self.tick()
  }

  /// Passes the progress on now.
  pub fn report(&mut self) {
    self.last = Some(Instant::now());
    if let Some(report) = &mut self.report {
      report(&self.progress);
    }
  }

  fn tick(&mut self) -> io::Result<()> {
    self.check()?;
    if self.last.is_none_or(|t| t.elapsed() >= REPORT_EVERY) {
      self.report();
    }
    Ok(())
  }
}

/// A paste to run once its conflicts are answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteRequest {
  pub sources: Vec<PathBuf>,
  pub dest: PathBuf,
  pub mode: PasteMode,
}

/// Conflicts of a paste, answered one by one before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictPrompt {
  pub conflicts: Vec<Conflict>,
  pub answers: Vec<(PathBuf, Resolution)>,
  /// The next answer goes for every remaining conflict too.
  pub apply_all: bool,
}

impl ConflictPrompt {
  /// The conflict waiting for an answer.
  pub fn current(&self) -> Option<&Conflict> {
    self.conflicts.get(self.answers.len())
  }

  /// Conflicts after the current one.
  pub fn remaining(&self) -> usize {
    self.conflicts.len().saturating_sub(self.answers.len() + 1)
  }

  /// Answers the current conflict (and with `apply_all` the rest). True once every
  /// conflict is answered.
  pub fn answer(&mut self, resolution: Resolution) -> bool {
    let rest = if self.apply_all {
      self.conflicts.len()
    } else {
      self.answers.len() + 1
    };
    let next = self.answers.len();
    for c in self.conflicts.iter().take(rest).skip(next) {
      self.answers.push((c.source.clone(), resolution));
    }
    self.answers.len() >= self.conflicts.len()
  }
}

/// A task as the app sees it.
#[derive(Debug, Clone)]
pub struct Job {
  pub id: JobId,
  pub kind: JobKind,
  /// What it works on: a name, or `3 items`.
  pub what: String,
  /// Where it puts things (copy, move and extract).
  pub dest: Option<PathBuf>,
  /// Number of the tab it was started from, whose label in the header shows it.
  pub tab: usize,
  pub progress: Progress,
  pub created: Instant,
  /// When the worker started on it; it is queued until then.
  pub started: Option<Instant>,
  pub paused: bool,
  pub cancelled: bool,
  /// The paste to run, for one whose conflicts need answers.
  pub request: Option<PasteRequest>,
  pub prompt: Option<ConflictPrompt>,
  /// Where a shell task's output goes, once the runtime has created the file.
  pub log: Option<PathBuf>,
  /// Bytes per second, smoothed.
  rate: f64,
  sample: Option<(Instant, u64)>,
}

impl Job {
  pub fn is_running(&self) -> bool {
    self.started.is_some() && self.prompt.is_none()
  }

  /// Bytes per second, once measured.
  pub fn rate(&self) -> Option<f64> {
    (self.rate > 0.0 && !self.paused).then_some(self.rate)
  }

  /// Time left at the current speed.
  pub fn eta(&self) -> Option<Duration> {
    let total = self.progress.bytes_total?;
    let left = total.saturating_sub(self.progress.bytes_done);
    let rate = self.rate()?;
    Some(Duration::from_secs_f64(left as f64 / rate))
  }

  pub fn update(&mut self, progress: Progress, now: Instant) {
    self.started.get_or_insert(now);
    match self.sample {
      Some((at, bytes)) => {
        let dt = now.saturating_duration_since(at);
        if dt >= RATE_WINDOW {
          let now_rate = progress.bytes_done.saturating_sub(bytes) as f64 / dt.as_secs_f64();
          self.rate = if self.rate > 0.0 {
            0.6 * self.rate + 0.4 * now_rate
          } else {
            now_rate
          };
          self.sample = Some((now, progress.bytes_done));
        }
      }
      None => self.sample = Some((now, progress.bytes_done)),
    }
    self.progress = progress;
  }

  pub fn set_paused(&mut self, paused: bool) {
    self.paused = paused;
    // Time spent paused would count as slow progress.
    self.sample = None;
  }
}

/// A finished task, shown for a while in the corner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
  pub ok: bool,
  /// `Copied`, `Delete failed`.
  pub title: String,
  pub what: String,
  /// How much and how long, or why it failed.
  pub detail: String,
  pub at: Instant,
}

/// The tasks, and how they are shown.
#[derive(Debug, Default)]
pub struct Jobs {
  pub list: Vec<Job>,
  next_id: JobId,
  /// The task selected in the list.
  pub selected: usize,
  /// Sent to the background (`b`, `<Esc>`); the status bar shows them instead.
  pub hidden: bool,
  /// Shown with `w`, without waiting for [`SHOW_AFTER`].
  pub forced: bool,
  pub toasts: Vec<Toast>,
  /// Logs of finished shell tasks, the latest last, for `:tasklog`.
  pub finished_logs: Vec<FinishedLog>,
}

/// The output of a finished shell task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedLog {
  /// The command line, as shown in the task list.
  pub what: String,
  pub log: PathBuf,
  pub ok: bool,
}

/// Logs of finished shell tasks kept for `:tasklog`.
pub const MAX_FINISHED_LOGS: usize = 20;

impl Jobs {
  /// Keeps a finished task's log. Returns the logs that no longer fit, oldest first, whose
  /// files can go.
  pub fn keep_log(&mut self, log: FinishedLog) -> Vec<PathBuf> {
    self.finished_logs.push(log);
    let excess = self.finished_logs.len().saturating_sub(MAX_FINISHED_LOGS);
    self.finished_logs.drain(..excess).map(|l| l.log).collect()
  }

  /// Every log file still known: of running tasks and of finished ones.
  pub fn log_files(&self) -> Vec<PathBuf> {
    let running = self.list.iter().filter_map(|j| j.log.clone());
    running
      .chain(self.finished_logs.iter().map(|l| l.log.clone()))
      .collect()
  }

  pub fn add(&mut self, kind: JobKind, what: String, dest: Option<PathBuf>, tab: usize) -> JobId {
    self.next_id += 1;
    self.list.push(Job {
      id: self.next_id,
      kind,
      what,
      dest,
      tab,
      progress: Progress::default(),
      created: Instant::now(),
      started: None,
      paused: false,
      cancelled: false,
      request: None,
      prompt: None,
      log: None,
      rate: 0.0,
      sample: None,
    });
    // Tasks sent to the background stay there: a new one joins them in the status bar.
    self.next_id
  }

  pub fn get_mut(&mut self, id: JobId) -> Option<&mut Job> {
    self.list.iter_mut().find(|j| j.id == id)
  }

  pub fn remove(&mut self, id: JobId) -> Option<Job> {
    let i = self.list.iter().position(|j| j.id == id)?;
    let job = self.list.remove(i);
    if self.list.is_empty() {
      self.hidden = false;
      self.forced = false;
    }
    self.selected = self.selected.min(self.list.len().saturating_sub(1));
    Some(job)
  }

  pub fn selected_job(&self) -> Option<&Job> {
    self.list.get(self.selected)
  }

  /// The first task whose conflicts wait for answers.
  pub fn prompting(&self) -> Option<&Job> {
    self.list.iter().find(|j| j.prompt.is_some())
  }

  /// Whether the popup is up: not sent to the background, and asked for (`w`), waiting for
  /// answers, or running long enough to be worth showing.
  pub fn popup_wanted(&self) -> bool {
    !self.hidden
      && (self.forced
        || self.prompting().is_some()
        || self.list.iter().any(|j| j.created.elapsed() >= SHOW_AFTER))
  }

  pub fn running(&self) -> usize {
    self.list.iter().filter(|j| j.is_running()).count()
  }

  pub fn queued(&self) -> usize {
    self
      .list
      .iter()
      .filter(|j| j.started.is_none() && j.prompt.is_none())
      .count()
  }

  pub fn toast(&mut self, ok: bool, title: String, what: String, detail: String) {
    if self.toasts.len() >= MAX_TOASTS {
      self.toasts.remove(0);
    }
    self.toasts.push(Toast {
      ok,
      title,
      what,
      detail,
      at: Instant::now(),
    });
  }

  /// Drops toasts that were shown long enough.
  pub fn expire_toasts(&mut self) {
    self.toasts.retain(|t| {
      let ttl = if t.ok { TOAST_OK } else { TOAST_FAILED };
      t.at.elapsed() < ttl
    });
  }
}

/// `0:42`, `1:02:03`.
pub fn format_duration(d: Duration) -> String {
  let secs = d.as_secs();
  let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
  if h > 0 {
    format!("{h}:{m:02}:{s:02}")
  } else {
    format!("{m}:{s:02}")
  }
}
