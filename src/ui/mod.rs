//! Rendering. Everything here reads [`App`] state and draws into a buffer; no I/O happens.

pub mod columns;
pub mod devicons;
pub mod header;
pub mod help;
pub mod icons;
pub mod layout;
pub mod status;
pub mod text;
pub mod which_key;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};

use crate::app::App;
use crate::config::Theme;
use crate::fs::{Entry, EntryKind, FileClass};

pub fn render(frame: &mut Frame, app: &App) {
  let area = frame.area();
  draw(frame.buffer_mut(), area, app);
  if let Some(editor) = &app.console
    && !app.show_help
    && area.height > 0
  {
    let (_, x) = status::console_view(editor, area.width);
    frame.set_cursor_position((area.x + x, area.bottom() - 1));
  }
}

/// Draws the full screen into `buf`.
pub fn draw(buf: &mut Buffer, area: Rect, app: &App) {
  let [header_area, body_area, status_area] = Layout::vertical([
    Constraint::Length(1),
    Constraint::Min(0),
    Constraint::Length(1),
  ])
  .areas(area);
  header::render(buf, header_area, app);
  columns::render(buf, body_area, app);
  status::render(buf, status_area, app);
  if app.console.is_none() && !app.show_help {
    which_key::render(buf, body_area, app);
  }
  if app.show_help {
    help::render(buf, area, app);
  }
}

/// Foreground style for an entry based on its type.
pub fn entry_style(entry: &Entry, theme: &Theme) -> Style {
  let color = match entry.class() {
    FileClass::Dir if entry.is_symlink() => theme.symlink,
    FileClass::Dir => theme.directory,
    FileClass::BrokenLink => theme.broken_link,
    _ if entry.is_symlink() => theme.symlink,
    FileClass::Executable => theme.executable,
    FileClass::Archive => theme.archive,
    FileClass::Image => theme.image,
    FileClass::Video => theme.video,
    FileClass::Audio => theme.audio,
    FileClass::Document => theme.document,
    FileClass::Code => theme.code,
    FileClass::Special => theme.special,
    FileClass::Text | FileClass::Other => theme.file,
  };
  // Ranger makes executables, sockets and devices bold (directories are kept regular).
  let bold = match entry.kind {
    EntryKind::Socket | EntryKind::CharDevice | EntryKind::BlockDevice => true,
    EntryKind::File => entry.class() == FileClass::Executable,
    _ => false,
  };
  let style = Style::new().fg(color.0);
  if bold {
    style.add_modifier(Modifier::BOLD)
  } else {
    style
  }
}
