//! The task popup: one task with its progress, the list of several, or a paste conflict
//! waiting for an answer. Finished tasks leave a toast in the corner for a while.

use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Widget};

use super::text::{display_width, truncate};
use crate::app::App;
use crate::config::Theme;
use crate::fs::meta::{format_mtime, human_size};
use crate::fs::ops::Stat;
use crate::jobs::{Job, format_duration};

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const BAR: &str = "━";

/// A spinner frame that moves with the time since `since`.
pub fn spinner(since: Instant) -> char {
  SPINNER[(since.elapsed().as_millis() / 100) as usize % SPINNER.len()]
}

/// A bar `cells` wide, filled to `fraction`.
pub fn bar(fraction: f64, cells: usize, fill: Style, track: Style) -> Vec<Span<'static>> {
  let filled = ((fraction * cells as f64).round() as usize).min(cells);
  vec![
    Span::styled(BAR.repeat(filled), fill),
    Span::styled(BAR.repeat(cells - filled), track),
  ]
}

/// ` 62%`.
pub fn percent(fraction: f64) -> String {
  format!("{:>3}%", (fraction * 100.0).floor() as u32)
}

/// Draws toasts over the bottom right of `body`, then the popup when it is up.
pub fn render(buf: &mut Buffer, body: Rect, app: &App) {
  render_toasts(buf, body, app);
  if !app.tasks_visible() || body.width < 20 || body.height < 7 {
    return;
  }
  if let Some(job) = app.jobs.prompting() {
    render_conflict(buf, body, app, job);
  } else if let [job] = app.jobs.list.as_slice() {
    render_single(buf, body, app, job);
  } else {
    render_list(buf, body, app);
  }
}

/// A rect `width` x `height` in the middle of `body`, cleared with a cell of margin on
/// either side so the frame stands off what is behind it.
fn popup_area(buf: &mut Buffer, body: Rect, width: u16, height: u16) -> Rect {
  let width = width.min(body.width.saturating_sub(4));
  let height = height.min(body.height);
  let x = body.x + (body.width - width) / 2;
  let y = body.y + (body.height - height) / 2;
  Clear.render(Rect::new(x - 1, y, width + 2, height), buf);
  Rect::new(x, y, width, height)
}

/// A bordered frame with a title on the left, a status on the right and key hints along
/// the bottom. Returns the area inside it.
fn frame(
  buf: &mut Buffer,
  area: Rect,
  border: Color,
  title: Line<'static>,
  status: Line<'static>,
  hints: Line<'static>,
  padding: Padding,
) -> Rect {
  let block = Block::bordered()
    .border_style(Style::new().fg(border))
    .title_top(title)
    .title_top(status.right_aligned())
    .title_bottom(hints.right_aligned())
    .padding(padding);
  let inner = block.inner(area);
  block.render(area, buf);
  inner
}

/// ` b background  p pause ` as styled spans.
fn hints(theme: &Theme, pairs: &[(&str, &str)], accent: Color) -> Line<'static> {
  let key = Style::new().fg(accent).add_modifier(Modifier::BOLD);
  let label = Style::new().fg(theme.dim.0);
  let mut spans = vec![Span::raw(" ")];
  for (i, (k, l)) in pairs.iter().enumerate() {
    if i > 0 {
      spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(k.to_string(), key));
    spans.push(Span::styled(format!(" {l}"), label));
  }
  spans.push(Span::raw(" "));
  Line::from(spans)
}

fn pause_hint(job: &Job) -> &'static str {
  if job.paused { "resume" } else { "pause" }
}

