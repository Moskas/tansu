//! App-level tests against a real temporary directory tree. Background work is executed
//! synchronously through `runtime::execute_sync`, so results are deterministic.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tempfile::TempDir;

use tansu::app::{App, Command, PreviewStatus};
use tansu::config::Config;
use tansu::keymap::{Key, Keymap};
use tansu::preview::{Preview, PreviewRegistry};
use tansu::runtime::execute_sync;
use tansu::worker::LatestWorker;

const WIDTH: u16 = 60;
const HEIGHT: u16 = 12;

/// Layout of the fixture (inside `<tmp>/work`):
///   alpha/{a.txt, b.txt, .secret}
///   beta/
///   .dotfile
///   file2.txt, file10.txt ("hello\nworld\n")
///   blob.bin (contains NUL bytes)
///   好きな歌のタイトル.mp3
fn fixture() -> (TempDir, PathBuf) {
  let tmp = tempfile::tempdir().unwrap();
  let work = tmp.path().join("work");
  fs::create_dir_all(work.join("alpha")).unwrap();
  fs::create_dir_all(work.join("beta")).unwrap();
  fs::write(work.join("alpha/a.txt"), "a\n").unwrap();
  fs::write(work.join("alpha/b.txt"), "b\n").unwrap();
  fs::write(work.join("alpha/.secret"), "").unwrap();
  fs::write(work.join(".dotfile"), "").unwrap();
  fs::write(work.join("file2.txt"), "two\n").unwrap();
  fs::write(work.join("file10.txt"), "hello\n\tworld\n").unwrap();
  fs::write(work.join("blob.bin"), [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
  fs::write(work.join("好きな歌のタイトル.mp3"), vec![b'x'; 2048]).unwrap();
  (tmp, work)
}

fn new_app(start: &Path) -> App {
  let config = Config {
    icons: false,
    ..Config::default()
  };
  let mut app = App::new(
    config,
    Keymap::default(),
    start.to_path_buf(),
    None,
    "tester@box".to_string(),
    None,
  );
  app.set_size(WIDTH, HEIGHT);
  pump(&mut app);
  app
}

/// Runs queued commands (and the commands their results trigger) until idle.
fn pump(app: &mut App) {
  let registry = PreviewRegistry::with_defaults();
  for _ in 0..100 {
    let cmds = app.take_commands();
    if cmds.is_empty() {
      return;
    }
    for cmd in cmds {
      for ev in execute_sync(cmd, &registry, app.config.preview) {
        app.handle_event(ev);
      }
    }
  }
  panic!("commands never settled");
}

fn press(app: &mut App, keys: &str) {
  for key in tansu::keymap::parse_sequence(keys).unwrap() {
    app.handle_key(key);
    pump(app);
  }
}

fn names(app: &App) -> Vec<String> {
  (0..app.visible_len())
    .map(|i| app.visible_entry(i).unwrap().name.clone())
    .collect()
}

fn selected(app: &App) -> String {
  app.selected().unwrap().name.clone()
}

fn render(app: &App) -> Vec<String> {
  let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
  terminal.draw(|f| tansu::ui::render(f, app)).unwrap();
  let buf = terminal.backend().buffer().clone();
  (0..HEIGHT)
    .map(|y| {
      let mut line = String::new();
      let mut x = 0;
      while x < WIDTH {
        let sym = buf[(x, y)].symbol();
        line.push_str(sym);
        // Wide characters occupy the next cell, which holds an empty placeholder.
        x += unicode_width::UnicodeWidthStr::width(sym).max(1) as u16;
      }
      line
    })
    .collect()
}

#[test]
fn lists_with_dirs_first_natural_order_and_hidden_filter() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  assert_eq!(
    names(&app),
    vec![
      "alpha",
      "beta",
      "blob.bin",
      "file2.txt",
      "file10.txt",
      "好きな歌のタイトル.mp3"
    ]
  );
  press(&mut app, ".");
  assert_eq!(names(&app)[2], ".dotfile");
  assert_eq!(app.visible_len(), 7);
  press(&mut app, ".");
  assert_eq!(app.visible_len(), 6);
}

#[test]
fn hidden_toggle_keeps_selection() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj");
  assert_eq!(selected(&app), "file2.txt");
  press(&mut app, ".");
  assert_eq!(selected(&app), "file2.txt");
}

#[test]
fn movement_keys() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  assert_eq!(selected(&app), "alpha");
  press(&mut app, "j");
  assert_eq!(selected(&app), "beta");
  press(&mut app, "G");
  assert_eq!(selected(&app), "好きな歌のタイトル.mp3");
  press(&mut app, "j");
  assert_eq!(app.cursor().index, 5, "cursor stays on last entry");
  press(&mut app, "gg");
  assert_eq!(selected(&app), "alpha");
  press(&mut app, "<C-d>");
  assert_eq!(app.cursor().index, (app.rows() / 2).max(1));
  press(&mut app, "<C-u>");
  assert_eq!(app.cursor().index, 0);
}

