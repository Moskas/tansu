//! The bordered body with parent, current and preview columns.

use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Widget};

use super::text::{display_width, truncate};
use super::{class_color, entry_style, icons, layout};
use crate::app::{App, DirState, PreviewStatus};
use crate::archive::MemberKind;
use crate::config::Theme;
use crate::fs::entry::class_for_extension;
use crate::fs::meta::human_size;
use crate::fs::{Entry, EntryKind, FileClass};
use crate::nav::passive_offset;
use crate::preview::{Listing, Preview, Row, SpanStyle, TermColor, metadata_lines};
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
      Some(encoded) => {
        let (rows, info) = layout::image_split(area.height, image.info.len(), image.details_first);
        encoded.render(
          Rect {
            height: rows,
            ..area
          },
          buf,
        );
        if info > 0 {
          // Right below the image, which may not fill the rows it was given, after a rule
          // that sets the details apart from it.
          let rule_y = area.y + encoded.rows().min(rows);
          let rule = "─".repeat(area.width as usize);
          buf.set_string(area.x, rule_y, rule, Style::new().fg(theme.border.0));
          let below = Rect::new(area.x, rule_y + 1, area.width, info);
          draw_metadata(buf, below, &image.info, 0, theme);
        }
      }
      // No graphics support (or encoding failed): describe the image instead.
      None => {
        let mut rows = vec![(
          "Image".to_string(),
          format!("{}x{}", image.width, image.height),
        )];
        rows.extend(image.info.iter().cloned());
        draw_metadata(buf, area, &rows, 0, theme);
      }
    },
    Preview::Metadata(rows) => draw_metadata(buf, area, rows, slot.scroll, theme),
    Preview::Listing(listing) => draw_listing(buf, area, listing, slot.scroll, theme),
  }
}

/// Metadata for the metadata view, refined by what the content preview found out: the file
/// type, and the dimensions of images.
pub fn metadata_rows(entry: &Entry, status: &PreviewStatus) -> Vec<(String, String)> {
  let content = match status {
    PreviewStatus::Ready(p) => Some(&**p),
    _ => None,
  };
  // A metadata preview already is the metadata, with whatever a renderer added to it.
  if let Some(Preview::Metadata(rows)) = content {
    return rows.clone();
  }
  // An image preview of something that is not an image file is a rendering of it (a video
  // frame, a document page): its pixel size says nothing about the file.
  let photo = entry.class() == FileClass::Image;
  let label = match content {
    Some(Preview::Text { .. } | Preview::Highlighted { .. }) if !entry.is_executable() => {
      Some("text file")
    }
    Some(Preview::Listing(listing)) => Some(listing.kind),
    Some(Preview::Image(_)) if photo => Some("image"),
    Some(Preview::Image(image)) => image.label.or(class_label(entry.class())),
    _ => None,
  };
  let mut rows = metadata_lines(entry, label);
  if let Some(Preview::Image(image)) = content {
    if photo {
      rows.insert(
        2,
        (
          "Dimensions".to_string(),
          format!("{}x{}", image.width, image.height),
        ),
      );
    }
    rows.extend(image.info.iter().cloned());
  }
  rows
}

/// The type shown for files previewed through a rendering.
fn class_label(class: FileClass) -> Option<&'static str> {
  match class {
    FileClass::Video => Some("video"),
    FileClass::Audio => Some("audio"),
    FileClass::Document => Some("document"),
    _ => None,
  }
}

/// Draws `rows` from `skip` on, keys aligned over all rows so scrolling does not shift them.
fn draw_metadata(
  buf: &mut Buffer,
  area: Rect,
  rows: &[(String, String)],
  skip: usize,
  theme: &Theme,
) {
  let key_w = key_width(rows);
  let shown = rows.iter().skip(skip).take(area.height as usize);
  for (row, (key, value)) in shown.enumerate() {
    let line = Rect::new(area.x, area.y + row as u16, area.width, 1);
    draw_key_value(buf, line, key, value, key_w, theme);
  }
}

fn key_width(rows: &[(String, String)]) -> usize {
  rows
    .iter()
    .map(|(k, _)| display_width(k))
    .max()
    .unwrap_or(0)
}

/// `Key    value` on `line`, the value starting after the widest key (`key_w`).
fn draw_key_value(
  buf: &mut Buffer,
  line: Rect,
  key: &str,
  value: &str,
  key_w: usize,
  theme: &Theme,
) {
  let key_style = Style::new().fg(theme.dim.0);
  let val_style = Style::new().fg(theme.fg.0);
  let (x, _) = buf.set_stringn(line.x, line.y, key, line.width as usize, key_style);
  let used = (x - line.x) as usize + (key_w.saturating_sub(display_width(key))) + 2;
  if used < line.width as usize {
    let room = line.width as usize - used;
    buf.set_stringn(line.x + used as u16, line.y, value, room, val_style);
  }
}

