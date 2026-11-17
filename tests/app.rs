//! App-level tests against a real temporary directory tree. Background work is executed
//! synchronously through `runtime::execute_sync`, so results are deterministic.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use tempfile::TempDir;

use tansu::app::{App, Command, PreviewStatus};
use tansu::config::Config;
use tansu::event::Event;
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
    vcs_aware: false,
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
  pump_with(app, &PreviewRegistry::with_defaults());
}

fn pump_with(app: &mut App, registry: &PreviewRegistry) {
  for _ in 0..100 {
    let cmds = app.take_commands();
    if cmds.is_empty() {
      return;
    }
    for cmd in cmds {
      for ev in execute_sync(cmd, registry, app.config.preview) {
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
  render_sized(app, WIDTH, HEIGHT)
}

fn render_sized(app: &App, width: u16, height: u16) -> Vec<String> {
  let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
  terminal.draw(|f| tansu::ui::render(f, app)).unwrap();
  let buf = terminal.backend().buffer().clone();
  (0..height)
    .map(|y| {
      let mut line = String::new();
      let mut x = 0;
      while x < width {
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
    image: None,
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
  assert!(header.trim_end().ends_with("/work/alpha"), "{header}");

  let expected_body = [
    "┌───────┬─────────────────────┬────────────────────────────┐",
    "│ / work│ / alpha            3│ - a.txt                    │",
    "│       │ / beta             0│ - b.txt                    │",
    "│       │ - blob.bin       7 B│                            │",
    "│       │ - file2.txt      4 B│                            │",
    "│       │ - file10.txt    13 B│                            │",
    "│       │ & 好きな歌の~ 2.05 k│                            │",
    "│       │                     │                            │",
    "│       │                     │                            │",
    "└───────┴─────────────────────┴────────────────────────────┘",
  ];
  assert_eq!(&lines[1..11], &expected_body, "\n{}", lines.join("\n"));

  let status = &lines[11];
  assert!(status.starts_with("drwx"), "{status}");
  assert!(status.trim_end().ends_with("1/6  All"), "{status}");
  assert!(status.contains("2.07k sum"), "{status}");
}

/// Runs git with an isolated configuration so the user's global settings (signing, hooks)
/// cannot interfere. Returns false if git is unavailable.
fn git(dir: &Path, args: &[&str]) -> bool {
  std::process::Command::new("git")
    .args([
      "-c",
      "user.name=tansu",
      "-c",
      "user.email=tansu@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "-c",
      "init.defaultBranch=main",
    ])
    .args(args)
    .current_dir(dir)
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status()
    .is_ok_and(|s| s.success())
}

fn marks(app: &App, dir: &Path, name: &str) -> String {
  let listing = app.listing(dir).expect("listing loaded");
  let entry = listing.entries.iter().find(|e| e.name == name).unwrap();
  app
    .vcs_marks(dir, entry)
    .iter()
    .map(|m| m.symbol())
    .collect()
}

#[test]
fn git_status_markers_in_repo_and_parent() {
  let tmp = tempfile::tempdir().unwrap();
  let repos = tmp.path().join("repos");
  let repo = repos.join("proj");
  fs::create_dir_all(repo.join("src")).unwrap();
  fs::create_dir_all(repo.join("target")).unwrap();
  fs::create_dir_all(repos.join("plain")).unwrap();
  fs::write(repo.join(".gitignore"), "target/\n").unwrap();
  fs::write(repo.join("src/lib.rs"), "fn a() {}\n").unwrap();
  fs::write(repo.join("README.md"), "hi\n").unwrap();
  fs::write(repo.join("target/out"), "x").unwrap();
  if !git(&repo, &["init", "-q"]) {
    eprintln!("git not available, skipping");
    return;
  }
  assert!(git(&repo, &["add", "."]));
  assert!(git(&repo, &["commit", "-q", "-m", "init"]));
  fs::write(repo.join("src/lib.rs"), "fn b() {}\n").unwrap();
  fs::write(repo.join("NOTES"), "new\n").unwrap();

  let mut app = new_app(&repo);
  app.config.vcs_aware = true;
  press(&mut app, "R");

  assert_eq!(
    marks(&app, &repo, "src"),
    "+",
    "directory takes child status"
  );
  assert_eq!(marks(&app, &repo, "target"), "·", "ignored directory");
  assert_eq!(marks(&app, &repo, "README.md"), "✓");
  assert_eq!(marks(&app, &repo, "NOTES"), "?");

  // Parent column: the repository root gets remote + summary, its sibling blanks.
  assert_eq!(marks(&app, &repos, "proj"), "⌂?");
  assert_eq!(marks(&app, &repos, "plain"), "  ");

  // Rendered: marks sit flush against each column's right separator.
  let lines = render_sized(&app, 80, HEIGHT);
  let column = |name: &str, col: usize| -> String {
    lines
      .iter()
      .map(|l| l.split('│').collect::<Vec<_>>())
      .find(|parts| parts.len() == 5 && parts[col].contains(name))
      .map(|parts| parts[col].to_string())
      .unwrap_or_else(|| panic!("{name} not in column {col}:\n{}", lines.join("\n")))
  };
  assert!(column("src", 2).ends_with(" 1 +"), "{:?}", column("src", 2));
  assert!(
    column("NOTES", 2).ends_with(" ?"),
    "{:?}",
    column("NOTES", 2)
  );
  assert!(column("proj", 1).ends_with("⌂?"), "{:?}", column("proj", 1));
  assert!(column("pla", 1).ends_with("~  "), "{:?}", column("pla", 1));

  // Nothing is tracked inside the repository's own .git directory.
  let head = app
    .listing(&repo)
    .unwrap()
    .entries
    .iter()
    .find(|e| e.name == ".git")
    .cloned()
    .unwrap();
  assert!(app.vcs_marks(&repo.join(".git"), &head).is_empty());

  // Status bar on a file in the repository: branch, remote + file status, commit.
  while app.selected().is_some_and(|e| e.name != "NOTES") {
    press(&mut app, "j");
  }
  let status = render_sized(&app, 140, HEIGHT).pop().unwrap();
  assert!(status.contains("(git: main) ⌂?"), "{status}");
  // On a directory that is not a repository root, ranger shows no git info.
  press(&mut app, "gg");
  let status = render_sized(&app, 140, HEIGHT).pop().unwrap();
  assert!(!status.contains("(git"), "{status}");

  // Inside src (and in the preview of src) files carry their own status.
  press(&mut app, "l");
  assert_eq!(app.cwd, repo.join("src"));
  assert_eq!(marks(&app, &repo.join("src"), "lib.rs"), "+");
}

#[test]
fn no_markers_outside_repositories() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  app.config.vcs_aware = true;
  press(&mut app, "R");
  let entry = app.selected().unwrap().clone();
  assert!(app.vcs_marks(&work, &entry).is_empty());
}

#[test]
fn snapshot_highlights_cursor_rows() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  let theme = app.config.theme.clone();
  let draw = |app: &App| {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    terminal.draw(|f| tansu::ui::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
  };
  // Like ranger: the selected rows (parent `work`, current `alpha`, preview `a.txt`) are the
  // entry's own colors in reverse video, edge to edge.
  let buf = draw(&app);
  for (x0, x1, color) in [
    (1, 8, theme.directory),
    (9, 30, theme.directory),
    (31, 59, theme.file),
  ] {
    for x in x0..x1 {
      let cell = &buf[(x, 2)];
      assert!(cell.modifier.contains(Modifier::REVERSED), "cell ({x}, 2)");
      assert_eq!(cell.fg, color.0, "cell ({x}, 2)");
    }
  }
  assert!(!buf[(9, 3)].modifier.contains(Modifier::REVERSED));

  // A configured text color keeps the entry color as the background.
  app.config.theme.cursor_fg = Some(theme.fg);
  let buf = draw(&app);
  let cell = &buf[(12, 2)];
  assert!(!cell.modifier.contains(Modifier::REVERSED));
  assert_eq!((cell.fg, cell.bg), (theme.fg.0, theme.directory.0));
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

#[test]
fn resizing_re_requests_image_previews_only() {
  let tmp = tempfile::tempdir().unwrap();
  image::RgbImage::new(8, 8)
    .save(tmp.path().join("a.png"))
    .unwrap();
  fs::write(tmp.path().join("b.txt"), "text\n").unwrap();
  let registry = PreviewRegistry::configured(&tansu::preview::HighlightConfig::default(), true);
  let mut app = new_app(tmp.path());
  // The first preview ran without the image previewer; reload with it.
  app.dispatch(tansu::action::Action::Reload);
  pump_with(&mut app, &registry);
  assert_eq!(selected(&app), "a.png");
  assert!(matches!(
    &app.preview.as_ref().unwrap().status,
    PreviewStatus::Ready(p) if matches!(**p, Preview::Image(_))
  ));

  app.set_size(WIDTH + 20, HEIGHT);
  let cells = app.preview_cells();
  assert!(matches!(
    app.take_commands().as_slice(),
    [Command::Preview { cells: c, .. }] if *c == cells
  ));
  pump_with(&mut app, &registry);

  app.dispatch(tansu::action::Action::MoveDown);
  pump_with(&mut app, &registry);
  assert_eq!(selected(&app), "b.txt");
  app.set_size(WIDTH, HEIGHT);
  assert_eq!(app.take_commands(), vec![]);
}

#[test]
fn metadata_toggle_replaces_file_previews_but_not_directory_listings() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  let screen = |app: &App| render_sized(app, 100, HEIGHT).join("\n");

  press(&mut app, "jjjj");
  assert_eq!(selected(&app), "file10.txt");
  assert!(screen(&app).contains("hello"));

  press(&mut app, "i");
  assert!(app.show_metadata);
  let shown = screen(&app);
  assert!(shown.contains("text file"), "{shown}");
  assert!(!shown.contains("hello"));

  // The mode sticks while moving; directories still list their contents.
  press(&mut app, "gg");
  assert_eq!(selected(&app), "alpha");
  assert!(screen(&app).contains("a.txt"));

  press(&mut app, "Gi");
  assert!(!app.show_metadata);
}

#[test]
fn command_line_open_with_queues_foreground_or_detached_program() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjj"); // file10.txt
  press(&mut app, ":open_with nvim -R");
  press_keys_without_pump(&mut app, "<Enter>");
  assert!(app.console.is_none());
  assert_eq!(
    app.take_commands(),
    vec![Command::Spawn {
      argv: vec!["nvim".into(), "-R".into()],
      path: work.join("file10.txt"),
      foreground: true,
    }]
  );
  // Keys typed into the command line are not bindings: `j` did not move the cursor.
  assert_eq!(selected(&app), "file10.txt");

  // Directories are passed to the program, not entered.
  press(&mut app, "gg");
  press(&mut app, ":open_with -d thunar");
  press_keys_without_pump(&mut app, "<Enter>");
  assert_eq!(app.cwd, work);
  assert_eq!(
    app.take_commands(),
    vec![Command::Spawn {
      argv: vec!["thunar".into()],
      path: work.join("alpha"),
      foreground: false,
    }]
  );
}

#[test]
fn command_line_runs_actions_and_reports_errors() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjj");
  press(&mut app, ":open");
  press_keys_without_pump(&mut app, "<Enter>");
  assert_eq!(
    app.take_commands(),
    vec![Command::Open(work.join("file10.txt"))]
  );
  press(&mut app, ":nope<Enter>");
  assert_eq!(app.message.as_ref().unwrap().text, "unknown command `nope`");
  press(&mut app, ":toggle_hidden<Enter>");
  assert!(app.show_hidden);
  press(&mut app, ":q<Enter>");
  assert!(app.should_quit);
}

#[test]
fn command_line_cancel_runs_nothing() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":quit<Esc>");
  assert!(app.console.is_none());
  assert!(!app.should_quit);
  press(&mut app, ":<BS>");
  assert!(app.console.is_none());
  assert!(app.take_commands().is_empty());
}

#[test]
fn command_line_is_drawn_in_the_status_row() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":open_with less");
  let lines = render(&app);
  assert_eq!(lines[HEIGHT as usize - 1].trim_end(), ":open_with less");
  let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
  terminal.draw(|f| tansu::ui::render(f, &app)).unwrap();
  terminal
    .backend_mut()
    .assert_cursor_position((15, HEIGHT - 1));
}

