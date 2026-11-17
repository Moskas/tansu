//! The list of entries a pending deletion would remove, drawn above the status bar while
//! the question there waits for an answer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use super::text::truncate;
use crate::app::App;

/// Draws the popup over the bottom of `body` (the area between header and status bar).
pub fn render(buf: &mut Buffer, body: Rect, app: &App) {
  let Some(confirm) = &app.confirm else {
    return;
  };
  if body.width < 4 || body.height < 2 {
    return;
  }
  let theme = &app.config.theme;
  let names = &confirm.names;
  let max_rows = body.height as usize - 1;
  let (shown, more) = if names.len() > max_rows {
    // The last row says how many are left out.
    let shown = max_rows.saturating_sub(1);
    (shown, names.len() - shown)
  } else {
    (names.len(), 0)
  };
  let rows = shown + usize::from(more > 0);
  let height = rows as u16 + 1;
  let area = Rect::new(body.x, body.bottom() - height, body.width, height);
  Clear.render(area, buf);
  let title = if confirm.trash {
    " Move to the trash "
  } else {
    " Delete permanently, directories with their contents "
  };
  let color = if confirm.trash {
    theme.border.0
  } else {
    theme.error.0
  };
  let block = Block::new()
    .borders(Borders::TOP)
    .title(title)
    .border_style(Style::new().fg(color));
  let inner = block.inner(area);
  block.render(area, buf);
  let width = inner.width.saturating_sub(1) as usize;
  let style = Style::new().fg(theme.fg.0);
  for (row, name) in names.iter().take(shown).enumerate() {
    buf.set_stringn(
      inner.x + 1,
      inner.y + row as u16,
      truncate(name, width),
      width,
      style,
    );
  }
  if more > 0 {
    let text = format!("... and {more} more");
    let dim = Style::new().fg(theme.dim.0);
    buf.set_stringn(inner.x + 1, inner.y + shown as u16, text, width, dim);
  }
}
