//! App-level tests against a real temporary directory tree. Background work is executed
//! synchronously through `runtime::execute_sync`, so results are deterministic.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
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
  press(&mut app, "r");

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
  press(&mut app, "r");
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
  press(&mut app, "r");
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
