use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use super::text::display_width;
use crate::app::{App, MessageLevel};
use crate::fs::meta::{format_mtime, format_permissions, human_size_compact};

/// Left: metadata of the selected entry (or a message). Right: totals and position.
pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
  if area.height == 0 || area.width == 0 {
    return;
  }
  let theme = &app.config.theme;
  let base = Style::new().fg(theme.status_fg.0);

  let len = app.visible_len();
  let pos = if len == 0 { 0 } else { app.cursor().index + 1 };
  let free = app
    .free_space()
    .map_or_else(|| "?".to_string(), human_size_compact);
  let right = format!(
    "{} sum, {free} free  {pos}/{len}  {}",
    human_size_compact(app.visible_file_bytes()),
    app.scroll_pos()
  );
  let right_w = display_width(&right) as u16;
  let right_x = area.right().saturating_sub(right_w).max(area.x);
  buf.set_stringn(right_x, area.y, &right, area.width as usize, base);

  let left_w = right_x.saturating_sub(area.x).saturating_sub(1);
  let left = Rect::new(area.x, area.y, left_w, 1);
  if let Some(msg) = &app.message {
    let color = match msg.level {
      MessageLevel::Error => theme.error.0,
      MessageLevel::Info => theme.status_fg.0,
    };
    Line::styled(msg.text.as_str(), Style::new().fg(color)).render(left, buf);
    return;
  }
  let Some(entry) = app.selected() else {
    return;
  };
  let size = if entry.is_dir() {
    app
      .dir_count(&entry.path)
      .map_or_else(|| human_size_compact(entry.size), |n| n.to_string())
  } else {
    human_size_compact(entry.size)
  };
  let mut spans = vec![
    Span::styled(
      format_permissions(entry.mode),
      Style::new().fg(theme.status_perms.0),
    ),
    Span::styled(
      format!(
        " {} {} {} {size} {}",
        entry.nlink,
        entry.owner,
        entry.group,
        format_mtime(entry.mtime)
      ),
      base,
    ),
  ];
  if let Some(target) = &entry.link_target {
    spans.push(Span::styled(
      format!(" -> {}", target.display()),
      Style::new().fg(theme.symlink.0),
    ));
  }
  Line::from(spans).render(left, buf);
}
