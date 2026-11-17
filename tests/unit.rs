//! Tests for pure building blocks: sorting, navigation, truncation, keymap, formatting, config.

use std::cmp::Ordering;

use crossterm::event::{KeyCode, KeyModifiers};

use tansu::action::Action;
use tansu::config::Config;
use tansu::console::{ConsoleCommand, ConsoleError, EditOutcome, LineEditor, parse};
use tansu::fs::meta::{format_permissions, human_size, human_size_compact};
use tansu::fs::sort::natural_cmp;
use tansu::keymap::{Key, KeyParseError, KeyResult, Keymap, parse_sequence};
use tansu::nav::{Cursor, ScrollPos, passive_offset};
use tansu::opener::run_foreground;
use tansu::ui::layout::split_columns;
use tansu::ui::status::console_view;
use tansu::ui::text::{display_width, truncate};

// ---- sorting ----

#[test]
fn natural_order_compares_digit_runs_numerically() {
  let mut names = vec![
    "file10", "file2", "File1", "file02", "file1b", "a", "B", "c",
  ];
  names.sort_by(|a, b| natural_cmp(a, b));
  assert_eq!(
    names,
    vec![
      "a", "B", "c", "File1", "file1b", "file2", "file02", "file10"
    ]
  );
}

#[test]
fn natural_order_is_case_insensitive_with_stable_tiebreak() {
  assert_eq!(natural_cmp("abc", "ABD"), Ordering::Less);
  assert_ne!(natural_cmp("abc", "ABC"), Ordering::Equal);
  assert_eq!(natural_cmp("abc", "abc"), Ordering::Equal);
  assert_eq!(natural_cmp("x9", "x10"), Ordering::Less);
  assert_eq!(
    natural_cmp("x99999999999999999999999", "x100000000000000000000000"),
    Ordering::Less
  );
}

// ---- navigation ----

#[test]
fn cursor_scrolls_to_keep_selection_visible() {
  let mut c = Cursor::default();
  c.move_by(1, 10, 4, 0);
  assert_eq!(
    c,
    Cursor {
      index: 1,
      offset: 0
    }
  );
  c.set(6, 10, 4, 0);
  assert_eq!(
    c,
    Cursor {
      index: 6,
      offset: 3
    }
  );
  c.move_by(-5, 10, 4, 0);
  assert_eq!(
    c,
    Cursor {
      index: 1,
      offset: 1
    }
  );
  c.move_by(-100, 10, 4, 0);
  assert_eq!(
    c,
    Cursor {
      index: 0,
      offset: 0
    }
  );
  c.move_by(100, 10, 4, 0);
  assert_eq!(
    c,
    Cursor {
      index: 9,
      offset: 6
    }
  );
}

#[test]
fn cursor_clamps_when_list_shrinks_or_viewport_grows() {
  let mut c = Cursor {
    index: 9,
    offset: 6,
  };
  c.fix_offset(5, 4, 0);
  assert_eq!(
    c,
    Cursor {
      index: 4,
      offset: 1
    }
  );
  c.fix_offset(5, 10, 0);
  assert_eq!(
    c,
    Cursor {
      index: 4,
      offset: 0
    }
  );
  c.set(3, 0, 10, 0);
  assert_eq!(c, Cursor::default());
}

#[test]
fn scroll_position_labels() {
  assert_eq!(ScrollPos::compute(0, 3, 10).to_string(), "All");
  assert_eq!(ScrollPos::compute(0, 30, 10).to_string(), "Top");
  assert_eq!(ScrollPos::compute(20, 30, 10).to_string(), "Bot");
  assert_eq!(ScrollPos::compute(10, 30, 10).to_string(), "50%");
}

#[test]
fn passive_offset_centers_selection() {
  assert_eq!(passive_offset(2, 5, 10), 0);
  assert_eq!(passive_offset(15, 40, 10), 10);
  assert_eq!(passive_offset(39, 40, 10), 30);
}

// ---- truncation ----

#[test]
fn truncate_short_names_untouched() {
  assert_eq!(truncate("hello", 5), "hello");
  assert_eq!(truncate("hello", 10), "hello");
}

#[test]
fn truncate_ascii_adds_tilde() {
  assert_eq!(truncate("Downloads", 8), "Downloa~");
  assert_eq!(truncate("abc", 1), "~");
  assert_eq!(truncate("abc", 0), "");
}

#[test]
fn truncate_measures_display_width_not_bytes() {
  // Each CJK character is two cells wide and three bytes long.
  let name = "好きになったのは";
  assert_eq!(display_width(name), 16);
  let t = truncate(name, 9);
  assert_eq!(t, "好きにな~");
  assert_eq!(display_width(&t), 9);
  // A wide char that would straddle the limit is dropped instead of split.
  let t = truncate(name, 8);
  assert_eq!(t, "好きに~");
  assert_eq!(display_width(&t), 7);
  assert!(display_width(&truncate("a好b", 3)) <= 3);
}

#[test]
fn truncate_mixed_width() {
  let t = truncate("[M3-58] 猫又 - Remix.flac", 12);
  assert_eq!(t, "[M3-58] 猫~");
  assert!(display_width(&t) <= 12);
}

// ---- keymap ----

fn ch(c: char) -> Key {
  Key::plain(KeyCode::Char(c))
}

#[test]
fn parse_plain_special_and_modified_keys() {
  assert_eq!(parse_sequence("j").unwrap(), vec![ch('j')]);
  assert_eq!(parse_sequence("gg").unwrap(), vec![ch('g'), ch('g')]);
  assert_eq!(
    parse_sequence("<C-d>").unwrap(),
    vec![Key::new(KeyCode::Char('d'), KeyModifiers::CONTROL)]
  );
  assert_eq!(
    parse_sequence("<Enter>").unwrap(),
    vec![Key::plain(KeyCode::Enter)]
  );
  assert_eq!(
    parse_sequence("<pagedown>").unwrap(),
    vec![Key::plain(KeyCode::PageDown)]
  );
  assert_eq!(parse_sequence("<S-a>").unwrap(), vec![ch('A')]);
  assert_eq!(parse_sequence("<lt>").unwrap(), vec![ch('<')]);
  assert_eq!(parse_sequence("<").unwrap(), vec![ch('<')]);
  assert_eq!(
    parse_sequence("<F5>").unwrap(),
    vec![Key::plain(KeyCode::F(5))]
  );
  assert_eq!(
    parse_sequence("<A-x>").unwrap(),
    vec![Key::new(KeyCode::Char('x'), KeyModifiers::ALT)]
  );
}

