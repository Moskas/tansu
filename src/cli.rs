//! Command line arguments, with ranger's file chooser options.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

pub const USAGE: &str = "usage: tansu [OPTIONS] [PATH]

Opens a three-column file manager at PATH (default: current directory).
Press ? inside tansu for key bindings.

Options:
  --choosefile=FILE    Opening a file writes its path to FILE and quits
  --choosefiles=FILE   Opening a file writes the marked or selected paths to FILE and quits
  --choosedir=FILE     On exit, write the current directory to FILE
  --selectfile=PATH    Start with PATH selected (same as passing PATH)
  --show-only-dirs     List directories only
  --cmd=COMMAND        Run a command line once the start directory is listed (repeatable)
  -h, --help           Show this help
  -V, --version        Show the version";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
  pub path: Option<PathBuf>,
  pub choose_file: Option<PathBuf>,
  pub choose_files: Option<PathBuf>,
  pub choose_dir: Option<PathBuf>,
  /// Command lines to run at startup, in order.
  pub cmds: Vec<String>,
  pub show_only_dirs: bool,
  pub help: bool,
  pub version: bool,
}

impl Args {
  /// Parses the arguments after the program name. Options take their value as `--opt=VALUE`
  /// or `--opt VALUE`, like ranger's.
  pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    let mut only_paths = false;
    while let Some(arg) = args.next() {
      let bytes = arg.as_bytes();
      if only_paths || !bytes.starts_with(b"-") || bytes == b"-" {
        if parsed.path.replace(PathBuf::from(&arg)).is_some() {
          return Err("too many arguments".into());
        }
        continue;
      }
      // Split on the first `=`; the value keeps its bytes, so non-UTF-8 paths work.
      let (name, inline) = match bytes.iter().position(|&b| b == b'=') {
        Some(i) => (
          &bytes[..i],
          Some(OsStr::from_bytes(&bytes[i + 1..]).to_os_string()),
        ),
        None => (bytes, None),
      };
      let name = String::from_utf8_lossy(name);
      let slot = match name.as_ref() {
        "--" => {
          only_paths = true;
          continue;
        }
        "-h" | "--help" => {
          parsed.help = true;
          continue;
        }
        "-V" | "--version" => {
          parsed.version = true;
          continue;
        }
        "--show-only-dirs" => {
          if inline.is_some() {
            return Err(format!("`{name}` takes no value"));
          }
          parsed.show_only_dirs = true;
          continue;
        }
        "--cmd" => {
          match inline.or_else(|| args.next()) {
            Some(v) if !v.is_empty() => parsed.cmds.push(v.to_string_lossy().into_owned()),
            _ => return Err(format!("`{name}` needs a value")),
          }
          continue;
        }
        "--choosefile" => &mut parsed.choose_file,
        "--choosefiles" => &mut parsed.choose_files,
        "--choosedir" => &mut parsed.choose_dir,
        "--selectfile" => &mut parsed.path,
        _ => return Err(format!("unknown option `{name}`")),
      };
      let value = match inline.or_else(|| args.next()) {
        Some(v) if !v.is_empty() => v,
        _ => return Err(format!("`{name}` needs a value")),
      };
      if slot.replace(PathBuf::from(value)).is_some() {
        return Err(format!("`{name}` repeats an earlier argument"));
      }
    }
    Ok(parsed)
  }
}