fn set_mtime(path: &Path, secs: u64) {
  let time = std::time::UNIX_EPOCH + Duration::from_secs(secs);
  fs::File::open(path).unwrap().set_modified(time).unwrap();
}

#[test]
fn sort_keys_keep_directories_first_and_the_selection() {
  let (_tmp, work) = fixture();
  set_mtime(&work.join("alpha"), 100);
  set_mtime(&work.join("beta"), 200);
  set_mtime(&work.join("blob.bin"), 300);
  set_mtime(&work.join("file2.txt"), 500);
  set_mtime(&work.join("file10.txt"), 400);
  set_mtime(&work.join("好きな歌のタイトル.mp3"), 100);
  let mut app = new_app(&work);
  press(&mut app, "jjj");
  assert_eq!(selected(&app), "file2.txt");

  press(&mut app, "os");
  assert_eq!(
    names(&app),
    vec![
      "alpha",
      "beta",
      "好きな歌のタイトル.mp3",
      "file10.txt",
      "blob.bin",
      "file2.txt"
    ]
  );
  assert_eq!(selected(&app), "file2.txt");
  assert_eq!(app.message.as_ref().unwrap().text, "sorted by size");

  press(&mut app, "om");
  assert_eq!(
    names(&app),
    vec![
      "beta",
      "alpha",
      "file2.txt",
      "file10.txt",
      "blob.bin",
      "好きな歌のタイトル.mp3"
    ]
  );
  assert_eq!(selected(&app), "file2.txt");

  press(&mut app, ":sort name<Enter>");
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
}

#[test]
fn sort_order_applies_to_listings_loaded_later() {
  let (_tmp, work) = fixture();
  fs::write(work.join("alpha/a.txt"), "a much longer file\n").unwrap();
  let mut app = new_app(&work);
  press(&mut app, "os");
  press(&mut app, "l");
  assert_eq!(names(&app), vec!["a.txt", "b.txt"]);
  fs::write(work.join("alpha/b.txt"), "an even longer file than a\n").unwrap();
  press(&mut app, "R");
  assert_eq!(names(&app), vec!["b.txt", "a.txt"]);
}

#[test]
fn config_sets_the_initial_sort_order() {
  let (_tmp, work) = fixture();
  let config = Config {
    icons: false,
    vcs_aware: false,
    sort: tansu::fs::SortKey::Size,
    ..Config::default()
  };
  let mut app = App::new(
    config,
    Keymap::default(),
    work.clone(),
    None,
    "tester@box".to_string(),
    None,
  );
  app.set_size(WIDTH, HEIGHT);
  pump(&mut app);
  assert_eq!(names(&app)[2], "好きな歌のタイトル.mp3");
}