#[test]
fn parse_rejects_bad_notation() {
  assert_eq!(parse_sequence(""), Err(KeyParseError::Empty));
  assert!(matches!(
    parse_sequence("<C-d"),
    Err(KeyParseError::Unterminated(_))
  ));
  assert!(matches!(
    parse_sequence("<Nope>"),
    Err(KeyParseError::UnknownKey(_))
  ));
  assert!(matches!(
    parse_sequence("<X-a>"),
    Err(KeyParseError::UnknownModifier(_))
  ));
}

#[test]
fn notation_round_trips() {
  for s in [
    "j", "gg", "<C-d>", "<Enter>", "<Down>", "G", "<lt>", "<A-x>",
  ] {
    let seq = parse_sequence(s).unwrap();
    assert_eq!(tansu::keymap::format_sequence(&seq), s);
  }
}

#[test]
fn shifted_letters_match_uppercase_bindings() {
  let mut km = Keymap::default();
  let shifted_g = Key::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
  assert_eq!(km.feed(shifted_g), KeyResult::Action(Action::MoveBottom));
}

#[test]
fn multi_key_sequences_resolve() {
  let mut km = Keymap::default();
  assert_eq!(km.feed(ch('g')), KeyResult::Pending);
  assert_eq!(km.feed(ch('g')), KeyResult::Action(Action::MoveTop));
  // A broken sequence falls back to the last key on its own.
  assert_eq!(km.feed(ch('g')), KeyResult::Pending);
  assert_eq!(km.feed(ch('j')), KeyResult::Action(Action::MoveDown));
  assert!(!km.is_pending());
  assert_eq!(km.feed(ch('z')), KeyResult::Unbound);
}

#[test]
fn continuations_list_the_keys_after_a_prefix() {
  use tansu::fs::SortKey;
  use tansu::keymap::Continuation;
  let mut km = Keymap::default();
  km.feed(ch('o'));
  assert_eq!(km.pending_notation(), "o");
  assert_eq!(
    km.continuations(),
    vec![
      (
        "c".into(),
        Continuation::Action(Action::Sort(SortKey::Ctime))
      ),
      (
        "m".into(),
        Continuation::Action(Action::Sort(SortKey::Mtime))
      ),
      (
        "n".into(),
        Continuation::Action(Action::Sort(SortKey::Name))
      ),
      (
        "s".into(),
        Continuation::Action(Action::Sort(SortKey::Size))
      ),
    ]
  );
  // Longer sequences show as a group under their next key.
  let overrides: Vec<(String, String)> = vec![
    ("gxa".into(), "quit".into()),
    ("gxb".into(), "reload".into()),
  ];
  let (mut km, warnings) = Keymap::with_overrides(overrides.iter().map(|(k, a)| (k, a)));
  assert!(warnings.is_empty());
  km.feed(ch('g'));
  let next = km.continuations();
  assert!(next.contains(&("g".into(), Continuation::Action(Action::MoveTop))));
  assert!(next.contains(&("x".into(), Continuation::Group(2))));
}

#[test]
fn config_overrides_add_replace_and_unbind() {
  let overrides: Vec<(String, String)> = vec![
    ("J".into(), "half_page_down".into()),
    ("q".into(), "none".into()),
    ("<C-n>".into(), "move_down".into()),
    ("x".into(), "explode".into()),
    ("<Bad>".into(), "quit".into()),
  ];
  let (km, warnings) = Keymap::with_overrides(overrides.iter().map(|(k, v)| (k, v)));
  assert_eq!(km.lookup(&[ch('J')]), Some(Action::HalfPageDown));
  assert_eq!(km.lookup(&[ch('q')]), None);
  assert_eq!(
    km.lookup(&parse_sequence("<C-n>").unwrap()),
    Some(Action::MoveDown)
  );
  assert_eq!(km.lookup(&[ch('j')]), Some(Action::MoveDown));
  assert_eq!(warnings.len(), 2, "{warnings:?}");
}

#[test]
fn every_action_has_a_default_binding_and_round_trips_by_name() {
  let km = Keymap::default();
  for a in Action::ALL {
    assert!(!km.keys_for(a).is_empty(), "{a} unbound");
    assert_eq!(a.name().parse::<Action>().unwrap(), a);
  }
}

// ---- formatting ----

#[test]
fn permission_strings() {
  assert_eq!(format_permissions(0o040755), "drwxr-xr-x");
  assert_eq!(format_permissions(0o100644), "-rw-r--r--");
  assert_eq!(format_permissions(0o120777), "lrwxrwxrwx");
  assert_eq!(format_permissions(0o104755), "-rwsr-xr-x");
  assert_eq!(format_permissions(0o041777), "drwxrwxrwt");
  assert_eq!(format_permissions(0o100600 | 0o1000), "-rw------T");
}

#[test]
fn human_sizes_match_ranger() {
  // Cases from ranger's own doctests (ranger/ext/human_readable.py).
  assert_eq!(human_size(54, false), "54 B");
  assert_eq!(human_size(1500, false), "1.5 k");
  assert_eq!(human_size(1023 << 20, false), "1.07 G");
  assert_eq!(human_size(54, true), "54 B");
  assert_eq!(human_size(1500, true), "1.46 Ki");
  assert_eq!(human_size(1023 << 20, true), "1023 Mi");
  // Values seen in ranger next to the same files.
  assert_eq!(human_size(48_026, false), "48 k");
  assert_eq!(human_size(407, false), "407 B");
  assert_eq!(human_size(7_210, false), "7.21 k");
  assert_eq!(human_size(0, false), "0");
  assert_eq!(human_size_compact(67_700, false), "67.7k");
  assert_eq!(human_size_compact(95_400_000_000, false), "95.4G");
  assert_eq!(human_size_compact(0, false), "0");
}

#[test]
fn human_size_rounding_carries_to_next_unit() {
  // `%.3g` would print `1e+03 k` here; tansu moves to the next unit instead.
  assert_eq!(human_size(999_600, false), "1 M");
  assert_eq!(human_size(999_400, false), "999 k");
  assert_eq!(human_size(9_996, false), "10 k");
}

// ---- layout ----

