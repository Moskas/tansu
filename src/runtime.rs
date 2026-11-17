//! Executes the side effects the app requests ([`Command`]s) and feeds results back as
//! [`Event`]s. The async [`Runtime`] is used by the binary; [`execute_sync`] by tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::Command;
use crate::clipboard::{self, Backend};
use crate::event::Event;
use crate::fs::{DirCount, Entry, PasteMode, count_entries, list_dir, ops};
use crate::graphics::ImageEncoder;
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

/// Pastes each of `sources` into `dest`, carrying on past failures.
pub fn paste(sources: &[PathBuf], dest: &Path, mode: PasteMode, overwrite: bool) -> Event {
  let results = sources
    .iter()
    .map(|src| (src.clone(), ops::paste(src, dest, mode, overwrite)))
    .collect();
  Event::Pasted {
    dest: dest.to_path_buf(),
    mode,
    results,
  }
}

/// Deletes each of `paths`, or moves it into the trash at `trash`, carrying on past failures.
pub fn delete(paths: &[PathBuf], trash: Option<&Path>) -> Event {
  let results = paths
    .iter()
    .map(|path| {
      let result = match trash {
        Some(root) => crate::fs::trash::trash(path, root).map(drop),
        None => ops::remove(path),
      };
      (path.clone(), result)
    })
    .collect();
  Event::Deleted {
    trashed: trash.is_some(),
    results,
  }
}

/// Extracts each of `archives` into `dest`, carrying on past failures. With `skip_damaged`,
/// damaged members are left out instead of failing their archive.
pub fn extract(archives: &[PathBuf], dest: &Path, skip_damaged: bool) -> Event {
  let results = archives
    .iter()
    .map(|a| (a.clone(), crate::archive::extract(a, dest, skip_damaged)))
    .collect();
  Event::Extracted {
    dest: dest.to_path_buf(),
    results,
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

/// A paste request: sources, destination directory, mode and whether to overwrite.
type PasteJob = (Vec<PathBuf>, PathBuf, PasteMode, bool);

/// Work for the file operation thread.
enum FileJob {
  Paste(PasteJob),
  /// Paths to delete, and the trash to move them into instead.
  Delete(Vec<PathBuf>, Option<PathBuf>),
  /// Archives to extract, and the directory to extract them into.
  Extract(Vec<PathBuf>, PathBuf, bool),
}

/// Starts the thread that runs pastes and deletions one after another, so they never race
/// for the same names and a large copy or delete does not hold up the UI.
fn spawn_file_worker(tx: Sender<Event>) -> Sender<FileJob> {
  let (job_tx, job_rx) = mpsc::channel::<FileJob>();
  let spawned = thread::Builder::new()
    .name("file-ops".into())
    .spawn(move || {
      while let Ok(job) = job_rx.recv() {
        let ev = match job {
          FileJob::Paste((sources, dest, mode, overwrite)) => {
            paste(&sources, &dest, mode, overwrite)
          }
          FileJob::Delete(paths, trash) => delete(&paths, trash.as_deref()),
          FileJob::Extract(archives, dest, skip) => extract(&archives, &dest, skip),
        };
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
  vcs: Sender<VcsRequest>,
  zoxide_add: Sender<PathBuf>,
  files: Sender<FileJob>,
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
    let vcs = spawn_vcs_worker(tx.clone());
    let zoxide_add = spawn_zoxide_add_worker();
    let files = spawn_file_worker(tx.clone());
    Runtime {
      tx,
      previews,
      counts,
      vcs,
      zoxide_add,
      files,
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
      Command::Preview {
        id,
        entry,
        cells,
        limits: extended,
      } => self.previews.submit((id, entry, cells, extended)),
      Command::CancelPreview => self.previews.cancel(),
      Command::CountDirs(dirs) => self.counts.submit(dirs),
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
      Command::Shell {
        script,
        cwd,
        mode,
        program,
      } => {
        let result = match mode {
          ShellMode::Silent => {
            let tx = self.tx.clone();
            let spawned = thread::Builder::new().name("shell".into()).spawn(move || {
              let result =
                crate::opener::run_shell_silent(&script, &cwd).map_err(|e| e.to_string());
              let _ = tx.send(Event::ProgramExited { program, result });
            });
            spawned
              .map(drop)
              .map_err(|e| format!("shell: could not start command: {e}"))
          }
          // Detached commands report nothing, but the listing is refreshed once they exit.
          ShellMode::Terminal => {
            let tx = self.tx.clone();
            let on_exit = move || drop(tx.send(Event::DetachedExited));
            crate::opener::spawn_shell_in_terminal(&script, &cwd, on_exit)
              .map_err(|e| e.to_string())
          }
          // Modes that need the terminal are run by the main loop; anything else is detached.
          _ => {
            let tx = self.tx.clone();
            let on_exit = move || drop(tx.send(Event::DetachedExited));
            crate::opener::spawn_shell_detached(&script, &cwd, on_exit).map_err(|e| e.to_string())
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
      Command::Paste {
        sources,
        dest,
        mode,
        overwrite,
      } => {
        let _ = self
          .files
          .send(FileJob::Paste((sources, dest, mode, overwrite)));
      }
      Command::Delete { paths, trash } => {
        let _ = self.files.send(FileJob::Delete(paths, trash));
      }
      Command::Extract {
        archives,
        dest,
        skip_damaged,
      } => {
        let _ = self
          .files
          .send(FileJob::Extract(archives, dest, skip_damaged));
      }
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
          let mut out = std::io::stdout().lock();
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

/// Runs a command to completion on the current thread. `Open`, `Spawn`, `Shell`, `ZoxideAdd`
/// and `Clipboard` are ignored, so tests never touch the user's programs, zoxide database or
/// clipboard.
pub fn execute_sync(cmd: Command, registry: &PreviewRegistry, limits: PreviewLimits) -> Vec<Event> {
  let token = CancelToken::never();
  match cmd {
    Command::LoadDir(path) => vec![load_dir(&path)],
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
    Command::Paste {
      sources,
      dest,
      mode,
      overwrite,
    } => vec![paste(&sources, &dest, mode, overwrite)],
    Command::Delete { paths, trash } => vec![delete(&paths, trash.as_deref())],
    Command::Extract {
      archives,
      dest,
      skip_damaged,
    } => vec![extract(&archives, &dest, skip_damaged)],
    Command::Create { path, dir } => vec![create(&path, dir)],
    Command::Open(_)
    | Command::Spawn { .. }
    | Command::Shell { .. }
    | Command::ZoxideAdd(_)
    | Command::Clipboard(_)
    | Command::CancelPreview => Vec::new(),
  }
}
