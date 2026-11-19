use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

use super::jobs;
use super::text::{clip, display_width, truncate};
use crate::app::{App, TabSummary, TabTask};
use crate::config::Theme;
use crate::fs::entry::sanitize;

/// `user@host /current/dir/selected`, like ranger's title bar: the directory part in the
/// path color (symlinked components in the symlink color), the selected entry's name in the
/// default color. When it does not fit, directory
/// names are cut down to their first character from the left (`/h/m/Downloads/...`).
pub fn render(buf: &mut Buffer, area: Rect, app: &App) {
  let theme = &app.config.theme;
  let user = Style::new().fg(theme.header_user.0).bold();
  let path = Style::new().fg(theme.header_path.0).bold();
  let link = Style::new().fg(theme.symlink.0).bold();
  let name = Style::new().fg(theme.fg.0).bold();

  let mut parts = vec![
    Part::fixed(app.user_host.clone(), user),
    Part::fixed(" ".into(), user),
  ];
  parts.extend(dir_parts(&app.cwd, &app.cwd_symlinks, path, link));
  if let Some(entry) = app.selected() {
    parts.push(Part::shrinkable(sanitize(&entry.name), name));
  }

  let tabs = tab_bar(app, area.width);
  for tab in &tabs {
    let x = area.x + tab.x;
    Line::from(tab.spans.clone()).render(Rect::new(x, area.y, tab.width, 1), buf);
  }
  // A cell between the path and the tabs.
  let tabs_width = tabs.first().map_or(0, |t| area.width - t.x + 1);
  let area = Rect {
    width: area.width.saturating_sub(tabs_width),
    ..area
  };

  let width = usize::from(area.width);
  let line = if shrink_from_left(&mut parts, width) {
    Line::from(
      parts
        .into_iter()
        .map(|p| Span::styled(p.text, p.style))
        .collect::<Vec<_>>(),
    )
  } else {
    // Even the fixed parts do not fit: cut the whole line.
    let text: String = parts.iter().map(|p| p.text.as_str()).collect();
    Line::styled(truncate(&text, width), user)
  };
  line.render(area, buf);
}

/// Cells the current tab's directory name takes at most.
const TAB_NAME_MAX: usize = 12;

/// A piece of the tab bar in the header: a tab's label, an overflow arrow or the position.
#[derive(Debug, Clone, PartialEq)]
pub struct TabLabel {
  /// The tab a click on it goes to.
  pub target: Option<usize>,
  /// Column of its first cell, from the header's left edge.
  pub x: u16,
  pub width: u16,
  pub spans: Vec<Span<'static>>,
}

impl TabLabel {
  pub fn text(&self) -> String {
    self.spans.iter().map(|s| s.content.as_ref()).collect()
  }
}

/// The tab bar of `app`'s header, `width` cells wide. Drawing and mouse input both use it.
pub fn tab_bar(app: &App, width: u16) -> Vec<TabLabel> {
  layout_tabs(
    &app.tab_summaries(),
    app.current_tab(),
    width,
    &app.config.theme,
  )
}

/// The tab labels at the right end of a header `width` cells wide, taking at most half of it:
/// ` 3 docs  4 site ⠋+4  5 src ⠋  62% `. Every tab shows its number, its directory's name and
/// its activity, the current one also its task's progress. Space runs out in steps: the other
/// tabs' names are cut shorter, then dropped (` 3  4⠋+4  5 src ⠋  62% `); then only the tabs
/// around the current one are shown, between arrows that carry the activity of the hidden
/// ones, followed by the current tab's position (`‹⠋ 4  5 src  6 ›+ 5/9`); then the current
/// label drops its percent, cuts its name shorter and drops it. Empty with a single tab, or
/// when even the current tab's number does not fit.
pub fn layout_tabs(
  tabs: &[TabSummary],
  current: usize,
  width: u16,
  theme: &Theme,
) -> Vec<TabLabel> {
  if tabs.len() < 2 {
    return Vec::new();
  }
  let Some(at) = tabs.iter().position(|t| t.number == current) else {
    return Vec::new();
  };
  DETAILS
    .iter()
    .find_map(|&detail| layout_window(tabs, at, width, theme, detail))
    .unwrap_or_default()
}

/// How much the tab bar shows. Names are cut to the given number of cells; 0 drops them.
#[derive(Debug, Clone, Copy)]
struct Detail {
  /// The other tabs' names.
  others_name: usize,
  /// The current tab's name.
  current_name: usize,
  /// The current tab's task percent.
  percent: bool,
  /// Whether some tabs may be hidden behind the arrows.
  overflow: bool,
}

