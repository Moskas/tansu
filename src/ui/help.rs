use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget};

use super::text::display_width;
use crate::action::Action;
use crate::app::App;

/// Centered overlay listing the current bindings (generated from the keymap).
pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
  let theme = &app.config.theme;
  // Bindings to command lines come first: the overlay does not scroll, and they are the
  // user's own.
  let mut rows: Vec<(String, String)> = app
    .keymap
    .line_bindings()
    .into_iter()
    .map(|(keys, line)| (keys, app.line_description(&line)))
    .collect();
  rows.extend(Action::ALL.iter().filter_map(|&a| {
    let keys = app.keymap.keys_for(a);
    (!keys.is_empty()).then(|| (keys.join(" "), a.description().to_string()))
  }));
  let key_w = rows
    .iter()
    .map(|(k, _)| display_width(k))
    .max()
    .unwrap_or(0);
  let desc_w = rows
    .iter()
    .map(|(_, d)| display_width(d))
    .max()
    .unwrap_or(0);

  let width = ((key_w + desc_w + 6) as u16).min(area.width);
  let height = (rows.len() as u16 + 4).min(area.height);
  let popup = Rect::new(
    area.x + (area.width - width) / 2,
    area.y + (area.height - height) / 2,
    width,
    height,
  );

  let key_style = Style::new().fg(theme.help_key.0);
  let desc_style = Style::new().fg(theme.fg.0);
  let mut lines: Vec<Line> = rows
    .iter()
    .map(|(keys, desc)| {
      let pad = key_w - display_width(keys) + 2;
      Line::from(vec![
        Span::styled(format!(" {keys}{}", " ".repeat(pad)), key_style),
        Span::styled(desc.as_str(), desc_style),
      ])
    })
    .collect();
  lines.push(Line::default());
  lines.push(Line::styled(
    " press any key to close",
    Style::new().fg(theme.dim.0),
  ));

  Clear.render(popup, buf);
  Paragraph::new(lines)
    .block(
      Block::bordered()
        .title(" Help ".bold())
        .border_style(Style::new().fg(theme.border.0)),
    )
    .render(popup, buf);
}
