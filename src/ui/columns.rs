//! The bordered body with parent, current and preview columns.

use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Widget};

use super::text::{display_width, truncate};
use super::{entry_style, icons, layout};
use crate::app::{App, DirState, PreviewStatus};
use crate::config::Theme;
use crate::fs::meta::human_size;
use crate::fs::{Entry, EntryKind};
use crate::nav::passive_offset;
use crate::preview::{Preview, SpanStyle, TermColor, metadata_lines};
use crate::vcs::{FileStatus, Mark, RemoteStatus};

pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
  if area.width < 3 || area.height < 3 {
    return;
  }
  let theme = &app.config.theme;
  let border = Style::new().fg(theme.border.0);
  Block::bordered().border_style(border).render(area, buf);
  let inner = area.inner(Margin::new(1, 1));
  let (cols, seps) = layout::split_columns(inner, app.config.column_ratios);
  for x in seps {
    put(buf, x, area.y, "┬", border);
    put(buf, x, area.bottom() - 1, "┴", border);
    for y in inner.y..inner.bottom() {
      put(buf, x, y, "│", border);
    }
  }
  if let Some(r) = cols[0] {
    render_parent(buf, r, app);
  }
  if let Some(r) = cols[1] {
    render_current(buf, r, app);
  }
  if let Some(r) = cols[2] {
    render_preview(buf, r, app);
  }
}

fn put(buf: &mut Buffer, x: u16, y: u16, symbol: &str, style: Style) {
  if let Some(cell) = buf.cell_mut((x, y)) {
    cell.set_symbol(symbol).set_style(style);
  }
}

fn render_parent(buf: &mut Buffer, area: Rect, app: &App) {
  let Some(parent) = app.cwd.parent() else {
    return;
  };
  match app.dir_state(parent) {
    Some(DirState::Loaded(listing)) => {
      let (entries, selected) = app.parent_entries(listing);
      draw_list(buf, area, parent, &entries, selected, app);
    }
    Some(DirState::Failed(_)) => placeholder(buf, area, "not accessible", alert_style(app)),
    _ => placeholder(buf, area, "...", Style::new().fg(app.config.theme.fg.0)),
  }
}

fn render_current(buf: &mut Buffer, area: Rect, app: &App) {
  if app.visible_len() == 0 {
    match app.dir_state(&app.cwd) {
      Some(DirState::Failed(_)) => placeholder(buf, area, "not accessible", alert_style(app)),
      Some(DirState::Loaded(_)) => placeholder(buf, area, "empty", alert_style(app)),
      _ => placeholder(buf, area, "...", Style::new().fg(app.config.theme.fg.0)),
    }
    return;
  }
  let cursor = app.cursor();
  for row in 0..area.height {
    let i = cursor.offset + row as usize;
    let Some(entry) = app.visible_entry(i) else {
      break;
    };
    let info = entry_info(entry, app);
    let marks = app.vcs_marks(&app.cwd, entry);
    let rect = Rect::new(area.x, area.y + row, area.width, 1);
    let highlighted = i == cursor.index;
    draw_row(buf, rect, entry, highlighted, info.as_deref(), &marks, app);
    // Entries that miss the search fade out; the cursor row stays readable.
    if !highlighted && app.dimmed_by_search(entry) {
      buf.set_style(rect, Style::new().add_modifier(Modifier::DIM));
    }
  }
}

fn render_preview(buf: &mut Buffer, area: Rect, app: &App) {
  let Some(slot) = &app.preview else {
    return;
  };
  let theme = &app.config.theme;
  if app.show_metadata
    && let Some(entry) = app
      .selected()
      .filter(|e| !e.is_dir() && e.path == slot.path)
  {
    let rows = metadata_rows(entry, &slot.status);
    return draw_metadata(buf, area, &rows, slot.scroll, theme);
  }
  let preview = match &slot.status {
    PreviewStatus::Loading => return,
    // The full error is already in the status line.
    PreviewStatus::Failed(_) => return placeholder(buf, area, "not accessible", alert_style(app)),
    PreviewStatus::Ready(p) => p,
  };
  match &**preview {
    Preview::Text { lines, .. } => {
      let style = Style::new().fg(theme.fg.0);
      let shown = lines.iter().skip(slot.scroll).take(area.height as usize);
      for (row, line) in shown.enumerate() {
        buf.set_stringn(
          area.x,
          area.y + row as u16,
          line,
          area.width as usize,
          style,
        );
      }
    }
    Preview::Directory(listing) => {
      let (entries, selected) = app.preview_entries(listing);
      if entries.is_empty() {
        return placeholder(buf, area, "empty", alert_style(app));
      }
      draw_list(buf, area, &listing.path, &entries, Some(selected), app);
    }
    Preview::Highlighted { lines, .. } => {
      let shown = lines.iter().skip(slot.scroll).take(area.height as usize);
      for (row, line) in shown.enumerate() {
        let y = area.y + row as u16;
        let mut x = area.x;
        for span in line {
          if x >= area.right() {
            break;
          }
          let room = (area.right() - x) as usize;
          (x, _) = buf.set_stringn(x, y, &span.text, room, span_style(span.style, theme));
        }
      }
    }
    Preview::Image(image) => match &slot.image {
      Some(encoded) => encoded.render(area, buf),
      // No graphics support (or encoding failed): describe the image instead.
      None => draw_metadata(
        buf,
        area,
        &[(
          "Image".to_string(),
          format!("{}x{}", image.width, image.height),
        )],
        0,
        theme,
      ),
    },
    Preview::Metadata(rows) => draw_metadata(buf, area, rows, slot.scroll, theme),
  }
}

