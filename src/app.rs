//! Application state and its update logic. Input arrives as [`Event`]s, is turned into
//! [`Action`]s through the keymap, and updates state. Side effects (I/O) are not performed
//! here; they are queued as [`Command`]s for the runtime, whose results come back as events.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;

use crate::action::Action;
use crate::clipboard;
use crate::config::Config;
use crate::console::{self, CompletionSource, ConsoleCommand, EditOutcome, LineEditor};
use crate::event::Event;
use crate::fs::sort::sort_entries_by;
use crate::fs::{DirCount, DirListing, Entry, PasteMode, SortKey};
use crate::graphics::EncodedImage;
use crate::keymap::{Key, KeyResult, Keymap};
use crate::nav::{Cursor, ScrollPos};
use crate::preview::Preview;
use crate::shell::{MacroContext, ShellFlags, ShellMode};
use crate::vcs::{DirVcs, FileStatus, Mark, RepoStatus, VcsRequest};

const PREVIEW_CACHE_LIMIT: usize = 128;
const IMAGE_CACHE_LIMIT: usize = 8;
const INFO_TIMEOUT: Duration = Duration::from_secs(4);

/// `1 item`, `3 items`.
fn items(n: usize) -> String {
  if n == 1 {
    "1 item".to_string()
  } else {
    format!("{n} items")
  }
}

/// Side effects requested by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
  LoadDir(PathBuf),
  Preview {
    id: u64,
    entry: Entry,
    /// Size of the preview column (columns, rows), for encoding images.
    cells: Option<(u16, u16)>,
  },
  /// The pending preview is no longer wanted.
  CancelPreview,
  CountDirs(Vec<PathBuf>),
  VcsScan(VcsRequest),
  Open(PathBuf),
  /// Run `argv` with `path` appended. A foreground program gets the terminal while tansu
  /// waits; a detached one runs in the background.
  Spawn {
    argv: Vec<String>,
    path: PathBuf,
    foreground: bool,
  },
  Rename {
    from: PathBuf,
    to: PathBuf,
  },
  /// Copy or move `sources` into the directory `dest`.
  Paste {
    sources: Vec<PathBuf>,
    dest: PathBuf,
    mode: PasteMode,
    overwrite: bool,
  },
  /// Put this on the system clipboard.
  Clipboard(clipboard::Content),
  /// Run `script` with the user's shell in `cwd`. `program` names it in messages.
  Shell {
    script: OsString,
    cwd: PathBuf,
    mode: ShellMode,
    program: String,
  },
  /// Record a visit to this directory in the zoxide database.
  ZoxideAdd(PathBuf),
  /// Look up the best zoxide match for `keywords`, other than `exclude`.
  Zoxide {
    keywords: Vec<String>,
    exclude: PathBuf,
  },
}

#[derive(Debug, Clone)]
pub enum DirState {
  Loading,
  Loaded(Arc<DirListing>),
  Failed(String),
}

#[derive(Debug, Clone)]
pub enum PreviewStatus {
  Loading,
  Ready(Arc<Preview>),
  Failed(String),
}

/// The preview for the currently selected entry.
#[derive(Debug, Clone)]
pub struct PreviewSlot {
  pub path: PathBuf,
  stamp: (i64, u64),
  /// Preview column size the content was requested for.
  cells: Option<(u16, u16)>,
  pub status: PreviewStatus,
  /// The encoded image of an image preview; `None` when it could not be encoded.
  pub image: Option<Arc<EncodedImage>>,
}

impl PreviewSlot {
  /// Whether the content depends on the preview size and was made for a different one.
  fn stale_size(&self, cells: Option<(u16, u16)>) -> bool {
    let sized = match &self.status {
      PreviewStatus::Loading => true,
      PreviewStatus::Ready(p) => matches!(**p, Preview::Image(_)),
      PreviewStatus::Failed(_) => false,
    };
    sized && self.cells != cells
  }
}

struct CachedPreview {
  stamp: (i64, u64),
  cells: Option<(u16, u16)>,
  content: Arc<Preview>,
  image: Option<Arc<EncodedImage>>,
}

/// Entries copied (`yy`) or cut (`dd`), waiting to be pasted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBuffer {
  pub paths: Vec<PathBuf>,
  pub mode: PasteMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLevel {
  Info,
  Error,
}

#[derive(Debug, Clone)]
pub struct Message {
  pub text: String,
  pub level: MessageLevel,
  at: Instant,
}