#[test]
fn r_prefills_rename_and_enter_renames_the_selection() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj");
  assert_eq!(selected(&app), "file2.txt");
  press(&mut app, "r");
  assert_eq!(app.console.as_ref().unwrap().input(), "rename file2.txt");
  // Erase the name and type a new one with a space in it.
  press(
    &mut app,
    "<BS><BS><BS><BS><BS><BS><BS><BS><BS>zz top.md<Enter>",
  );
  assert!(app.console.is_none());
  assert!(work.join("zz top.md").exists());
  assert!(!work.join("file2.txt").exists());
  assert_eq!(selected(&app), "zz top.md");
  assert_eq!(app.message.as_ref().unwrap().text, "renamed to zz top.md");
}

#[test]
fn tab_completes_the_selected_name_in_rename() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjj");
  press(&mut app, ":rename<Tab>");
  assert_eq!(app.console.as_ref().unwrap().input(), "rename ");
  press(&mut app, "<Tab>");
  assert_eq!(app.console.as_ref().unwrap().input(), "rename file10.txt");
  press(&mut app, "<C-u>rename fi<Tab>");
  assert_eq!(app.console.as_ref().unwrap().input(), "rename file10.txt");
  // An argument that is not a prefix of the name is left alone.
  press(&mut app, "<C-u>rename other<Tab>");
  assert_eq!(app.console.as_ref().unwrap().input(), "rename other");
  press(&mut app, "<C-u>reload<Tab>");
  assert_eq!(app.console.as_ref().unwrap().input(), "reload");
}

#[test]
fn rename_without_a_name_opens_the_prefilled_line() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":rename<Enter>");
  assert_eq!(app.console.as_ref().unwrap().input(), "rename alpha");
}

#[test]
fn rename_refuses_existing_targets_and_invalid_names() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj");
  press(&mut app, ":rename file10.txt<Enter>");
  assert_eq!(
    app.message.as_ref().unwrap().text,
    "rename: `file10.txt` already exists"
  );
  assert_eq!(fs::read_to_string(work.join("file2.txt")).unwrap(), "two\n");
  press(&mut app, ":rename a/b<Enter>");
  assert_eq!(
    app.message.as_ref().unwrap().text,
    "rename: invalid name `a/b`"
  );
  press(&mut app, ":rename ..<Enter>");
  assert_eq!(
    app.message.as_ref().unwrap().text,
    "rename: invalid name `..`"
  );
  assert_eq!(selected(&app), "file2.txt");
}

#[test]
fn renaming_a_directory_drops_its_stale_listing() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "l");
  press(&mut app, "h");
  press(&mut app, ":rename gamma<Enter>");
  assert_eq!(selected(&app), "gamma");
  assert!(app.listing(&work.join("alpha")).is_none());
  press(&mut app, "l");
  assert_eq!(names(&app), vec!["a.txt", "b.txt"]);
}

fn console_input(app: &App) -> String {
  app.console.as_ref().unwrap().input().to_string()
}

#[test]
fn tab_cycles_through_matching_commands() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":tog<Tab>");
  assert_eq!(console_input(&app), "toggle_help");
  press(&mut app, "<Tab>");
  assert_eq!(console_input(&app), "toggle_hidden");
  press(&mut app, "<Tab>");
  assert_eq!(console_input(&app), "toggle_mark");
  press(&mut app, "<Tab><Tab>");
  assert_eq!(console_input(&app), "toggle_help");
  press(&mut app, "<S-Tab>");
  assert_eq!(console_input(&app), "toggle_metadata");
  // Editing starts a new completion from the edited line.
  press(&mut app, "<C-u>so<Tab>");
  assert_eq!(console_input(&app), "sort ");
  press(&mut app, "<Tab>");
  assert_eq!(console_input(&app), "sort name");
  press(&mut app, "<Tab>");
  assert_eq!(console_input(&app), "sort size");
  press(&mut app, "<Enter>");
  assert_eq!(app.sort, tansu::fs::SortKey::Size);
}

#[test]
fn shift_tab_starts_from_the_last_candidate() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":sort <S-Tab>");
  assert_eq!(console_input(&app), "sort ctime");
}

#[test]
fn tab_completes_command_then_selected_name() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "j:ren<Tab>");
  assert_eq!(console_input(&app), "rename ");
  press(&mut app, "<Tab>");
  assert_eq!(console_input(&app), "rename beta");
}

#[test]
fn cd_enters_relative_absolute_and_previous_directories() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":cd alpha<Enter>");
  assert_eq!(app.cwd, work.join("alpha"));
  assert_eq!(names(&app), vec!["a.txt", "b.txt"]);
  press(&mut app, ":cd ../beta/.<Enter>");
  assert_eq!(app.cwd, work.join("beta"));
  press(&mut app, ":cd -<Enter>");
  assert_eq!(app.cwd, work.join("alpha"));
  press(&mut app, ":cd -<Enter>");
  assert_eq!(app.cwd, work.join("beta"));
  let absolute = format!(":cd {}<Enter>", work.display());
  press(&mut app, &absolute);
  assert_eq!(app.cwd, work);
}

#[test]
fn cd_to_a_file_selects_it_and_missing_paths_are_errors() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work.join("alpha"));
  press(&mut app, ":cd ../file10.txt<Enter>");
  assert_eq!(app.cwd, work);
  assert_eq!(selected(&app), "file10.txt");
  press(&mut app, ":cd nowhere<Enter>");
  assert_eq!(app.cwd, work);
  assert_eq!(
    app.message.as_ref().unwrap().text,
    format!("cd: {}: no such directory", work.join("nowhere").display())
  );
}

#[test]
fn cd_without_argument_goes_home() {
  let (_tmp, work) = fixture();
  let mut app = App::new(
    Config {
      icons: false,
      vcs_aware: false,
      ..Config::default()
    },
    Keymap::default(),
    work.clone(),
    None,
    "tester@box".to_string(),
    Some(work.join("beta")),
  );
  app.set_size(WIDTH, HEIGHT);
  pump(&mut app);
  press(&mut app, ":cd<Enter>");
  assert_eq!(app.cwd, work.join("beta"));
  press(&mut app, ":cd ~/../alpha<Enter>");
  assert_eq!(app.cwd, work.join("alpha"));
  press(&mut app, ":cd -<Enter>");
  assert_eq!(app.cwd, work.join("beta"));
}

#[test]
fn tab_completes_cd_directories() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":cd <Tab>");
  assert_eq!(console_input(&app), "cd alpha/");
  press(&mut app, "<Tab>");
  assert_eq!(console_input(&app), "cd beta/");
  press(&mut app, "<C-u>cd b<Tab>");
  assert_eq!(console_input(&app), "cd beta/");
  press(&mut app, "<Enter>");
  assert_eq!(app.cwd, work.join("beta"));
}

#[test]
fn z_queries_zoxide_and_enters_the_match() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":z foo bar");
  press_keys_without_pump(&mut app, "<Enter>");
  assert_eq!(
    app.take_commands(),
    vec![Command::Zoxide {
      keywords: vec!["foo".into(), "bar".into()],
      exclude: work.clone(),
    }]
  );
  app.handle_event(tansu::event::Event::Zoxide(Ok(work.join("alpha"))));
  pump(&mut app);
  assert_eq!(app.cwd, work.join("alpha"));
  app.handle_event(tansu::event::Event::Zoxide(Err(
    "z: no match for `nope`".into(),
  )));
  assert_eq!(app.cwd, work.join("alpha"));
  assert_eq!(app.message.as_ref().unwrap().text, "z: no match for `nope`");
}