#[test]
fn columns_follow_ratios() {
  let inner = ratatui::layout::Rect::new(1, 2, 58, 10);
  let (cols, seps) = split_columns(inner, [1, 3, 4]);
  let widths: Vec<u16> = cols.iter().map(|c| c.unwrap().width).collect();
  assert_eq!(widths, vec![7, 21, 28]);
  assert_eq!(seps, vec![8, 30]);
  let (cols, seps) = split_columns(inner, [0, 1, 1]);
  assert!(cols[0].is_none());
  assert_eq!(seps.len(), 1);
}

// ---- config ----

#[test]
fn sort_key_parses_from_config() {
  use tansu::fs::SortKey;
  assert_eq!(Config::default().sort, SortKey::Name);
  for (text, key) in [
    ("name", SortKey::Name),
    ("size", SortKey::Size),
    ("mtime", SortKey::Mtime),
    ("ctime", SortKey::Ctime),
  ] {
    let cfg = Config::parse(&format!("sort = \"{text}\"")).unwrap();
    assert_eq!(cfg.sort, key);
  }
  assert!(Config::parse("sort = \"atime\"").is_err());
}

#[test]
fn empty_config_is_default() {
  assert_eq!(Config::parse("").unwrap(), Config::default());
}

#[test]
fn partial_config_overrides_only_given_fields() {
  let cfg = Config::parse(
    r##"
      show_hidden = true
      icons = false
      opener = "mimeo --quiet"
      column_ratios = [1, 2, 2]

      [preview]
      max_lines = 50

      [theme]
      cursor_bg = "#ff0000"
      directory = "lightblue"

      [keys]
      "J" = "half_page_down"
    "##,
  )
  .unwrap();
  assert!(cfg.show_hidden);
  assert!(!cfg.icons);
  assert_eq!(cfg.opener, "mimeo --quiet");
  assert_eq!(cfg.column_ratios, [1, 2, 2]);
  assert_eq!(cfg.preview.max_lines, 50);
  assert_eq!(
    cfg.preview.max_bytes,
    tansu::preview::PreviewLimits::default().max_bytes
  );
  assert_eq!(
    cfg.theme.cursor_bg.map(|c| c.0),
    Some(ratatui::style::Color::Rgb(255, 0, 0))
  );
  assert_eq!(cfg.theme.cursor_fg, None);
  assert_eq!(cfg.theme.directory.0, ratatui::style::Color::LightBlue);
  assert_eq!(cfg.theme.fg, tansu::config::Theme::default().fg);
  assert_eq!(
    cfg.keys.get("J").map(String::as_str),
    Some("half_page_down")
  );
}

#[test]
fn invalid_config_is_rejected_and_load_falls_back() {
  assert!(Config::parse("bogus_key = 1").is_err());
  assert!(Config::parse("[theme]\nfg = \"notacolor\"").is_err());

  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("config.toml");
  std::fs::write(&path, "column_ratios = \"wide\"").unwrap();
  let (cfg, warnings) = Config::load(Some(&path));
  assert_eq!(cfg, Config::default());
  assert_eq!(warnings.len(), 1);

  let (cfg, warnings) = Config::load(Some(&dir.path().join("missing.toml")));
  assert_eq!(cfg, Config::default());
  assert!(warnings.is_empty());
}

#[test]
fn validation_repairs_bad_values() {
  let mut cfg = Config::parse("column_ratios = [1, 0, 1]\nopener = \"  \"").unwrap();
  let warnings = cfg.validate();
  assert_eq!(warnings.len(), 2);
  assert_eq!(cfg.column_ratios, [1, 3, 4]);
  assert_eq!(cfg.opener, "xdg-open");
}

#[test]
fn readme_sample_config_parses_without_warnings() {
  let readme = include_str!("../README.md");
  let start = readme.find("```toml\n").expect("toml block") + "```toml\n".len();
  let len = readme[start..].find("```").unwrap();
  let mut cfg = Config::parse(&readme[start..start + len]).expect("sample config parses");
  assert!(cfg.validate().is_empty());
  assert_eq!(
    cfg.theme,
    tansu::config::Theme::default(),
    "README lists the defaults"
  );
  let (_, warnings) = Keymap::with_overrides(&cfg.keys);
  assert!(warnings.is_empty(), "{warnings:?}");
}

// ---- vcs ----

#[test]
fn porcelain_v2_parsing() {
  use std::path::Path;
  use tansu::vcs::{FileStatus as F, RemoteStatus, parse_status};

  let root = Path::new("/r");
  let out: &[u8] = b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0\
# branch.ab +1 -2\0\
1 .M N... 100644 100644 100644 aaa bbb src/lib.rs\0\
1 M. N... 100644 100644 100644 aaa bbb docs/a b.md\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new name.rs\0old name.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.txt\0\
1 .D N... 100644 100644 000000 aaa aaa gone.rs\0\
? new/\0! target/\0? notes.txt\0";
  let s = parse_status(root, out);
  assert_eq!(s.remote, RemoteStatus::Diverged);
  assert_eq!(s.branch.as_deref(), Some("main"));
  assert_eq!(s.summary, F::Conflict);
  let st = |p: &str, dir| s.status_of(&root.join(p), dir);
  assert_eq!(st("src/lib.rs", false), F::Changed);
  assert_eq!(st("src", true), F::Changed);
  assert_eq!(
    st("docs/a b.md", false),
    F::Staged,
    "paths may contain spaces"
  );
  assert_eq!(st("new name.rs", false), F::Staged);
  assert_eq!(
    st("old name.rs", false),
    F::Sync,
    "rename source is not a record"
  );
  assert_eq!(st("conflict.txt", false), F::Conflict);
  assert_eq!(st("gone.rs", false), F::Deleted);
  assert_eq!(st("new", true), F::Untracked);
  assert_eq!(
    st("new/deep/file", false),
    F::Untracked,
    "inherits untracked dir"
  );
  assert_eq!(st("target", true), F::Ignored);
  assert_eq!(st("target/debug", true), F::Ignored);
  assert_eq!(st("README.md", false), F::Sync);
  assert_eq!(st("", true), F::Conflict, "root shows summary");
}