#[test]
fn entering_and_leaving_remembers_cursor_per_directory() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "l");
  assert_eq!(app.cwd, work.join("alpha"));
  assert_eq!(names(&app), vec!["a.txt", "b.txt"]);
  press(&mut app, "j");
  assert_eq!(selected(&app), "b.txt");

  press(&mut app, "h");
  assert_eq!(app.cwd, work);
  assert_eq!(
    selected(&app),
    "alpha",
    "parent selects the directory we left"
  );

  press(&mut app, "jl");
  assert_eq!(app.cwd, work.join("beta"));
  assert_eq!(app.visible_len(), 0);
  press(&mut app, "hkl");
  assert_eq!(app.cwd, work.join("alpha"));
  assert_eq!(selected(&app), "b.txt", "cursor restored inside alpha");
}

#[test]
fn opening_a_file_requests_the_opener() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjj"); // file10.txt
  app.handle_key(Key::plain(KeyCode::Enter));
  let cmds = app.take_commands();
  assert_eq!(cmds, vec![Command::Open(work.join("file10.txt"))]);
}

#[test]
fn quit_and_ctrl_c() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  app.handle_key(Key::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
  assert!(app.should_quit);
}

#[test]
fn previews_text_directory_and_binary() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);

  let content = |app: &App| match &app.preview.as_ref().unwrap().status {
    PreviewStatus::Ready(p) => (**p).clone(),
    other => panic!("preview not ready: {other:?}"),
  };

  match content(&app) {
    Preview::Directory(l) => assert_eq!(l.entries.len(), 3, "hidden entries kept"),
    other => panic!("expected directory preview, got {other:?}"),
  }

  press(&mut app, "jjjj");
  assert_eq!(selected(&app), "file10.txt");
  match content(&app) {
    Preview::Text { lines, truncated } => {
      assert_eq!(lines, vec!["hello", "    world"]);
      assert!(!truncated);
    }
    other => panic!("expected text preview, got {other:?}"),
  }

  press(&mut app, "kk");
  assert_eq!(selected(&app), "blob.bin");
  match content(&app) {
    Preview::Metadata(rows) => {
      assert!(rows.contains(&("Type".to_string(), "binary file".to_string())));
    }
    other => panic!("expected metadata preview, got {other:?}"),
  }
}

#[test]
fn text_preview_is_capped() {
  let tmp = tempfile::tempdir().unwrap();
  let line = "0123456789".repeat(10) + "\n";
  fs::write(tmp.path().join("big.txt"), line.repeat(50_000)).unwrap();
  let app = new_app(tmp.path());
  let limits = app.config.preview;
  match &app.preview.as_ref().unwrap().status {
    PreviewStatus::Ready(p) => match &**p {
      Preview::Text { lines, truncated } => {
        assert!(*truncated);
        assert!(lines.len() <= limits.max_lines);
      }
      other => panic!("unexpected {other:?}"),
    },
    other => panic!("unexpected {other:?}"),
  }
}

#[test]
fn unreadable_directory_reports_error_without_panicking() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  // Simulate a directory vanishing between listing and entering it.
  fs::remove_dir_all(work.join("alpha")).unwrap();
  press(&mut app, "l");
  let msg = app.message.as_ref().expect("error message shown");
  assert!(msg.text.contains("not found"), "{}", msg.text);
  let lines = render(&app);
  assert!(lines.iter().any(|l| l.contains("not found")));
}

#[test]
fn stale_preview_results_are_ignored() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  // Jump straight to an entry that was never previewed: the request is queued, not run.
  app.handle_key(Key::plain(KeyCode::Char('G')));
  let cmds = app.take_commands();
  let Some(Command::Preview { id, .. }) = cmds.first().cloned() else {
    panic!("expected a preview request, got {cmds:?}");
  };
  // A result for an older request id must not replace the pending one.
  app.handle_event(tansu::event::Event::PreviewReady {
    id: id - 1,
    path: work.join("好きな歌のタイトル.mp3"),
    result: Ok(Preview::Text {
      lines: vec!["stale".into()],
      truncated: false,
    }),
  });
  assert!(matches!(
    app.preview.as_ref().unwrap().status,
    PreviewStatus::Loading
  ));
}

#[test]
fn latest_worker_cancels_superseded_jobs() {
  let (tx, rx) = mpsc::channel();
  let (started_tx, started_rx) = mpsc::channel();
  let mut worker = LatestWorker::spawn("test", move |n: u32, token| {
    if n == 1 {
      started_tx.send(()).unwrap();
      // Long-running job: spin until a newer job supersedes it.
      let start = std::time::Instant::now();
      while !token.is_cancelled() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
      }
      tx.send((n, token.is_cancelled())).unwrap();
    } else {
      tx.send((n, false)).unwrap();
    }
  });
  worker.submit(1);
  started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
  worker.submit(2);
  let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
  assert_eq!(first, (1, true), "first job observed cancellation");
  let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
  assert_eq!(second, (2, false));
}