#[test]
fn z_with_a_directory_or_dash_works_like_cd() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, ":z beta");
  press_keys_without_pump(&mut app, "<Enter>");
  assert!(
    !app
      .take_commands()
      .iter()
      .any(|c| matches!(c, Command::Zoxide { .. }))
  );
  pump(&mut app);
  assert_eq!(app.cwd, work.join("beta"));
  press(&mut app, ":z -<Enter>");
  assert_eq!(app.cwd, work);
}

#[test]
fn entering_directories_records_them_in_zoxide_unless_disabled() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  // The start directory is not recorded; the shell already did that.
  press_keys_without_pump(&mut app, "l");
  let cmds = app.take_commands();
  assert!(cmds.contains(&Command::ZoxideAdd(work.join("alpha"))));
  pump(&mut app);
  // Moving within a directory records nothing.
  press_keys_without_pump(&mut app, "j");
  assert!(
    !app
      .take_commands()
      .iter()
      .any(|c| matches!(c, Command::ZoxideAdd(_)))
  );

  let mut app = App::new(
    Config {
      zoxide_add: false,
      ..Config::default()
    },
    Keymap::default(),
    work.clone(),
    None,
    "tester@box".to_string(),
    None,
  );
  pump(&mut app);
  press_keys_without_pump(&mut app, "l");
  assert!(
    !app
      .take_commands()
      .iter()
      .any(|c| matches!(c, Command::ZoxideAdd(_)))
  );
}

// ---- copy, cut and paste ----

fn message(app: &App) -> String {
  app.message.as_ref().unwrap().text.clone()
}

#[test]
fn yy_and_pp_copy_the_selection_without_overwriting() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjyy");
  assert_eq!(message(&app), "copied file2.txt");
  press(&mut app, "ggjl");
  assert_eq!(app.cwd, work.join("beta"));
  press(&mut app, "pp");
  assert_eq!(message(&app), "copied file2.txt");
  assert_eq!(selected(&app), "file2.txt");
  assert_eq!(
    fs::read_to_string(work.join("beta/file2.txt")).unwrap(),
    "two\n"
  );
  assert!(work.join("file2.txt").exists(), "the source is kept");

  // The buffer stays, and existing names get ranger's suffixes.
  press(&mut app, "pp");
  assert_eq!(selected(&app), "file2.txt_");
  press(&mut app, "pp");
  assert_eq!(selected(&app), "file2.txt_0");
  let mut listed = names(&app);
  listed.sort();
  assert_eq!(listed, vec!["file2.txt", "file2.txt_", "file2.txt_0"]);
}

#[test]
fn dd_and_pp_move_the_selection_and_empty_the_buffer() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjdd");
  assert_eq!(message(&app), "cut file2.txt");
  press(&mut app, "ggjlpp");
  assert_eq!(message(&app), "moved file2.txt");
  assert_eq!(
    fs::read_to_string(work.join("beta/file2.txt")).unwrap(),
    "two\n"
  );
  assert!(!work.join("file2.txt").exists());
  assert!(app.buffer.is_none());
  press(&mut app, "h");
  assert!(!names(&app).contains(&"file2.txt".to_string()));
  press(&mut app, "pp");
  assert_eq!(message(&app), "nothing to paste");
}

#[test]
fn po_overwrites_existing_entries() {
  let (_tmp, work) = fixture();
  fs::write(work.join("beta/file2.txt"), "old\n").unwrap();
  let mut app = new_app(&work);
  press(&mut app, "jjjyyggjlpo");
  assert_eq!(
    fs::read_to_string(work.join("beta/file2.txt")).unwrap(),
    "two\n"
  );
  assert_eq!(names(&app), vec!["file2.txt"]);
}

#[test]
fn uy_and_ud_clear_the_buffer() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "yyuy");
  assert!(app.buffer.is_none());
  press(&mut app, "ddud");
  assert!(app.buffer.is_none());
}

#[test]
fn pasting_into_the_same_directory() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjyypp");
  assert_eq!(selected(&app), "file2.txt_");
  assert_eq!(
    fs::read_to_string(work.join("file2.txt_")).unwrap(),
    "two\n"
  );
  // Moving an entry onto itself leaves it alone.
  press(&mut app, "ggjjjddpp");
  assert_eq!(fs::read_to_string(work.join("file2.txt")).unwrap(), "two\n");
  assert_eq!(selected(&app), "file2.txt");
}

#[test]
fn directories_copy_recursively_and_never_into_themselves() {
  let (_tmp, work) = fixture();
  std::os::unix::fs::symlink("a.txt", work.join("alpha/link")).unwrap();
  let mut app = new_app(&work);
  press(&mut app, "yyl");
  assert_eq!(app.cwd, work.join("alpha"));
  press(&mut app, "pp");
  assert_eq!(message(&app), "paste: cannot paste `alpha` into itself");
  assert!(!work.join("alpha/alpha").exists());

  press(&mut app, "hjlpp");
  let copy = work.join("beta/alpha");
  assert_eq!(fs::read_to_string(copy.join("a.txt")).unwrap(), "a\n");
  assert!(copy.join(".secret").exists());
  let link = copy.join("link");
  assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
  assert_eq!(fs::read_link(link).unwrap(), Path::new("a.txt"));
}

#[test]
fn overwriting_a_directory_merges_into_it() {
  let (_tmp, work) = fixture();
  fs::create_dir(work.join("beta/alpha")).unwrap();
  fs::write(work.join("beta/alpha/a.txt"), "old\n").unwrap();
  fs::write(work.join("beta/alpha/x.txt"), "x\n").unwrap();
  let mut app = new_app(&work);
  press(&mut app, "ddjlpo");
  assert_eq!(message(&app), "moved alpha");
  let merged = work.join("beta/alpha");
  assert_eq!(fs::read_to_string(merged.join("a.txt")).unwrap(), "a\n");
  assert_eq!(fs::read_to_string(merged.join("b.txt")).unwrap(), "b\n");
  assert_eq!(fs::read_to_string(merged.join("x.txt")).unwrap(), "x\n");
  assert!(!work.join("alpha").exists());
}

#[test]
fn a_failed_paste_is_reported_and_the_cut_buffer_kept() {
  let (_tmp, work) = fixture();
  fs::create_dir(work.join("beta/file2.txt")).unwrap();
  let mut app = new_app(&work);
  press(&mut app, "jjjddggjlpo");
  assert_eq!(
    message(&app),
    "paste: file2.txt: cannot overwrite a directory with a non-directory"
  );
  assert!(work.join("file2.txt").exists());
  assert!(app.buffer.is_some());
}

#[test]
fn yank_keys_put_paths_and_names_on_the_clipboard() {
  use tansu::clipboard::Content;
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjj"); // file10.txt
  let yank = |app: &mut App, keys: &str| {
    press_keys_without_pump(app, keys);
    app.take_commands()
  };
  let path = work.join("file10.txt");
  assert_eq!(
    yank(&mut app, "yp"),
    vec![Command::Clipboard(Content::Text(
      path.to_string_lossy().into_owned()
    ))]
  );
  assert_eq!(message(&app), format!("yanked {}", path.display()));
  assert_eq!(
    yank(&mut app, "yd"),
    vec![Command::Clipboard(Content::Text(
      work.to_string_lossy().into_owned()
    ))]
  );
  assert_eq!(
    yank(&mut app, "yn"),
    vec![Command::Clipboard(Content::Text("file10.txt".into()))]
  );
  assert_eq!(
    yank(&mut app, "y."),
    vec![Command::Clipboard(Content::Text("file10".into()))]
  );
  assert_eq!(
    yank(&mut app, "yf"),
    vec![Command::Clipboard(Content::Files(vec![path]))]
  );
  assert_eq!(message(&app), "yanked file file10.txt");
}