#[test]
fn ignored_contents_do_not_mark_directories() {
  use std::path::Path;
  use tansu::vcs::{FileStatus as F, RemoteStatus, parse_status};

  let root = Path::new("/r");
  let s = parse_status(root, b"# branch.head main\0! build/out.o\0");
  assert_eq!(s.remote, RemoteStatus::None, "no upstream");
  assert_eq!(s.branch.as_deref(), Some("main"));
  let detached = parse_status(root, b"# branch.oid abc\0# branch.head (detached)\0");
  assert_eq!(detached.branch, None);
  assert_eq!(s.summary, F::Sync);
  assert_eq!(s.status_of(&root.join("build"), true), F::Sync);
  assert_eq!(s.status_of(&root.join("build/out.o"), false), F::Ignored);
}

#[test]
fn xy_codes_follow_ranger_table() {
  use tansu::vcs::FileStatus as F;
  assert_eq!(F::from_xy(b'M', b'.'), F::Staged);
  assert_eq!(F::from_xy(b'A', b'.'), F::Staged);
  assert_eq!(F::from_xy(b'.', b'M'), F::Changed);
  assert_eq!(F::from_xy(b'M', b'M'), F::Changed);
  assert_eq!(F::from_xy(b'.', b'D'), F::Deleted);
  assert_eq!(F::from_xy(b'U', b'U'), F::Conflict);
  assert_eq!(F::from_xy(b'A', b'A'), F::Conflict);
  assert_eq!(F::from_xy(b'.', b'T'), F::Unknown);
}

#[test]
fn head_commit_parsing() {
  use tansu::vcs::parse_head;
  let c = parse_head(b"1763290000\0Fix the \x1b[31mthing\n").unwrap();
  assert_eq!(c.time, 1_763_290_000);
  assert_eq!(
    c.summary, "Fix the ?[31mthing",
    "control characters are neutralized"
  );
  assert!(parse_head(b"").is_none());
}

// ---- ANSI parsing ----

use tansu::preview::ansi::{SpanStyle, StyledSpan, TermColor, parse as parse_ansi};

fn span(text: &str, style: SpanStyle) -> StyledSpan {
  StyledSpan {
    style,
    text: text.to_string(),
  }
}

#[test]
fn ansi_colors_and_attributes_become_spans() {
  let lines = parse_ansi("\x1b[1;31mfn\x1b[0m main\x1b[38;5;208m(\x1b[38;2;1;2;3m)\x1b[39m\n");
  let red_bold = SpanStyle {
    fg: TermColor::Indexed(1),
    bold: true,
    ..SpanStyle::default()
  };
  let indexed = SpanStyle {
    fg: TermColor::Indexed(208),
    ..SpanStyle::default()
  };
  let rgb = SpanStyle {
    fg: TermColor::Rgb(1, 2, 3),
    ..SpanStyle::default()
  };
  assert_eq!(
    lines,
    vec![vec![
      span("fn", red_bold),
      span(" main", SpanStyle::default()),
      span("(", indexed),
      span(")", rgb),
    ]]
  );
}

#[test]
fn ansi_styles_carry_across_lines_and_bright_colors_map_high() {
  let lines = parse_ansi("\x1b[92ma\nb\x1b[0m\nc");
  let green = SpanStyle {
    fg: TermColor::Indexed(10),
    ..SpanStyle::default()
  };
  assert_eq!(
    lines,
    vec![
      vec![span("a", green)],
      vec![span("b", green)],
      vec![span("c", SpanStyle::default())],
    ]
  );
}

#[test]
fn non_sgr_escapes_and_control_characters_are_neutralized() {
  // Cursor movement, a screen clear, an OSC title and a bell must not reach the terminal.
  let lines = parse_ansi("a\x1b[2J\x1b[Hb\x1b]0;title\x07c\x1b]8;;x\x1b\\d\x07\te\r");
  let text: String = lines[0].iter().map(|s| s.text.as_str()).collect();
  assert_eq!(text, "abcd?   e");
  assert!(!text.contains('\x1b'));
}

#[test]
fn cursor_keeps_scroll_margin_like_ranger() {
  // 20 rows with a margin of 3: the view starts moving once the cursor passes row 16.
  let mut c = Cursor::default();
  c.set(16, 100, 20, 3);
  assert_eq!(c.offset, 0);
  c.move_by(1, 100, 20, 3);
  assert_eq!((c.index, c.offset), (17, 1));
  // Moving back up keeps the view until the cursor is within 3 rows of the top.
  c.set(4, 100, 20, 3);
  assert_eq!(c.offset, 1);
  c.move_by(-1, 100, 20, 3);
  assert_eq!((c.index, c.offset), (3, 0));
  // The ends of the list still clamp.
  c.set(99, 100, 20, 3);
  assert_eq!(c.offset, 80);
}

#[test]
fn cursor_is_centered_when_the_view_is_shorter_than_twice_the_margin() {
  // 13 rows and ranger's default margin of 8, as in a small terminal.
  let mut c = Cursor::default();
  c.set(12, 246, 13, 8);
  assert_eq!(c.offset, 6, "cursor on the middle row");
  c.move_by(1, 246, 13, 8);
  assert_eq!(c.offset, 7);
  c.set(2, 246, 13, 8);
  assert_eq!(c.offset, 0, "no scrolling above the first entry");
  c.set(245, 246, 13, 8);
  assert_eq!(c.offset, 233, "no empty space below the last entry");
}

// ---- command line ----

fn edit(keys: &str) -> (LineEditor, EditOutcome) {
  let mut editor = LineEditor::new();
  let mut outcome = EditOutcome::Continue;
  for key in parse_sequence(keys).unwrap() {
    outcome = editor.handle_key(key);
  }
  (editor, outcome)
}

#[test]
fn line_editor_inserts_and_moves_by_chars() {
  let (e, _) = edit("ab<Left>X<Home>Y<End>Z");
  assert_eq!(e.input(), "YaXbZ");
  assert_eq!(e.cursor(), 5);
  // Multibyte text is edited per char, never split inside one.
  let (e, _) = edit("好き<Left><BS>歌<Right><Del>x");
  assert_eq!(e.input(), "歌きx");
  assert_eq!(e.before_cursor(), "歌きx");
}

#[test]
fn line_editor_kill_keys() {
  let (e, _) = edit("open_with nvim  <C-w>");
  assert_eq!(e.input(), "open_with ");
  let (e, _) = edit("abc def<Left><Left><C-u>");
  assert_eq!(e.input(), "ef");
  assert_eq!(e.cursor(), 0);
  let (e, _) = edit("abc def<Home><Right><C-k>");
  assert_eq!(e.input(), "a");
}

