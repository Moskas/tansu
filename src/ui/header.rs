use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::App;
use crate::fs::entry::sanitize;

/// `user@host /current/path`
pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
  let theme = &app.config.theme;
  let path = sanitize(&app.cwd.to_string_lossy());
  let (dir, name) = match path.rfind('/') {
    Some(i) => path.split_at(i + 1),
    None => ("", path.as_str()),
  };
  Line::from(vec![
    Span::styled(
      app.user_host.as_str(),
      Style::new().fg(theme.header_user.0).bold(),
    ),
    Span::raw(" "),
    Span::styled(dir, Style::new().fg(theme.header_path.0).bold()),
    Span::styled(name, Style::new().fg(theme.fg.0).bold()),
  ])
  .render(area, buf);
}
