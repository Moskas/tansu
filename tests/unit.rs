//! Tests for pure building blocks: sorting, navigation, truncation, keymap, formatting, config.

use std::cmp::Ordering;

use crossterm::event::{KeyCode, KeyModifiers};

use tansu::action::Action;
use tansu::config::Config;
use tansu::fs::meta::{format_permissions, human_size, human_size_compact};
use tansu::fs::sort::natural_cmp;
use tansu::keymap::{Key, KeyParseError, KeyResult, Keymap, parse_sequence};
use tansu::nav::{Cursor, ScrollPos, passive_offset};
use tansu::ui::layout::split_columns;
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
  c.move_by(1, 10, 4);
  assert_eq!(
    c,
    Cursor {
      index: 1,
      offset: 0
    }
  );
  c.set(6, 10, 4);
  assert_eq!(
    c,
    Cursor {
      index: 6,
      offset: 3
    }
  );
  c.move_by(-5, 10, 4);
  assert_eq!(
    c,
    Cursor {
      index: 1,
      offset: 1
    }
  );
  c.move_by(-100, 10, 4);
  assert_eq!(
    c,
    Cursor {
      index: 0,
      offset: 0
    }
  );
  c.move_by(100, 10, 4);
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
  c.fix_offset(5, 4);
  assert_eq!(
    c,
    Cursor {
      index: 4,
      offset: 1
    }
  );
  c.fix_offset(5, 10);
  assert_eq!(
    c,
    Cursor {
      index: 4,
      offset: 0
    }
  );
  c.set(3, 0, 10);
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
  assert_eq!(cfg.theme.cursor_bg.0, ratatui::style::Color::Rgb(255, 0, 0));
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