impl Detail {
  const fn new(others_name: usize, current_name: usize, percent: bool, overflow: bool) -> Self {
    Detail {
      others_name,
      current_name,
      percent,
      overflow,
    }
  }
}

/// From everything shown down to the current tab's number alone.
const DETAILS: [Detail; 6] = [
  Detail::new(TAB_NAME_MAX, TAB_NAME_MAX, true, false),
  Detail::new(TAB_NAME_MAX / 2, TAB_NAME_MAX, true, false),
  Detail::new(0, TAB_NAME_MAX, true, true),
  Detail::new(0, TAB_NAME_MAX, false, true),
  Detail::new(0, TAB_NAME_MAX / 2, false, true),
  Detail::new(0, 0, false, true),
];

/// The tab bar with the current tab at index `at`, drawn with `detail`, or `None` when it
/// does not fit that way.
fn layout_window(
  tabs: &[TabSummary],
  at: usize,
  width: u16,
  theme: &Theme,
  detail: Detail,
) -> Option<Vec<TabLabel>> {
  let budget = usize::from(width) / 2;
  let labels: Vec<Vec<Span<'static>>> = tabs
    .iter()
    .enumerate()
    .map(|(i, t)| tab_spans(t, i == at, detail, theme))
    .collect();
  let widths: Vec<usize> = labels.iter().map(|l| spans_width(l)).collect();
  let dim = Style::new().fg(theme.dim.0);

  // The arrows and the position for a window of tabs `lo..hi`; nothing when all are shown.
  let chrome = |lo: usize, hi: usize| -> Option<[Vec<Span<'static>>; 3]> {
    if lo == 0 && hi == tabs.len() {
      return None;
    }
    let arrow = |glyph: &str, hidden: &[TabSummary]| {
      let mut spans = vec![Span::styled(
        if hidden.is_empty() { " " } else { glyph }.to_string(),
        dim,
      )];
      spans.extend(hidden_activity(hidden, theme));
      spans
    };
    Some([
      arrow("‹", &tabs[..lo]),
      arrow("›", &tabs[hi..]),
      vec![Span::styled(format!(" {}/{}", at + 1, tabs.len()), dim)],
    ])
  };
  let used = |lo: usize, hi: usize| -> usize {
    let chrome_width = chrome(lo, hi).map_or(0, |c| c.iter().map(|s| spans_width(s)).sum());
    widths[lo..hi].iter().sum::<usize>() + chrome_width
  };

  // Grow the window around the current tab, keeping it centred while both sides have more.
  let (mut lo, mut hi) = (at, at + 1);
  if used(lo, hi) > budget {
    return None;
  }
  loop {
    let right_first = hi - at - 1 <= at - lo;
    let grow_right = hi < tabs.len() && used(lo, hi + 1) <= budget;
    let grow_left = lo > 0 && used(lo - 1, hi) <= budget;
    match (grow_right, grow_left) {
      (true, true) if right_first => hi += 1,
      (true, true) => lo -= 1,
      (true, false) => hi += 1,
      (false, true) => lo -= 1,
      (false, false) => break,
    }
  }
  if !detail.overflow && (lo, hi) != (0, tabs.len()) {
    return None;
  }

  let mut pieces: Vec<(Option<usize>, Vec<Span<'static>>)> = Vec::new();
  let chrome = chrome(lo, hi);
  if let Some([left, ..]) = &chrome {
    pieces.push((lo.checked_sub(1).map(|i| tabs[i].number), left.clone()));
  }
  for i in lo..hi {
    pieces.push((Some(tabs[i].number), labels[i].clone()));
  }
  if let Some([_, right, position]) = chrome {
    pieces.push((tabs.get(hi).map(|t| t.number), right));
    pieces.push((None, position));
  }
  let mut x = width - used(lo, hi) as u16;
  let labels = pieces
    .into_iter()
    .map(|(target, spans)| {
      let w = spans_width(&spans) as u16;
      let label = TabLabel {
        target,
        x,
        width: w,
        spans,
      };
      x += w;
      label
    })
    .collect();
  Some(labels)
}

fn spans_width(spans: &[Span]) -> usize {
  spans.iter().map(|s| display_width(&s.content)).sum()
}

/// The glyph for a tab's task: `!` waiting for an answer, `‖` paused, a spinner running.
fn task_glyph(task: TabTask, theme: &Theme) -> (String, Style) {
  match task {
    TabTask::Waiting => ("!".into(), Style::new().fg(theme.warning.0).bold()),
    TabTask::Paused => ("‖".into(), Style::new().fg(theme.dim.0)),
    TabTask::Running { since, .. } => (
      jobs::spinner(since).to_string(),
      Style::new().fg(theme.progress.0),
    ),
  }
}

/// A tab's label as `detail` has it: ` 4 site ⠋+3 ` or ` 4⠋+3 ` for another tab,
/// ` 5 src ⠋  62% ` (highlighted) for the current one.
fn tab_spans(tab: &TabSummary, current: bool, detail: Detail, theme: &Theme) -> Vec<Span<'static>> {
  if current {
    let style = Style::new().fg(theme.header_user.0).bold().reversed();
    let mut text = format!(" {}", tab.number);
    if detail.current_name > 0 {
      text.push(' ');
      text.push_str(&truncate(&tab.name, detail.current_name));
    }
    if let Some(task) = tab.task {
      text.push(' ');
      text.push_str(&task_glyph(task, theme).0);
      if let (
        true,
        TabTask::Running {
          fraction: Some(f), ..
        },
      ) = (detail.percent, task)
      {
        // Fixed width, so the labels do not move as it counts up.
        text.push(' ');
        text.push_str(&jobs::percent(f));
      }
    }
    text.push(' ');
    return vec![Span::styled(text, style)];
  }
  let mut spans = vec![Span::styled(
    format!(" {}", tab.number),
    Style::new().fg(theme.dim.0),
  )];
  if detail.others_name > 0 {
    spans.push(Span::styled(
      format!(" {}", truncate(&tab.name, detail.others_name)),
      Style::new().fg(theme.fg.0),
    ));
    if tab.task.is_some() || tab.marks > 0 {
      spans.push(Span::raw(" "));
    }
  }
  if let Some(task) = tab.task {
    let (glyph, style) = task_glyph(task, theme);
    spans.push(Span::styled(glyph, style));
  }
  if tab.marks > 0 {
    spans.push(Span::styled(
      format!("+{}", tab.marks),
      Style::new().fg(theme.tab_marks.0),
    ));
  }
  spans.push(Span::raw(" "));
  spans
}

