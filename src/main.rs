use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use tansu::app::{App, Command};
use tansu::cli::{Args, USAGE};
use tansu::config::Config;
use tansu::event::{self, Event, InputHandle};
use tansu::fs::meta;
use tansu::graphics::ImageEncoder;
use tansu::keymap::Keymap;
use tansu::opener;
use tansu::preview::{PreviewRegistry, ThumbnailCache};
use tansu::runtime::Runtime;
use tansu::shell::ShellMode;
use tansu::terminal::TerminalGuard;
use tansu::ui;

const TICK: Duration = Duration::from_millis(250);

fn main() -> Result<()> {
  let args = match Args::parse(std::env::args_os().skip(1)) {
    Ok(args) => args,
    Err(e) => bail!("{e}\n\n{USAGE}"),
  };
  if args.help {
    println!("{USAGE}");
    return Ok(());
  }
  if args.version {
    println!("tansu {}", env!("CARGO_PKG_VERSION"));
    return Ok(());
  }
  let choose_file = args.choose_file.map(output_path).transpose()?;
  let choose_files = args.choose_files.map(output_path).transpose()?;
  let choose_dir = args.choose_dir.map(output_path).transpose()?;
  let (start, select) = resolve_start(args.path)?;

  meta::init_timezone();
  let (config, mut warnings) = Config::load(Config::default_path().as_deref());
  let (keymap, key_warnings) = Keymap::with_overrides(&config.keys);
  warnings.extend(key_warnings);
  let dirs = directories::BaseDirs::new();
  let home = dirs.as_ref().map(|d| d.home_dir().to_path_buf());

  let mut app = App::new(config, keymap, start, select, meta::user_at_host(), home);
  app.chooser = choose_file.is_some() || choose_files.is_some();
  // $XDG_DATA_HOME/Trash, the home trash of the freedesktop.org specification.
  app.trash_dir = dirs.as_ref().map(|d| d.data_dir().join("Trash"));
  if let Some(first) = warnings.first() {
    let more = warnings.len() - 1;
    let suffix = if more > 0 {
      format!(" (+{more} more)")
    } else {
      String::new()
    };
    app.set_error(format!("{first}{suffix}"));
  }

  let mut guard = TerminalGuard::new()?;
  // The terminal query reads stdin, so it has to finish before the input thread starts.
  let images = app.config.preview_images.then(ImageEncoder::detect);
  if app.config.mouse_enabled
    && let Err(e) = guard.enable_mouse()
  {
    app.set_error(format!("{e:#}"));
  }
  let (tx, rx) = mpsc::channel();
  let input = event::spawn_input_thread(tx.clone());
  let mut registry = PreviewRegistry::configured(&app.config.highlight, app.config.preview_images);
  if let Some(dirs) = &dirs {
    registry.set_thumbnails(ThumbnailCache::new(
      dirs.cache_dir().join("tansu").join("thumbnails"),
    ));
  }
  let mut runtime = Runtime::new(
    tx,
    Arc::new(registry),
    app.config.preview,
    images,
    app.config.opener.clone(),
  );

  let size = guard.terminal.size()?;
  app.set_size(size.width, size.height);

  loop {
    // Handling a command can queue more (a foreground program reloads the directory).
    loop {
      let cmds = app.take_commands();
      if cmds.is_empty() {
        break;
      }
      for cmd in cmds {
        match cmd {
          Command::Spawn {
            argv,
            path,
            foreground: true,
          } => run_foreground(&mut guard, &input, &mut app, &argv, &path)?,
          Command::Shell {
            script,
            cwd,
            mode: mode @ (ShellMode::Foreground { .. } | ShellMode::Pager),
            program,
          } => run_shell(&mut guard, &input, &mut app, &script, &cwd, mode, &program)?,
          cmd => {
            if let Some(err) = runtime.execute(cmd) {
              app.set_error(err);
            }
          }
        }
      }
    }
    guard.terminal.draw(|f| ui::render(f, &app))?;

    let ev = match rx.recv_timeout(TICK) {
      Ok(ev) => ev,
      Err(mpsc::RecvTimeoutError::Timeout) => Event::Tick,
      Err(mpsc::RecvTimeoutError::Disconnected) => break,
    };
    app.handle_event(ev);
    // Apply everything already queued before redrawing, so bursts (key repeat, many
    // background results) cost one frame instead of one per event.
    while let Ok(ev) = rx.try_recv() {
      app.handle_event(ev);
      if app.should_quit {
        break;
      }
    }
    // A steady stream of task progress would keep `Tick` from ever arriving.
    app.tick();
    if app.should_quit {
      break;
    }
  }
  drop(guard);

  // Like ranger: the chooser files are written only when something was chosen, so a caller
  // can tell a cancelled pick from an empty file it created itself.
  if let Some(chosen) = &app.chosen {
    if let Some(file) = &choose_file {
      write_output(file, chosen.file.as_os_str().as_bytes())?;
    }
    if let Some(file) = &choose_files {
      let mut list = Vec::new();
      for path in &chosen.files {
        list.extend_from_slice(path.as_os_str().as_bytes());
        list.push(b'\n');
      }
      write_output(file, &list)?;
    }
  }
  if let Some(file) = &choose_dir {
    write_output(file, app.cwd.as_os_str().as_bytes())?;
  }
  Ok(())
}