#[test]
fn command_line_shell_queues_command_in_current_directory() {
  use tansu::shell::ShellMode;
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjj"); // file10.txt
  press(&mut app, ":shell -w wc -l %f");
  press_keys_without_pump(&mut app, "<Enter>");
  assert_eq!(
    app.take_commands(),
    vec![Command::Shell {
      script: "wc -l 'file10.txt'".into(),
      cwd: work.clone(),
      mode: ShellMode::Foreground { wait_enter: true },
      program: "wc".into(),
    }]
  );

  // `!` and `s` open the command line as `shell `, as does a bare `:shell`.
  for keys in ["!", "s", ":shell<Enter>"] {
    press(&mut app, keys);
    assert_eq!(app.console.as_ref().unwrap().input(), "shell ", "{keys}");
    press(&mut app, "<Esc>");
  }

  // In an empty directory `%f` has nothing to expand to, but other commands still run.
  press(&mut app, "gg");
  press(&mut app, "jl"); // beta/
  assert_eq!(app.cwd, work.join("beta"));
  press(&mut app, ":shell cat %f<Enter>");
  assert_eq!(
    app.message.as_ref().unwrap().text,
    "shell: %f: nothing selected"
  );
  press(&mut app, ":shell -s touch new");
  press_keys_without_pump(&mut app, "<Enter>");
  assert_eq!(
    app.take_commands(),
    vec![Command::Shell {
      script: "touch new".into(),
      cwd: work.join("beta"),
      mode: ShellMode::Silent,
      program: "touch".into(),
    }]
  );
}

#[test]
fn which_key_shows_what_can_follow_a_prefix() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  let screen = |app: &App| render_sized(app, 100, HEIGHT).join("\n");
  let status = |app: &App| render_sized(app, 100, HEIGHT).pop().unwrap();
  assert!(!status(&app).starts_with("o-"));
  press(&mut app, "o");
  let shown = screen(&app);
  assert!(shown.contains(" o- "), "{shown}");
  assert!(shown.contains("n : Sort by name"), "{shown}");
  assert!(shown.contains("s : Sort by size"), "{shown}");
  // The status bar shows the prefix instead of the entry's metadata.
  assert!(status(&app).starts_with("o-"), "{}", status(&app));

  // Finishing or breaking off the sequence closes it.
  press(&mut app, "m");
  assert!(!screen(&app).contains("Sort by name"));
  press(&mut app, "g<Esc>");
  assert!(!screen(&app).contains("Jump to first entry"));

  // Turned off, only the status bar prefix remains.
  app.config.which_key = false;
  press(&mut app, "o");
  assert!(!screen(&app).contains("Sort by name"));
  assert!(status(&app).starts_with("o-"));
}

#[test]
fn shell_c_macro_expands_to_the_copy_buffer() {
  use tansu::shell::ShellMode;
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjjyy"); // file2.txt
  press(&mut app, ":shell -f cp %c beta");
  press_keys_without_pump(&mut app, "<Enter>");
  let quoted = format!("'{}'", work.join("file2.txt").display());
  assert_eq!(
    app.take_commands(),
    vec![Command::Shell {
      script: format!("cp {quoted} beta").into(),
      cwd: work.clone(),
      mode: ShellMode::Detached,
      program: "cp".into(),
    }]
  );
}

fn marked_names(app: &App) -> Vec<String> {
  app
    .marked_entries()
    .iter()
    .map(|e| e.name.clone())
    .collect()
}

#[test]
fn space_marks_the_selection_and_moves_down() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj<Space><Space>");
  assert_eq!(marked_names(&app), vec!["file2.txt", "file10.txt"]);
  assert_eq!(selected(&app), "好きな歌のタイトル.mp3");
  // Pressing it on a marked entry unmarks it.
  press(&mut app, "k<Space>");
  assert_eq!(marked_names(&app), vec!["file2.txt"]);
  // On the last entry the cursor stays.
  press(&mut app, "G<Space>");
  assert_eq!(selected(&app), "好きな歌のタイトル.mp3");
  assert_eq!(marked_names(&app).len(), 2);
}

#[test]
fn v_toggles_the_marks_of_every_visible_entry() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "v");
  assert_eq!(marked_names(&app), names(&app));
  assert!(
    !app.is_marked(&work.join(".dotfile")),
    "hidden entries stay unmarked"
  );
  press(&mut app, "v");
  assert!(marked_names(&app).is_empty());
  press(&mut app, "v");
  assert_eq!(marked_names(&app).len(), names(&app).len());
  press(&mut app, "v");
  // Like ranger, existing marks are inverted.
  press(&mut app, "<Space>v");
  assert!(!app.is_marked(&work.join("alpha")));
  assert_eq!(marked_names(&app).len(), names(&app).len() - 1);
}

#[test]
fn marks_are_kept_per_directory() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj<Space>gglv");
  assert_eq!(marked_names(&app), vec!["a.txt", "b.txt"]);
  press(&mut app, "h");
  assert_eq!(marked_names(&app), vec!["file2.txt"]);
  // `uv` clears only the current directory's marks.
  press(&mut app, "<Space>uv");
  assert!(marked_names(&app).is_empty());
  assert!(app.is_marked(&work.join("alpha/a.txt")));
}

#[test]
fn yy_and_dd_act_on_all_marked_entries() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj<Space><Space>yy");
  assert_eq!(message(&app), "copied 2 items");
  press(&mut app, "ggjlpp");
  assert_eq!(message(&app), "copied 2 items");
  let mut listed = names(&app);
  listed.sort();
  assert_eq!(listed, vec!["file10.txt", "file2.txt"]);

  // Moved entries lose their marks.
  press(&mut app, "v");
  assert_eq!(marked_names(&app).len(), 2);
  press(&mut app, "ddhpp");
  assert_eq!(message(&app), "moved 2 items");
  assert!(!work.join("beta/file2.txt").exists());
  assert!(
    app.is_marked(&work.join("file2.txt")),
    "the original was marked before"
  );
  press(&mut app, "ggjl");
  assert!(names(&app).is_empty());
  assert!(!app.is_marked(&work.join("beta/file2.txt")));
}

#[test]
fn shell_macros_use_the_marked_entries() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj<Space><Space>");
  app.run_command_line("shell -f echo %s");
  let cmds = app.take_commands();
  let Some(Command::Shell { script, .. }) = cmds.last() else {
    panic!("no shell command: {cmds:?}");
  };
  assert_eq!(script, "echo 'file2.txt' 'file10.txt'");
}

#[test]
fn marked_entries_are_bold_in_the_marked_color() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj<Space>");
  let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
  terminal.draw(|f| tansu::ui::render(f, &app)).unwrap();
  let buf = terminal.backend().buffer();
  // `file2.txt` is the fourth row of the current column.
  let cell = &buf[(12, 5)];
  assert_eq!(cell.fg, app.config.theme.marked.0);
  assert!(cell.modifier.contains(Modifier::BOLD));
  assert!(!buf[(12, 6)].modifier.contains(Modifier::BOLD));
  assert!(render(&app)[HEIGHT as usize - 1].contains("1 marked"));
}

// Mouse tests use the default 60x12 layout: 8 list rows starting at y=2, the parent column at
// x=1..8, the current column at x=9..30 and the preview column at x=31..56.
const PARENT_X: u16 = 3;
const CURRENT_X: u16 = 15;
const PREVIEW_X: u16 = 35;
const FIRST_ROW: u16 = 2;

fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
  app.handle_event(Event::Mouse(MouseEvent {
    kind,
    column,
    row,
    modifiers: KeyModifiers::NONE,
  }));
}

fn left(app: &mut App, column: u16, row: u16) {
  mouse(app, MouseEventKind::Down(MouseButton::Left), column, row);
}

fn right(app: &mut App, column: u16, row: u16) {
  mouse(app, MouseEventKind::Down(MouseButton::Right), column, row);
}

#[test]
fn mouse_layout_matches_rendering() {
  let (_tmp, work) = fixture();
  let app = new_app(&work);
  let screen = render(&app);
  let row: Vec<char> = screen[FIRST_ROW as usize].chars().collect();
  assert_eq!(row[0], '│');
  assert_eq!(row[8], '│', "separator between parent and current");
  assert_eq!(row[30], '│', "separator between current and preview");
}

#[test]
fn click_selects_and_double_or_right_click_opens() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  left(&mut app, CURRENT_X, FIRST_ROW + 3);
  assert_eq!(selected(&app), "file2.txt");
  pump(&mut app);
  left(&mut app, CURRENT_X, FIRST_ROW + 3);
  assert_eq!(
    app.take_commands(),
    vec![Command::Open(work.join("file2.txt"))]
  );

  right(&mut app, CURRENT_X, FIRST_ROW + 4);
  assert_eq!(selected(&app), "file10.txt");
  let cmds = app.take_commands();
  assert!(cmds.contains(&Command::Open(work.join("file10.txt"))));

  // A single click after a double click starts over.
  left(&mut app, CURRENT_X, FIRST_ROW + 1);
  pump(&mut app);
  left(&mut app, CURRENT_X, FIRST_ROW + 2);
  assert_eq!(selected(&app), "blob.bin");
  assert!(
    !app
      .take_commands()
      .iter()
      .any(|c| matches!(c, Command::Open(_)))
  );
}

#[test]
fn clicks_outside_entries_are_ignored() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "j");
  for (x, y) in [
    (CURRENT_X, FIRST_ROW + 7), // below the last entry
    (8, FIRST_ROW),             // column separator
    (30, FIRST_ROW),            // column separator
    (0, FIRST_ROW),             // border
    (CURRENT_X, 0),             // header
    (CURRENT_X, HEIGHT - 1),    // status bar
  ] {
    left(&mut app, x, y);
    assert_eq!(selected(&app), "beta", "click at ({x}, {y})");
    assert_eq!(app.cwd, work);
  }
}

#[test]
fn wheel_moves_the_cursor() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  // A directory preview does not scroll on its own, so the wheel moves the cursor.
  mouse(&mut app, MouseEventKind::ScrollDown, PREVIEW_X, FIRST_ROW);
  assert_eq!(selected(&app), "file2.txt");
  mouse(&mut app, MouseEventKind::ScrollDown, CURRENT_X, FIRST_ROW);
  assert_eq!(app.cursor().index, 5);
  mouse(&mut app, MouseEventKind::ScrollUp, CURRENT_X, FIRST_ROW);
  assert_eq!(app.cursor().index, 2);
}

#[test]
fn parent_column_click_and_wheel() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "l");
  assert_eq!(app.cwd, work.join("alpha"));

  // The wheel over the parent column steps through sibling directories.
  mouse(&mut app, MouseEventKind::ScrollDown, PARENT_X, FIRST_ROW);
  pump(&mut app);
  assert_eq!(app.cwd, work.join("beta"));
  mouse(&mut app, MouseEventKind::ScrollDown, PARENT_X, FIRST_ROW);
  pump(&mut app);
  assert_eq!(app.cwd, work.join("beta"), "no directory after beta");
  mouse(&mut app, MouseEventKind::ScrollUp, PARENT_X, FIRST_ROW);
  pump(&mut app);
  assert_eq!(app.cwd, work.join("alpha"));

  // A click goes to the parent with the clicked entry selected.
  left(&mut app, PARENT_X, FIRST_ROW + 4);
  pump(&mut app);
  assert_eq!(app.cwd, work);
  assert_eq!(selected(&app), "file10.txt");
}

#[test]
fn preview_column_click_enters_the_directory() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  assert_eq!(selected(&app), "alpha");
  left(&mut app, PREVIEW_X, FIRST_ROW + 1);
  pump(&mut app);
  assert_eq!(app.cwd, work.join("alpha"));
  assert_eq!(selected(&app), "b.txt");

  // A right click on a file's preview opens the file.
  right(&mut app, PREVIEW_X, FIRST_ROW);
  assert!(
    app
      .take_commands()
      .contains(&Command::Open(work.join("alpha/b.txt")))
  );
}

#[test]
fn mouse_with_help_or_command_line_open() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "?");
  left(&mut app, CURRENT_X, FIRST_ROW + 1);
  assert!(!app.show_help);
  assert_eq!(
    selected(&app),
    "alpha",
    "the closing click is not also a selection"
  );

  press(&mut app, ":");
  left(&mut app, CURRENT_X, FIRST_ROW + 1);
  mouse(&mut app, MouseEventKind::ScrollDown, CURRENT_X, FIRST_ROW);
  assert!(app.console.is_some());
  assert_eq!(selected(&app), "alpha");
}

#[test]
fn click_resets_a_pending_key_sequence() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "g");
  assert!(app.keymap.is_pending());
  left(&mut app, CURRENT_X, FIRST_ROW + 1);
  assert!(!app.keymap.is_pending());
  press(&mut app, "g");
  assert_eq!(
    selected(&app),
    "beta",
    "a lone g after the click does nothing"
  );
}

#[test]
fn double_click_opens_even_when_the_first_click_scrolls() {
  let (_tmp, work) = fixture();
  let long = work.join("long");
  fs::create_dir(&long).unwrap();
  for i in 0..20 {
    fs::write(long.join(format!("f{i:02}")), "").unwrap();
  }
  let mut app = new_app(&long);
  left(&mut app, CURRENT_X, FIRST_ROW + 7);
  let first = selected(&app);
  assert_eq!(first, "f07");
  assert_ne!(app.cursor().offset, 0, "the click scrolled the list");
  left(&mut app, CURRENT_X, FIRST_ROW + 7);
  assert!(
    app
      .take_commands()
      .contains(&Command::Open(long.join(first)))
  );
}

/// A directory with `long.txt` (lines `line 0` to `line 99`), selected, and `short.txt`.
fn long_text_app() -> (TempDir, App) {
  let tmp = tempfile::tempdir().unwrap();
  let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
  fs::write(tmp.path().join("long.txt"), text).unwrap();
  fs::write(tmp.path().join("short.txt"), "short\n").unwrap();
  let app = new_app(tmp.path());
  (tmp, app)
}

fn preview_scroll(app: &App) -> usize {
  app.preview.as_ref().unwrap().scroll
}

fn preview_lines(app: &App) -> usize {
  match &app.preview.as_ref().unwrap().status {
    PreviewStatus::Ready(p) => match &**p {
      Preview::Text { lines, .. } => lines.len(),
      other => panic!("unexpected {other:?}"),
    },
    other => panic!("unexpected {other:?}"),
  }
}

#[test]
fn keys_scroll_the_text_preview_independently_of_the_cursor() {
  let (_tmp, mut app) = long_text_app();
  assert_eq!(selected(&app), "long.txt");
  press(&mut app, "<C-j><C-j><C-j>");
  assert_eq!(preview_scroll(&app), 3);
  assert_eq!(selected(&app), "long.txt", "the cursor stays");
  let screen = render(&app);
  assert!(screen[FIRST_ROW as usize].contains("line 3"));
  press(&mut app, "<C-k>");
  assert_eq!(preview_scroll(&app), 2);
  press(&mut app, "<C-k><C-k><C-k>");
  assert_eq!(preview_scroll(&app), 0, "stops at the top");

  // Another entry starts at the top again.
  press(&mut app, "<C-j>jk");
  assert_eq!(preview_scroll(&app), 0);
}