/// What an overflow arrow carries for the tabs hidden on its side: the most pressing task
/// glyph among them, and `+` when any has marked entries.
fn hidden_activity(hidden: &[TabSummary], theme: &Theme) -> Vec<Span<'static>> {
  let rank = |task: &TabTask| match task {
    TabTask::Waiting => 0,
    TabTask::Running { .. } => 1,
    TabTask::Paused => 2,
  };
  let mut spans = Vec::new();
  if let Some(task) = hidden.iter().filter_map(|t| t.task).min_by_key(rank) {
    let (glyph, style) = task_glyph(task, theme);
    spans.push(Span::styled(glyph, style));
  }
  if hidden.iter().any(|t| t.marks > 0) {
    spans.push(Span::styled("+", Style::new().fg(theme.tab_marks.0)));
  }
  spans
}

struct Part {
  text: String,
  style: Style,
  /// Fixed parts (`user@host`, separators) are never shortened.
  fixed: bool,
}

impl Part {
  fn fixed(text: String, style: Style) -> Self {
    Part {
      text,
      style,
      fixed: true,
    }
  }

  fn shrinkable(text: String, style: Style) -> Self {
    Part {
      text,
      style,
      fixed: false,
    }
  }

  /// Width a shrinkable part may be cut down to: its first character.
  fn min_width(&self) -> usize {
    self
      .text
      .chars()
      .next()
      .map_or(0, |c| UnicodeWidthChar::width(c).unwrap_or(0))
  }
}

/// `/`, then every directory name followed by a fixed `/`, so cut names keep their separators.
/// Components listed in `symlinks` (as paths up to and including them) get `link_style`.
fn dir_parts(
  dir: &Path,
  symlinks: &HashSet<PathBuf>,
  style: Style,
  link_style: Style,
) -> Vec<Part> {
  let mut parts = Vec::new();
  let mut prefix = PathBuf::new();
  for component in dir.components() {
    prefix.push(component);
    let text = sanitize(&component.as_os_str().to_string_lossy());
    match component {
      Component::RootDir => parts.push(Part::fixed("/".into(), style)),
      Component::Prefix(_) => parts.push(Part::fixed(text, style)),
      _ => {
        let style = if symlinks.contains(&prefix) {
          link_style
        } else {
          style
        };
        parts.push(Part::shrinkable(text, style));
        parts.push(Part::fixed("/".into(), style));
      }
    }
  }
  parts
}

