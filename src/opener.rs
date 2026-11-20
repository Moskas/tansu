//! Launching files with an external program, either detached from the TUI or in the
//! foreground with the terminal handed over to it.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use crate::terminal;

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
  #[error("no opener configured")]
  NoOpener,
  #[error("cannot open a terminal window: no display found")]
  NoDisplay,
  #[error("failed to run `{program}`: {source}")]
  Spawn {
    program: String,
    source: std::io::Error,
  },
}

/// Names of the executables in the `PATH` directories, sorted and deduplicated.
pub fn path_programs() -> Vec<String> {
  std::env::var_os("PATH").map_or_else(Vec::new, |p| programs_in(&p))
}

/// Names of the executables in the directories of a `PATH`-style list.
pub fn programs_in(path: &OsStr) -> Vec<String> {
  let mut names: Vec<String> = std::env::split_paths(path)
    .filter_map(|dir| std::fs::read_dir(dir).ok())
    .flatten()
    .flatten()
    // `metadata` follows symlinks, which is how Nix profiles expose their programs.
    .filter(|e| {
      std::fs::metadata(e.path()).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
    .filter_map(|e| e.file_name().into_string().ok())
    .collect();
  names.sort();
  names.dedup();
  names
}

/// Runs `opener <path>` detached. `opener` is split on whitespace.
pub fn open(opener: &str, path: &Path) -> Result<(), OpenError> {
  let argv: Vec<String> = opener.split_whitespace().map(str::to_string).collect();
  spawn_detached(&argv, path)
}

/// Runs `argv` with `path` appended, in its own process group with null stdio, so it neither
/// draws over the TUI nor receives the terminal's signals. A thread reaps the child when it
/// exits.
pub fn spawn_detached(argv: &[String], path: &Path) -> Result<(), OpenError> {
  let (program, cmd) = command(argv, path)?;
  detach(program, cmd, || {})
}

/// Spawns `cmd` detached; the reaping thread calls `on_exit` once it has exited.
fn detach(
  program: &str,
  mut cmd: Command,
  on_exit: impl FnOnce() + Send + 'static,
) -> Result<(), OpenError> {
  let mut child = cmd
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .process_group(0)
    .spawn()
    .map_err(spawn_error(program))?;
  thread::spawn(move || {
    let _ = child.wait();
    on_exit();
  });
  Ok(())
}

/// Runs `argv` with `path` appended on the terminal and waits for it. The caller must have
/// released the terminal (cooked mode, main screen) and stopped reading input.
///
/// The child stays in tansu's process group, so it is the terminal's foreground job and can
/// read input. Terminal signals (Ctrl-C, Ctrl-\) then reach both processes, so tansu ignores
/// them until the child exits.
pub fn run_foreground(argv: &[String], path: &Path) -> Result<ExitStatus, OpenError> {
  let (program, cmd) = command(argv, path)?;
  wait_on_terminal(program, cmd, false)
}

/// Spawns `cmd` on the terminal and waits for it, then, with `wait_enter`, for the Enter key.
fn wait_on_terminal(
  program: &str,
  mut cmd: Command,
  wait_enter: bool,
) -> Result<ExitStatus, OpenError> {
  let mut child = cmd
    .stdin(terminal::child_stdin())
    .stdout(terminal::child_stdout())
    .spawn()
    .map_err(spawn_error(program))?;
  // Installed after the spawn so the child keeps the default dispositions. They stay
  // ignored through the Enter prompt, since tansu is not restoring the terminal on Ctrl-C.
  let _signals = IgnoreTerminalSignals::install();
  let status = child.wait().map_err(spawn_error(program))?;
  if wait_enter {
    press_enter();
  }
  Ok(status)
}

/// Waits for a line on the cooked terminal, like ranger after `:shell -w`.
fn press_enter() {
  let mut out = terminal::TermOut;
  let _ = write!(out, "Press ENTER to continue");
  let _ = out.flush();
  terminal::read_line();
}

fn command<'a>(argv: &'a [String], path: &Path) -> Result<(&'a str, Command), OpenError> {
  let (program, args) = argv.split_first().ok_or(OpenError::NoOpener)?;
  let mut cmd = Command::new(program);
  cmd.args(args).arg(path);
  Ok((program, cmd))
}

fn spawn_error(program: &str) -> impl Fn(std::io::Error) -> OpenError + '_ {
  move |source| OpenError::Spawn {
    program: program.to_string(),
    source,
  }
}

/// The shell that runs `:shell` commands: `$SHELL`, or `/bin/sh` when it is unset or fish,
/// whose syntax differs from the POSIX shells (as in ranger).
pub fn user_shell() -> OsString {
  match std::env::var_os("SHELL") {
    Some(shell) if !shell.is_empty() && !shell.as_bytes().windows(4).any(|w| w == b"fish") => shell,
    _ => OsString::from("/bin/sh"),
  }
}

/// `<shell> -c <script>`, run in `cwd`, with the shell's name for messages.
fn shell_command(script: &OsStr, cwd: &Path) -> (String, Command) {
  let shell = user_shell();
  let mut cmd = Command::new(&shell);
  cmd.arg("-c").arg(script).current_dir(cwd);
  (shell.to_string_lossy().into_owned(), cmd)
}