/// A playlist, cue sheet or rip log from line `skip` on: the details, a rule, then the
/// entries (`number  title · detail      time`) or lines of text.
fn draw_listing(buf: &mut Buffer, area: Rect, listing: &Listing, skip: usize, theme: &Theme) {
  let key_w = key_width(&listing.info);
  let number_w = listing
    .rows
    .iter()
    .filter_map(|r| match r {
      Row::Entry { number, .. } => Some(display_width(number)),
      Row::Text(_) | Row::Member { .. } => None,
    })
    .max()
    .unwrap_or(0);
  let widest = |field: fn(&Row) -> Option<&String>| {
    listing
      .rows
      .iter()
      .filter_map(|r| field(r).map(|s| display_width(s)))
      .max()
      .unwrap_or(0)
  };
  let member_columns = MemberColumns {
    size: widest(|r| match r {
      Row::Member { size, .. } => size.as_ref(),
      _ => None,
    }),
    date: widest(|r| match r {
      Row::Member { modified, .. } => modified.as_ref(),
      _ => None,
    }),
  };
  let has_rule = !listing.info.is_empty() && !listing.rows.is_empty();
  let dim = Style::new().fg(theme.dim.0);
  let fg = Style::new().fg(theme.fg.0);
  let width = area.width as usize;
  for screen_row in 0..area.height {
    let i = skip + screen_row as usize;
    let line = Rect::new(area.x, area.y + screen_row, area.width, 1);
    if let Some((key, value)) = listing.info.get(i) {
      draw_key_value(buf, line, key, value, key_w, theme);
      continue;
    }
    let mut r = i - listing.info.len();
    if has_rule {
      if r == 0 {
        let rule = "─".repeat(width);
        buf.set_string(line.x, line.y, rule, Style::new().fg(theme.border.0));
        continue;
      }
      r -= 1;
    }
    match listing.rows.get(r) {
      None => break,
      Some(Row::Text(text)) => {
        buf.set_stringn(line.x, line.y, text, width, fg);
      }
      Some(Row::Member {
        name,
        kind,
        size,
        modified,
      }) => {
        let color = match kind {
          MemberKind::Dir => theme.directory,
          MemberKind::Link => theme.symlink,
          MemberKind::Other => theme.special,
          MemberKind::File => {
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
            let class = ext.as_deref().and_then(class_for_extension);
            class_color(class.unwrap_or(FileClass::Other), theme)
          }
        };
        draw_member(
          buf,
          line,
          name,
          Style::new().fg(color.0),
          size.as_deref(),
          modified.as_deref(),
          member_columns,
          dim,
        );
      }
      Some(Row::Entry {
        number,
        title,
        detail,
        time,
      }) => {
        // Time flush right; number right-aligned in its column; the title (and detail)
        // in between, truncated to fit.
        let time_w = time.as_deref().map_or(0, |t| display_width(t) + 1);
        let pad = number_w.saturating_sub(display_width(number));
        let lead = pad + display_width(number) + 2;
        if lead + time_w >= width {
          buf.set_stringn(line.x, line.y, truncate(title, width), width, fg);
          continue;
        }
        let x = line.x + pad as u16;
        buf.set_stringn(x, line.y, number, display_width(number), dim);
        let room = width - lead - time_w;
        let title_text = truncate(title, room);
        let (x, _) = buf.set_stringn(line.x + lead as u16, line.y, &title_text, room, fg);
        if let Some(detail) = detail {
          let used = (x - line.x) as usize - lead;
          if room > used + 3 {
            let text = format!(" · {detail}");
            buf.set_stringn(x, line.y, truncate(&text, room - used), room - used, dim);
          }
        }
        if let Some(time) = time {
          let tx = line.right() - display_width(time) as u16;
          buf.set_stringn(tx, line.y, time, display_width(time), dim);
        }
      }
    }
  }
}

/// Widths of the date and size columns of archive members.
#[derive(Clone, Copy)]
struct MemberColumns {
  size: usize,
  date: usize,
}

/// Columns given up, date first, when the name would get less than this.
const MIN_MEMBER_NAME: usize = 16;

/// `path      2024-01-02 10:00   12 k`: the name, then the date and size flush right.
#[allow(clippy::too_many_arguments)]
fn draw_member(
  buf: &mut Buffer,
  line: Rect,
  name: &str,
  style: Style,
  size: Option<&str>,
  date: Option<&str>,
  columns: MemberColumns,
  dim: Style,
) {
  let width = line.width as usize;
  let size_w = if columns.size > 0 {
    columns.size + 1
  } else {
    0
  };
  let date_w = if columns.date > 0 {
    columns.date + 2
  } else {
    0
  };
  let (show_size, show_date) = if width >= MIN_MEMBER_NAME + size_w + date_w {
    (true, true)
  } else {
    (width >= MIN_MEMBER_NAME + size_w, false)
  };
  let right = if show_size { size_w } else { 0 } + if show_date { date_w } else { 0 };
  let room = width - right;
  buf.set_stringn(line.x, line.y, truncate(name, room), room, style);
  if show_size && let Some(size) = size {
    let x = line.right() - display_width(size) as u16;
    buf.set_stringn(x, line.y, size, display_width(size), dim);
  }
  if show_date && let Some(date) = date {
    let x = line.right() - (size_w + columns.date) as u16;
    buf.set_stringn(x, line.y, date, columns.date, dim);
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
      let fg = fg.map_or(entry, |c| c.0);
      match bg {
        Some(bg) => base.fg(fg).bg(bg.0),
        // The terminal's default foreground cannot be set as a background; reverse video
        // puts it there (and `fg`, set as the background, in front).
        None if entry == Color::Reset => base
          .fg(Color::Reset)
          .bg(fg)
          .add_modifier(Modifier::REVERSED),
        None => base.fg(fg).bg(entry),
      }
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