/// Shortens shrinkable parts left to right, each down to at most its first character, until
/// the line fits in `width`. Returns false if it cannot fit even then.
fn shrink_from_left(parts: &mut [Part], width: usize) -> bool {
  let total: usize = parts.iter().map(|p| display_width(&p.text)).sum();
  let mut over = total.saturating_sub(width);
  for part in parts.iter_mut().filter(|p| !p.fixed) {
    if over == 0 {
      break;
    }
    let len = display_width(&part.text);
    let target = len.saturating_sub(over).max(part.min_width());
    // `clip` never splits a wide character, so it may free one cell more than asked.
    part.text = clip(&part.text, target).to_string();
    over = over.saturating_sub(len - display_width(&part.text));
  }
  over == 0
}

#[cfg(test)]
mod tests {
  use super::*;

  fn render_to_string(dir: &str, name: Option<&str>, width: usize) -> String {
    let mut parts = vec![
      Part::fixed("u@h".into(), Style::new()),
      Part::fixed(" ".into(), Style::new()),
    ];
    parts.extend(dir_parts(
      Path::new(dir),
      &HashSet::new(),
      Style::new(),
      Style::new(),
    ));
    if let Some(name) = name {
      parts.push(Part::shrinkable(name.into(), Style::new()));
    }
    if shrink_from_left(&mut parts, width) {
      parts.iter().map(|p| p.text.as_str()).collect()
    } else {
      String::from("<overflow>")
    }
  }

  #[test]
  fn fits_unchanged() {
    assert_eq!(
      render_to_string("/home/me", Some("a.rs"), 80),
      "u@h /home/me/a.rs"
    );
    assert_eq!(render_to_string("/", Some("etc"), 80), "u@h /etc");
    assert_eq!(render_to_string("/", None, 80), "u@h /");
  }

  #[test]
  fn shrinks_leading_directories_first() {
    // Full: "u@h /home/moskas/Downloads/src/rofi.rs" (38 cells).
    let full = "/home/moskas/Downloads/src";
    assert_eq!(
      render_to_string(full, Some("rofi.rs"), 34),
      "u@h /h/moska/Downloads/src/rofi.rs"
    );
    assert_eq!(
      render_to_string(full, Some("rofi.rs"), 30),
      "u@h /h/m/Downloads/src/rofi.rs"
    );
    // The cut stops partway through a name when that is enough.
    assert_eq!(
      render_to_string(full, Some("rofi.rs"), 27),
      "u@h /h/m/Downlo/src/rofi.rs"
    );
  }

  #[test]
  fn shrinks_selected_name_last() {
    assert_eq!(
      render_to_string("/home/me", Some("long-name.rs"), 14),
      "u@h /h/m/long-"
    );
  }

  #[test]
  fn reports_overflow_when_fixed_parts_do_not_fit() {
    assert_eq!(render_to_string("/home/me", Some("x"), 8), "<overflow>");
  }

  fn tab(number: usize) -> TabSummary {
    TabSummary {
      number,
      name: format!("dir{number}"),
      task: None,
      marks: 0,
    }
  }

  /// The tab bar as `(target, x, text)`.
  fn bar(tabs: &[TabSummary], current: usize, width: u16) -> Vec<(Option<usize>, u16, String)> {
    layout_tabs(tabs, current, width, &Theme::default())
      .iter()
      .map(|l| (l.target, l.x, l.text()))
      .collect()
  }

  #[test]
  fn tab_labels_sit_at_the_right_end() {
    assert_eq!(bar(&[tab(1)], 1, 40), []);
    let tabs = [tab(1), tab(2), tab(5)];
    assert_eq!(
      bar(&tabs, 2, 40),
      [
        (Some(1), 26, " 1 ".into()),
        (Some(2), 29, " 2 dir2 ".into()),
        (Some(5), 37, " 5 ".into()),
      ]
    );
    // Too narrow for even the current tab's number to leave room for the path.
    assert_eq!(bar(&tabs, 2, 15), []);
  }

  #[test]
  fn other_tabs_names_go_first_when_space_runs_out() {
    let tabs = [
      TabSummary {
        name: "Downloads".into(),
        marks: 4,
        ..tab(1)
      },
      tab(2),
      tab(3),
    ];
    let texts = |width| -> Vec<String> { bar(&tabs, 3, width).into_iter().map(|l| l.2).collect() };
    assert_eq!(texts(80), [" 1 Downloads +4 ", " 2 dir2 ", " 3 dir3 "]);
    // 32 cells needed, 30 at hand: the names are cut.
    assert_eq!(texts(60), [" 1 Downl~ +4 ", " 2 dir2 ", " 3 dir3 "]);
    // Then dropped, while the current tab keeps its own.
    assert_eq!(texts(50), [" 1+4 ", " 2 ", " 3 dir3 "]);
  }