/// `9/13 files · 372 M / 615 M · 48 M/s · 0:05 left`, as much as is known.
fn stats(job: &Job, binary: bool) -> String {
  let p = &job.progress;
  let mut parts = Vec::new();
  if let Some(total) = p.files_total {
    let unit = job.kind.unit();
    parts.push(format!("{}/{total} {unit}", p.files_done.min(total)));
  }
  match p.bytes_total {
    Some(total) if total > 0 => parts.push(format!(
      "{} / {}",
      human_size(p.bytes_done.min(total), binary),
      human_size(total, binary)
    )),
    _ if p.bytes_done > 0 => parts.push(human_size(p.bytes_done, binary)),
    _ => {}
  }
  if let Some(rate) = job.rate() {
    parts.push(format!("{}/s", human_size(rate as u64, binary)));
  }
  if let Some(eta) = job.eta() {
    parts.push(format!("{} left", format_duration(eta)));
  }
  parts.join(" · ")
}

/// The right side of a running task's title: its bar and percentage, a spinner while that
/// is unknown, or why it does not move.
fn status_line(job: &Job, theme: &Theme, cells: usize) -> Line<'static> {
  let dim = Style::new().fg(theme.dim.0);
  let track = Style::new().fg(theme.progress_track.0);
  let mut spans = vec![Span::raw(" ")];
  if job.started.is_none() {
    spans.push(Span::styled("queued", dim));
  } else {
    match job.progress.fraction() {
      Some(f) => {
        let fill = if job.paused || job.cancelled {
          dim
        } else {
          Style::new().fg(theme.progress.0)
        };
        spans.extend(bar(f, cells, fill, track));
        spans.push(Span::styled(
          format!(" {}", percent(f)),
          Style::new().fg(theme.fg.0).add_modifier(Modifier::BOLD),
        ));
      }
      None => spans.push(Span::styled(
        spinner(job.created).to_string(),
        Style::new().fg(theme.progress.0),
      )),
    }
    if job.paused {
      spans.push(Span::styled("  paused", dim));
    }
  }
  spans.push(Span::raw(" "));
  Line::from(spans)
}

/// The popup for a single task.
fn render_single(buf: &mut Buffer, body: Rect, app: &App, job: &Job) {
  let theme = &app.config.theme;
  let width = (body.width * 7 / 10).clamp(50, 96);
  let area = popup_area(buf, body, width, 7);
  let accent = theme.progress.0;
  let (verb, verb_color) = if job.cancelled {
    ("Cancelling", theme.dim.0)
  } else {
    (job.kind.verb(), accent)
  };
  let title = Line::from(Span::styled(
    format!(" {verb} "),
    Style::new().fg(verb_color).add_modifier(Modifier::BOLD),
  ));
  let cells = (area.width as usize / 3).clamp(8, 24);
  let keys = [("b", "background"), ("p", pause_hint(job)), ("x", "cancel")];
  let inner = frame(
    buf,
    area,
    theme.border.0,
    title,
    status_line(job, theme, cells),
    hints(theme, &keys, accent),
    Padding::new(2, 2, 1, 1),
  );
  let width = inner.width as usize;
  let current = if job.progress.current.is_empty() {
    job.what.clone()
  } else {
    job.progress.current.clone()
  };
  let dim = Style::new().fg(theme.dim.0);
  let mut lines = vec![
    (
      current,
      Style::new().fg(theme.fg.0).add_modifier(Modifier::BOLD),
    ),
    (stats(job, app.config.binary_size_prefix), dim),
  ];
  if let Some(dest) = &job.dest {
    lines.push((format!("→ {}/", app.short_path(dest)), dim));
  }
  for (row, (text, style)) in lines.into_iter().enumerate().take(inner.height as usize) {
    buf.set_stringn(
      inner.x,
      inner.y + row as u16,
      truncate(&text, width),
      width,
      style,
    );
  }
}

/// `[2025.05.16] WARP2ONE… → ~/Downloads`.
fn job_what(job: &Job, app: &App) -> String {
  match &job.dest {
    Some(dest) => format!("{} → {}", job.what, app.short_path(dest)),
    None => job.what.clone(),
  }
}