#[test]
fn scrolling_stops_at_the_end_of_a_complete_preview() {
  let (_tmp, mut app) = long_text_app();
  for _ in 0..200 {
    press(&mut app, "<C-j>");
  }
  // 100 lines in 8 rows: the last line ends up on the bottom row.
  assert_eq!(preview_scroll(&app), 92);
  let screen = render(&app);
  assert!(screen[FIRST_ROW as usize + 7].contains("line 99"));
}

#[test]
fn wheel_over_a_text_preview_scrolls_it() {
  let (_tmp, mut app) = long_text_app();
  mouse(&mut app, MouseEventKind::ScrollDown, PREVIEW_X, FIRST_ROW);
  assert_eq!(preview_scroll(&app), 3);
  assert_eq!(selected(&app), "long.txt");
  mouse(&mut app, MouseEventKind::ScrollDown, CURRENT_X, FIRST_ROW);
  assert_eq!(
    selected(&app),
    "short.txt",
    "over the current column it moves the cursor"
  );
}

#[test]
fn metadata_view_scrolls_and_toggling_resets_the_scroll() {
  let (_tmp, mut app) = long_text_app();
  app.set_size(WIDTH, 6); // two preview rows, fewer than the metadata block has
  press(&mut app, "<C-j>");
  press(&mut app, "i");
  assert_eq!(preview_scroll(&app), 0);
  let top = render_sized(&app, WIDTH, 6)[FIRST_ROW as usize].clone();
  press(&mut app, "<C-j>");
  assert_eq!(preview_scroll(&app), 1);
  let scrolled = render_sized(&app, WIDTH, 6);
  assert_ne!(scrolled[FIRST_ROW as usize], top);
}

#[test]
fn scrolling_near_the_end_of_a_truncated_preview_loads_more() {
  let (_tmp, mut app) = long_text_app();
  app.config.preview.max_lines = 10;
  press(&mut app, "R");
  assert_eq!(preview_lines(&app), 10);
  press(&mut app, "<C-j>");
  assert_eq!(
    preview_lines(&app),
    100,
    "extended previews are bounded by bytes only"
  );
  assert_eq!(preview_scroll(&app), 1, "the scroll position is kept");
  // Leaving and coming back reuses the extended preview from the cache.
  press(&mut app, "jk");
  assert_eq!(preview_lines(&app), 100);
}

#[test]
fn extended_previews_grow_by_bytes_until_the_file_is_read() {
  let tmp = tempfile::tempdir().unwrap();
  // 5000 lines of 10 bytes.
  let text: String = (0..5000).map(|i| format!("line {i:04}\n")).collect();
  fs::write(tmp.path().join("big.txt"), text).unwrap();
  let mut app = new_app(tmp.path());
  app.config.preview.max_bytes = 1024;
  press(&mut app, "R");
  let first = preview_lines(&app);
  assert!(first <= 103, "{first} lines from 1 KiB");
  // Nothing more is read until the end is within two screens.
  for _ in 0..80 {
    press(&mut app, "<C-j>");
  }
  assert_eq!(preview_lines(&app), first);
  for _ in 0..20 {
    press(&mut app, "<C-j>");
  }
  let second = preview_lines(&app);
  assert!(second > 300 && second <= 410, "{second} lines from 4 KiB");
  // Keep scrolling: the preview grows until the whole file is in.
  for _ in 0..5000 {
    press_keys_without_pump(&mut app, "<C-j>");
    pump(&mut app);
  }
  assert_eq!(preview_lines(&app), 5000);
}

#[test]
fn wheel_over_a_loading_file_preview_does_not_move_the_cursor() {
  let (_tmp, mut app) = long_text_app();
  press_keys_without_pump(&mut app, "j");
  assert!(matches!(
    app.preview.as_ref().unwrap().status,
    PreviewStatus::Loading
  ));
  mouse(&mut app, MouseEventKind::ScrollUp, PREVIEW_X, FIRST_ROW);
  assert_eq!(selected(&app), "short.txt");
}

#[test]
fn slash_opens_the_command_line_with_search() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "/");
  assert_eq!(app.console.as_ref().unwrap().input(), "search ");
  press(&mut app, "f10<Enter>");
  assert!(app.console.is_none());
  assert_eq!(selected(&app), "file10.txt");
}

#[test]
fn search_jumps_to_the_best_fuzzy_match_and_n_cycles() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  // "file" is a substring of file2.txt and file10.txt; "fil" is also fuzzy in nothing else.
  app.run_command_line("search fil");
  pump(&mut app);
  assert_eq!(
    selected(&app),
    "file2.txt",
    "equal scores keep listing order"
  );
  assert!(app.message.as_ref().unwrap().text.contains("1/2"));
  press(&mut app, "n");
  assert_eq!(selected(&app), "file10.txt");
  press(&mut app, "n");
  assert_eq!(selected(&app), "file2.txt", "wraps around");
  press(&mut app, "N");
  assert_eq!(selected(&app), "file10.txt");
}

#[test]
fn search_with_wildcards() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  app.run_command_line("search *.bin");
  pump(&mut app);
  assert_eq!(selected(&app), "blob.bin");
  app.run_command_line("search b*");
  pump(&mut app);
  assert_eq!(selected(&app), "beta");
  press(&mut app, "n");
  assert_eq!(selected(&app), "blob.bin");
}

#[test]
fn search_without_matches_or_previous_search_reports_errors() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "n");
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("no previous search")
  );
  app.run_command_line("search zzz");
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("no match for `zzz`")
  );
  assert_eq!(selected(&app), "alpha");
}

#[test]
fn search_skips_hidden_entries_and_persists_across_directories() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  app.run_command_line("search *.txt");
  pump(&mut app);
  assert_eq!(selected(&app), "file2.txt");
  press(&mut app, "gg");
  press(&mut app, "l"); // alpha: a.txt, b.txt, .secret
  press(&mut app, "n");
  assert_eq!(
    selected(&app),
    "b.txt",
    "n moves past the match under the cursor"
  );
  app.run_command_line("search secret");
  assert!(app.message.as_ref().unwrap().text.contains("no match"));
}

/// Whether the name in the current column's `row` is drawn dim.
fn row_dimmed(app: &App, row: u16) -> bool {
  let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
  terminal.draw(|f| tansu::ui::render(f, app)).unwrap();
  terminal.backend().buffer()[(CURRENT_X, FIRST_ROW + row)]
    .modifier
    .contains(Modifier::DIM)
}

#[test]
fn search_dims_non_matches_until_escape_and_n_brings_it_back() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  assert!(!row_dimmed(&app, 1), "no search, nothing dim");
  app.run_command_line("search *.txt");
  pump(&mut app);
  assert_eq!(selected(&app), "file2.txt");
  assert!(row_dimmed(&app, 0), "alpha does not match");
  assert!(!row_dimmed(&app, 4), "file10.txt matches");
  press(&mut app, "<Esc>");
  assert!(!row_dimmed(&app, 0));
  press(&mut app, "n");
  assert!(row_dimmed(&app, 0));
  // The cursor row is never dimmed, matching or not.
  press(&mut app, "gg");
  assert!(!row_dimmed(&app, 0));
}

