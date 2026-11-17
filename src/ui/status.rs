use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

use super::text::{clip, display_width};
use crate::app::{App, MessageLevel};
use crate::console::LineEditor;
use crate::fs::FileClass;
use crate::fs::meta::{format_mtime, format_permissions, human_size_compact};
use crate::ui::columns::{entry_info, mark_color};
use crate::vcs::Mark;

/// Left: metadata of the selected entry (or a message). Right: totals and position.
pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
  if area.height == 0 || area.width == 0 {
    return;
  }
  let theme = &app.config.theme;
  let base = Style::new().fg(theme.status_fg.0);
  if let Some(editor) = &app.console {
    let (text, _) = console_view(editor, area.width);
    Line::styled(text, base).render(area, buf);
    return;
  }
  if let Some(confirm) = &app.confirm {
    let color = if confirm.is_warning() {
      theme.error.0
    } else {
      theme.status_fg.0
    };
    let style = Style::new().fg(color).add_modifier(Modifier::BOLD);
    Line::styled(confirm.question(), style).render(area, buf);
    return;
  }
  let binary = app.config.binary_size_prefix;

  let len = app.visible_len();
  let pos = if len == 0 { 0 } else { app.cursor().index + 1 };
  let free = app
    .free_space()
    .map_or_else(|| "?".to_string(), |b| human_size_compact(b, binary));
  let marked = app.marked_entries();
  let right = if marked.is_empty() {
    format!(
      "{} sum, {free} free  {pos}/{len}  {}",
      human_size_compact(app.visible_file_bytes(), binary),
      app.scroll_pos()
    )
  } else {
    // Like ranger: the size and count of the marked entries in place of the totals.
    let bytes = marked.iter().filter(|e| !e.is_dir()).map(|e| e.size).sum();
    format!(
      "{} sum, {} marked  {pos}/{len}  {}",
      human_size_compact(bytes, binary),
      marked.len(),
      app.scroll_pos()
    )
  };
  let right_w = display_width(&right) as u16;
  let right_x = area.right().saturating_sub(right_w).max(area.x);
  buf.set_stringn(right_x, area.y, &right, area.width as usize, base);

  let left_w = right_x.saturating_sub(area.x).saturating_sub(1);
  let left = Rect::new(area.x, area.y, left_w, 1);
  // An unfinished key sequence, shown like emacs shows a pending prefix (`g-`).
  if app.keymap.is_pending() {
    let prefix = format!("{}-", app.keymap.pending_notation());
    Line::styled(prefix, Style::new().fg(theme.help_key.0)).render(left, buf);
    return;
  }
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
  // Ranger colors permissions by ownership: yours vs someone else's.
  let perms_color = if entry.uid == app.uid {
    theme.status_perms
  } else {
    theme.status_perms_other
  };
  let mut segments = vec![
    Span::styled(
      format_permissions(entry.mode),
      Style::new().fg(perms_color.0),
    ),
    Span::styled(
      format!(" {} {} {}", entry.nlink, entry.owner, entry.group),
      base,
    ),
  ];
  match &entry.link_target {
    // Symlinks show their target in place of size and date.
    Some(target) => {
      let color = if entry.class() == FileClass::BrokenLink {
        theme.broken_link
      } else {
        theme.symlink
      };
      segments.push(Span::styled(
        format!(" -> {}", target.display()),
        Style::new().fg(color.0),
      ));
    }
    None => {
      // The column's info string without spaces: `407B`, `15`, `fifo`.
      let size = entry_info(entry, app).map(|s| s.replace(' ', ""));
      let text = match size {
        Some(size) if !size.is_empty() => format!(" {size} {}", format_mtime(entry.mtime)),
        _ => format!(" {}", format_mtime(entry.mtime)),
      };
      segments.push(Span::styled(text, base));
    }
  }
  if let Some((repo, status)) = app.vcs_selection() {
    let info = match &repo.branch {
      Some(branch) => format!(" (git: {branch})"),
      None => " (git)".to_string(),
    };
    segments.push(Span::styled(info, Style::new().fg(theme.vcs_info.0)));
    segments.push(Span::raw(" "));
    segments.push(Span::styled(
      Mark::Remote(repo.remote).symbol().to_string(),
      Style::new().fg(mark_color(Mark::Remote(repo.remote), theme)),
    ));
    segments.push(Span::styled(
      Mark::File(status).symbol().to_string(),
      Style::new().fg(mark_color(Mark::File(status), theme)),
    ));
    if let Some(head) = &repo.head {
      segments.push(Span::styled(
        format!(" {}", format_mtime(head.time)),
        Style::new().fg(theme.vcs_date.0),
      ));
      let summary: String = head.summary.chars().take(COMMIT_SUMMARY_LEN).collect();
      segments.push(Span::styled(
        format!(" {summary}"),
        Style::new().fg(theme.vcs_commit.0),
      ));
    }
  }
  Line::from(fitting(segments, left_w as usize)).render(left, buf);
}

/// Longest commit summary shown (ranger's `vcs_msg_length` default).
const COMMIT_SUMMARY_LEN: usize = 50;

/// Like ranger, drops whole segments from the end until the rest fits. The first segment
/// is always kept (and clipped by the caller if even it is too wide).
fn fitting(segments: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
  let mut used = 0;
  let mut out = Vec::new();
  for (i, seg) in segments.into_iter().enumerate() {
    let w = display_width(&seg.content);
    if i > 0 && used + w > width {
      break;
    }
    used += w;
    out.push(seg);
  }
  out
}

/// The command line as shown in a row of `width` cells: `:` and the input, scrolled so the
/// cursor stays visible. Returns the text and the cursor's column within the row.
pub fn console_view(editor: &LineEditor, width: u16) -> (String, u16) {
  let width = usize::from(width);
  let full = format!(":{}", editor.input());
  let cursor = 1 + display_width(editor.before_cursor());
  // Leave a cell for the cursor itself at the end of the row.
  let excess = (cursor + 1).saturating_sub(width);
  let mut dropped = 0;
  let mut start = 0;
  for (i, c) in full.char_indices() {
    if dropped >= excess {
      start = i;
      break;
    }
    dropped += UnicodeWidthChar::width(c).unwrap_or(0);
    start = i + c.len_utf8();
  }
  let text = clip(&full[start..], width).to_string();
  (text, (cursor - dropped) as u16)
}
