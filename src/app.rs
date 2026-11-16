//! Application state and its update logic. Input arrives as [`Event`]s, is turned into
//! [`Action`]s through the keymap, and updates state. Side effects (I/O) are not performed
//! here; they are queued as [`Command`]s for the runtime, whose results come back as events.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::action::Action;
use crate::config::Config;
use crate::event::Event;
use crate::fs::{DirCount, DirListing, Entry};
use crate::graphics::EncodedImage;
use crate::keymap::{Key, KeyResult, Keymap};
use crate::nav::{Cursor, ScrollPos};
use crate::preview::Preview;
use crate::vcs::{DirVcs, FileStatus, Mark, RepoStatus, VcsRequest};

const PREVIEW_CACHE_LIMIT: usize = 128;
const IMAGE_CACHE_LIMIT: usize = 8;
const INFO_TIMEOUT: Duration = Duration::from_secs(4);

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
  pub show_hidden: bool,
  pub show_help: bool,
  /// Show file metadata instead of the content preview (directories keep their listing).
  pub show_metadata: bool,
  pub should_quit: bool,
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
      config,
      keymap,
      user_host,
      uid: crate::fs::meta::current_uid(),
      home,
      cwd: start.clone(),
      show_help: false,
      show_metadata: false,
      should_quit: false,
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
    if let KeyResult::Action(action) = self.keymap.feed(key) {
      self.dispatch(action);
    }
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
    self.cwd = path.clone();
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
    let want = if is_cwd {
      self.selected().map(|e| e.name.clone())
    } else {
      None
    };
    match result {
      Ok(listing) => {
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
    let Some(slot) = self.preview.as_mut().filter(|s| s.path == path) else {
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