#[test]
fn line_editor_submit_and_cancel() {
  assert_eq!(
    edit("reload<Enter>").1,
    EditOutcome::Submit("reload".into())
  );
  assert_eq!(edit("abc<Esc>").1, EditOutcome::Cancel);
  assert_eq!(edit("abc<C-c>").1, EditOutcome::Cancel);
  // Backspace only leaves once the line is empty.
  assert_eq!(edit("a<BS>").1, EditOutcome::Continue);
  assert_eq!(edit("a<BS><BS>").1, EditOutcome::Cancel);
}

#[test]
fn parses_console_commands() {
  let open_with = |argv: &[&str], detach| {
    Some(Ok(ConsoleCommand::OpenWith {
      argv: argv.iter().map(|s| s.to_string()).collect(),
      detach,
    }))
  };
  assert_eq!(parse("   "), None);
  assert_eq!(
    parse("open"),
    Some(Ok(ConsoleCommand::Action(Action::Open)))
  );
  assert_eq!(
    parse(" reload "),
    Some(Ok(ConsoleCommand::Action(Action::Reload)))
  );
  assert_eq!(parse("q"), Some(Ok(ConsoleCommand::Action(Action::Quit))));
  assert_eq!(
    parse("open_with nvim -R"),
    open_with(&["nvim", "-R"], false)
  );
  assert_eq!(
    parse("open_with -d mpv --fs"),
    open_with(&["mpv", "--fs"], true)
  );
  assert_eq!(
    parse("open_with"),
    Some(Err(ConsoleError::MissingProgram("open_with")))
  );
  assert_eq!(
    parse("open_with -d"),
    Some(Err(ConsoleError::MissingProgram("open_with")))
  );
  assert_eq!(
    parse("frobnicate"),
    Some(Err(ConsoleError::Unknown("frobnicate".into())))
  );
  assert!(matches!(
    parse("reload now"),
    Some(Err(ConsoleError::UnexpectedArgument { .. }))
  ));
}

#[test]
fn parses_rename_command() {
  assert_eq!(
    parse("rename  my file.txt "),
    Some(Ok(ConsoleCommand::Rename("my file.txt".into())))
  );
  assert_eq!(
    parse("rename"),
    Some(Ok(ConsoleCommand::Action(Action::Rename)))
  );
  assert_eq!(
    parse("  rename   "),
    Some(Ok(ConsoleCommand::Action(Action::Rename)))
  );
}

#[test]
fn parses_sort_command() {
  use tansu::fs::SortKey;
  assert_eq!(
    parse("sort mtime"),
    Some(Ok(ConsoleCommand::Action(Action::Sort(SortKey::Mtime))))
  );
  assert_eq!(parse("sort"), Some(Err(ConsoleError::MissingSortKey)));
  assert_eq!(
    parse("sort atime"),
    Some(Err(ConsoleError::UnknownSortKey("atime".into())))
  );
  assert!(matches!(
    parse("sort size name"),
    Some(Err(ConsoleError::UnexpectedArgument { .. }))
  ));
  // Every sort action's name is the command line that runs it, and binds in the config.
  for key in SortKey::ALL {
    let action = Action::Sort(key);
    assert_eq!(
      parse(action.name()),
      Some(Ok(ConsoleCommand::Action(action)))
    );
    assert_eq!(action.name().parse::<Action>(), Ok(action));
  }
}

#[test]
fn console_view_scrolls_to_keep_cursor_visible() {
  let (e, _) = edit("abcdef");
  assert_eq!(console_view(&e, 20), (":abcdef".to_string(), 7));
  // 7 cells of text plus the cursor cell do not fit in 5: the start scrolls away.
  assert_eq!(console_view(&e, 5), ("cdef".to_string(), 4));
  let (e, _) = edit("abcdef<Home>");
  assert_eq!(console_view(&e, 5), (":abcd".to_string(), 1));
  // Wide characters count two cells.
  let (e, _) = edit("好きな歌");
  assert_eq!(console_view(&e, 6), ("な歌".to_string(), 4));
}

// ---- opener ----

#[test]
fn foreground_program_gets_the_path_as_its_last_argument() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("name with spaces & $chars");
  std::fs::write(&path, "").unwrap();
  let argv = ["test".to_string(), "-f".to_string()];
  assert!(run_foreground(&argv, &path).unwrap().success());
  let missing = tmp.path().join("missing");
  assert!(!run_foreground(&argv, &missing).unwrap().success());
  assert!(run_foreground(&["tansu-no-such-program".to_string()], &path).is_err());
}

// ---- command line completion ----

fn complete_with(line: &str, selected: Option<&str>) -> Vec<String> {
  use tansu::console::{CompletionSource, completions};
  let programs = vec!["mpv".to_string(), "nvim".to_string(), "nvim-qt".to_string()];
  let list = || programs.as_slice();
  let files = || {
    ["a b.txt", "notes.md", "nvim.log"]
      .map(str::to_string)
      .to_vec()
  };
  let subdirs = |typed: &str| -> Vec<String> {
    match typed {
      "" => vec![".git", "docs", "src"],
      "src/" => vec!["fs", "preview"],
      "~/" => vec!["Downloads"],
      _ => vec![],
    }
    .into_iter()
    .map(str::to_string)
    .collect()
  };
  completions(
    line,
    &CompletionSource {
      selected,
      programs: &list,
      subdirs: &subdirs,
      files: &files,
    },
  )
}

#[test]
fn completes_cd_paths() {
  assert_eq!(complete_with("cd ", None), vec!["cd docs/", "cd src/"]);
  assert_eq!(complete_with("cd s", None), vec!["cd src/"]);
  assert_eq!(complete_with("cd .", None), vec!["cd .git/"]);
  assert_eq!(
    complete_with("cd src/", None),
    vec!["cd src/fs/", "cd src/preview/"]
  );
  assert_eq!(complete_with("cd src/p", None), vec!["cd src/preview/"]);
  assert_eq!(complete_with("cd ~", None), vec!["cd ~/"]);
  assert_eq!(complete_with("cd ~/D", None), vec!["cd ~/Downloads/"]);
  assert!(complete_with("cd nope/", None).is_empty());
  assert!(complete_with("z ", None).is_empty());
}