#[test]
fn latest_worker_cancel_stops_job_without_replacement() {
  let (tx, rx) = mpsc::channel();
  let (started_tx, started_rx) = mpsc::channel();
  let mut worker = LatestWorker::spawn("test", move |_: (), token| {
    started_tx.send(()).unwrap();
    let start = std::time::Instant::now();
    while !token.is_cancelled() && start.elapsed() < Duration::from_secs(5) {
      std::thread::sleep(Duration::from_millis(1));
    }
    tx.send(token.is_cancelled()).unwrap();
  });
  worker.submit(());
  started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
  worker.cancel();
  assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
}

#[test]
fn moving_onto_cached_entry_cancels_pending_preview() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  // alpha (dir, listing cached) -> G queues a preview for the never-seen mp3 ...
  app.handle_key(Key::plain(KeyCode::Char('G')));
  assert!(matches!(
    app.take_commands().as_slice(),
    [Command::Preview { .. }]
  ));
  // ... and jumping back to the cached directory cancels it instead of letting it run.
  press_keys_without_pump(&mut app, "gg");
  assert_eq!(app.take_commands(), vec![Command::CancelPreview]);
}

fn press_keys_without_pump(app: &mut App, keys: &str) {
  for key in tansu::keymap::parse_sequence(keys).unwrap() {
    app.handle_key(key);
  }
}

#[test]
fn snapshot_main_layout() {
  let (_tmp, work) = fixture();
  let app = new_app(&work);
  let lines = render(&app);

  let header = &lines[0];
  assert!(header.starts_with("tester@box /"), "{header}");
  assert!(header.trim_end().ends_with("/work"), "{header}");

  let expected_body = [
    "┌───────┬─────────────────────┬────────────────────────────┐",
    "│/ work │/ alpha            2 │- a.txt                     │",
    "│       │/ beta             0 │- b.txt                     │",
    "│       │- blob.bin       7 B │                            │",
    "│       │- file2.txt      4 B │                            │",
    "│       │- file10.txt    13 B │                            │",
    "│       │& 好きな歌の~ 2.00 K │                            │",
    "│       │                     │                            │",
    "│       │                     │                            │",
    "└───────┴─────────────────────┴────────────────────────────┘",
  ];
  assert_eq!(&lines[1..11], &expected_body, "\n{}", lines.join("\n"));

  let status = &lines[11];
  assert!(status.starts_with("drwx"), "{status}");
  assert!(status.trim_end().ends_with("1/6  All"), "{status}");
  assert!(status.contains("2.02K sum"), "{status}");
}

#[test]
fn snapshot_highlights_cursor_rows() {
  let (_tmp, work) = fixture();
  let app = new_app(&work);
  let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
  terminal.draw(|f| tansu::ui::render(f, &app)).unwrap();
  let buf = terminal.backend().buffer();
  let cursor_bg = app.config.theme.cursor_bg.0;
  // Parent row for `work`, current row for `alpha`, preview row for `a.txt`, edge to edge.
  for (x0, x1) in [(1, 8), (9, 30), (31, 59)] {
    for x in x0..x1 {
      assert_eq!(buf[(x, 2)].bg, cursor_bg, "cell ({x}, 2)");
    }
  }
  assert_ne!(buf[(9, 3)].bg, cursor_bg);
}

#[test]
fn help_overlay_lists_bindings_and_closes_on_any_key() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "?");
  assert!(app.show_help);
  let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
  terminal.draw(|f| tansu::ui::render(f, &app)).unwrap();
  let text: String = terminal
    .backend()
    .buffer()
    .content()
    .iter()
    .map(|c| c.symbol())
    .collect();
  assert!(text.contains("Help"));
  assert!(text.contains("<C-d>"));
  assert!(text.contains("Toggle hidden files"));
  press(&mut app, "j");
  assert!(!app.show_help);
  assert_eq!(selected(&app), "alpha", "closing key is not also an action");
}

#[test]
fn resize_reflows_layout() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "G");
  app.set_size(WIDTH, 6); // two list rows
  assert_eq!(app.rows(), 2);
  assert!(app.cursor().index >= app.cursor().offset);
  assert!(app.cursor().index < app.cursor().offset + app.rows());
  let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
  terminal.draw(|f| tansu::ui::render(f, &app)).unwrap();
  // Tiny terminals must not panic either.
  for (w, h) in [(1, 1), (2, 3), (5, 2), (0, 0)] {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    app.set_size(w, h);
    t.draw(|f| tansu::ui::render(f, &app)).unwrap();
  }
}
