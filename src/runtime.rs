//! Executes the side effects the app requests ([`Command`]s) and feeds results back as
//! [`Event`]s. The async [`Runtime`] is used by the binary; [`execute_sync`] by tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::app::Command;
use crate::clipboard::{self, Backend};
use crate::event::Event;
use crate::fs::ops::OnConflict;
use crate::fs::{DirCount, Entry, PasteMode, count_entries, dir_mtime, list_dir, ops};
use crate::graphics::ImageEncoder;
use crate::jobs::{Control, JobId, Meter, Progress};
use crate::preview::{Preview, PreviewContext, PreviewError, PreviewLimits, PreviewRegistry};
use crate::shell::ShellMode;
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

/// Reports the directories whose modification time is no longer the one their listing saw.
pub fn check_dirs(dirs: &[(PathBuf, Option<SystemTime>)]) -> Option<Event> {
  let changed: Vec<PathBuf> = dirs
    .iter()
    .filter(|(path, seen)| dir_mtime(path) != *seen)
    .map(|(path, _)| path.clone())
    .collect();
  (!changed.is_empty()).then_some(Event::DirsChanged(changed))
}

/// Asks zoxide for the best match for `keywords`, skipping `exclude` (the current
/// directory) the way the shell's `z` does.
pub fn zoxide_query(keywords: &[String], exclude: &Path) -> Event {
  use std::os::unix::ffi::OsStrExt;
  let output = std::process::Command::new("zoxide")
    .arg("query")
    .arg("--exclude")
    .arg(exclude)
    .arg("--")
    .args(keywords)
    .stdin(std::process::Stdio::null())
    .output();
  let result = match output {
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
      Err("z: zoxide is not installed".to_string())
    }
    Err(e) => Err(format!("z: could not run zoxide: {e}")),
    Ok(out) if out.status.success() => {
      let path = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
      Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path)))
    }
    Ok(out) => {
      let stderr = String::from_utf8_lossy(&out.stderr);
      if stderr.contains("no match") {
        Err(format!("z: no match for `{}`", keywords.join(" ")))
      } else {
        Err(format!("z: {}", stderr.trim()))
      }
    }
  };
  Event::Zoxide(result)
}

/// Asks `file` for the MIME type of `path`, following symlinks, for the `[[open]]` rules.
pub fn detect_mime(path: &Path) -> Event {
  let output = std::process::Command::new("file")
    .args(["--brief", "--mime-type", "-L", "--"])
    .arg(path)
    .stdin(std::process::Stdio::null())
    .output();
  let mime = match output {
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
      Err("open: `file` is not installed, so MIME rules are skipped".to_string())
    }
    Err(e) => Err(format!("open: could not run `file`: {e}")),
    Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
    Ok(out) => Err(format!(
      "open: file: {}",
      String::from_utf8_lossy(&out.stderr).trim()
    )),
  };
  Event::MimeDetected {
    path: path.to_path_buf(),
    mime,
  }
}

/// Starts the thread that records directory visits with `zoxide add`, one at a time so
/// concurrent writes to the database cannot lose updates. Without zoxide installed it stops
/// after the first attempt.
fn spawn_zoxide_add_worker() -> Sender<PathBuf> {
  let (tx, rx) = mpsc::channel::<PathBuf>();
  let spawned = thread::Builder::new()
    .name("zoxide-add".into())
    .spawn(move || {
      while let Ok(dir) = rx.recv() {
        let status = std::process::Command::new("zoxide")
          .arg("add")
          .arg("--")
          .arg(&dir)
          .stdin(std::process::Stdio::null())
          .stdout(std::process::Stdio::null())
          .stderr(std::process::Stdio::null())
          .status();
        if matches!(&status, Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
          return;
        }
      }
    });
  if let Err(e) = spawned {
    eprintln!("tansu: could not start zoxide worker: {e}");
  }
  tx
}