/// The popup listing several tasks, two rows each.
fn render_list(buf: &mut Buffer, body: Rect, app: &App) {
  let theme = &app.config.theme;
  let jobs = &app.jobs;
  let rows = (body.height.saturating_sub(4) / 2).max(1) as usize;
  let shown = jobs.list.len().min(rows);
  let width = (body.width * 3 / 4).clamp(56, 110);
  let area = popup_area(buf, body, width, shown as u16 * 2 + 4);
  let accent = theme.progress.0;
  let dim = Style::new().fg(theme.dim.0);
  let title = Line::from(Span::styled(
    " Tasks ",
    Style::new().fg(accent).add_modifier(Modifier::BOLD),
  ));
  let mut counts = vec![format!("{} running", jobs.running())];
  if jobs.queued() > 0 {
    counts.push(format!("{} queued", jobs.queued()));
  }
  let status = Line::styled(format!(" {} ", counts.join(" · ")), dim);
  let pause = jobs.selected_job().map_or("pause", pause_hint);
  let keys = [
    ("j/k", "select"),
    ("p", pause),
    ("x", "cancel"),
    ("esc", "hide"),
  ];
  let inner = frame(
    buf,
    area,
    theme.border.0,
    title,
    status,
    hints(theme, &keys, accent),
    Padding::new(0, 1, 1, 1),
  );
  let first = jobs
    .selected
    .saturating_sub(shown - 1)
    .min(jobs.list.len() - shown);
  // The task the queued ones wait for.
  let running = jobs.list.iter().find(|j| j.is_running());
  for (row, (i, job)) in jobs
    .list
    .iter()
    .enumerate()
    .skip(first)
    .take(shown)
    .enumerate()
  {
    let y = inner.y + row as u16 * 2;
    if y + 1 >= inner.bottom() {
      break;
    }
    let selected = i == jobs.selected;
    render_row(
      buf,
      Rect::new(inner.x, y, inner.width, 2),
      app,
      job,
      selected,
      running,
    );
  }
}

/// One task of the list: verb, what, bar and percentage, and below the file it is on with
/// its speed.
fn render_row(
  buf: &mut Buffer,
  area: Rect,
  app: &App,
  job: &Job,
  selected: bool,
  running: Option<&Job>,
) {
  const VERB: usize = 8;
  const CELLS: usize = 18;
  let theme = &app.config.theme;
  let dim = Style::new().fg(theme.dim.0);
  let queued = job.started.is_none() && job.prompt.is_none();
  let verb_color = if job.prompt.is_some() {
    theme.warning.0
  } else if queued || job.cancelled {
    theme.dim.0
  } else {
    theme.progress.0
  };
  let marker = if selected { "▌" } else { " " };
  let marker_style = Style::new().fg(theme.progress.0);
  let fraction = job.progress.fraction().filter(|_| !queued);
  let (pct, pct_style) = match fraction {
    Some(f) => (
      percent(f),
      Style::new().fg(theme.fg.0).add_modifier(Modifier::BOLD),
    ),
    None if queued => ("   —".to_string(), dim),
    None => (
      format!("   {}", spinner(job.created)),
      Style::new().fg(theme.progress.0),
    ),
  };
  let fill = if job.paused || job.cancelled {
    dim
  } else {
    Style::new().fg(verb_color)
  };
  let track = Style::new().fg(theme.progress_track.0);
  let fixed = 2 + VERB + 2 + 1 + CELLS + 1 + 4;
  let flexible = (area.width as usize).saturating_sub(fixed);
  let what = truncate(&job_what(job, app), flexible);
  let pad = flexible.saturating_sub(display_width(&what));
  let mut top = vec![
    Span::styled(marker, marker_style),
    Span::raw(" "),
    Span::styled(
      format!("{:<VERB$}", job.kind.name()),
      Style::new().fg(verb_color).add_modifier(Modifier::BOLD),
    ),
    Span::raw("  "),
    Span::styled(what, Style::new().fg(theme.fg.0)),
    Span::raw(" ".repeat(pad + 1)),
  ];
  top.extend(bar(fraction.unwrap_or(0.0), CELLS, fill, track));
  top.push(Span::styled(format!(" {pct}"), pct_style));
  Line::from(top).render(Rect::new(area.x, area.y, area.width, 1), buf);

  let (sub, meta) = if job.prompt.is_some() {
    ("waiting for an answer".to_string(), String::new())
  } else if queued {
    let sub = match running {
      Some(r) => format!("waiting for {} to finish", r.kind.name()),
      None => "waiting".to_string(),
    };
    (sub, "queued".to_string())
  } else {
    let meta = if job.paused {
      "paused".to_string()
    } else if job.cancelled {
      "cancelling".to_string()
    } else {
      let binary = app.config.binary_size_prefix;
      let mut parts = Vec::new();
      if let Some(rate) = job.rate() {
        parts.push(format!("{}/s", human_size(rate as u64, binary)));
      }
      if let Some(eta) = job.eta() {
        parts.push(format_duration(eta));
      }
      parts.join(" · ")
    };
    (job.progress.current.clone(), meta)
  };
  let meta_w = display_width(&meta);
  let sub_w = (area.width as usize).saturating_sub(2 + VERB + 2 + meta_w + 1);
  let sub = truncate(&sub, sub_w);
  let pad = sub_w.saturating_sub(display_width(&sub));
  let bottom = vec![
    Span::styled(marker, marker_style),
    Span::raw(" "),
    Span::styled(format!("{:>VERB$}", "↳"), dim),
    Span::raw("  "),
    Span::styled(sub, dim),
    Span::raw(" ".repeat(pad + 1)),
    Span::styled(meta, dim),
  ];
  Line::from(bottom).render(Rect::new(area.x, area.y + 1, area.width, 1), buf);
}