#[test]
fn ms_marks_every_match() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "ms");
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("no previous search")
  );
  press(&mut app, "<Space>"); // alpha stays marked
  app.run_command_line("search *.txt");
  pump(&mut app);
  press(&mut app, "ms");
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("marked 2 items")
  );
  let marked: Vec<String> = app
    .marked_entries()
    .iter()
    .map(|e| e.name.clone())
    .collect();
  assert_eq!(marked, vec!["alpha", "file2.txt", "file10.txt"]);
}

/// Like `new_app`, with the trash in `<tmp>/Trash` so tests never touch the real one.
fn app_with_trash(tmp: &TempDir, start: &Path) -> App {
  let mut app = new_app(start);
  app.trash_dir = Some(tmp.path().join("Trash"));
  app
}

#[test]
fn delete_asks_first_and_any_other_key_cancels() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj");
  assert_eq!(selected(&app), "file2.txt");
  press(&mut app, "dD");
  let confirm = app.confirm.clone().expect("asks before deleting");
  assert_eq!(confirm.names, vec!["file2.txt"]);
  let screen = render(&app);
  assert!(screen[HEIGHT as usize - 1].contains("Delete `file2.txt` permanently? (y/N)"));
  assert!(screen.iter().any(|l| l.contains("Delete permanently")));

  for answer in ["n", "<Enter>", "<Esc>", "Y", "<C-y>"] {
    press(&mut app, "dD");
    press(&mut app, answer);
    assert!(app.confirm.is_none());
    assert!(
      work.join("file2.txt").exists(),
      "`{answer}` does not delete"
    );
  }
}

#[test]
fn y_deletes_the_marked_entries_recursively() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "<Space>jj<Space>"); // alpha/, then <Space> moved down: file2.txt
  press(&mut app, "dD");
  assert_eq!(
    app.confirm.as_ref().unwrap().names,
    vec!["alpha/", "file2.txt"]
  );
  press(&mut app, "y");
  assert!(!work.join("alpha").exists());
  assert!(!work.join("file2.txt").exists());
  assert!(work.join("blob.bin").exists());
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("deleted 2 items")
  );
  assert!(app.marked_entries().is_empty());
  assert!(!names(&app).contains(&"alpha".to_string()));
}

#[test]
fn deleting_a_symlink_keeps_its_target() {
  let (_tmp, work) = fixture();
  std::os::unix::fs::symlink(work.join("alpha"), work.join("link")).unwrap();
  let mut app = new_app(&work);
  app.run_command_line("search link");
  pump(&mut app);
  assert_eq!(selected(&app), "link");
  press(&mut app, "dD");
  assert_eq!(app.confirm.as_ref().unwrap().names, vec!["link"]);
  press(&mut app, "y");
  assert!(work.join("link").symlink_metadata().is_err());
  assert!(work.join("alpha/a.txt").exists());
}

#[test]
fn every_way_of_deleting_asks() {
  let (tmp, work) = fixture();
  let mut app = app_with_trash(&tmp, &work);
  app.run_command_line("delete");
  assert!(app.confirm.is_some());
  press(&mut app, "n");
  app.run_command_line("trash");
  assert!(app.confirm.as_ref().is_some_and(|c| c.trash));
  // A click answers no.
  mouse(
    &mut app,
    MouseEventKind::Down(MouseButton::Left),
    CURRENT_X,
    FIRST_ROW,
  );
  assert!(app.confirm.is_none());
  assert!(work.join("alpha").exists());
}

#[test]
fn trash_moves_entries_with_trash_info() {
  let (tmp, work) = fixture();
  let mut app = app_with_trash(&tmp, &work);
  press(&mut app, "jjj");
  press(&mut app, "dT");
  let screen = render(&app);
  assert!(screen[HEIGHT as usize - 1].contains("Move `file2.txt` to the trash? (y/N)"));
  press(&mut app, "y");
  assert!(!work.join("file2.txt").exists());
  let trash = tmp.path().join("Trash");
  assert_eq!(
    fs::read_to_string(trash.join("files/file2.txt")).unwrap(),
    "two\n"
  );
  let info = fs::read_to_string(trash.join("info/file2.txt.trashinfo")).unwrap();
  assert!(info.starts_with("[Trash Info]\n"));
  assert!(info.contains(&format!("Path={}\n", work.join("file2.txt").display())));
  assert!(info.contains("DeletionDate=20"));
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("trashed 1 item")
  );

  // A second file2.txt gets its own name in the trash.
  fs::write(work.join("file2.txt"), "again\n").unwrap();
  press(&mut app, "R");
  app.run_command_line("search file2.txt");
  pump(&mut app);
  press(&mut app, "dTy");
  assert_eq!(
    fs::read_to_string(trash.join("files/file2.txt.2")).unwrap(),
    "again\n"
  );
  assert!(trash.join("info/file2.txt.2.trashinfo").exists());
}

#[test]
fn trash_without_a_trash_directory_refuses() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "dT");
  assert!(app.confirm.is_none(), "refused before asking");
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("no trash directory")
  );
  assert!(work.join("alpha").exists());
}

#[test]
fn md_and_mf_create_and_select_entries() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "md");
  assert_eq!(app.console.as_ref().unwrap().input(), "mkdir ");
  press(&mut app, "<Esc>mf");
  assert_eq!(app.console.as_ref().unwrap().input(), "touch ");
  press(&mut app, "<Esc>");

  app.run_command_line("mkdir new dir");
  pump(&mut app);
  assert!(work.join("new dir").is_dir());
  assert_eq!(selected(&app), "new dir");

  app.run_command_line("touch notes.md");
  pump(&mut app);
  assert!(work.join("notes.md").is_file());
  assert_eq!(selected(&app), "notes.md");

  // Missing parents are created; the directory leading to the file is selected.
  app.run_command_line("touch deep/er/file.txt");
  pump(&mut app);
  assert!(work.join("deep/er/file.txt").is_file());
  assert_eq!(selected(&app), "deep");
}

#[test]
fn creating_an_existing_entry_is_an_error() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  app.run_command_line("mkdir alpha");
  pump(&mut app);
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("`alpha` already exists")
  );
  app.run_command_line("touch file2.txt");
  pump(&mut app);
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("`file2.txt` already exists")
  );
  assert_eq!(fs::read_to_string(work.join("file2.txt")).unwrap(), "two\n");
  app.run_command_line("mkdir ./");
  assert!(app.message.as_ref().unwrap().text.contains("invalid name"));
}

#[test]
fn the_answer_deletes_what_was_listed_even_after_the_view_changed() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  press(&mut app, "jjj");
  press(&mut app, "dD");
  // A directory change while the question is open (e.g. from a background event).
  app.enter_dir(work.join("alpha"), None);
  pump(&mut app);
  assert_eq!(selected(&app), "a.txt");
  press(&mut app, "y");
  assert!(!work.join("file2.txt").exists());
  assert!(work.join("alpha/a.txt").exists());
  assert!(work.join("alpha/b.txt").exists());
}

#[test]
fn mkdir_and_touch_expand_a_leading_tilde() {
  let (_tmp, work) = fixture();
  let mut app = new_app(&work);
  app.home = Some(work.join("beta"));
  app.run_command_line("mkdir ~/made");
  pump(&mut app);
  app.run_command_line("touch ~/made/file");
  pump(&mut app);
  assert!(work.join("beta/made/file").is_file());
  assert!(!work.join("~").exists(), "no directory literally named ~");

  app.home = None;
  app.run_command_line("mkdir ~/other");
  pump(&mut app);
  assert!(
    app
      .message
      .as_ref()
      .unwrap()
      .text
      .contains("home directory unknown")
  );
  assert!(!work.join("~").exists());
}
