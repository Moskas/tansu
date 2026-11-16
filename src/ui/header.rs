use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::App;
use crate::fs::entry::sanitize;

/// `user@host /current/dir/selected`, like ranger's title bar: the directory part in the
/// path color, the selected entry's name in the default color.
pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
  let theme = &app.config.theme;
  let mut dir = sanitize(&app.cwd.to_string_lossy());
  let name = match app.selected() {
    Some(entry) => {
      if !dir.ends_with('/') {
        dir.push('/');
      }
      entry.name.as_str()
    }
    None => "",
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
