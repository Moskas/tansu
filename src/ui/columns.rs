//! The bordered body with parent, current and preview columns.

use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Widget};

use super::text::{display_width, truncate};
use super::{entry_style, icons, layout};
use crate::app::{App, DirState, PreviewStatus};
use crate::fs::meta::human_size;
use crate::fs::{Entry, EntryKind};
use crate::nav::passive_offset;
use crate::preview::Preview;

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
      let entries: Vec<&Entry> = listing
        .entries
        .iter()
        .filter(|e| app.is_visible(e) || e.path == app.cwd)
        .collect();
      let selected = entries.iter().position(|e| e.path == app.cwd);
      draw_list(buf, area, &entries, selected, app);
    }
    Some(DirState::Failed(msg)) => placeholder(buf, area, msg, error_style(app)),
    _ => {}
  }
}

fn render_current(buf: &mut Buffer, area: Rect, app: &App) {
  if app.visible_len() == 0 {
    match app.dir_state(&app.cwd) {
      Some(DirState::Failed(msg)) => placeholder(buf, area, msg, error_style(app)),
      Some(DirState::Loaded(_)) => placeholder(buf, area, "empty", error_style(app)),
      _ => placeholder(buf, area, "loading...", dim_style(app)),
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
    draw_row(
      buf,
      Rect::new(area.x, area.y + row, area.width, 1),
      entry,
      i == cursor.index,
      info.as_deref(),
      app,
    );
  }
}

fn render_preview(buf: &mut Buffer, area: Rect, app: &App) {
  let Some(slot) = &app.preview else {
    return;
  };
  let theme = &app.config.theme;
  let preview = match &slot.status {
    PreviewStatus::Loading => return,
    PreviewStatus::Failed(msg) => return placeholder(buf, area, msg, error_style(app)),
    PreviewStatus::Ready(p) => p,
  };
  match &**preview {
    Preview::Text { lines, .. } => {
      let style = Style::new().fg(theme.fg.0);
      for (row, line) in lines.iter().take(area.height as usize).enumerate() {
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
      let entries: Vec<&Entry> = listing
        .entries
        .iter()
        .filter(|e| app.is_visible(e))
        .collect();
      if entries.is_empty() {
        return placeholder(buf, area, "empty", error_style(app));
      }
      let selected = app
        .remembered(&listing.path)
        .and_then(|name| entries.iter().position(|e| e.name == name))
        .unwrap_or(0);
      draw_list(buf, area, &entries, Some(selected), app);
    }
    Preview::Metadata(rows) => {
      let key_w = rows
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(0);
      let key_style = Style::new().fg(theme.dim.0);
      let val_style = Style::new().fg(theme.fg.0);
      for (row, (key, value)) in rows.iter().take(area.height as usize).enumerate() {
        let y = area.y + row as u16;
        let (x, _) = buf.set_stringn(area.x, y, key, area.width as usize, key_style);
        let used = (x - area.x) as usize + (key_w - display_width(key)) + 2;
        if used < area.width as usize {
          let room = area.width as usize - used;
          buf.set_stringn(area.x + used as u16, y, value, room, val_style);
        }
      }
    }
  }
}

/// Right-aligned detail: item count for directories, size for regular files.
fn entry_info(entry: &Entry, app: &App) -> Option<String> {
  match entry.kind {
    EntryKind::Dir => app.dir_count(&entry.path).map(|n| n.to_string()),
    EntryKind::File => Some(human_size(entry.size)),
    _ => None,
  }
}

/// Draws a list that is shown but not navigated (parent, directory preview).
fn draw_list(buf: &mut Buffer, area: Rect, entries: &[&Entry], selected: Option<usize>, app: &App) {
  let height = area.height as usize;
  let offset = passive_offset(selected.unwrap_or(0), entries.len(), height);
  for (row, (i, entry)) in entries
    .iter()
    .enumerate()
    .skip(offset)
    .take(height)
    .enumerate()
  {
    draw_row(
      buf,
      Rect::new(area.x, area.y + row as u16, area.width, 1),
      entry,
      selected == Some(i),
      None,
      app,
    );
  }
}

/// One entry row: `icon name~ ... info `. The highlighted row is filled edge to edge.
fn draw_row(
  buf: &mut Buffer,
  row: Rect,
  entry: &Entry,
  highlighted: bool,
  info: Option<&str>,
  app: &App,
) {
  let theme = &app.config.theme;
  let width = row.width as usize;
  if width == 0 {
    return;
  }
  let style = if highlighted {
    let s = Style::new().fg(theme.cursor_fg.0).bg(theme.cursor_bg.0);
    buf.set_style(row, s);
    s
  } else {
    entry_style(entry, theme)
  };
  let prefix = format!("{} ", icons::icon(entry, app.config.icons));
  let prefix_w = display_width(&prefix);
  // `info` is drawn as " info " so it never touches the name or the separator.
  let info_w = info.map_or(1, |s| display_width(s) + 2);
  let name_room = width.saturating_sub(prefix_w + info_w);
  let name = truncate(&entry.name, name_room);
  buf.set_stringn(row.x, row.y, format!("{prefix}{name}"), width, style);
  if let Some(info) = info
    && width >= prefix_w + info_w
  {
    let x = row.x + (width - info_w + 1) as u16;
    buf.set_stringn(x, row.y, info, info_w - 2, style);
  }
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

fn error_style(app: &App) -> Style {
  Style::new().fg(app.config.theme.error.0)
}

fn dim_style(app: &App) -> Style {
  Style::new().fg(app.config.theme.dim.0)
}