pub struct App {
  pub config: Config,
  pub keymap: Keymap,
  /// `user@host` for the header.
  pub user_host: String,
  /// Uid of the user running tansu, for ownership-dependent styling.
  pub uid: u32,
  pub home: Option<PathBuf>,
  pub cwd: PathBuf,
  /// Components of `cwd` that are symlinks (as paths, `cwd` included), for the header.
  pub cwd_symlinks: HashSet<PathBuf>,
  pub show_hidden: bool,
  /// Order of entries in every listing; directories always come first.
  pub sort: SortKey,
  pub show_help: bool,
  /// The `:` command line, while it is open.
  pub console: Option<LineEditor>,
  /// Tab completion candidates for the command line and the one currently shown.
  completion: Option<(Vec<String>, usize)>,
  /// Programs on `PATH`, scanned the first time `open_with` is completed.
  programs: OnceCell<Vec<String>>,
  /// Show file metadata instead of the content preview (directories keep their listing).
  pub show_metadata: bool,
  pub should_quit: bool,
  /// Copy/cut buffer, kept across directory changes.
  pub buffer: Option<FileBuffer>,
  pub message: Option<Message>,
  pub preview: Option<PreviewSlot>,
  dirs: HashMap<PathBuf, DirState>,
  counts: HashMap<PathBuf, DirCount>,
  cursor: Cursor,
  /// Indices into the current listing that pass the hidden filter.
  visible: Vec<usize>,
  visible_file_bytes: u64,
  /// Selected entry name per directory, restored when coming back.
  remembered: HashMap<PathBuf, String>,
  rows: usize,
  width: u16,
  pending_dirs: HashSet<PathBuf>,
  preview_cache: HashMap<PathBuf, CachedPreview>,
  next_preview_id: u64,
  pending_preview: Option<u64>,
  /// VCS context per listed directory, and repository status per root.
  vcs_dirs: HashMap<PathBuf, DirVcs>,
  repos: HashMap<PathBuf, Arc<RepoStatus>>,
  /// Directories whose next VCS scan must bypass the cache (after reload).
  vcs_force: HashSet<PathBuf>,
  /// Directory left by the last directory change, for `:cd -`.
  previous_dir: Option<PathBuf>,
  /// Entry to select once the given directory is listed again (after a rename).
  select_after_load: Option<(PathBuf, String)>,
  commands: Vec<Command>,
}

impl App {
  /// Creates the app showing `start`, with `select` (a file name in it) under the cursor.
  pub fn new(
    config: Config,
    keymap: Keymap,
    start: PathBuf,
    select: Option<String>,
    user_host: String,
    home: Option<PathBuf>,
  ) -> Self {
    let mut app = App {
      show_hidden: config.show_hidden,
      sort: config.sort,
      config,
      keymap,
      user_host,
      uid: crate::fs::meta::current_uid(),
      home,
      cwd: start.clone(),
      cwd_symlinks: HashSet::new(),
      show_help: false,
      console: None,
      completion: None,
      programs: OnceCell::new(),
      show_metadata: false,
      should_quit: false,
      buffer: None,
      message: None,
      preview: None,
      dirs: HashMap::new(),
      counts: HashMap::new(),
      cursor: Cursor::default(),
      visible: Vec::new(),
      visible_file_bytes: 0,
      remembered: HashMap::new(),
      rows: 0,
      width: 0,
      pending_dirs: HashSet::new(),
      preview_cache: HashMap::new(),
      next_preview_id: 0,
      pending_preview: None,
      vcs_dirs: HashMap::new(),
      repos: HashMap::new(),
      vcs_force: HashSet::new(),
      previous_dir: None,
      select_after_load: None,
      commands: Vec::new(),
    };
    app.enter_dir(start, select);
    app
  }

  /// Drains the side effects requested since the last call.
  pub fn take_commands(&mut self) -> Vec<Command> {
    std::mem::take(&mut self.commands)
  }

  // ---- read access for rendering ----

  pub fn cursor(&self) -> Cursor {
    self.cursor
  }

  pub fn rows(&self) -> usize {
    self.rows
  }

  pub fn dir_state(&self, path: &Path) -> Option<&DirState> {
    self.dirs.get(path)
  }

  pub fn listing(&self, path: &Path) -> Option<&Arc<DirListing>> {
    match self.dirs.get(path) {
      Some(DirState::Loaded(l)) => Some(l),
      _ => None,
    }
  }

  pub fn visible_len(&self) -> usize {
    self.visible.len()
  }

  pub fn visible_entry(&self, i: usize) -> Option<&Entry> {
    let idx = *self.visible.get(i)?;
    self.listing(&self.cwd)?.entries.get(idx)
  }

  pub fn selected(&self) -> Option<&Entry> {
    self.visible_entry(self.cursor.index)
  }

  /// Number of entries in a directory, hidden ones included (as ranger counts).
  pub fn dir_count(&self, path: &Path) -> Option<usize> {
    self.counts.get(path).map(|c| c.all)
  }

  /// Repository context for the status bar, as ranger shows it: when the selection is a
  /// repository root, or a file in a tracked current directory. Returns the repository and
  /// the selection's own status.
  pub fn vcs_selection(&self) -> Option<(&RepoStatus, FileStatus)> {
    if !self.config.vcs_aware {
      return None;
    }
    let entry = self.selected()?;
    if entry.is_dir() {
      let repo = self.repos.get(&entry.path)?;
      return Some((repo, repo.summary));
    }
    let root = self.vcs_dirs.get(&self.cwd)?.root.as_ref()?;
    let repo = self.repos.get(root)?;
    Some((repo, repo.status_of(&entry.path, false)))
  }

  /// VCS marks for `entry`, listed in `dir`: a remote mark for repository roots (or a
  /// blank when siblings are roots), then the file status. Empty outside version control.
  pub fn vcs_marks(&self, dir: &Path, entry: &Entry) -> Vec<Mark> {
    if !self.config.vcs_aware {
      return Vec::new();
    }
    let Some(info) = self.vcs_dirs.get(dir) else {
      return Vec::new();
    };
    // Like ranger, nothing is tracked inside the repository's own `.git` directory.
    if let Some(root) = &info.root
      && dir.starts_with(root.join(".git"))
    {
      return Vec::new();
    }
    let tracked = info.root.as_ref().and_then(|r| self.repos.get(r));
    let own = if entry.is_dir() {
      self.repos.get(&entry.path)
    } else {
      None
    };
    let status = match (own, tracked) {
      (Some(repo), _) => repo.summary,
      (None, Some(repo)) => repo.status_of(&entry.path, entry.is_dir()),
      (None, None) if info.has_repo_child => return vec![Mark::Blank, Mark::Blank],
      (None, None) => return Vec::new(),
    };
    let mut marks = Vec::with_capacity(2);
    match own {
      Some(repo) => marks.push(Mark::Remote(repo.remote)),
      None if info.has_repo_child => marks.push(Mark::Blank),
      None => {}
    }
    marks.push(Mark::File(status));
    marks
  }