  #[test]
  fn tab_labels_show_activity() {
    let since = std::time::Instant::now();
    let running = TabTask::Running {
      since,
      fraction: Some(0.62),
    };
    let spin = jobs::spinner(since).to_string();
    let tabs = [
      TabSummary { marks: 4, ..tab(1) },
      TabSummary {
        task: Some(TabTask::Waiting),
        marks: 2,
        ..tab(2)
      },
      TabSummary {
        task: Some(running),
        ..tab(3)
      },
    ];
    let texts: Vec<String> = bar(&tabs, 3, 80).into_iter().map(|l| l.2).collect();
    assert_eq!(
      texts,
      [
        " 1 dir1 +4 ",
        " 2 dir2 !+2 ",
        &format!(" 3 dir3 {spin}  62% ")
      ]
    );
    // Long names are cut.
    let long = [
      tab(1),
      TabSummary {
        name: "a-very-long-directory".into(),
        ..tab(2)
      },
    ];
    assert_eq!(bar(&long, 2, 80)[1].2, " 2 a-very-long~ ");
  }

  #[test]
  fn overflowing_tabs_centre_on_the_current_one() {
    let since = std::time::Instant::now();
    let spin = jobs::spinner(since).to_string();
    let mut tabs: Vec<TabSummary> = (1..=9).map(tab).collect();
    tabs[0].task = Some(TabTask::Running {
      since,
      fraction: None,
    });
    tabs[8].marks = 1;
    // All nine take 32 cells; 44 leaves room for 22.
    let labels = bar(&tabs, 5, 44);
    let texts: Vec<&str> = labels.iter().map(|l| l.2.as_str()).collect();
    assert_eq!(
      texts,
      [&format!("‹{spin}"), " 4 ", " 5 dir5 ", " 6 ", "›+", " 5/9"]
    );
    let targets: Vec<Option<usize>> = labels.iter().map(|l| l.0).collect();
    assert_eq!(targets, [Some(3), Some(4), Some(5), Some(6), Some(7), None]);
    assert_eq!(labels.last().map(|l| l.1 + 4), Some(44));
    // At the first tab, nothing is hidden on the left: the arrow is a blank.
    let texts: Vec<String> = bar(&tabs, 1, 44).into_iter().map(|l| l.2).collect();
    assert_eq!(texts[0], " ");
    assert_eq!(texts[1], format!(" 1 dir1 {spin} "));
    assert_eq!(texts.last().map(String::as_str), Some(" 1/9"));
  }

  #[test]
  fn a_tight_current_label_drops_detail_before_the_bar_goes() {
    let since = std::time::Instant::now();
    let spin = jobs::spinner(since).to_string();
    let mut tabs: Vec<TabSummary> = (1..=9).map(tab).collect();
    tabs[0].task = Some(TabTask::Running {
      since,
      fraction: None,
    });
    tabs[8].marks = 1;
    tabs[4].name = "a-very-long-directory".into();
    tabs[4].task = Some(TabTask::Running {
      since,
      fraction: Some(0.62),
    });
    // With the percent, the label and the arrows take 31 cells of the 30 that 60 leaves.
    let texts: Vec<String> = bar(&tabs, 5, 60).into_iter().map(|l| l.2).collect();
    assert_eq!(
      texts,
      [
        format!("‹{spin}"),
        format!(" 5 a-very-long~ {spin} "),
        " 6 ".into(),
        "›+".into(),
        " 5/9".into(),
      ]
    );
    // Narrower still, the name goes too.
    let texts: Vec<String> = bar(&tabs, 5, 26).into_iter().map(|l| l.2).collect();
    assert_eq!(texts[1], format!(" 5 {spin} "));
  }

  #[test]
  fn wide_characters_are_not_split() {
    // "日本" is 4 cells; it can only shrink to its first character (2 cells).
    assert_eq!(render_to_string("/日本/x", Some("f"), 11), "u@h /日/x/f");
  }

  #[test]
  fn symlinked_components_use_link_style() {
    let link = Style::new().italic();
    let symlinks = HashSet::from([PathBuf::from("/home/me")]);
    let parts = dir_parts(Path::new("/home/me/src"), &symlinks, Style::new(), link);
    let styled: Vec<(&str, bool)> = parts
      .iter()
      .map(|p| (p.text.as_str(), p.style == link))
      .collect();
    assert_eq!(
      styled,
      [
        ("/", false),
        ("home", false),
        ("/", false),
        ("me", true),
        ("/", true),
        ("src", false),
        ("/", false),
      ]
    );
  }
}