/// Runs a shell script on the terminal and waits for it, like [`run_foreground`].
pub fn run_shell(script: &OsStr, cwd: &Path, wait_enter: bool) -> Result<ExitStatus, OpenError> {
  let (program, cmd) = shell_command(script, cwd);
  wait_on_terminal(&program, cmd, wait_enter)
}

/// Runs a shell script with its stdout and stderr piped into `less`, which gets the terminal.
/// Like ranger, the script is not waited for after the pager is closed: it finishes on its
/// own or dies writing to the closed pipe. Returns the script's status if it has exited,
/// otherwise the pager's.
pub fn run_shell_paged(script: &OsStr, cwd: &Path) -> Result<ExitStatus, OpenError> {
  let (program, mut cmd) = shell_command(script, cwd);
  let (reader, writer) = std::io::pipe().map_err(spawn_error(&program))?;
  let stderr = writer.try_clone().map_err(spawn_error(&program))?;
  cmd.stdin(Stdio::null()).stdout(writer).stderr(stderr);
  let mut child = cmd.spawn().map_err(spawn_error(&program))?;
  // `cmd` holds tansu's copies of the write end; the pager only sees the end of the output
  // once they are closed.
  drop(cmd);
  let mut pager = Command::new(PAGER);
  pager.stdin(reader).stdout(terminal::child_stdout());
  let spawned = pager.spawn();
  drop(pager);
  let mut pager = match spawned {
    Ok(pager) => pager,
    Err(source) => {
      let _ = child.kill();
      let _ = child.wait();
      return Err(spawn_error(PAGER)(source));
    }
  };
  let _signals = IgnoreTerminalSignals::install();
  let pager_status = pager.wait().map_err(spawn_error(PAGER))?;
  if let Ok(Some(status)) = child.try_wait() {
    return Ok(status);
  }
  thread::spawn(move || {
    let _ = child.wait();
  });
  Ok(pager_status)
}

/// Pager for `:shell -p`, as in ranger.
const PAGER: &str = "less";

/// Runs a shell script with null stdio, in its own process group, and waits for it.
pub fn run_shell_silent(script: &OsStr, cwd: &Path) -> Result<ExitStatus, OpenError> {
  let (program, mut cmd) = shell_command(script, cwd);
  cmd
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .process_group(0)
    .status()
    .map_err(spawn_error(&program))
}

/// Starts a shell script detached, like [`spawn_detached`], calling `on_exit` when it exits.
pub fn spawn_shell_detached(
  script: &OsStr,
  cwd: &Path,
  on_exit: impl FnOnce() + Send + 'static,
) -> Result<(), OpenError> {
  let (program, cmd) = shell_command(script, cwd);
  detach(&program, cmd, on_exit)
}

/// Starts a shell script detached in a new terminal window, as `<terminal> -e <script>`.
pub fn spawn_shell_in_terminal(
  script: &OsStr,
  cwd: &Path,
  on_exit: impl FnOnce() + Send + 'static,
) -> Result<(), OpenError> {
  if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
    return Err(OpenError::NoDisplay);
  }
  let mut wrapped = OsString::from(terminal_emulator());
  wrapped.push(" -e ");
  wrapped.push(script);
  spawn_shell_detached(&wrapped, cwd, on_exit)
}

/// The terminal emulator for `:shell -t`, chosen like ranger: `$TERMCMD`, `$TERM`,
/// `x-terminal-emulator` and `xterm`, the first whose program is on `PATH`.
fn terminal_emulator() -> String {
  let configured = std::env::var("TERMCMD")
    .or_else(|_| std::env::var("TERM"))
    .unwrap_or_default();
  match configured.split_whitespace().next() {
    Some(program) if is_on_path(program) => configured,
    _ if is_on_path("x-terminal-emulator") => "x-terminal-emulator".to_string(),
    _ => "xterm".to_string(),
  }
}

fn is_on_path(program: &str) -> bool {
  let executable = |p: &Path| {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
  };
  if program.contains('/') {
    return executable(Path::new(program));
  }
  std::env::var_os("PATH")
    .is_some_and(|path| std::env::split_paths(&path).any(|dir| executable(&dir.join(program))))
}

/// Ignores SIGINT and SIGQUIT while alive; restores the previous handlers on drop.
struct IgnoreTerminalSignals {
  previous: [(libc::c_int, libc::sighandler_t); 2],
}

impl IgnoreTerminalSignals {
  fn install() -> Self {
    let previous = [libc::SIGINT, libc::SIGQUIT].map(|sig| {
      // SAFETY: installing SIG_IGN has no handler code to run, so it cannot violate
      // async-signal safety.
      (sig, unsafe { libc::signal(sig, libc::SIG_IGN) })
    });
    IgnoreTerminalSignals { previous }
  }
}

impl Drop for IgnoreTerminalSignals {
  fn drop(&mut self) {
    for (sig, handler) in self.previous {
      if handler != libc::SIG_ERR {
        // SAFETY: restores the disposition that was in place before `install`.
        unsafe {
          libc::signal(sig, handler);
        }
      }
    }
  }
}