  /// Name of the entry last selected in `dir`, if any.
  pub fn remembered(&self, dir: &Path) -> Option<&str> {
    self.remembered.get(dir).map(String::as_str)
  }

  pub fn visible_file_bytes(&self) -> u64 {
    self.visible_file_bytes
  }

  pub fn free_space(&self) -> Option<u64> {
    self.listing(&self.cwd).and_then(|l| l.free)
  }

  pub fn scroll_pos(&self) -> ScrollPos {
    ScrollPos::compute(self.cursor.offset, self.visible.len(), self.rows)
  }

  pub fn is_visible(&self, entry: &Entry) -> bool {
    self.show_hidden || !entry.is_hidden()
  }

  // ---- updates ----

  pub fn handle_event(&mut self, event: Event) {
    match event {
      Event::Key(k) => {
        let Some(key) = Key::from_event(&k) else {
          return;
        };
        self.handle_key(key);
      }
      Event::Resize(w, h) => self.set_size(w, h),
      Event::Tick => self.tick(),
      Event::DirLoaded { path, result } => self.on_dir_loaded(path, result),
      Event::PreviewReady {
        id,
        path,
        result,
        image,
      } => self.on_preview(id, path, result, image),
      Event::Counts(counts) => self.counts.extend(counts),
      Event::Renamed { from, to, result } => self.on_renamed(from, to, result),
      Event::Pasted {
        dest,
        mode,
        results,
      } => self.on_pasted(dest, mode, results),
      Event::Clipboard(result) => {
        if let Err(e) = result {
          self.set_error(e);
        }
      }
      Event::ProgramExited { program, result } => self.on_foreground_exit(&program, result),
      Event::DetachedExited => {
        let cwd = self.cwd.clone();
        self.request_dir(&cwd, true);
      }
      Event::Zoxide(result) => match result {
        Ok(path) => self.go_to(path, console::Z),
        Err(e) => self.set_error(e),
      },
      Event::Vcs { dir, info, repos } => {
        self.vcs_dirs.insert(dir, info);
        for repo in repos {
          self.repos.insert(repo.root.clone(), repo);
        }
      }
    }
  }

  pub fn handle_key(&mut self, key: Key) {
    if self.show_help {
      // Any key closes the help overlay.
      self.show_help = false;
      self.keymap.reset();
      return;
    }
    if self.console.is_some() {
      let forward = match key.code {
        KeyCode::Tab if key.mods.is_empty() => Some(true),
        KeyCode::BackTab => Some(false),
        _ => None,
      };
      if let Some(forward) = forward {
        self.complete_command_line(forward);
        return;
      }
    }
    if let Some(editor) = &mut self.console {
      match editor.handle_key(key) {
        EditOutcome::Continue => {}
        EditOutcome::Cancel => self.console = None,
        EditOutcome::Submit(line) => {
          self.console = None;
          self.run_command_line(&line);
        }
      }
      return;
    }
    if let KeyResult::Action(action) = self.keymap.feed(key) {
      self.dispatch(action);
    }
  }

  /// Parses and runs a line typed into the command line.
  pub fn run_command_line(&mut self, line: &str) {
    match console::parse(line) {
      None => {}
      Some(Err(e)) => self.set_error(e.to_string()),
      Some(Ok(ConsoleCommand::Action(action))) => self.dispatch(action),
      Some(Ok(ConsoleCommand::OpenWith { argv, detach })) => self.open_with(argv, detach),
      Some(Ok(ConsoleCommand::Rename(name))) => self.rename_selected(&name),
      Some(Ok(ConsoleCommand::Cd(path))) => self.cd(&path, console::CD),
      Some(Ok(ConsoleCommand::Zoxide(words))) => self.zoxide(words),
      Some(Ok(ConsoleCommand::Shell { flags, command })) => self.shell(&flags, &command),
    }
  }

  /// `:shell [-flags] command`: expands the macros and runs the command in the current
  /// directory, the way the flags ask for.
  fn shell(&mut self, flags: &str, command: &str) {
    let flags = ShellFlags::parse(flags);
    let selected = self.selected().map(|e| e.path.clone());
    let copied = self
      .buffer
      .as_ref()
      .map_or_else(Vec::new, |b| b.paths.clone());
    let ctx = MacroContext {
      dir: &self.cwd,
      selected: selected.as_deref(),
      copied: copied.as_slice(),
    };
    let script = match crate::shell::expand_macros(command, &ctx) {
      Ok(script) => flags.apply_sudo(script),
      Err(e) => {
        self.set_error(format!("{}: {e}", console::SHELL));
        return;
      }
    };
    let program = command
      .split_whitespace()
      .next()
      .unwrap_or_default()
      .to_string();
    if matches!(flags.mode, ShellMode::Detached | ShellMode::Terminal) {
      self.set_info(format!("running {program}"));
    }
    self.commands.push(Command::Shell {
      script,
      cwd: self.cwd.clone(),
      mode: flags.mode,
      program,
    });
  }

