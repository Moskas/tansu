//! Writing to the system clipboard through `wl-copy` (Wayland) or `xclip`/`xsel` (X11).

use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// What is put on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
  /// Plain text, such as a path or a file name.
  Text(String),
  /// Files, offered as `text/uri-list` so GUI programs paste the files themselves.
  Files(Vec<PathBuf>),
}

const URI_LIST: &str = "text/uri-list";

/// `file://` URIs for `paths`, one per line with CRLF endings as RFC 2483 asks.
pub fn uri_list(paths: &[PathBuf]) -> String {
  paths
    .iter()
    .map(|p| format!("file://{}\r\n", percent_encode(p.as_os_str().as_bytes())))
    .collect()
}

/// Percent-encodes everything except unreserved characters and `/`.
fn percent_encode(bytes: &[u8]) -> String {
  let mut out = String::with_capacity(bytes.len());
  for &b in bytes {
    if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
      out.push(char::from(b));
    } else {
      out.push_str(&format!("%{b:02X}"));
    }
  }
  out
}

/// Clipboard programs to try, in order, for the current session.
fn candidates(files: bool) -> Vec<Vec<&'static str>> {
  let set = |var: &str| std::env::var_os(var).is_some_and(|v| !v.is_empty());
  let mut out = Vec::new();
  if set("WAYLAND_DISPLAY") {
    let mut argv = vec!["wl-copy"];
    if files {
      argv.extend(["--type", URI_LIST]);
    }
    out.push(argv);
  }
  if set("DISPLAY") {
    let mut argv = vec!["xclip", "-selection", "clipboard", "-in"];
    if files {
      argv.extend(["-target", URI_LIST]);
    }
    out.push(argv);
    // xsel cannot set a content type, so it only takes text.
    if !files {
      out.push(vec!["xsel", "--clipboard", "--input"]);
    }
  }
  out
}

/// How content reaches the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
  /// A clipboard program, given the data on stdin.
  Program(Vec<&'static str>),
  /// The OSC 52 escape sequence, which the caller writes to the terminal.
  Osc52,
}

/// Picks the first clipboard program on `PATH` for the current session. Without one, text
/// falls back to OSC 52; files cannot, since OSC 52 carries no content type.
pub fn backend(content: &Content) -> Result<Backend, String> {
  let files = matches!(content, Content::Files(_));
  if let Some(argv) = candidates(files).into_iter().find(|argv| on_path(argv[0])) {
    return Ok(Backend::Program(argv));
  }
  if files {
    return Err("clipboard: yanking files needs wl-copy or xclip and a display".into());
  }
  Ok(Backend::Osc52)
}

fn on_path(program: &str) -> bool {
  std::env::var_os("PATH")
    .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The bytes put on the clipboard.
fn data(content: &Content) -> String {
  match content {
    Content::Text(text) => text.clone(),
    Content::Files(paths) => uri_list(paths),
  }
}

/// Runs a clipboard program with `content` on its stdin and waits for it.
///
/// The programs keep running in the background to serve the clipboard, holding on to the
/// descriptors they inherited, so their output is discarded rather than read.
pub fn run(argv: &[&str], content: &Content) -> Result<(), String> {
  let mut child = Command::new(argv[0])
    .args(&argv[1..])
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .process_group(0)
    .spawn()
    .map_err(|e| format!("clipboard: could not run {}: {e}", argv[0]))?;
  if let Some(mut stdin) = child.stdin.take() {
    let _ = stdin.write_all(data(content).as_bytes());
  }
  match child.wait() {
    Ok(status) if status.success() => Ok(()),
    Ok(status) => Err(format!("clipboard: {} failed ({status})", argv[0])),
    Err(e) => Err(format!("clipboard: {}: {e}", argv[0])),
  }
}

/// The OSC 52 sequence asking the terminal to put `content` on the system clipboard.
pub fn osc52(content: &Content) -> String {
  format!("\x1b]52;c;{}\x07", base64(data(content).as_bytes()))
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
  const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
  for chunk in bytes.chunks(3) {
    let n = chunk
      .iter()
      .enumerate()
      .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
    for i in 0..4 {
      if i <= chunk.len() {
        out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
      } else {
        out.push('=');
      }
    }
  }
  out
}