#[test]
fn parses_cd_and_z_commands() {
  assert_eq!(
    parse("cd  ~/My Documents "),
    Some(Ok(ConsoleCommand::Cd("~/My Documents".into())))
  );
  assert_eq!(parse("cd"), Some(Ok(ConsoleCommand::Cd(String::new()))));
  assert_eq!(
    parse("z foo  bar"),
    Some(Ok(ConsoleCommand::Zoxide(vec!["foo".into(), "bar".into()])))
  );
  assert_eq!(parse("z"), Some(Ok(ConsoleCommand::Zoxide(Vec::new()))));
}

#[test]
fn resolves_typed_paths() {
  use std::path::{Path, PathBuf};
  use tansu::fs::path::resolve;
  let cwd = Path::new("/home/u/src");
  let home = Some(Path::new("/home/u"));
  assert_eq!(
    resolve("docs", cwd, home),
    PathBuf::from("/home/u/src/docs")
  );
  assert_eq!(resolve("../x/./y", cwd, home), PathBuf::from("/home/u/x/y"));
  assert_eq!(resolve("/etc/../usr", cwd, home), PathBuf::from("/usr"));
  assert_eq!(resolve("~", cwd, home), PathBuf::from("/home/u"));
  assert_eq!(resolve("~/a", cwd, home), PathBuf::from("/home/u/a"));
  assert_eq!(resolve("~a", cwd, home), PathBuf::from("/home/u/src/~a"));
  assert_eq!(resolve("~", cwd, None), PathBuf::from("/home/u/src/~"));
  assert_eq!(resolve("../../../..", cwd, home), PathBuf::from("/"));
  assert_eq!(resolve("", cwd, home), PathBuf::from("/home/u/src"));
}

#[test]
fn completes_command_names() {
  assert_eq!(
    complete_with("tog", None),
    vec![
      "toggle_help",
      "toggle_hidden",
      "toggle_mark",
      "toggle_metadata"
    ]
  );
  // Commands that take an argument get a trailing space.
  assert_eq!(complete_with("ren", None), vec!["rename "]);
  assert_eq!(complete_with("co", None), vec!["command_line", "copy"]);
  assert_eq!(complete_with("cd", None), vec!["cd "]);
  assert_eq!(complete_with("z", None), vec!["z "]);
  assert_eq!(complete_with("  so", None), vec!["sort "]);
  assert_eq!(complete_with("open", None), vec!["open", "open_with "]);
  assert!(complete_with("zzz", None).is_empty());
  let all = complete_with("", None);
  assert!(all.contains(&"quit".to_string()) && all.contains(&"rename ".to_string()));
  assert!(
    !all
      .iter()
      .any(|n| n.starts_with("sort ") && n.len() > "sort ".len())
  );
}

#[test]
fn completes_command_arguments() {
  assert_eq!(
    complete_with("rename ", Some("a b.txt")),
    vec!["rename a b.txt"]
  );
  assert_eq!(
    complete_with("rename a ", Some("a b.txt")),
    vec!["rename a b.txt"]
  );
  assert!(complete_with("rename x", Some("a b.txt")).is_empty());
  assert!(complete_with("rename ", None).is_empty());
  assert_eq!(
    complete_with("sort ", None),
    vec!["sort name", "sort size", "sort mtime", "sort ctime"]
  );
  assert_eq!(complete_with("sort m", None), vec!["sort mtime"]);
  assert_eq!(
    complete_with("open_with nv", None),
    vec!["open_with nvim", "open_with nvim-qt"]
  );
  assert_eq!(
    complete_with("open_with -d m", None),
    vec!["open_with -d mpv"]
  );
  assert!(complete_with("open_with nvim -", None).is_empty());
  assert!(complete_with("reload ", None).is_empty());
}

#[test]
fn programs_are_executables_on_path() {
  use std::os::unix::fs::PermissionsExt;
  let a = tempfile::tempdir().unwrap();
  let b = tempfile::tempdir().unwrap();
  let exec = |dir: &std::path::Path, name: &str, mode: u32| {
    let path = dir.join(name);
    std::fs::write(&path, "").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
  };
  exec(a.path(), "zeta", 0o755);
  exec(a.path(), "plain", 0o644);
  exec(b.path(), "alpha", 0o755);
  exec(b.path(), "zeta", 0o755);
  std::fs::create_dir(b.path().join("subdir")).unwrap();
  std::os::unix::fs::symlink(a.path().join("zeta"), b.path().join("linked")).unwrap();
  let path =
    std::env::join_paths([a.path(), b.path(), std::path::Path::new("/nonexistent")]).unwrap();
  assert_eq!(
    tansu::opener::programs_in(&path),
    vec!["alpha", "linked", "zeta"]
  );
}

#[test]
fn zoxide_add_can_be_disabled() {
  assert!(Config::default().zoxide_add);
  assert!(!Config::parse("zoxide_add = false").unwrap().zoxide_add);
}

// ---- clipboard ----

#[test]
fn uri_list_percent_encodes_paths() {
  use std::path::PathBuf;
  let paths = vec![
    PathBuf::from("/home/me/a b.txt"),
    PathBuf::from("/tmp/好き#1%.md"),
  ];
  assert_eq!(
    tansu::clipboard::uri_list(&paths),
    "file:///home/me/a%20b.txt\r\nfile:///tmp/%E5%A5%BD%E3%81%8D%231%25.md\r\n"
  );
}

#[test]
fn base64_matches_rfc_4648_vectors() {
  use tansu::clipboard::base64;
  let cases = [
    ("", ""),
    ("f", "Zg=="),
    ("fo", "Zm8="),
    ("foo", "Zm9v"),
    ("foob", "Zm9vYg=="),
    ("fooba", "Zm9vYmE="),
    ("foobar", "Zm9vYmFy"),
  ];
  for (input, encoded) in cases {
    assert_eq!(base64(input.as_bytes()), encoded, "{input:?}");
  }
  assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
}

#[test]
fn osc52_wraps_base64_text() {
  use tansu::clipboard::{Content, osc52};
  assert_eq!(
    osc52(&Content::Text("/tmp/a".into())),
    "\x1b]52;c;L3RtcC9h\x07"
  );
}

// ---- :shell ----