  /// Opens the command line as `shell `, ready for a command.
  fn prompt_shell(&mut self) {
    self.keymap.reset();
    self.console = Some(LineEditor::with_input(format!("{} ", console::SHELL)));
  }

  /// `:cd`: no argument goes home, `-` to the previous directory, anything else is a path.
  fn cd(&mut self, arg: &str, command: &str) {
    let target = match arg {
      "" => self.home.clone(),
      "-" => self.previous_dir.clone(),
      _ => Some(crate::fs::path::resolve(
        arg,
        &self.cwd,
        self.home.as_deref(),
      )),
    };
    match target {
      Some(path) => self.go_to(path, command),
      None if arg == "-" => self.set_error(format!("{command}: no previous directory")),
      None => self.set_error(format!("{command}: home directory unknown")),
    }
  }

  /// `:z`, behaving like the shell function: no argument, `-` or an existing directory
  /// path work as in `:cd`; anything else is a zoxide query.
  fn zoxide(&mut self, words: Vec<String>) {
    if let [word] = words.as_slice() {
      let path = crate::fs::path::resolve(word, &self.cwd, self.home.as_deref());
      if word == "-" || path.is_dir() {
        let word = word.clone();
        self.cd(&word, console::Z);
        return;
      }
    }
    if words.is_empty() {
      self.cd("", console::Z);
      return;
    }
    self.commands.push(Command::Zoxide {
      keywords: words,
      exclude: self.cwd.clone(),
    });
  }