/// `300 k  2026-11-13 21:42`, or `dir` for a directory.
fn stat_cells(stat: &Stat, binary: bool) -> (String, String) {
  let size = if stat.dir {
    "dir".to_string()
  } else {
    human_size(stat.size, binary)
  };
  (size, format_mtime(stat.mtime))
}

/// The popup asking what to do about a name a paste found taken.
fn render_conflict(buf: &mut Buffer, body: Rect, app: &App, job: &Job) {
  let theme = &app.config.theme;
  let Some(prompt) = &job.prompt else {
    return;
  };
  let Some(conflict) = prompt.current() else {
    return;
  };
  let warn = theme.warning.0;
  let dim = Style::new().fg(theme.dim.0);
  let fg = Style::new().fg(theme.fg.0);
  let key = Style::new().fg(warn).add_modifier(Modifier::BOLD);
  let binary = app.config.binary_size_prefix;

  let mut lines: Vec<Line<'static>> = Vec::new();
  let dest = job
    .request
    .as_ref()
    .map(|r| app.short_path(&r.dest))
    .unwrap_or_default();
  let here = if conflict.itself {
    format!(" is already in {dest}/: it is the entry being copied")
  } else {
    format!(" already exists in {dest}/")
  };
  lines.push(Line::from(vec![
    Span::styled(conflict.name.clone(), fg.add_modifier(Modifier::BOLD)),
    Span::styled(here, dim),
  ]));
  let rename = vec![
    Span::styled("r", key),
    Span::styled(" rename ", fg),
    Span::styled(format!("→ {}", conflict.renamed), dim),
  ];
  // Copied into its own directory: there is nothing to compare or replace.
  if conflict.itself {
    lines.push(Line::default());
    let mut options = vec![Span::styled("s", key), Span::styled(" skip   ", fg)];
    options.extend(rename.clone());
    lines.push(Line::from(options));
  }
  let newer = conflict.incoming.mtime.cmp(&conflict.existing.mtime);
  let compared = [
    ("incoming", &conflict.incoming, newer.is_gt()),
    ("existing", &conflict.existing, newer.is_lt()),
  ];
  if !conflict.itself {
    lines.push(Line::default());
  }
  for (label, stat, is_newer) in compared.into_iter().filter(|_| !conflict.itself) {
    let (size, time) = stat_cells(stat, binary);
    let mut spans = vec![
      Span::styled(format!("  {label:<10}"), dim),
      Span::styled(format!("{size:<9}"), fg),
      Span::styled(format!("{time}  "), fg),
    ];
    if is_newer {
      spans.push(Span::styled("newer", Style::new().fg(theme.success.0)));
    }
    lines.push(Line::from(spans));
  }
  if !conflict.itself {
    lines.push(Line::default());
    let overwrite = if conflict.incoming.dir && conflict.existing.dir {
      "merge"
    } else {
      "overwrite"
    };
    let mut options = vec![
      Span::styled("o", key),
      Span::styled(format!(" {overwrite}   "), fg),
      Span::styled("s", key),
      Span::styled(" skip   ", fg),
      Span::styled("n", key),
      Span::styled(" keep newer   ", fg),
    ];
    options.extend(rename);
    lines.push(Line::from(options));
  }
  let remaining = prompt.remaining();
  if remaining > 0 {
    let check = if prompt.apply_all { "[x]" } else { "[ ]" };
    let what = if remaining == 1 {
      "the remaining conflict".to_string()
    } else {
      format!("the {remaining} remaining conflicts")
    };
    lines.push(Line::default());
    lines.push(Line::from(vec![
      Span::styled(check, fg),
      Span::styled(format!(" apply to {what} "), fg),
      Span::styled("(", dim),
      Span::styled("a", key),
      Span::styled(" toggles)", dim),
    ]));
  }
  // Short of room, the blank lines go first.
  let room = body.height.saturating_sub(4) as usize;
  if lines.len() > room {
    lines.retain(|l| !l.spans.is_empty());
  }

  let (done, total) = (prompt.answers.len() + 1, prompt.conflicts.len());
  let title = Line::from(Span::styled(
    format!(" {} · conflict {done} of {total} ", job.kind.name()),
    Style::new().fg(warn).add_modifier(Modifier::BOLD),
  ));
  let mut status = vec![Span::raw(" ")];
  status.extend(bar(0.0, 12, dim, Style::new().fg(theme.progress_track.0)));
  status.push(Span::styled("  waiting ", dim));
  let keys = [("X", "cancel task"), ("esc", "decide later")];
  let width = (body.width * 3 / 4).clamp(56, 96);
  let area = popup_area(buf, body, width, lines.len() as u16 + 4);
  let inner = frame(
    buf,
    area,
    warn,
    title,
    Line::from(status),
    hints(theme, &keys, warn),
    Padding::new(2, 2, 1, 1),
  );
  for (row, line) in lines.into_iter().enumerate().take(inner.height as usize) {
    line.render(
      Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
      buf,
    );
  }
}

