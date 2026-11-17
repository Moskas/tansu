//! The which-key popup: while a key sequence is unfinished, the keys that can follow it and
//! what they do, in columns above the status bar.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use super::text::{display_width, truncate};
use crate::app::App;
use crate::keymap::Continuation;

/// Blank columns between two entries.
const GAP: usize = 3;

/// Draws the popup over the bottom of `body` (the area between header and status bar).
pub fn render(buf: &mut Buffer, body: Rect, app: &App) {
  if !app.config.which_key || !app.keymap.is_pending() || body.width < 4 || body.height < 2 {
    return;
  }
  let theme = &app.config.theme;
  let entries: Vec<(String, String, Style)> = app
    .keymap
    .continuations()
    .into_iter()
    .map(|(key, next)| match next {
      Continuation::Action(action) => (key, action.description().to_string(), Style::new()),
      Continuation::Group(n) => {
        let plural = if n == 1 { "binding" } else { "bindings" };
        let style = Style::new().fg(theme.help_group.0);
        (key, format!("+{n} {plural}"), style)
      }
    })
    .collect();
  if entries.is_empty() {
    return;
  }

  let key_w = entries
    .iter()
    .map(|(k, _, _)| display_width(k))
    .max()
    .unwrap_or(0);
  let desc_w = entries
    .iter()
    .map(|(_, d, _)| display_width(d))
    .max()
    .unwrap_or(0);
  let inner_w = body.width as usize - 2;
  let cell_w = (key_w + 3 + desc_w + GAP).min(inner_w + GAP);
  let cols = ((inner_w + GAP) / cell_w).max(1);
  let max_rows = body.height as usize - 1;
  let rows = entries.len().div_ceil(cols).min(max_rows);
  let shown = (rows * cols).min(entries.len());
  // Spread the columns over the full width, like which-key.
  let cell_w = (inner_w + GAP) / cols;

  let height = rows as u16 + 1;
  let area = Rect::new(body.x, body.bottom() - height, body.width, height);
  Clear.render(area, buf);
  let mut title = format!(" {}- ", app.keymap.pending_notation());
  if shown < entries.len() {
    title.push_str(&format!("(+{} more) ", entries.len() - shown));
  }
  let block = Block::new()
    .borders(Borders::TOP)
    .title(title)
    .border_style(Style::new().fg(theme.border.0));
  let inner = block.inner(area);
  block.render(area, buf);

  let key_style = Style::new().fg(theme.help_key.0);
  let sep_style = Style::new().fg(theme.dim.0);
  let desc_base = Style::new().fg(theme.fg.0);
  // Column-major, so the keys read down each column in order.
  for (i, (key, desc, style)) in entries.iter().take(shown).enumerate() {
    let (col, row) = (i / rows, i % rows);
    let x = inner.x + 1 + (col * cell_w) as u16;
    let width = cell_w
      .saturating_sub(GAP)
      .min(inner.right().saturating_sub(x) as usize);
    let desc_room = width.saturating_sub(key_w + 3);
    let line = Line::from(vec![
      Span::styled(
        format!("{}{key}", " ".repeat(key_w - display_width(key))),
        key_style,
      ),
      Span::styled(" : ", sep_style),
      Span::styled(truncate(desc, desc_room), desc_base.patch(*style)),
    ]);
    line.render(Rect::new(x, inner.y + row as u16, width as u16, 1), buf);
  }
}