#[test]
fn parses_shell_command() {
  let shell = |flags: &str, command: &str| {
    Some(Ok(ConsoleCommand::Shell {
      flags: flags.into(),
      command: command.into(),
    }))
  };
  // The command is kept as typed, for the shell to split.
  assert_eq!(
    parse("shell echo 'a  b' | wc -c"),
    shell("", "echo 'a  b' | wc -c")
  );
  assert_eq!(parse("shell -pw  du -sh %s"), shell("pw", "du -sh %s"));
  assert_eq!(parse("shell - ls"), shell("", "ls"));
  assert_eq!(
    parse("shell"),
    Some(Ok(ConsoleCommand::Action(Action::Shell)))
  );
  assert_eq!(parse("shell -p"), Some(Err(ConsoleError::MissingCommand)));
}

#[test]
fn shell_flags_choose_how_the_command_runs() {
  use tansu::shell::{ShellFlags, ShellMode};
  let mode = |flags: &str| ShellFlags::parse(flags).mode;
  assert_eq!(mode(""), ShellMode::Foreground { wait_enter: false });
  assert_eq!(mode("w"), ShellMode::Foreground { wait_enter: true });
  assert_eq!(mode("p"), ShellMode::Pager);
  assert_eq!(mode("pw"), ShellMode::Pager);
  assert_eq!(mode("s"), ShellMode::Silent);
  assert_eq!(mode("f"), ShellMode::Detached);
  assert_eq!(mode("fs"), ShellMode::Detached);
  assert_eq!(mode("fw"), ShellMode::Detached);
  assert_eq!(mode("t"), ShellMode::Terminal);
  // Unknown flags and `c` are ignored; an uppercase flag cancels its lowercase one.
  assert_eq!(mode("cxq"), ShellMode::Foreground { wait_enter: false });
  assert_eq!(mode("fF"), ShellMode::Foreground { wait_enter: false });
  assert_eq!(mode("pP"), ShellMode::Foreground { wait_enter: false });
  // sudo keeps the terminal for its password prompt.
  let sudo = ShellFlags::parse("r");
  assert_eq!(sudo.mode, ShellMode::Foreground { wait_enter: false });
  assert_eq!(sudo.apply_sudo("ls".into()), "sudo ls");
  let sudo_bg = ShellFlags::parse("rf");
  assert_eq!(sudo_bg.mode, ShellMode::Foreground { wait_enter: false });
  assert_eq!(sudo_bg.apply_sudo("ls".into()), "sudo -b ls");
  assert_eq!(ShellFlags::parse("").apply_sudo("ls".into()), "ls");
}

#[test]
fn shell_macros_expand_to_quoted_paths() {
  use std::path::{Path, PathBuf};
  use tansu::shell::{MacroContext, MacroError, expand_macros};
  let file = Path::new("/home/u/it's a file.txt");
  let selection = [file.to_path_buf()];
  let ctx = MacroContext {
    dir: Path::new("/home/u"),
    selected: Some(file),
    selection: &selection,
    copied: &[],
  };
  let expand = |s: &str| expand_macros(s, &ctx).map(|o| o.into_string().unwrap());
  assert_eq!(expand("vim %f").unwrap(), r"vim 'it'\''s a file.txt'");
  assert_eq!(expand("cd %d").unwrap(), "cd '/home/u'");
  assert_eq!(
    expand("cp %p %d/backup").unwrap(),
    r"cp '/home/u/it'\''s a file.txt' '/home/u'/backup"
  );
  assert_eq!(expand("du %s").unwrap(), r"du 'it'\''s a file.txt'");
  assert_eq!(expand("echo %{f}x").unwrap(), r"echo 'it'\''s a file.txt'x");
  // `%%` is a literal `%`; unknown macros, a lone `%` and unclosed braces stay as written.
  assert_eq!(expand("date +%%d").unwrap(), "date +%d");
  assert_eq!(
    expand("echo %fx %Y 100% %{f").unwrap(),
    "echo %fx %Y 100% %{f"
  );
  assert_eq!(expand("echo %t%c.").unwrap(), "echo .");
  // The next tab is this one; its file and selection are full paths.
  assert_eq!(
    expand("cp %F %D").unwrap(),
    r"cp '/home/u/it'\''s a file.txt' '/home/u'"
  );
  assert_eq!(expand("%S").unwrap(), expand("%P").unwrap());
  assert_eq!(expand("%S").unwrap(), expand("%p").unwrap());

  let empty = MacroContext {
    dir: Path::new("/tmp/empty"),
    selected: None,
    selection: &[],
    copied: &[],
  };
  assert_eq!(expand_macros("touch new %s", &empty).unwrap(), "touch new ");
  assert_eq!(
    expand_macros("cat %f", &empty),
    Err(MacroError::NothingSelected("f".into()))
  );
  assert_eq!(expand_macros("ls %S %P", &empty).unwrap(), "ls  ");

  // `%c` is every path in the copy/cut buffer.
  let copied = [PathBuf::from("/a/x y"), PathBuf::from("/b/z")];
  let buffer = MacroContext {
    dir: Path::new("/tmp"),
    selected: None,
    selection: &[],
    copied: &copied,
  };
  assert_eq!(
    expand_macros("cp %c .", &buffer).unwrap(),
    "cp '/a/x y' '/b/z' ."
  );

  // With marks, `%s` and `%p` are every marked entry; `%f` stays the selected one.
  let marked = [PathBuf::from("/d/a b"), PathBuf::from("/d/c")];
  let multi = MacroContext {
    dir: Path::new("/d"),
    selected: Some(Path::new("/d/c")),
    selection: &marked,
    copied: &[],
  };
  let expand = |s: &str| expand_macros(s, &multi).map(|o| o.into_string().unwrap());
  assert_eq!(expand("rm %s").unwrap(), "rm 'a b' 'c'");
  assert_eq!(expand("rm %p").unwrap(), "rm '/d/a b' '/d/c'");
  assert_eq!(expand("%P").unwrap(), expand("%p").unwrap());
  assert_eq!(expand("vim %f").unwrap(), "vim 'c'");
}

#[test]
fn shell_escape_backslashes_metacharacters() {
  use tansu::shell::shell_escape;
  assert_eq!(shell_escape("plain.txt"), "plain.txt");
  assert_eq!(shell_escape("a b&(c)$.txt"), r"a\ b\&\(c\)\$.txt");
  assert_eq!(shell_escape(r"back\slash"), r"back\\slash");
  assert_eq!(shell_escape("new\nline"), "'new\nline'");
}