/// Metadata for the metadata view, refined by what the content preview found out: the file
/// type, and the dimensions of images.
pub fn metadata_rows(entry: &Entry, status: &PreviewStatus) -> Vec<(String, String)> {
  let content = match status {
    PreviewStatus::Ready(p) => Some(&**p),
    _ => None,
  };
  let label = match content {
    Some(Preview::Text { .. } | Preview::Highlighted { .. }) if !entry.is_executable() => {
      Some("text file")
    }
    Some(Preview::Image(_)) => Some("image"),
    _ => None,
  };
  let mut rows = metadata_lines(entry, label);
  if let Some(Preview::Image(image)) = content {
    rows.insert(
      2,
      (
        "Dimensions".to_string(),
        format!("{}x{}", image.width, image.height),
      ),
    );
  }
  rows
}

/// Draws `rows` from `skip` on, keys aligned over all rows so scrolling does not shift them.
fn draw_metadata(
  buf: &mut Buffer,
  area: Rect,
  rows: &[(String, String)],
  skip: usize,
  theme: &Theme,
) {
  let key_w = rows
    .iter()
    .map(|(k, _)| display_width(k))
    .max()
    .unwrap_or(0);
  let key_style = Style::new().fg(theme.dim.0);
  let val_style = Style::new().fg(theme.fg.0);
  let shown = rows.iter().skip(skip).take(area.height as usize);
  for (row, (key, value)) in shown.enumerate() {
    let y = area.y + row as u16;
    let (x, _) = buf.set_stringn(area.x, y, key, area.width as usize, key_style);
    let used = (x - area.x) as usize + (key_w - display_width(key)) + 2;
    if used < area.width as usize {
      let room = area.width as usize - used;
      buf.set_stringn(area.x + used as u16, y, value, room, val_style);
    }
  }
}

/// Converts a highlighter style; the default foreground is the theme's.
fn span_style(s: SpanStyle, theme: &Theme) -> Style {
  let color = |c: TermColor| match c {
    TermColor::Default => None,
    TermColor::Indexed(n) => Some(Color::Indexed(n)),
    TermColor::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
  };
  let mut style = Style::new().fg(color(s.fg).unwrap_or(theme.fg.0));
  if let Some(bg) = color(s.bg) {
    style = style.bg(bg);
  }
  let modifiers = [
    (s.bold, Modifier::BOLD),
    (s.dim, Modifier::DIM),
    (s.italic, Modifier::ITALIC),
    (s.underline, Modifier::UNDERLINED),
  ];
  for (on, m) in modifiers {
    if on {
      style = style.add_modifier(m);
    }
  }
  style
}

/// Ranger's info string: ` 15` for directories, ` 407 B` for files, `->` prefixed for
/// symlinks, and `dev` / `fifo` / `sock` for special files.
pub fn entry_info(entry: &Entry, app: &App) -> Option<String> {
  let info = match entry.kind {
    EntryKind::Dir => app.dir_count(&entry.path).map(|n| format!(" {n}")),
    EntryKind::File => Some(format!(
      " {}",
      human_size(entry.size, app.config.binary_size_prefix)
    )),
    EntryKind::CharDevice | EntryKind::BlockDevice => Some("dev".to_string()),
    EntryKind::Fifo => Some("fifo".to_string()),
    EntryKind::Socket => Some("sock".to_string()),
    EntryKind::Unknown => None,
  };
  if entry.is_symlink() {
    Some(format!("->{}", info.unwrap_or_default()))
  } else {
    info
  }
}