/// Finished tasks, newest at the bottom, over the bottom right of `body`.
fn render_toasts(buf: &mut Buffer, body: Rect, app: &App) {
  let theme = &app.config.theme;
  let mut bottom = body.bottom().saturating_sub(1);
  for toast in app.jobs.toasts.iter().rev() {
    if bottom < body.y + 3 || body.width < 12 {
      break;
    }
    let (mark, color) = if toast.ok {
      ("✓", theme.success.0)
    } else {
      ("✗", theme.error.0)
    };
    let title = format!("{mark} {}", toast.title);
    let detail = format!(" · {}", toast.detail);
    let full = display_width(&title) + 1 + display_width(&toast.what) + display_width(&detail);
    let max = body.width.saturating_sub(6) as usize;
    let width = full.min(max);
    // The task's name gives way first.
    let what_w = width.saturating_sub(display_width(&title) + 1 + display_width(&detail));
    let line = Line::from(vec![
      Span::styled(title, Style::new().fg(color).add_modifier(Modifier::BOLD)),
      Span::raw(" "),
      Span::styled(truncate(&toast.what, what_w), Style::new().fg(theme.fg.0)),
      Span::styled(detail, Style::new().fg(theme.dim.0)),
    ]);
    let w = width as u16 + 4;
    let area = Rect::new(body.right() - w - 2, bottom - 3, w, 3);
    Clear.render(Rect::new(area.x - 1, area.y, area.width + 2, 3), buf);
    let block = Block::bordered()
      .border_style(Style::new().fg(color))
      .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    block.render(area, buf);
    line.render(inner, buf);
    bottom -= 3;
  }
}