/// Renames `from` to `to`, refusing to replace an existing entry.
pub fn rename(from: &Path, to: &Path) -> Event {
  let name = |p: &Path| {
    p.file_name()
      .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
  };
  let result = if to.symlink_metadata().is_ok() {
    Err(format!("rename: `{}` already exists", name(to)))
  } else {
    std::fs::rename(from, to).map_err(|e| format!("rename: {}: {e}", name(from)))
  };
  Event::Renamed {
    from: from.to_path_buf(),
    to: to.to_path_buf(),
    result,
  }
}

/// Pastes each of `sources` into `dest`, carrying on past failures and counting with
/// `meter`. With [`OnConflict::Ask`], conflicts are looked for first; when there are any,
/// they are reported and nothing is pasted.
pub fn paste(
  job: JobId,
  sources: &[PathBuf],
  dest: &Path,
  mode: PasteMode,
  on_conflict: &OnConflict,
  meter: &mut Meter,
) -> Event {
  let mut results = Vec::new();
  if meter.check().is_ok() {
    if *on_conflict == OnConflict::Ask {
      let conflicts: Vec<_> = sources
        .iter()
        .filter_map(|s| ops::probe(s, dest, mode))
        .collect();
      if !conflicts.is_empty() {
        return Event::PasteConflicts { job, conflicts };
      }
    }
    // Started: counting a large tree takes a while, and it should not look queued.
    meter.progress.current = "counting…".to_string();
    meter.report();
    let (mut files, mut bytes) = (0, 0);
    for src in sources {
      let (f, b) = ops::paste_totals(src, dest, mode, meter);
      files += f;
      bytes += b;
    }
    meter.progress.files_total = Some(files);
    meter.progress.bytes_total = Some(bytes);
    meter.report();
    for src in sources {
      let result = ops::paste_with(src, dest, mode, on_conflict.for_source(src), meter);
      // What was stopped halfway was undone: it is left out like the rest.
      if meter.stopped() {
        break;
      }
      results.push((src.clone(), result));
    }
  }
  Event::Pasted {
    job,
    dest: dest.to_path_buf(),
    mode,
    results,
    stopped: meter.stopped(),
  }
}

/// Deletes each of `paths`, or moves it into the trash at `trash`, carrying on past failures.
/// `meter` counts them, and can stop between them.
pub fn delete(job: JobId, paths: &[PathBuf], trash: Option<&Path>, meter: &mut Meter) -> Event {
  meter.progress.files_total = Some(paths.len() as u64);
  meter.report();
  let mut results = Vec::new();
  for path in paths {
    let name = path.file_name().unwrap_or(path.as_os_str());
    if meter.item(&name.to_string_lossy()).is_err() {
      break;
    }
    let result = match trash {
      Some(root) => crate::fs::trash::trash(path, root).map(drop),
      None => ops::remove(path),
    };
    meter.item_done();
    results.push((path.clone(), result));
  }
  Event::Deleted {
    job,
    trashed: trash.is_some(),
    results,
    stopped: meter.stopped(),
  }
}

/// Extracts each of `archives` into `dest`, carrying on past failures and counting with
/// `meter`. With `skip_damaged`, damaged members are left out instead of failing their
/// archive.
pub fn extract(
  job: JobId,
  archives: &[PathBuf],
  dest: &Path,
  skip_damaged: bool,
  meter: &mut Meter,
) -> Event {
  meter.report();
  let mut files = Some(0);
  let mut bytes = 0;
  for archive in archives {
    let (f, b) = crate::archive::extract_totals(archive);
    files = files.zip(f).map(|(a, b)| a + b);
    bytes += b;
  }
  meter.progress.files_total = files;
  meter.progress.bytes_total = Some(bytes);
  meter.report();
  let mut results = Vec::new();
  for archive in archives {
    let result = crate::archive::extract_with(archive, dest, skip_damaged, meter);
    if meter.stopped() {
      break;
    }
    results.push((archive.clone(), result));
  }
  Event::Extracted {
    job,
    dest: dest.to_path_buf(),
    results,
    stopped: meter.stopped(),
  }
}