  /// Opens `path`: a directory is entered, a file is selected in its directory.
  fn go_to(&mut self, path: PathBuf, command: &str) {
    match std::fs::metadata(&path) {
      Ok(m) if m.is_dir() => self.enter_dir(path, None),
      Ok(_) => {
        let name = path
          .file_name()
          .map(|n| crate::fs::entry::sanitize(&n.to_string_lossy()));
        if let Some(parent) = path.parent() {
          self.enter_dir(parent.to_path_buf(), name);
        }
      }
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
        self.set_error(format!("{command}: {}: no such directory", path.display()));
      }
      Err(e) => self.set_error(format!("{command}: {}: {e}", path.display())),
    }
  }

  /// Tab (`forward`) or Shift-Tab in the command line. When several candidates match,
  /// pressing it again cycles through them, like ranger.
  fn complete_command_line(&mut self, forward: bool) {
    let Some(input) = self.console.as_ref().map(|e| e.input().to_string()) else {
      return;
    };
    let cycling = self
      .completion
      .as_ref()
      .is_some_and(|(lines, i)| lines.len() > 1 && lines[*i] == input);
    if !cycling {
      let programs = || {
        self
          .programs
          .get_or_init(crate::opener::path_programs)
          .as_slice()
      };
      let subdirs = |typed: &str| {
        let dir = crate::fs::path::resolve(typed, &self.cwd, self.home.as_deref());
        crate::fs::path::subdirectories(&dir)
      };
      let files = || {
        (0..self.visible.len())
          .filter_map(|i| self.visible_entry(i))
          .map(|e| e.name.clone())
          .collect()
      };
      let src = CompletionSource {
        selected: self.selected().map(|e| e.name.as_str()),
        programs: &programs,
        subdirs: &subdirs,
        files: &files,
      };
      let lines = console::completions(&input, &src);
      if lines.is_empty() {
        self.completion = None;
        return;
      }
      // Shift-Tab starts from the last candidate, so the first step lands on it.
      let start = if forward { lines.len() - 1 } else { 0 };
      self.completion = Some((lines, start));
    }
    let Some((lines, i)) = &mut self.completion else {
      return;
    };
    let len = lines.len();
    *i = if forward {
      (*i + 1) % len
    } else {
      (*i + len - 1) % len
    };
    let line = lines[*i].clone();
    if let Some(editor) = &mut self.console {
      editor.set_input(line);
    }
  }

  /// Opens the command line as `rename <selected name>`, ready for editing.
  fn prompt_rename(&mut self) {
    let Some(name) = self.selected().map(|e| e.name.clone()) else {
      self.set_error("nothing selected");
      return;
    };
    self.keymap.reset();
    self.console = Some(LineEditor::with_input(format!(
      "{} {name}",
      console::RENAME
    )));
  }

  /// Renames the selected entry within the current directory.
  fn rename_selected(&mut self, new_name: &str) {
    let Some(entry) = self.selected().cloned() else {
      self.set_error("nothing selected");
      return;
    };
    if new_name == "." || new_name == ".." || new_name.contains(['/', '\0']) {
      self.set_error(format!("rename: invalid name `{new_name}`"));
      return;
    }
    if new_name == entry.name {
      return;
    }
    let Some(dir) = entry.path.parent() else {
      return;
    };
    let to = dir.join(new_name);
    self.commands.push(Command::Rename {
      from: entry.path,
      to,
    });
  }

  fn on_renamed(&mut self, from: PathBuf, to: PathBuf, result: Result<(), String>) {
    if let Err(e) = result {
      self.set_error(e);
      return;
    }
    let (Some(dir), Some(name)) = (to.parent(), to.file_name()) else {
      return;
    };
    let dir = dir.to_path_buf();
    let name = crate::fs::entry::sanitize(&name.to_string_lossy());
    // Listings under a renamed directory are stale.
    self.dirs.retain(|p, _| !p.starts_with(&from));
    self.counts.retain(|p, _| !p.starts_with(&from));
    self.preview_cache.remove(&from);
    self.set_info(format!("renamed to {name}"));
    self.remembered.insert(dir.clone(), name.clone());
    self.select_after_load = Some((dir.clone(), name));
    self.request_dir(&dir, true);
  }

  /// Runs `argv` on the selected entry. Directories are passed as they are, not entered.
  fn open_with(&mut self, argv: Vec<String>, detach: bool) {
    let Some(entry) = self.selected().cloned() else {
      self.set_error("nothing selected");
      return;
    };
    if detach {
      self.set_info(format!("opening {} with {}", entry.name, argv[0]));
    }
    self.commands.push(Command::Spawn {
      argv,
      path: entry.path,
      foreground: !detach,
    });
  }

  /// `yy` / `dd`: puts the selected entry into the buffer, replacing what was there.
  fn fill_buffer(&mut self, mode: PasteMode) {
    let Some(entry) = self.selected().cloned() else {
      self.set_error("nothing selected");
      return;
    };
    self.buffer = Some(FileBuffer {
      paths: vec![entry.path],
      mode,
    });
    let verb = match mode {
      PasteMode::Copy => "copied",
      PasteMode::Move => "cut",
    };
    self.set_info(format!("{verb} {}", entry.name));
  }

  /// `pp` / `po`: pastes the buffer into the current directory.
  fn paste(&mut self, overwrite: bool) {
    let Some(buffer) = self.buffer.clone() else {
      self.set_error("nothing to paste");
      return;
    };
    let verb = match buffer.mode {
      PasteMode::Copy => "copying",
      PasteMode::Move => "moving",
    };
    self.set_info(format!("{verb} {}", items(buffer.paths.len())));
    self.commands.push(Command::Paste {
      sources: buffer.paths,
      dest: self.cwd.clone(),
      mode: buffer.mode,
      overwrite,
    });
  }

  fn on_pasted(
    &mut self,
    dest: PathBuf,
    mode: PasteMode,
    results: Vec<(PathBuf, Result<PathBuf, String>)>,
  ) {
    let mut refresh = vec![dest.clone()];
    let mut pasted = Vec::new();
    let mut errors = Vec::new();
    for (src, result) in results {
      let target = match result {
        Ok(target) => target,
        Err(e) => {
          errors.push(e);
          continue;
        }
      };
      // Listings under a moved source, or a directory merged into, are stale.
      let stale =
        |p: &PathBuf| p.starts_with(&target) || (mode == PasteMode::Move && p.starts_with(&src));
      self.dirs.retain(|p, _| !stale(p));
      self.counts.retain(|p, _| !stale(p));
      self.preview_cache.retain(|p, _| !stale(p));
      if mode == PasteMode::Move {
        if let Some(parent) = src.parent() {
          refresh.push(parent.to_path_buf());
        }
        // A moved entry can only be pasted once.
        if let Some(buffer) = &mut self.buffer
          && buffer.mode == PasteMode::Move
        {
          buffer.paths.retain(|p| *p != src);
        }
      }
      pasted.push(target);
    }
    if self.buffer.as_ref().is_some_and(|b| b.paths.is_empty()) {
      self.buffer = None;
    }

    if let Some(first) = errors.first() {
      let more = errors.len() - 1;
      let suffix = if more > 0 {
        format!(" (+{more} more)")
      } else {
        String::new()
      };
      self.set_error(format!("{first}{suffix}"));
    } else {
      let verb = match mode {
        PasteMode::Copy => "copied",
        PasteMode::Move => "moved",
      };
      let what = match pasted.as_slice() {
        [one] => one
          .file_name()
          .map(|n| crate::fs::entry::sanitize(&n.to_string_lossy()))
          .unwrap_or_default(),
        many => items(many.len()),
      };
      self.set_info(format!("{verb} {what}"));
    }
    if let Some(name) = pasted.first().and_then(|t| t.file_name()) {
      let name = crate::fs::entry::sanitize(&name.to_string_lossy());
      self.remembered.insert(dest.clone(), name.clone());
      self.select_after_load = Some((dest.clone(), name));
    }
    refresh.dedup();
    for dir in refresh {
      if dir == self.cwd || self.dirs.contains_key(&dir) {
        self.vcs_force.insert(dir.clone());
        self.request_dir(&dir, true);
      }
    }
  }

  /// `yp`, `yd`, `yn`, `y.` and `yf`: puts a path, a name or the selected file itself on the
  /// system clipboard.
  fn yank(&mut self, action: Action) {
    let path = if action == Action::YankDir {
      self.cwd.clone()
    } else {
      match self.selected() {
        Some(entry) => entry.path.clone(),
        None => {
          self.set_error("nothing selected");
          return;
        }
      }
    };
    let name = || path.file_name().unwrap_or(path.as_os_str());
    let content = match action {
      Action::YankFile => clipboard::Content::Files(vec![path.clone()]),
      Action::YankName => clipboard::Content::Text(name().to_string_lossy().into_owned()),
      Action::YankNameWithoutExt => {
        let stem = path.file_stem().unwrap_or_else(name);
        clipboard::Content::Text(stem.to_string_lossy().into_owned())
      }
      _ => clipboard::Content::Text(path.to_string_lossy().into_owned()),
    };
    let shown = match &content {
      clipboard::Content::Text(text) => crate::fs::entry::sanitize(text),
      clipboard::Content::Files(_) => {
        format!(
          "file {}",
          crate::fs::entry::sanitize(&name().to_string_lossy())
        )
      }
    };
    self.set_info(format!("yanked {shown}"));
    self.commands.push(Command::Clipboard(content));
  }

  /// Called after a foreground program returned the terminal, or a silent shell command
  /// finished. The program may have changed files, so the current directory is listed again.
  pub fn on_foreground_exit(&mut self, program: &str, result: Result<ExitStatus, String>) {
    match result {
      Ok(status) if status.success() => {}
      Ok(status) => match status.code() {
        Some(code) => self.set_error(format!("`{program}` exited with status {code}")),
        None => self.set_error(format!("`{program}` was terminated ({status})")),
      },
      Err(e) => self.set_error(e),
    }
    let cwd = self.cwd.clone();
    self.request_dir(&cwd, true);
  }

  pub fn set_size(&mut self, width: u16, height: u16) {
    self.width = width;
    self.rows = crate::ui::layout::body_rows(height);
    self
      .cursor
      .fix_offset(self.visible.len(), self.rows, self.config.scroll_offset);
    // Image previews are encoded for the column size, so a new size needs a new encoding.
    if self
      .preview
      .as_ref()
      .is_some_and(|slot| slot.stale_size(self.preview_cells()))
    {
      self.refresh_preview();
    }
  }

  /// Size of the preview column (columns, rows), or `None` when it is hidden or empty.
  pub fn preview_cells(&self) -> Option<(u16, u16)> {
    let inner = ratatui::layout::Rect::new(0, 0, self.width.saturating_sub(2), self.rows as u16);
    let (cols, _) = crate::ui::layout::split_columns(inner, self.config.column_ratios);
    cols[2]
      .filter(|r| r.width > 0 && r.height > 0)
      .map(|r| (r.width, r.height))
  }

  pub fn tick(&mut self) {
    if let Some(m) = &self.message
      && m.level == MessageLevel::Info
      && m.at.elapsed() >= INFO_TIMEOUT
    {
      self.message = None;
    }
  }

  pub fn set_error(&mut self, text: impl Into<String>) {
    self.set_message(text.into(), MessageLevel::Error);
  }

  pub fn set_info(&mut self, text: impl Into<String>) {
    self.set_message(text.into(), MessageLevel::Info);
  }

  fn set_message(&mut self, text: String, level: MessageLevel) {
    self.message = Some(Message {
      text,
      level,
      at: Instant::now(),
    });
  }

  pub fn dispatch(&mut self, action: Action) {
    self.message = None;
    let page = self.rows.max(1) as isize;
    match action {
      Action::MoveUp => self.move_cursor(-1),
      Action::MoveDown => self.move_cursor(1),
      Action::HalfPageUp => self.move_cursor(-(page / 2).max(1)),
      Action::HalfPageDown => self.move_cursor((page / 2).max(1)),
      Action::PageUp => self.move_cursor(-page),
      Action::PageDown => self.move_cursor(page),
      Action::MoveTop => self.set_cursor(0),
      Action::MoveBottom => self.set_cursor(self.visible.len().saturating_sub(1)),
      Action::GoParent => self.go_parent(),
      Action::Open => self.open_selected(),
      Action::GoHome => match self.home.clone() {
        Some(home) => self.enter_dir(home, None),
        None => self.set_error("home directory unknown"),
      },
      Action::ToggleHidden => {
        self.show_hidden = !self.show_hidden;
        let want = self.selected().map(|e| e.name.clone());
        self.rebuild_visible(want);
        self.refresh_preview();
        let state = if self.show_hidden { "shown" } else { "hidden" };
        self.set_info(format!("hidden files {state}"));
      }
      Action::Sort(key) => self.set_sort(key),
      Action::Rename => self.prompt_rename(),
      Action::Copy => self.fill_buffer(PasteMode::Copy),
      Action::Cut => self.fill_buffer(PasteMode::Move),
      Action::Paste => self.paste(false),
      Action::PasteOverwrite => self.paste(true),
      Action::ClearBuffer => {
        self.buffer = None;
        self.set_info("buffer cleared");
      }
      Action::YankPath
      | Action::YankDir
      | Action::YankName
      | Action::YankNameWithoutExt
      | Action::YankFile => self.yank(action),
      Action::Shell => self.prompt_shell(),
      Action::Reload => self.reload(),
      Action::ToggleMetadata => {
        self.show_metadata = !self.show_metadata;
        let mode = if self.show_metadata {
          "metadata"
        } else {
          "content"
        };
        self.set_info(format!("preview shows {mode}"));
      }
      Action::ToggleHelp => self.show_help = !self.show_help,
      Action::CommandLine => {
        self.keymap.reset();
        self.console = Some(LineEditor::new());
      }
      Action::Quit => self.should_quit = true,
    }
  }

  fn move_cursor(&mut self, delta: isize) {
    let before = self.cursor.index;
    let margin = self.config.scroll_offset;
    self
      .cursor
      .move_by(delta, self.visible.len(), self.rows, margin);
    if self.cursor.index != before {
      self.refresh_preview();
    }
  }

  fn set_cursor(&mut self, index: usize) {
    let before = self.cursor.index;
    self.cursor.set(
      index,
      self.visible.len(),
      self.rows,
      self.config.scroll_offset,
    );
    if self.cursor.index != before {
      self.refresh_preview();
    }
  }

  fn go_parent(&mut self) {
    let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else {
      return;
    };
    let name = self
      .cwd
      .file_name()
      .map(|n| crate::fs::entry::sanitize(&n.to_string_lossy()));
    self.enter_dir(parent, name);
  }

  fn open_selected(&mut self) {
    let Some(entry) = self.selected().cloned() else {
      return;
    };
    if entry.is_dir() {
      self.enter_dir(entry.path, None);
    } else {
      self.set_info(format!("opening {}", entry.name));
      self.commands.push(Command::Open(entry.path));
    }
  }

  /// Makes `path` the current directory. Cached listings show immediately; a fresh listing
  /// is always requested so the view does not go stale.
  pub fn enter_dir(&mut self, path: PathBuf, select: Option<String>) {
    if let Some(e) = self.selected() {
      let name = e.name.clone();
      self.remembered.insert(self.cwd.clone(), name);
    }
    if let Some(name) = select {
      self.remembered.insert(path.clone(), name);
    }
    if path != self.cwd {
      self.previous_dir = Some(self.cwd.clone());
      if self.config.zoxide_add {
        self.commands.push(Command::ZoxideAdd(path.clone()));
      }
    }
    self.cwd = path.clone();
    self.cwd_symlinks = crate::fs::path::symlinked_ancestors(&path);
    self.cursor = Cursor::default();
    self.visible.clear();
    self.rebuild_visible(None);
    self.request_dir(&path, true);
    if let Some(parent) = path.parent() {
      self.request_dir(parent, false);
    }
    if let Some(DirState::Failed(msg)) = self.dirs.get(&path) {
      let msg = msg.clone();
      self.set_error(msg);
    }
    self.refresh_preview();
  }

  fn request_dir(&mut self, path: &Path, force: bool) {
    if self.pending_dirs.contains(path) {
      return;
    }
    match self.dirs.get(path) {
      Some(_) if !force => return,
      Some(_) => {}
      None => {
        self.dirs.insert(path.to_path_buf(), DirState::Loading);
      }
    }
    self.pending_dirs.insert(path.to_path_buf());
    self.commands.push(Command::LoadDir(path.to_path_buf()));
  }

  fn reload(&mut self) {
    let parent = self.cwd.parent().map(Path::to_path_buf);
    let cwd = self.cwd.clone();
    self
      .dirs
      .retain(|p, _| *p == cwd || Some(p.as_path()) == parent.as_deref());
    self.preview_cache.clear();
    self.preview = None;
    self.cancel_preview();
    self.vcs_force.insert(cwd.clone());
    if let Some(parent) = &parent {
      self.vcs_force.insert(parent.clone());
    }
    self.request_dir(&cwd, true);
    if let Some(parent) = parent {
      self.request_dir(&parent, true);
    }
    self.refresh_preview();
    self.set_info("reloaded");
  }

  /// Re-sorts every loaded listing by `key`, keeping the selected entry under the cursor.
  fn set_sort(&mut self, key: SortKey) {
    self.sort = key;
    let want = self.selected().map(|e| e.name.clone());
    for state in self.dirs.values_mut() {
      if let DirState::Loaded(listing) = state {
        sort_entries_by(&mut Arc::make_mut(listing).entries, key);
      }
    }
    // A directory preview not backed by `dirs` (a truncated listing) is re-sorted on its own.
    if let Some(slot) = &mut self.preview
      && let PreviewStatus::Ready(content) = &slot.status
      && let Preview::Directory(listing) = &**content
    {
      let mut listing = DirListing::clone(listing);
      sort_entries_by(&mut listing.entries, key);
      slot.status = PreviewStatus::Ready(Arc::new(Preview::Directory(Arc::new(listing))));
    }
    self.rebuild_visible(want);
    self.refresh_preview();
    self.set_info(format!("sorted by {key}"));
  }

  /// Applies the current sort order to a listing, which arrives in name order.
  fn sort_listing(&self, listing: &mut DirListing) {
    if self.sort != SortKey::Name {
      sort_entries_by(&mut listing.entries, self.sort);
    }
  }

  /// Recomputes the filtered view of the current directory, keeping `want` (or the
  /// remembered entry) under the cursor when it still exists.
  fn rebuild_visible(&mut self, want: Option<String>) {
    let want = want.or_else(|| self.remembered.get(&self.cwd).cloned());
    let Some(listing) = self.listing(&self.cwd).cloned() else {
      self.visible.clear();
      self.visible_file_bytes = 0;
      self.cursor = Cursor::default();
      return;
    };
    self.visible = listing
      .entries
      .iter()
      .enumerate()
      .filter(|(_, e)| self.is_visible(e))
      .map(|(i, _)| i)
      .collect();
    self.visible_file_bytes = self
      .visible
      .iter()
      .map(|&i| &listing.entries[i])
      .filter(|e| !e.is_dir())
      .map(|e| e.size)
      .sum();
    let index = want
      .and_then(|name| {
        self
          .visible
          .iter()
          .position(|&i| listing.entries[i].name == name)
      })
      .unwrap_or(self.cursor.index);
    self.cursor.set(
      index,
      self.visible.len(),
      self.rows,
      self.config.scroll_offset,
    );
  }

  fn on_dir_loaded(&mut self, path: PathBuf, result: Result<DirListing, String>) {
    self.pending_dirs.remove(&path);
    let is_cwd = path == self.cwd;
    let select = self
      .select_after_load
      .take_if(|(dir, _)| *dir == path)
      .map(|(_, name)| name);
    let want = if is_cwd {
      select.or_else(|| self.selected().map(|e| e.name.clone()))
    } else {
      None
    };
    match result {
      Ok(mut listing) => {
        self.sort_listing(&mut listing);
        self.counts.insert(path.clone(), listing.count());
        self.request_vcs(&listing);
        let subdirs: Vec<PathBuf> = if is_cwd {
          listing
            .entries
            .iter()
            .filter(|e| e.is_dir())
            .map(|e| e.path.clone())
            .collect()
        } else {
          Vec::new()
        };
        self
          .dirs
          .insert(path.clone(), DirState::Loaded(Arc::new(listing)));
        if is_cwd {
          self.rebuild_visible(want);
          if !subdirs.is_empty() {
            self.commands.push(Command::CountDirs(subdirs));
          }
        }
      }
      Err(msg) => {
        self
          .dirs
          .insert(path.clone(), DirState::Failed(msg.clone()));
        if is_cwd {
          self.rebuild_visible(None);
          self.set_error(msg);
        }
      }
    }
    let previewing = self.preview.as_ref().is_some_and(|p| p.path == path);
    if is_cwd || previewing {
      self.refresh_preview();
    }
  }

  /// Queues a VCS scan for a freshly loaded listing.
  fn request_vcs(&mut self, listing: &DirListing) {
    if !self.config.vcs_aware {
      return;
    }
    let subdirs = listing
      .entries
      .iter()
      .filter(|e| e.is_dir())
      .map(|e| e.path.clone())
      .collect();
    self.commands.push(Command::VcsScan(VcsRequest {
      dir: listing.path.clone(),
      subdirs,
      force: self.vcs_force.remove(&listing.path),
    }));
  }

  /// Abandons an in-flight preview job, if any, so the worker stops early.
  fn cancel_preview(&mut self) {
    if self.pending_preview.take().is_some() {
      self.commands.push(Command::CancelPreview);
    }
  }

  /// Points the preview at the selected entry, reusing cached content or requesting it.
  fn refresh_preview(&mut self) {
    let Some(entry) = self.selected().cloned() else {
      self.preview = None;
      self.cancel_preview();
      return;
    };
    let stamp = (entry.mtime, entry.size);
    let cells = self.preview_cells();
    if entry.is_dir()
      && let Some(listing) = self.listing(&entry.path).cloned()
    {
      self.preview = Some(PreviewSlot {
        path: entry.path,
        stamp,
        cells,
        status: PreviewStatus::Ready(Arc::new(Preview::Directory(listing))),
        image: None,
      });
      self.cancel_preview();
      return;
    }
    if let Some(slot) = &self.preview
      && slot.path == entry.path
      && slot.stamp == stamp
      && !slot.stale_size(cells)
    {
      return;
    }
    if let Some(cached) = self.preview_cache.get(&entry.path)
      && cached.stamp == stamp
      && (cached.cells == cells || !matches!(*cached.content, Preview::Image(_)))
    {
      self.preview = Some(PreviewSlot {
        path: entry.path,
        stamp,
        cells,
        status: PreviewStatus::Ready(Arc::clone(&cached.content)),
        image: cached.image.clone(),
      });
      self.cancel_preview();
      return;
    }
    self.next_preview_id += 1;
    let id = self.next_preview_id;
    self.pending_preview = Some(id);
    self.preview = Some(PreviewSlot {
      path: entry.path.clone(),
      stamp,
      cells,
      status: PreviewStatus::Loading,
      image: None,
    });
    self.commands.push(Command::Preview { id, entry, cells });
  }

  fn on_preview(
    &mut self,
    id: u64,
    path: PathBuf,
    result: Result<Preview, String>,
    image: Option<EncodedImage>,
  ) {
    if self.pending_preview != Some(id) {
      return; // stale result for an entry the cursor already left
    }
    self.pending_preview = None;
    if self.preview.as_ref().is_none_or(|s| s.path != path) {
      return;
    }
    let result = result.map(|mut preview| {
      if let Preview::Directory(listing) = &mut preview {
        self.sort_listing(Arc::make_mut(listing));
      }
      preview
    });
    let Some(slot) = self.preview.as_mut() else {
      return;
    };
    let mut scan = None;
    match result {
      Ok(preview) => {
        if let Preview::Directory(listing) = &preview {
          scan = Some(Arc::clone(listing));
          if !listing.truncated {
            self.counts.insert(path.clone(), listing.count());
            self
              .dirs
              .insert(path.clone(), DirState::Loaded(Arc::clone(listing)));
          }
        }
        let content = Arc::new(preview);
        let image = image.map(Arc::new);
        if !matches!(*content, Preview::Directory(_)) {
          let is_image = |c: &CachedPreview| matches!(*c.content, Preview::Image(_));
          if self.preview_cache.len() >= PREVIEW_CACHE_LIMIT {
            self.preview_cache.clear();
          }
          // Decoded images are large; keep only a few of them.
          if matches!(*content, Preview::Image(_))
            && self.preview_cache.values().filter(|c| is_image(c)).count() >= IMAGE_CACHE_LIMIT
          {
            self.preview_cache.retain(|_, c| !is_image(c));
          }
          self.preview_cache.insert(
            path,
            CachedPreview {
              stamp: slot.stamp,
              cells: slot.cells,
              content: Arc::clone(&content),
              image: image.clone(),
            },
          );
        }
        slot.status = PreviewStatus::Ready(content);
        slot.image = image;
      }
      Err(msg) => {
        slot.status = PreviewStatus::Failed(msg.clone());
        self.set_error(msg);
      }
    }
    if let Some(listing) = scan {
      self.request_vcs(&listing);
    }
  }
}