/// Makes an output file argument absolute and rejects one that exists but is not writable,
/// before the UI starts.
fn output_path(path: PathBuf) -> Result<PathBuf> {
  let path =
    std::path::absolute(&path).with_context(|| format!("cannot resolve {}", path.display()))?;
  if path.exists() {
    // Opening for append checks write permission without changing the file.
    std::fs::OpenOptions::new()
      .append(true)
      .open(&path)
      .with_context(|| format!("cannot write to {}", path.display()))?;
  }
  Ok(path)
}

fn write_output(path: &Path, contents: &[u8]) -> Result<()> {
  std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

/// Returns the directory to open and, if `path` is a file, its name to select.
fn resolve_start(path: Option<PathBuf>) -> Result<(PathBuf, Option<String>)> {
  let path = match path {
    Some(p) => p,
    None => std::env::current_dir().context("reading current directory")?,
  };
  let path = path
    .canonicalize()
    .with_context(|| format!("cannot open {}", path.display()))?;
  if path.is_dir() {
    return Ok((path, None));
  }
  let name = path
    .file_name()
    .map(|n| tansu::fs::entry::sanitize(&n.to_string_lossy()));
  let parent = path
    .parent()
    .map(PathBuf::from)
    .unwrap_or_else(|| "/".into());
  Ok((parent, name))
}

/// Runs a program on the terminal with tansu suspended, then restores the UI.
fn run_foreground(
  guard: &mut TerminalGuard,
  input: &InputHandle,
  app: &mut App,
  argv: &[String],
  path: &Path,
) -> Result<()> {
  let program = argv.first().map_or("", String::as_str);
  suspend_for(guard, input, app, program, || {
    opener::run_foreground(argv, path)
  })
}

/// Runs a `:shell` command that needs the terminal (foreground or paged).
fn run_shell(
  guard: &mut TerminalGuard,
  input: &InputHandle,
  app: &mut App,
  script: &OsStr,
  cwd: &Path,
  mode: ShellMode,
  program: &str,
) -> Result<()> {
  suspend_for(guard, input, app, program, || match mode {
    ShellMode::Pager => opener::run_shell_paged(script, cwd),
    ShellMode::Foreground { wait_enter } => opener::run_shell(script, cwd, wait_enter),
    _ => opener::run_shell(script, cwd, false),
  })
}

/// Hands the terminal to `run` with tansu suspended, then restores the UI.
fn suspend_for(
  guard: &mut TerminalGuard,
  input: &InputHandle,
  app: &mut App,
  program: &str,
  run: impl FnOnce() -> Result<ExitStatus, opener::OpenError>,
) -> Result<()> {
  input.pause();
  let result = guard.suspended(run);
  input.resume();
  let result = result?.map_err(|e| e.to_string());
  // Resizes while the program ran were not seen.
  let size = guard.terminal.size()?;
  app.set_size(size.width, size.height);
  app.on_foreground_exit(program, result);
  Ok(())
}
