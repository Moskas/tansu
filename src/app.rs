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

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use crate::action::Action;
use crate::clipboard;
use crate::config::Config;
use crate::console::{self, CompletionSource, ConsoleCommand, EditOutcome, LineEditor};
use crate::event::Event;
use crate::fs::sort::sort_entries_by;
use crate::fs::{DirCount, DirListing, Entry, PasteMode, SortKey};
use crate::graphics::EncodedImage;
use crate::keymap::{Key, KeyResult, Keymap};
use crate::nav::{Cursor, ScrollPos, passive_offset};
use crate::preview::{Preview, PreviewLimits};
use crate::search::Pattern;
use crate::shell::{MacroContext, ShellFlags, ShellMode};
use crate::vcs::{DirVcs, FileStatus, Mark, RepoStatus, VcsRequest};

const PREVIEW_CACHE_LIMIT: usize = 128;
const IMAGE_CACHE_LIMIT: usize = 8;
const INFO_TIMEOUT: Duration = Duration::from_secs(4);
/// Rows the cursor moves per mouse wheel step.
const WHEEL_ROWS: isize = 3;
/// Shown by `n`, `N` and `ms` when there is no search to act on.
const NO_SEARCH: &str = "no active search; press / to search first";
/// Longest gap between two clicks on the same entry that counts as a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Most bytes a text preview reads after being extended by scrolling.
const EXTENDED_MAX_BYTES: usize = 16 * 1024 * 1024;

/// The entry drawn on `row` of a parent or preview column `height` rows tall, scrolled the
/// way the renderer scrolls it.
fn passive_hit<'a>(
  entries: &[&'a Entry],
  selected: usize,
  height: usize,
  row: usize,
) -> Option<&'a Entry> {
  let offset = passive_offset(selected, entries.len(), height);
  entries.get(offset + row).copied()
}

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
    /// Replaces the configured limits, for a text preview extended by scrolling.
    limits: Option<PreviewLimits>,
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
  /// Delete these paths for good, or move them into the trash at `trash`.
  Delete {
    paths: Vec<PathBuf>,
    trash: Option<PathBuf>,
  },
  /// Create a directory (`dir`) or an empty file, with missing parents.
  Create {
    path: PathBuf,
    dir: bool,
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
  /// First row of text or metadata shown; scrolls independently of the cursor.
  pub scroll: usize,
  /// How many times a truncated text preview was extended by scrolling to its end.
  level: u32,
}

impl PreviewSlot {
  fn new(path: PathBuf, stamp: (i64, u64), cells: Option<(u16, u16)>) -> Self {
    PreviewSlot {
      path,
      stamp,
      cells,
      status: PreviewStatus::Loading,
      image: None,
      scroll: 0,
      level: 0,
    }
  }

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
  level: u32,
}

/// A deletion waiting for the user to confirm it with `y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
  /// Move to the trash instead of deleting for good.
  pub trash: bool,
  /// What will be deleted, fixed when the deletion was asked for, so nothing that happens
  /// before the answer (a reload, a moved cursor) changes it.
  pub paths: Vec<PathBuf>,
  /// The names to list, directories ending in `/` (they go with their contents).
  pub names: Vec<String>,
}

impl Confirm {
  /// The question shown in the status bar.
  pub fn question(&self) -> String {
    let what = match self.paths.as_slice() {
      [one] => format!("`{}`", display_name(one)),
      many => items(many.len()),
    };
    if self.trash {
      format!("Move {what} to the trash? (y/N)")
    } else {
      format!("Delete {what} permanently? (y/N)")
    }
  }
}