/// Creates a directory or an empty file.
pub fn create(path: &Path, dir: bool) -> Event {
  Event::Created {
    path: path.to_path_buf(),
    dir,
    result: ops::create(path, dir),
  }
}

/// Work for the file operation thread.
enum FileJob {
  Paste {
    sources: Vec<PathBuf>,
    dest: PathBuf,
    mode: PasteMode,
    on_conflict: OnConflict,
  },
  /// Paths to delete, and the trash to move them into instead.
  Delete {
    paths: Vec<PathBuf>,
    trash: Option<PathBuf>,
  },
  Extract {
    archives: Vec<PathBuf>,
    dest: PathBuf,
    skip_damaged: bool,
  },
}

/// A task for the file operation thread, and what pauses or cancels it.
struct FileTask {
  job: JobId,
  control: Arc<Control>,
  work: FileJob,
}

/// Runs a task, counting with `meter`.
fn run_file_job(job: JobId, work: FileJob, meter: &mut Meter) -> Event {
  match work {
    FileJob::Paste {
      sources,
      dest,
      mode,
      on_conflict,
    } => paste(job, &sources, &dest, mode, &on_conflict, meter),
    FileJob::Delete { paths, trash } => delete(job, &paths, trash.as_deref(), meter),
    FileJob::Extract {
      archives,
      dest,
      skip_damaged,
    } => extract(job, &archives, &dest, skip_damaged, meter),
  }
}

/// The task a command starts on the file operation thread, if it is one.
fn file_job(cmd: Command) -> Result<(JobId, FileJob), Box<Command>> {
  Ok(match cmd {
    Command::Paste {
      job,
      sources,
      dest,
      mode,
      on_conflict,
    } => (
      job,
      FileJob::Paste {
        sources,
        dest,
        mode,
        on_conflict,
      },
    ),
    Command::Delete { job, paths, trash } => (job, FileJob::Delete { paths, trash }),
    Command::Extract {
      job,
      archives,
      dest,
      skip_damaged,
    } => (
      job,
      FileJob::Extract {
        archives,
        dest,
        skip_damaged,
      },
    ),
    cmd => return Err(Box::new(cmd)),
  })
}

/// Starts the thread that runs pastes, deletions and extractions one after another, so they
/// never race for the same names and a large copy or delete does not hold up the UI. Each
/// reports its progress as it goes.
fn spawn_file_worker(tx: Sender<Event>) -> Sender<FileTask> {
  let (job_tx, job_rx) = mpsc::channel::<FileTask>();
  let spawned = thread::Builder::new()
    .name("file-ops".into())
    .spawn(move || {
      while let Ok(FileTask { job, control, work }) = job_rx.recv() {
        let stop = || control.checkpoint();
        let progress_tx = tx.clone();
        let mut report = |p: &Progress| {
          let _ = progress_tx.send(Event::JobProgress {
            job,
            progress: p.clone(),
          });
        };
        let mut meter = Meter::new(&stop, &mut report);
        let ev = run_file_job(job, work, &mut meter);
        if tx.send(ev).is_err() {
          return;
        }
      }
    });
  if let Err(e) = spawned {
    eprintln!("tansu: could not start file worker: {e}");
  }
  job_tx
}