#[test]
fn completes_shell_commands() {
  // The first word is a program, followed by a space.
  assert_eq!(
    complete_with("shell nv", None),
    vec!["shell nvim ", "shell nvim-qt "]
  );
  assert_eq!(complete_with("shell -p m", None), vec!["shell -p mpv "]);
  assert!(complete_with("shell -p", None).is_empty());
  // After a space, the selected name; without a selection, `%s`.
  assert_eq!(
    complete_with("shell nvim ", Some("a b.txt")),
    vec![r"shell nvim a\ b.txt "]
  );
  assert_eq!(complete_with("shell nvim ", None), vec!["shell nvim %s "]);
  // A partial word completes to names in the current directory.
  assert_eq!(
    complete_with("shell nvim -O n", None),
    vec!["shell nvim -O notes.md", "shell nvim -O nvim.log"]
  );
  assert_eq!(
    complete_with("shell cat a", None),
    vec![r"shell cat a\ b.txt"]
  );
  assert_eq!(complete_with("sh", None), vec!["shell "]);
}

#[test]
fn shell_commands_run_in_the_given_directory() {
  use tansu::opener::run_shell_silent;
  use tansu::shell::{MacroContext, expand_macros};
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join(r#"it's $HOME & "x".txt"#);
  std::fs::write(&file, "").unwrap();
  let selection = [file.clone()];
  let ctx = MacroContext {
    dir: tmp.path(),
    selected: Some(&file),
    selection: &selection,
    copied: &[],
  };
  // `%f` is a bare name, so this only passes when the shell runs in `dir`.
  let script = expand_macros("test -f %f && test -d %d && test -f %p", &ctx).unwrap();
  assert!(run_shell_silent(&script, tmp.path()).unwrap().success());
  let other = tempfile::tempdir().unwrap();
  assert!(!run_shell_silent(&script, other.path()).unwrap().success());
}

mod search {
  use tansu::search::Pattern;

  fn score(pattern: &str, name: &str) -> Option<i64> {
    Pattern::parse(pattern).unwrap().score(name)
  }

  #[test]
  fn empty_pattern_is_none() {
    assert_eq!(Pattern::parse(""), None);
  }

  #[test]
  fn wildcards_match_the_whole_name() {
    assert!(Pattern::parse("*.rs").unwrap().is_glob());
    assert!(score("*.rs", "main.rs").is_some());
    assert!(score("*.rs", "main.rs.bak").is_none());
    assert!(score("ma*", "main.rs").is_some());
    assert!(score("ma*", "xmain.rs").is_none());
    assert!(score("*ai*", "main.rs").is_some());
    assert!(score("file?.txt", "file2.txt").is_some());
    assert!(score("file?.txt", "file10.txt").is_none());
    assert!(score("*", "").is_some());
    assert!(score("a*b*c", "aXbYbZc").is_some());
    assert!(score("a*b*c", "aXbYcZ").is_none());
  }

  #[test]
  fn backslash_makes_wildcards_literal() {
    assert!(!Pattern::parse("a\\*").unwrap().is_glob());
    assert!(score("why\\?", "why?").is_some());
    assert!(score("why\\?*", "why?.md").is_some());
    assert!(score("why\\?*", "whyX.md").is_none());
  }

  #[test]
  fn fuzzy_matches_chars_in_order() {
    assert!(!Pattern::parse("mrs").unwrap().is_glob());
    assert!(score("mrs", "main.rs").is_some());
    assert!(score("rsm", "main.rs").is_none());
    assert!(score("toolong", "tool").is_none());
  }

  #[test]
  fn fuzzy_prefers_consecutive_and_word_start_matches() {
    let better = |p: &str, a: &str, b: &str| {
      let (sa, sb) = (score(p, a), score(p, b));
      assert!(sa > sb, "`{p}`: {a} ({sa:?}) should beat {b} ({sb:?})");
    };
    better("main", "main.rs", "my_animation.rs");
    better("mrs", "main.rs", "mirrors");
    better("ct", "config.toml", "cat");
    better("readme", "README.md", "reader_made.txt");
  }

  #[test]
  fn smart_case() {
    assert!(score("readme", "README.md").is_some());
    assert!(score("README", "readme.md").is_none());
    assert!(score("*.MD", "README.md").is_none());
    assert!(score("*.md", "README.MD").is_some());
  }

  #[test]
  fn non_ascii_names() {
    assert!(score("歌", "好きな歌のタイトル.mp3").is_some());
    assert!(score("*タイトル*", "好きな歌のタイトル.mp3").is_some());
    assert!(score("ÉTÉ", "été.txt").is_none());
    assert!(score("été", "ÉTÉ.txt").is_some());
  }
}

mod trash {
  use std::fs;

  use tansu::fs::trash::trash;

  #[test]
  fn path_is_percent_encoded_and_the_trash_is_private() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a b%é.txt");
    fs::write(&file, "x").unwrap();
    let root = tmp.path().join("Trash");
    let target = trash(&file, &root).unwrap();
    assert_eq!(target, root.join("files/a b%é.txt"));
    let info = fs::read_to_string(root.join("info/a b%é.txt.trashinfo")).unwrap();
    assert!(info.contains("/a%20b%25%C3%A9.txt\n"), "{info}");
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(&root).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
  }

  #[test]
  fn refuses_what_is_already_in_the_trash_and_missing_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Trash");
    fs::create_dir_all(root.join("files")).unwrap();
    fs::write(root.join("files/old"), "").unwrap();
    assert!(
      trash(&root.join("files/old"), &root)
        .unwrap_err()
        .contains("already in the trash")
    );
    assert!(trash(&tmp.path().join("missing"), &root).is_err());
    // Nothing was reserved for the failures.
    assert_eq!(fs::read_dir(root.join("info")).map_or(0, |d| d.count()), 0);
  }
}

#[test]
fn image_details_need_the_image_to_keep_two_thirds_of_the_column() {
  use tansu::ui::layout::image_split;
  assert_eq!(image_split(40, 0, false), (40, 0));
  assert_eq!(image_split(40, 7, false), (32, 7));
  assert_eq!(image_split(24, 7, false), (16, 7));
  assert_eq!(image_split(23, 7, false), (23, 0));
  assert_eq!(image_split(0, 3, false), (0, 0));
  // Details first: shown while the image keeps 4 rows.
  assert_eq!(image_split(14, 9, true), (4, 9));
  assert_eq!(image_split(13, 9, true), (13, 0));
  assert_eq!(image_split(14, 9, false), (14, 0));
}