/// The last component of `path`, made safe to draw.
pub fn display_name(path: &Path) -> String {
  path
    .file_name()
    .map(|n| crate::fs::entry::sanitize(&n.to_string_lossy()))
    .unwrap_or_default()
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
  /// A deletion waiting for confirmation; the next key answers it.
  pub confirm: Option<Confirm>,
  /// The trash `dT` moves entries into (`$XDG_DATA_HOME/Trash`). `None` refuses to trash.
  pub trash_dir: Option<PathBuf>,
  /// Tab completion candidates for the command line and the one currently shown.
  completion: Option<(Vec<String>, usize)>,
  /// Programs on `PATH`, scanned the first time `open_with` is completed.
  programs: OnceCell<Vec<String>>,
  /// Show file metadata instead of the content preview (directories keep their listing).
  pub show_metadata: bool,
  pub should_quit: bool,
  /// Copy/cut buffer, kept across directory changes.
  pub buffer: Option<FileBuffer>,
  /// Marked entries (`<Space>`, `v`), by full path. Like ranger, marks are kept per directory
  /// across directory changes; operations use the marks of the current directory only.
  marked: HashSet<PathBuf>,
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
  /// The active search as typed, and parsed. Kept across directory changes, like vim's,
  /// until cleared with `<Esc>` or by cancelling the search prompt. While it is set, entries
  /// that do not match it are drawn dim.
  search: Option<(String, Pattern)>,
  /// Directory left by the last directory change, for `:cd -`.
  previous_dir: Option<PathBuf>,
  /// Entry to select once the given directory is listed again (after a rename).
  select_after_load: Option<(PathBuf, String)>,
  /// The last left click in the current column (its row and the entry it selected), for
  /// detecting double clicks.
  last_click: Option<(Instant, usize, PathBuf)>,
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
      confirm: None,
      trash_dir: None,
      completion: None,
      programs: OnceCell::new(),
      show_metadata: false,
      should_quit: false,
      buffer: None,
      marked: HashSet::new(),
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
      search: None,
      previous_dir: None,
      select_after_load: None,
      last_click: None,
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

  pub fn is_marked(&self, path: &Path) -> bool {
    self.marked.contains(path)
  }

  /// Marked entries of the current directory that pass the hidden filter, in listing order.
  pub fn marked_entries(&self) -> Vec<&Entry> {
    (0..self.visible.len())
      .filter_map(|i| self.visible_entry(i))
      .filter(|e| self.marked.contains(&e.path))
      .collect()
  }

  /// What file operations act on: the marked entries, or the selected one when none are.
  pub fn selection(&self) -> Vec<&Entry> {
    let marked = self.marked_entries();
    if marked.is_empty() {
      self.selected().into_iter().collect()
    } else {
      marked
    }
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

  /// The entries the parent column shows (`listing` is the parent's) and the position of the
  /// current directory among them. The current directory is kept even when it is hidden.
  pub fn parent_entries<'a>(&self, listing: &'a DirListing) -> (Vec<&'a Entry>, Option<usize>) {
    let entries: Vec<&Entry> = listing
      .entries
      .iter()
      .filter(|e| self.is_visible(e) || e.path == self.cwd)
      .collect();
    let selected = entries.iter().position(|e| e.path == self.cwd);
    (entries, selected)
  }

  /// The entries a directory preview shows and the one drawn selected: the entry last
  /// selected in that directory, or the first.
  pub fn preview_entries<'a>(&self, listing: &'a DirListing) -> (Vec<&'a Entry>, usize) {
    let entries: Vec<&Entry> = listing
      .entries
      .iter()
      .filter(|e| self.is_visible(e))
      .collect();
    let selected = self
      .remembered(&listing.path)
      .and_then(|name| entries.iter().position(|e| e.name == name))
      .unwrap_or(0);
    (entries, selected)
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
      Event::Mouse(m) => self.handle_mouse(m),
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
      Event::Deleted { trashed, results } => self.on_deleted(trashed, results),
      Event::Created { path, dir, result } => self.on_created(path, dir, result),
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
    // Answered before the keymap sees the key: `y` also starts the yank bindings.
    if let Some(confirm) = self.confirm.take() {
      if key.code == KeyCode::Char('y') && key.mods.is_empty() {
        self.run_delete(confirm);
      } else {
        self.set_info("cancelled");
      }
      return;
    }
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
      let searching = editor.input().split_whitespace().next() == Some(console::SEARCH);
      match editor.handle_key(key) {
        EditOutcome::Continue => {}
        EditOutcome::Cancel => {
          self.console = None;
          // Backing out of a search ends the previous one too, so `n` does not revive it.
          if searching {
            self.clear_search();
          }
        }
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

  /// Mouse input, following ranger: the wheel moves the cursor (over the parent column it
  /// steps through sibling directories, over a text or metadata preview it scrolls that
  /// preview), a left click selects the entry under it (in the
  /// parent or preview column, by entering that column's directory), and a right click or
  /// a double click opens it.
  pub fn handle_mouse(&mut self, ev: MouseEvent) {
    if self.confirm.is_some() {
      // A click answers no; the wheel is ignored so it cannot move what is listed.
      if matches!(ev.kind, MouseEventKind::Down(_)) {
        self.confirm = None;
        self.set_info("cancelled");
      }
      return;
    }
    if self.show_help {
      // Any click closes the help overlay, like any key.
      if matches!(ev.kind, MouseEventKind::Down(_)) {
        self.show_help = false;
        self.keymap.reset();
      }
      return;
    }
    if self.console.is_some() {
      return;
    }
    let inner = crate::ui::layout::body_inner(self.width, self.rows as u16);
    let (cols, _) = crate::ui::layout::split_columns(inner, self.config.column_ratios);
    let pos = Position::new(ev.column, ev.row);
    let Some((column, area)) =
      (0..3).find_map(|i| cols[i].filter(|r| r.contains(pos)).map(|r| (i, r)))
    else {
      return;
    };
    let row = (ev.row - area.y) as usize;
    let last_click = self.last_click.take();
    self.keymap.reset();
    self.message = None;
    match ev.kind {
      MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
        let dir = if ev.kind == MouseEventKind::ScrollUp {
          -1
        } else {
          1
        };
        // A file's preview scrolls on its own (or not at all, for images), decided by the
        // selection rather than by whether the preview has loaded yet. Over a directory
        // listing the wheel moves the cursor like the other columns.
        let over_file = column == 2 && self.selected().is_some_and(|e| !e.is_dir());
        if column == 0 {
          self.step_parent(dir);
        } else if over_file {
          self.scroll_preview(dir * WHEEL_ROWS);
        } else {
          self.move_cursor(dir * WHEEL_ROWS);
        }
      }
      MouseEventKind::Down(MouseButton::Left) => self.click(column, row, false, last_click),
      MouseEventKind::Down(MouseButton::Right) => self.click(column, row, true, last_click),
      _ => {}
    }
  }

  /// A click on `row` of `column`; `open` for a right click.
  fn click(
    &mut self,
    column: usize,
    row: usize,
    open: bool,
    last_click: Option<(Instant, usize, PathBuf)>,
  ) {
    match column {
      0 => {
        let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else {
          return;
        };
        let Some(listing) = self.listing(&parent).cloned() else {
          return;
        };
        let (entries, selected) = self.parent_entries(&listing);
        if let Some(entry) = passive_hit(&entries, selected.unwrap_or(0), self.rows, row) {
          self.click_passive(&parent, entry, open);
        }
      }
      1 => {
        // The first click may have scrolled the list, so a double click is two clicks on
        // the same row, whatever entry is under it now, while the selection is unchanged.
        let double = !open
          && last_click.is_some_and(|(at, r, p)| {
            r == row && at.elapsed() <= DOUBLE_CLICK && self.selected().is_some_and(|e| e.path == p)
          });
        if double {
          return self.open_selected();
        }
        let index = self.cursor.offset + row;
        let Some(path) = self.visible_entry(index).map(|e| e.path.clone()) else {
          return;
        };
        self.set_cursor(index);
        if open {
          self.open_selected();
        } else {
          self.last_click = Some((Instant::now(), row, path));
        }
      }
      _ => {
        let Some(slot) = &self.preview else {
          return;
        };
        let content = match &slot.status {
          PreviewStatus::Ready(p) => Arc::clone(p),
          _ => return,
        };
        let dir = slot.path.clone();
        match &*content {
          Preview::Directory(listing) => {
            let (entries, selected) = self.preview_entries(listing);
            if let Some(entry) = passive_hit(&entries, selected, self.rows, row) {
              self.click_passive(&dir, entry, open);
            }
          }
          // A file's preview: a right click opens the file, as in ranger.
          _ if open => self.open_selected(),
          _ => {}
        }
      }
    }
  }

  /// A click on `entry`, listed in `dir` in the parent or preview column. A right click on a
  /// directory enters it; otherwise `dir` is entered with `entry` selected.
  fn click_passive(&mut self, dir: &Path, entry: &Entry, open: bool) {
    if open && entry.is_dir() {
      self.enter_dir(entry.path.clone(), None);
    } else {
      self.enter_dir(dir.to_path_buf(), Some(entry.name.clone()));
    }
  }

  /// Enters the next (`delta` > 0) or previous sibling directory of the current one.
  fn step_parent(&mut self, delta: isize) {
    let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else {
      return;
    };
    let Some(listing) = self.listing(&parent).cloned() else {
      return;
    };
    let (entries, Some(at)) = self.parent_entries(&listing) else {
      return;
    };
    let next = if delta > 0 {
      entries[at + 1..].iter().find(|e| e.is_dir())
    } else {
      entries[..at].iter().rev().find(|e| e.is_dir())
    };
    if let Some(entry) = next {
      self.enter_dir(entry.path.clone(), None);
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
      Some(Ok(ConsoleCommand::Search(pattern))) => self.start_search(pattern),
      Some(Ok(ConsoleCommand::Create { name, dir })) => self.create(&name, dir),
    }
  }

  /// `:shell [-flags] command`: expands the macros and runs the command in the current
  /// directory, the way the flags ask for.
  fn shell(&mut self, flags: &str, command: &str) {
    let flags = ShellFlags::parse(flags);
    let selected = self.selected().map(|e| e.path.clone());
    let selection: Vec<PathBuf> = self.selection().iter().map(|e| e.path.clone()).collect();
    let copied = self
      .buffer
      .as_ref()
      .map_or_else(Vec::new, |b| b.paths.clone());
    let ctx = MacroContext {
      dir: &self.cwd,
      selected: selected.as_deref(),
      selection: &selection,
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

  /// `:search`: remembers the pattern and jumps to the best match.
  fn start_search(&mut self, text: String) {
    let Some(pattern) = Pattern::parse(&text) else {
      return;
    };
    self.search = Some((text, pattern));
    self.jump_to_match(None);
  }

  /// Indices of the visible entries matching the search, best first. Equal scores (every
  /// wildcard match) keep listing order.
  fn search_matches(&self) -> Vec<usize> {
    let Some((_, pattern)) = &self.search else {
      return Vec::new();
    };
    let mut scored: Vec<(i64, usize)> = (0..self.visible.len())
      .filter_map(|i| Some((pattern.score(&self.visible_entry(i)?.name)?, i)))
      .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
  }

  /// Selects a match of the last search: the best one for `None`, otherwise the next
  /// (`Some(true)`) or previous one in match order from the selected entry, wrapping around.
  /// From an entry that does not match, `n` goes to the best match and `N` to the worst.
  fn jump_to_match(&mut self, forward: Option<bool>) {
    let Some((text, _)) = &self.search else {
      self.set_info(NO_SEARCH);
      return;
    };
    let text = text.clone();
    let matches = self.search_matches();
    if matches.is_empty() {
      self.set_error(format!("no match for `{text}`"));
      return;
    }
    let n = matches.len();
    let at = matches.iter().position(|&i| i == self.cursor.index);
    let k = match (forward, at) {
      (None, _) | (Some(true), None) => 0,
      (Some(true), Some(k)) => (k + 1) % n,
      (Some(false), Some(k)) => (k + n - 1) % n,
      (Some(false), None) => n - 1,
    };
    self.set_cursor(matches[k]);
    self.set_info(format!("match {}/{n} for `{text}`", k + 1));
  }

  /// `<Esc>`: ends the active search; entries are no longer dimmed and `n`/`N` have nothing
  /// to go to until the next search.
  fn clear_search(&mut self) {
    if self.search.take().is_some() {
      self.set_info("search cleared");
    }
  }

  /// Whether `entry` is drawn dim for not matching the active search.
  pub fn dimmed_by_search(&self, entry: &Entry) -> bool {
    match &self.search {
      Some((_, pattern)) => pattern.score(&entry.name).is_none(),
      _ => false,
    }
  }

  /// `ms`: marks every visible entry matching the last search, keeping existing marks.
  fn mark_matches(&mut self) {
    let Some((text, _)) = &self.search else {
      self.set_info(NO_SEARCH);
      return;
    };
    let text = text.clone();
    let paths: Vec<PathBuf> = self
      .search_matches()
      .into_iter()
      .filter_map(|i| self.visible_entry(i).map(|e| e.path.clone()))
      .collect();
    if paths.is_empty() {
      self.set_error(format!("no match for `{text}`"));
      return;
    }
    self.set_info(format!("marked {} matching `{text}`", items(paths.len())));
    self.marked.extend(paths);
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

  /// `<Space>`: toggles the mark on the selected entry and moves down, like ranger.
  fn toggle_mark(&mut self) {
    let Some(path) = self.selected().map(|e| e.path.clone()) else {
      return;
    };
    if !self.marked.remove(&path) {
      self.marked.insert(path);
    }
    self.move_cursor(1);
  }

  /// `v`: toggles the mark on every visible entry, like ranger's `mark_files all=True
  /// toggle=True`. Starting with no marks, it marks everything; pressed again, it unmarks.
  fn invert_marks(&mut self) {
    let paths: Vec<PathBuf> = (0..self.visible.len())
      .filter_map(|i| self.visible_entry(i))
      .map(|e| e.path.clone())
      .collect();
    for path in paths {
      if !self.marked.remove(&path) {
        self.marked.insert(path);
      }
    }
  }

  /// `yy` / `dd`: puts the marked entries (or the selected one) into the buffer, replacing
  /// what was there.
  fn fill_buffer(&mut self, mode: PasteMode) {
    let (paths, what) = match self.selection().as_slice() {
      [] => {
        self.set_error("nothing selected");
        return;
      }
      [one] => (vec![one.path.clone()], one.name.clone()),
      many => (
        many.iter().map(|e| e.path.clone()).collect(),
        items(many.len()),
      ),
    };
    self.buffer = Some(FileBuffer { paths, mode });
    let verb = match mode {
      PasteMode::Copy => "copied",
      PasteMode::Move => "cut",
    };
    self.set_info(format!("{verb} {what}"));
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

  /// `dD` / `dT`: asks to delete (or trash) the marked entries, or the selection. Nothing
  /// happens until the answer; every way of deleting goes through here.
  fn ask_delete(&mut self, trash: bool) {
    if trash && self.trash_dir.is_none() {
      self.set_error("trash: no trash directory (XDG_DATA_HOME or HOME is unset)");
      return;
    }
    let selection = self.selection();
    if selection.is_empty() {
      self.set_error("nothing selected");
      return;
    }
    let paths = selection.iter().map(|e| e.path.clone()).collect();
    // Symlinks to directories are removed themselves, so they are listed as plain entries.
    let names = selection
      .iter()
      .map(|e| match e.kind {
        crate::fs::EntryKind::Dir if !e.is_symlink() => format!("{}/", e.name),
        _ => e.name.clone(),
      })
      .collect();
    self.keymap.reset();
    self.confirm = Some(Confirm {
      trash,
      paths,
      names,
    });
  }

  /// Carries out a confirmed deletion.
  fn run_delete(&mut self, confirm: Confirm) {
    let trash = if confirm.trash {
      match &self.trash_dir {
        Some(dir) => Some(dir.clone()),
        None => {
          self.set_error("trash: no trash directory (XDG_DATA_HOME or HOME is unset)");
          return;
        }
      }
    } else {
      None
    };
    let verb = if confirm.trash {
      "trashing"
    } else {
      "deleting"
    };
    self.set_info(format!("{verb} {}", items(confirm.paths.len())));
    self.commands.push(Command::Delete {
      paths: confirm.paths,
      trash,
    });
  }

  fn on_deleted(&mut self, trashed: bool, results: Vec<(PathBuf, Result<(), String>)>) {
    let mut done = 0;
    let mut errors = Vec::new();
    let mut refresh = Vec::new();
    for (path, result) in results {
      if let Err(e) = result {
        errors.push(e);
        continue;
      }
      done += 1;
      let gone = |p: &PathBuf| p.starts_with(&path);
      self.dirs.retain(|p, _| !gone(p));
      self.counts.retain(|p, _| !gone(p));
      self.preview_cache.retain(|p, _| !gone(p));
      self.marked.retain(|p| !gone(p));
      if let Some(buffer) = &mut self.buffer {
        buffer.paths.retain(|p| !gone(p));
      }
      if let Some(parent) = path.parent() {
        refresh.push(parent.to_path_buf());
      }
    }
    if self.buffer.as_ref().is_some_and(|b| b.paths.is_empty()) {
      self.buffer = None;
    }
    let verb = if trashed { "trashed" } else { "deleted" };
    match errors.as_slice() {
      [] => self.set_info(format!("{verb} {}", items(done))),
      [first, rest @ ..] => {
        let more = if rest.is_empty() {
          String::new()
        } else {
          format!(" (+{} more)", rest.len())
        };
        self.set_error(format!("{verb} {}, failed: {first}{more}", items(done)));
      }
    }
    refresh.sort();
    refresh.dedup();
    for dir in refresh {
      if dir == self.cwd || self.dirs.contains_key(&dir) {
        self.vcs_force.insert(dir.clone());
        self.request_dir(&dir, true);
      }
    }
  }

  /// `:mkdir` / `:touch`: creates `name`, a path relative to the current directory where a
  /// leading `~` is the home directory, as in `:cd`.
  fn create(&mut self, name: &str, dir: bool) {
    let command = if dir { console::MKDIR } else { console::TOUCH };
    if name.split('/').all(|c| c.is_empty() || c == ".") {
      self.set_error(format!("{command}: invalid name `{name}`"));
      return;
    }
    // Never create a directory literally named `~`: removing it later is a classic way to
    // lose a home directory.
    if name.starts_with('~') && self.home.is_none() {
      self.set_error(format!("{command}: home directory unknown"));
      return;
    }
    let path = crate::fs::path::resolve(name, &self.cwd, self.home.as_deref());
    self.commands.push(Command::Create { path, dir });
  }

  fn on_created(&mut self, path: PathBuf, dir: bool, result: Result<(), String>) {
    if let Err(e) = result {
      self.set_error(e);
      return;
    }
    let kind = if dir { "directory" } else { "file" };
    self.set_info(format!("created {kind} {}", display_name(&path)));
    // Select what was created, or the directory leading to it, when it is in view.
    let cwd = self.cwd.clone();
    if let Some(first) = path
      .strip_prefix(&cwd)
      .ok()
      .and_then(|rel| rel.components().next())
    {
      let name = crate::fs::entry::sanitize(&first.as_os_str().to_string_lossy());
      self.remembered.insert(cwd.clone(), name.clone());
      self.select_after_load = Some((cwd.clone(), name));
    }
    self.vcs_force.insert(cwd.clone());
    self.request_dir(&cwd, true);
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

  /// `yp`, `yd`, `yn`, `y.` and `yf`: puts paths, names or the files themselves on the
  /// system clipboard. All but `yd` act on the marked entries (or the selected one); several
  /// paths or names are put one per line.
  fn yank(&mut self, action: Action) {
    let paths: Vec<PathBuf> = if action == Action::YankDir {
      vec![self.cwd.clone()]
    } else {
      self.selection().iter().map(|e| e.path.clone()).collect()
    };
    if paths.is_empty() {
      self.set_error("nothing selected");
      return;
    }
    let name = |p: &Path| p.file_name().unwrap_or(p.as_os_str()).to_owned();
    let text = |p: &PathBuf| {
      let s = match action {
        Action::YankName => name(p),
        Action::YankNameWithoutExt => p.file_stem().map_or_else(|| name(p), ToOwned::to_owned),
        _ => p.as_os_str().to_owned(),
      };
      s.to_string_lossy().into_owned()
    };
    let content = match action {
      Action::YankFile => clipboard::Content::Files(paths.clone()),
      _ => clipboard::Content::Text(paths.iter().map(text).collect::<Vec<_>>().join("\n")),
    };
    let shown = match (&content, paths.as_slice()) {
      (_, [_, _, ..]) => items(paths.len()),
      (clipboard::Content::Text(text), _) => crate::fs::entry::sanitize(text),
      (clipboard::Content::Files(_), _) => format!(
        "file {}",
        crate::fs::entry::sanitize(&name(&paths[0]).to_string_lossy())
      ),
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
    let inner = crate::ui::layout::body_inner(self.width, self.rows as u16);
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
      Action::PreviewUp => {
        self.scroll_preview(-1);
      }
      Action::PreviewDown => {
        self.scroll_preview(1);
      }
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
      Action::ToggleMark => self.toggle_mark(),
      Action::InvertMarks => self.invert_marks(),
      Action::UnmarkAll => {
        let cwd = self.cwd.clone();
        self.marked.retain(|p| p.parent() != Some(cwd.as_path()));
      }
      Action::Rename => self.prompt_rename(),
      Action::Copy => self.fill_buffer(PasteMode::Copy),
      Action::Cut => self.fill_buffer(PasteMode::Move),
      Action::Paste => self.paste(false),
      Action::PasteOverwrite => self.paste(true),
      Action::Delete => self.ask_delete(false),
      Action::Trash => self.ask_delete(true),
      Action::MakeDir | Action::Touch => {
        let command = if action == Action::MakeDir {
          console::MKDIR
        } else {
          console::TOUCH
        };
        self.keymap.reset();
        self.console = Some(LineEditor::with_input(format!("{command} ")));
      }
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
      Action::Search => {
        self.keymap.reset();
        self.console = Some(LineEditor::with_input(format!("{} ", console::SEARCH)));
      }
      Action::SearchNext => self.jump_to_match(Some(true)),
      Action::SearchPrev => self.jump_to_match(Some(false)),
      Action::ClearSearch => self.clear_search(),
      Action::MarkMatches => self.mark_matches(),
      Action::Reload => self.reload(),
      Action::ToggleMetadata => {
        self.show_metadata = !self.show_metadata;
        if let Some(slot) = &mut self.preview {
          slot.scroll = 0;
        }
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
        // Entries that are gone (renamed, moved, deleted) lose their marks.
        let present: HashSet<&Path> = listing.entries.iter().map(|e| e.path.as_path()).collect();
        self
          .marked
          .retain(|p| p.parent() != Some(path.as_path()) || present.contains(p.as_path()));
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
        status: PreviewStatus::Ready(Arc::new(Preview::Directory(listing))),
        ..PreviewSlot::new(entry.path, stamp, cells)
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
        status: PreviewStatus::Ready(Arc::clone(&cached.content)),
        image: cached.image.clone(),
        level: cached.level,
        ..PreviewSlot::new(entry.path, stamp, cells)
      });
      self.cancel_preview();
      return;
    }
    self.preview = Some(PreviewSlot::new(entry.path.clone(), stamp, cells));
    self.request_preview(entry, cells, None);
  }

  fn request_preview(
    &mut self,
    entry: Entry,
    cells: Option<(u16, u16)>,
    limits: Option<PreviewLimits>,
  ) {
    self.next_preview_id += 1;
    let id = self.next_preview_id;
    self.pending_preview = Some(id);
    self.commands.push(Command::Preview {
      id,
      entry,
      cells,
      limits,
    });
  }

  /// Rows of the preview that can scroll: text, highlighted text and metadata. `None` for
  /// directory listings, images and previews that are not ready.
  fn preview_len(&self) -> Option<usize> {
    let slot = self.preview.as_ref()?;
    if self.show_metadata
      && let Some(entry) = self
        .selected()
        .filter(|e| !e.is_dir() && e.path == slot.path)
    {
      return Some(crate::ui::columns::metadata_rows(entry, &slot.status).len());
    }
    match &slot.status {
      PreviewStatus::Ready(p) => match &**p {
        Preview::Text { lines, .. } => Some(lines.len()),
        Preview::Highlighted { lines, .. } => Some(lines.len()),
        Preview::Metadata(rows) => Some(rows.len()),
        Preview::Directory(_) | Preview::Image(_) => None,
      },
      _ => None,
    }
  }

  /// Scrolls the preview by `delta` rows, keeping its last row at the bottom of the column
  /// at most. Returns false when the preview does not scroll.
  fn scroll_preview(&mut self, delta: isize) -> bool {
    let Some(len) = self.preview_len() else {
      return false;
    };
    let height = self.preview_cells().map_or(0, |(_, h)| h as usize);
    let Some(slot) = self.preview.as_mut() else {
      return false;
    };
    let max = len.saturating_sub(height);
    slot.scroll = slot.scroll.saturating_add_signed(delta).min(max);
    // Nearing the end of a truncated text: ask for more of the file in the background.
    if !self.show_metadata && slot.scroll + 2 * height >= len {
      self.extend_preview();
    }
    true
  }

  /// Requests the selected text preview again with larger limits when it was truncated.
  /// The current content stays shown, at the same scroll position, until the result comes.
  fn extend_preview(&mut self) {
    if self.pending_preview.is_some() {
      return;
    }
    let Some(slot) = &self.preview else {
      return;
    };
    let truncated = match &slot.status {
      PreviewStatus::Ready(p) => match &**p {
        Preview::Text { truncated, .. } | Preview::Highlighted { truncated, .. } => *truncated,
        _ => false,
      },
      _ => false,
    };
    let Some(entry) = self.selected().filter(|e| e.path == slot.path).cloned() else {
      return;
    };
    // Each extension reads 4 times more bytes, up to a ceiling. Lines are no longer capped,
    // so a file of many short lines is not cut off before the byte limit.
    let base = self.config.preview;
    let bytes = |level: u32| {
      base
        .max_bytes
        .saturating_mul(4usize.saturating_pow(level))
        .min(EXTENDED_MAX_BYTES)
    };
    if !truncated || (slot.level > 0 && bytes(slot.level) >= EXTENDED_MAX_BYTES) {
      return;
    }
    let level = slot.level + 1;
    let limits = PreviewLimits {
      max_bytes: bytes(level),
      max_lines: usize::MAX,
      ..base
    };
    let cells = slot.cells;
    if let Some(slot) = self.preview.as_mut() {
      slot.level = level;
    }
    self.request_preview(entry, cells, Some(limits));
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
              level: slot.level,
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