/// Runs the registry for one entry and, for images, encodes the result for `cells` when an
/// encoder is available. Returns `None` if the job was cancelled.
pub fn run_preview(
  registry: &PreviewRegistry,
  limits: PreviewLimits,
  encoder: Option<&ImageEncoder>,
  id: u64,
  entry: &Entry,
  cells: Option<(u16, u16)>,
  token: &CancelToken,
) -> Option<Event> {
  let cancelled = || token.is_cancelled();
  let ctx = PreviewContext {
    limits,
    cancelled: &cancelled,
    thumbnails: None,
  };
  let mut result = match registry.preview(entry, &ctx) {
    Err(PreviewError::Cancelled) => return None,
    r => r.map_err(|e| e.to_string()),
  };
  let image = match (&mut result, encoder, cells) {
    (Ok(Preview::Image(img)), Some(encoder), Some((width, height))) => {
      // Details below the image take some of the column.
      let (rows, _) = crate::ui::layout::image_split(height, img.info.len(), img.details_first);
      let encoded = encoder.encode(&img.image, (width, rows));
      if token.is_cancelled() {
        return None;
      }
      encoded.map(|(encoded, scaled)| {
        // Only the scaled copy is cached, not the full-resolution decode.
        img.image = Arc::new(scaled);
        encoded
      })
    }
    _ => None,
  };
  Some(Event::PreviewReady {
    id,
    path: entry.path.clone(),
    result,
    image,
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

/// A preview request: the job id, the entry, and the preview column size.
type PreviewJob = (u64, Entry, Option<(u16, u16)>, Option<PreviewLimits>);

/// Background execution: one thread per directory listing, latest-wins workers for
/// previews and item counts so stale work is dropped as the cursor moves, and an ordered
/// worker for VCS status.
pub struct Runtime {
  tx: Sender<Event>,
  previews: LatestWorker<PreviewJob>,
  counts: LatestWorker<Vec<PathBuf>>,
  /// Checks for directories changed by other programs; a stat stuck on a dead mount never
  /// piles up requests behind it.
  watch: LatestWorker<Vec<(PathBuf, Option<SystemTime>)>>,
  vcs: Sender<VcsRequest>,
  zoxide_add: Sender<PathBuf>,
  files: Sender<FileTask>,
  /// What pauses and cancels each task given to the file thread and not finished yet.
  controls: HashMap<JobId, Arc<Control>>,
  opener: String,
}

impl Runtime {
  /// `images` encodes image previews for the terminal; without it they are not drawn.
  pub fn new(
    tx: Sender<Event>,
    registry: Arc<PreviewRegistry>,
    limits: PreviewLimits,
    images: Option<ImageEncoder>,
    opener: String,
  ) -> Self {
    let ptx = tx.clone();
    let previews = LatestWorker::spawn(
      "preview",
      move |(id, entry, cells, extended): PreviewJob, token| {
        let encoder = images.as_ref();
        let limits = extended.unwrap_or(limits);
        if let Some(ev) = run_preview(&registry, limits, encoder, id, &entry, cells, token) {
          let _ = ptx.send(ev);
        }
      },
    );
    let ctx = tx.clone();
    let counts = LatestWorker::spawn("counts", move |dirs: Vec<PathBuf>, token| {
      run_counts(&dirs, token, &mut |ev| {
        let _ = ctx.send(ev);
      });
    });
    let wtx = tx.clone();
    let watch = LatestWorker::spawn("watch", move |dirs: Vec<_>, _| {
      if let Some(ev) = check_dirs(&dirs) {
        let _ = wtx.send(ev);
      }
    });
    let vcs = spawn_vcs_worker(tx.clone());
    let zoxide_add = spawn_zoxide_add_worker();
    let files = spawn_file_worker(tx.clone());
    Runtime {
      tx,
      previews,
      counts,
      watch,
      vcs,
      zoxide_add,
      files,
      controls: HashMap::new(),
      opener,
    }
  }

  /// Queues a task on the file thread.
  fn submit(&mut self, job: JobId, work: FileJob) {
    // The worker holds the other reference until the task is done.
    self.controls.retain(|_, c| Arc::strong_count(c) > 1);
    let control = Arc::new(Control::default());
    self.controls.insert(job, Arc::clone(&control));
    let _ = self.files.send(FileTask { job, control, work });
  }

  /// Starts a command. Returns an error message for failures that happen synchronously.
  pub fn execute(&mut self, cmd: Command) -> Option<String> {
    let cmd = match file_job(cmd) {
      Ok((job, work)) => {
        self.submit(job, work);
        return None;
      }
      Err(cmd) => *cmd,
    };
    match cmd {
      Command::CancelJob(job) => {
        if let Some(control) = self.controls.get(&job) {
          control.cancel();
        }
      }
      Command::PauseJob(job, paused) => {
        if let Some(control) = self.controls.get(&job) {
          control.set_paused(paused);
        }
      }
      Command::ShellTask {
        job,
        script,
        cwd,
        program,
        env,
      } => {
        self.controls.retain(|_, c| Arc::strong_count(c) > 1);
        let control = Arc::new(Control::default());
        self.controls.insert(job, Arc::clone(&control));
        let tx = self.tx.clone();
        let spawned = thread::Builder::new()
          .name("shell-task".into())
          .spawn(move || {
            run_shell_task(job, &script, &cwd, &program, &env, &control, &mut |ev| {
              let _ = tx.send(ev);
            });
          });
        if let Err(e) = spawned {
          return Some(format!("shell: could not start task: {e}"));
        }
      }
      Command::RemoveLog(path) => {
        let _ = std::fs::remove_file(path);
      }
      Command::LoadDir(path) => {
        let tx = self.tx.clone();
        let spawned = thread::Builder::new().name("list".into()).spawn(move || {
          let _ = tx.send(load_dir(&path));
        });
        if let Err(e) = spawned {
          return Some(format!("could not start listing: {e}"));
        }
      }
      Command::Preview {
        id,
        entry,
        cells,
        limits: extended,
      } => self.previews.submit((id, entry, cells, extended)),
      Command::CancelPreview => self.previews.cancel(),
      Command::CountDirs(dirs) => self.counts.submit(dirs),
      Command::CheckDirs(dirs) => self.watch.submit(dirs),
      Command::ZoxideAdd(dir) => {
        let _ = self.zoxide_add.send(dir);
      }
      Command::VcsScan(req) => {
        let _ = self.vcs.send(req);
      }
      // Foreground programs need the terminal, so the main loop runs them; anything that
      // reaches the runtime is started detached.
      Command::Spawn { argv, path, .. } => {
        if let Err(e) = crate::opener::spawn_detached(&argv, &path) {
          return Some(e.to_string());
        }
      }
      Command::Open(path) => {
        if let Err(e) = crate::opener::open(&self.opener, &path) {
          return Some(e.to_string());
        }
      }
      Command::Zoxide { keywords, exclude } => {
        let tx = self.tx.clone();
        let spawned = thread::Builder::new().name("zoxide".into()).spawn(move || {
          let _ = tx.send(zoxide_query(&keywords, &exclude));
        });
        if let Err(e) = spawned {
          return Some(format!("z: could not start zoxide: {e}"));
        }
      }
      Command::DetectMime(path) => {
        let tx = self.tx.clone();
        let spawned = thread::Builder::new().name("file".into()).spawn(move || {
          let _ = tx.send(detect_mime(&path));
        });
        if let Err(e) = spawned {
          return Some(format!("open: could not start `file`: {e}"));
        }
      }
      Command::Shell {
        script,
        cwd,
        mode,
        program,
        env,
      } => {
        let result = match mode {
          ShellMode::Silent => {
            let tx = self.tx.clone();
            let spawned = thread::Builder::new().name("shell".into()).spawn(move || {
              let (result, commands) = crate::opener::with_command_file(&env, |env| {
                crate::opener::run_shell_silent(&script, &cwd, env).map_err(|e| e.to_string())
              });
              let _ = tx.send(Event::ProgramExited {
                program,
                result,
                commands,
              });
            });
            spawned
              .map(drop)
              .map_err(|e| format!("shell: could not start command: {e}"))
          }
          // Detached commands (`-f`, and `-t` in a new terminal window) report nothing, but
          // once they exit the listing is refreshed and the command lines they wrote are run.
          // Modes that need the terminal are run by the main loop.
          _ => {
            let tx = self.tx.clone();
            let (env, file) = match crate::opener::CommandFile::create() {
              Ok(file) => (file.env(&env), Some(file)),
              Err(_) => (env, None),
            };
            let on_exit = move || {
              let commands = file.map_or(Ok(Vec::new()), crate::opener::CommandFile::take_lines);
              drop(tx.send(Event::DetachedExited { commands }));
            };
            let spawned = if mode == ShellMode::Terminal {
              crate::opener::spawn_shell_in_terminal(&script, &cwd, &env, on_exit)
            } else {
              crate::opener::spawn_shell_detached(&script, &cwd, &env, on_exit)
            };
            spawned.map_err(|e| e.to_string())
          }
        };
        if let Err(e) = result {
          return Some(e);
        }
      }
      // A rename is a single fast syscall; it runs here so its result arrives in order.
      Command::Rename { from, to } => {
        let _ = self.tx.send(rename(&from, &to));
      }
      // Taken by `file_job` above.
      Command::Paste { .. } | Command::Delete { .. } | Command::Extract { .. } => {}
      // Creating is a single fast syscall or two, like a rename.
      Command::Create { path, dir } => {
        let _ = self.tx.send(create(&path, dir));
      }
      Command::Clipboard(content) => match clipboard::backend(&content) {
        Err(e) => return Some(e),
        // Commands run on the main thread between draws, so the sequence cannot land in
        // the middle of a frame.
        Ok(Backend::Osc52) => {
          use std::io::Write;
          let mut out = crate::terminal::TermOut;
          let written = out
            .write_all(clipboard::osc52(&content).as_bytes())
            .and_then(|()| out.flush());
          if let Err(e) = written {
            return Some(format!("clipboard: {e}"));
          }
        }
        Ok(Backend::Program(argv)) => {
          let tx = self.tx.clone();
          let spawned = thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
              let _ = tx.send(Event::Clipboard(clipboard::run(&argv, &content)));
            });
          if let Err(e) = spawned {
            return Some(format!("clipboard: could not start: {e}"));
          }
        }
      },
    }
    None
  }
}

/// How often a shell task is checked for output, exit, pause and cancel.
const SHELL_TASK_POLL: Duration = Duration::from_millis(250);
/// How long a cancelled shell task gets after SIGTERM before SIGKILL.
const SHELL_TASK_KILL_AFTER: Duration = Duration::from_secs(3);
/// Bytes read from the end of a task's log to find its last line.
const SHELL_TASK_TAIL: u64 = 4096;

/// Runs a shell task (`:shell -b`) until it exits: its output goes into a new log, the last
/// line of which is reported as its progress. Pausing stops its process group and resuming
/// continues it; cancelling sends SIGTERM, then SIGKILL if it does not exit.
pub fn run_shell_task(
  job: JobId,
  script: &std::ffi::OsStr,
  cwd: &Path,
  program: &str,
  env: &crate::opener::ShellEnv,
  control: &Control,
  emit: &mut dyn FnMut(Event),
) {
  let done = |result: Result<std::process::ExitStatus, String>, commands| Event::ShellTaskDone {
    job,
    result,
    commands,
  };
  let (log, file) = match crate::opener::create_task_log() {
    Ok(log) => log,
    Err(e) => {
      emit(done(
        Err(format!("{program}: cannot create the log: {e}")),
        Ok(Vec::new()),
      ));
      return;
    }
  };
  emit(Event::ShellTaskStarted {
    job,
    log: log.clone(),
  });
  let cmd_file = crate::opener::CommandFile::create().ok();
  let env = match &cmd_file {
    Some(f) => f.env(env),
    None => env.to_vec(),
  };
  let mut child = match crate::opener::spawn_shell_task(script, cwd, &env, &file) {
    Ok(child) => child,
    Err(e) => {
      emit(done(Err(e.to_string()), Ok(Vec::new())));
      return;
    }
  };
  drop(file);
  let pgid = child.id();
  let mut last_line = String::new();
  let mut paused = false;
  let mut terminated: Option<Instant> = None;
  let status = loop {
    match child.try_wait() {
      Ok(Some(status)) => break Ok(status),
      Ok(None) => {}
      Err(e) => break Err(format!("{program}: {e}")),
    }
    if control.is_cancelled() {
      match terminated {
        None => {
          // A stopped group would not act on SIGTERM.
          crate::opener::signal_group(pgid, libc::SIGCONT);
          crate::opener::signal_group(pgid, libc::SIGTERM);
          terminated = Some(Instant::now());
        }
        Some(at) if at.elapsed() >= SHELL_TASK_KILL_AFTER => {
          crate::opener::signal_group(pgid, libc::SIGKILL);
        }
        Some(_) => {}
      }
    } else if control.is_paused() != paused {
      paused = !paused;
      let signal = if paused { libc::SIGSTOP } else { libc::SIGCONT };
      crate::opener::signal_group(pgid, signal);
    }
    if let Some(line) = read_last_line(&log)
      && line != last_line
    {
      last_line = line;
      emit(Event::JobProgress {
        job,
        progress: Progress {
          current: last_line.clone(),
          ..Progress::default()
        },
      });
    }
    thread::sleep(SHELL_TASK_POLL);
  };
  if let Some(line) = read_last_line(&log)
    && line != last_line
  {
    emit(Event::JobProgress {
      job,
      progress: Progress {
        current: line,
        ..Progress::default()
      },
    });
  }
  let commands = cmd_file.map_or(Ok(Vec::new()), crate::opener::CommandFile::take_lines);
  emit(done(status, commands));
}

/// The last line of a task's log, from its last few KiB.
fn read_last_line(log: &Path) -> Option<String> {
  use std::io::{Read, Seek, SeekFrom};
  let mut file = std::fs::File::open(log).ok()?;
  let len = file.metadata().ok()?.len();
  file
    .seek(SeekFrom::Start(len.saturating_sub(SHELL_TASK_TAIL)))
    .ok()?;
  let mut tail = Vec::new();
  file.take(SHELL_TASK_TAIL).read_to_end(&mut tail).ok()?;
  crate::opener::last_output_line(&tail)
}

/// Runs a command to completion on the current thread. `Open`, `Spawn`, `Shell`, `ShellTask`,
/// `ZoxideAdd` and `Clipboard` are ignored, so tests never touch the user's programs, zoxide database or
/// clipboard.
pub fn execute_sync(cmd: Command, registry: &PreviewRegistry, limits: PreviewLimits) -> Vec<Event> {
  let token = CancelToken::never();
  let cmd = match file_job(cmd) {
    Ok((job, work)) => return vec![run_file_job(job, work, &mut Meter::silent())],
    Err(cmd) => *cmd,
  };
  match cmd {
    Command::LoadDir(path) => vec![load_dir(&path)],
    Command::CheckDirs(dirs) => check_dirs(&dirs).into_iter().collect(),
    Command::Preview {
      id,
      entry,
      cells,
      limits: extended,
    } => {
      let limits = extended.unwrap_or(limits);
      run_preview(registry, limits, None, id, &entry, cells, &token)
        .into_iter()
        .collect()
    }
    Command::CountDirs(dirs) => {
      let mut out = Vec::new();
      run_counts(&dirs, &token, &mut |ev| out.push(ev));
      out
    }
    Command::VcsScan(req) => VcsScanner::default().scan(&req).into_iter().collect(),
    Command::Rename { from, to } => vec![rename(&from, &to)],
    Command::Zoxide { keywords, exclude } => vec![zoxide_query(&keywords, &exclude)],
    Command::DetectMime(path) => vec![detect_mime(&path)],
    Command::Create { path, dir } => vec![create(&path, dir)],
    // Tasks run to the end at once here, so there is nothing to pause or cancel.
    Command::Paste { .. }
    | Command::Delete { .. }
    | Command::Extract { .. }
    | Command::CancelJob(_)
    | Command::PauseJob(..) => Vec::new(),
    Command::Open(_)
    | Command::Spawn { .. }
    | Command::Shell { .. }
    | Command::ShellTask { .. }
    | Command::RemoveLog(_)
    | Command::ZoxideAdd(_)
    | Command::Clipboard(_)
    | Command::CancelPreview => Vec::new(),
  }
}