/// Draws a list that is shown but not navigated (parent, directory preview). `dir` is the
/// directory the entries belong to, used for VCS lookups.
fn draw_list(
  buf: &mut Buffer,
  area: Rect,
  dir: &Path,
  entries: &[&Entry],
  selected: Option<usize>,
  app: &App,
) {
  let height = area.height as usize;
  let offset = passive_offset(selected.unwrap_or(0), entries.len(), height);
  for (row, (i, entry)) in entries
    .iter()
    .enumerate()
    .skip(offset)
    .take(height)
    .enumerate()
  {
    let marks = app.vcs_marks(dir, entry);
    draw_row(
      buf,
      Rect::new(area.x, area.y + row as u16, area.width, 1),
      entry,
      selected == Some(i),
      None,
      &marks,
      app,
    );
  }
}

/// One entry row, laid out like ranger: a one-cell gutter (reserved for tag markers),
/// `icon name~`, padding, the info string, and VCS marks flush with the right edge.
/// Info and marks are dropped when they would leave fewer than three cells for the name.
/// The highlighted row is filled edge to edge. Marked entries are bold in the marked color.
fn draw_row(
  buf: &mut Buffer,
  row: Rect,
  entry: &Entry,
  highlighted: bool,
  info: Option<&str>,
  marks: &[Mark],
  app: &App,
) {
  let theme = &app.config.theme;
  let width = row.width as usize;
  if width == 0 {
    return;
  }
  let mut base = entry_style(entry, theme);
  if app.is_marked(&entry.path) {
    base = base.fg(theme.marked.0).add_modifier(Modifier::BOLD);
  }
  let style = if highlighted {
    let s = cursor_style(base, theme);
    buf.set_style(row, s);
    s
  } else {
    base
  };

  let mut space = width;
  let gutter = usize::from(space > 3);
  space -= gutter;

  let show_marks = !marks.is_empty() && space > marks.len() + 2;
  if show_marks {
    space -= marks.len();
  }
  let sep = usize::from(show_marks);
  let info = info.filter(|s| space > display_width(s) + sep + 2);
  let info_w = info.map_or(0, |s| display_width(s) + sep);
  space -= info_w;

  let title = format!("{} {}", icons::icon(entry, app.config.icons), entry.name);
  let title = truncate(&title, space);
  buf.set_stringn(row.x + gutter as u16, row.y, &title, space, style);

  let right_x = row.x + (width - info_w - if show_marks { marks.len() } else { 0 }) as u16;
  if let Some(info) = info {
    buf.set_stringn(right_x, row.y, info, info_w, style);
  }
  if show_marks {
    let start = right_x + info_w as u16;
    for (x, &mark) in (start..).zip(marks) {
      let mark_style = if highlighted {
        style
      } else {
        Style::new()
          .fg(mark_color(mark, theme))
          .remove_modifier(Modifier::BOLD)
      };
      buf.set_stringn(x, row.y, mark.symbol().to_string(), 1, mark_style);
    }
  }
}

/// The selected row's style. Like ranger, the entry's own style in reverse video (its color
/// becomes the background) unless the theme sets cursor colors, which replace one side each.
fn cursor_style(base: Style, theme: &Theme) -> Style {
  match (theme.cursor_fg, theme.cursor_bg) {
    (None, None) => base.add_modifier(Modifier::REVERSED),
    (fg, bg) => {
      let entry = base.fg.unwrap_or(theme.fg.0);
      base
        .fg(fg.map_or(entry, |c| c.0))
        .bg(bg.map_or(entry, |c| c.0))
    }
  }
}

/// VCS mark colors, following ranger's default colorscheme.
pub fn mark_color(mark: Mark, theme: &Theme) -> Color {
  let c = match mark {
    Mark::File(s) => match s {
      FileStatus::Conflict => theme.vcs_conflict,
      FileStatus::Untracked => theme.vcs_untracked,
      FileStatus::Deleted | FileStatus::Changed | FileStatus::Unknown => theme.vcs_changed,
      FileStatus::Staged => theme.vcs_staged,
      FileStatus::Ignored => theme.vcs_ignored,
      FileStatus::Sync => theme.vcs_sync,
    },
    Mark::Remote(r) => match r {
      RemoteStatus::Sync | RemoteStatus::None => theme.vcs_sync,
      RemoteStatus::Ahead => theme.vcs_ahead,
      RemoteStatus::Behind => theme.vcs_behind,
      RemoteStatus::Diverged => theme.vcs_diverged,
    },
    Mark::Blank => theme.fg,
  };
  c.0
}

fn placeholder(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
  if area.height == 0 {
    return;
  }
  buf.set_stringn(
    area.x,
    area.y,
    truncate(text, area.width as usize),
    area.width as usize,
    style,
  );
}

/// Ranger draws "empty" and "not accessible" on a red background.
fn alert_style(app: &App) -> Style {
  Style::new()
    .fg(app.config.theme.fg.0)
    .bg(app.config.theme.error.0)
}
