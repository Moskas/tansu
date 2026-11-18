//! Archives read through external programs: 7-Zip (`7zz` or `7z`), unrar and bsdtar
//! (libarchive). None of them may ever ask for a password, which would take over the
//! terminal: each gets an empty one, and no standard input.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use super::{Contents, ListError, Member, MemberKind};
use crate::jobs::Meter;
use crate::preview::PreviewError;
use crate::preview::thumbnail::run_command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
  SevenZip,
  Unrar,
  Bsdtar,
}

impl Tool {
  /// Program names to try, in order.
  fn programs(self) -> &'static [&'static str] {
    match self {
      Tool::SevenZip => &["7zz", "7z"],
      Tool::Unrar => &["unrar"],
      Tool::Bsdtar => &["bsdtar"],
    }
  }

  fn list_args(self, path: &Path) -> Vec<&OsStr> {
    let flags: &[&str] = match self {
      Tool::SevenZip => &["l", "-slt", "-p"],
      Tool::Unrar => &["lt", "-p-"],
      Tool::Bsdtar => &["-tvf"],
    };
    let mut args: Vec<&OsStr> = flags.iter().map(OsStr::new).collect();
    args.push(path.as_os_str());
    args
  }

  fn parse(self, text: &str, contents: &mut Contents, max: usize) -> Option<()> {
    match self {
      Tool::SevenZip => parse_7z(text, contents, max),
      Tool::Unrar => {
        parse_unrar(text, contents, max);
        Some(())
      }
      Tool::Bsdtar => {
        parse_bsdtar(text, contents, max);
        Some(())
      }
    }
  }
}

/// Lists `path` with the first of `tools` that is installed and reads it.
pub fn list(
  path: &Path,
  tools: &[Tool],
  label: &str,
  max: usize,
  cancelled: &dyn Fn() -> bool,
) -> Result<Contents, ListError> {
  let mut failure = None;
  for &tool in tools {
    for program in tool.programs() {
      let mut command = Command::new(program);
      command.args(tool.list_args(path));
      listing_locale(&mut command);
      let output = match run_command(command, cancelled) {
        Ok(Some(output)) => output,
        Ok(None) => continue,
        Err(PreviewError::Cancelled) => return Err(ListError::Cancelled),
        Err(e) => return Err(ListError::Failed(e.to_string())),
      };
      if !output.success {
        // Another program may know the format better.
        failure = Some(format!("{program} could not read it"));
        break;
      }
      let text = String::from_utf8_lossy(&output.stdout);
      let mut contents = Contents::new(label);
      if tool.parse(&text, &mut contents, max).is_some() {
        return Ok(contents);
      }
      failure = Some(format!("{program} printed an unexpected listing"));
      break;
    }
  }
  Err(failure.map_or(ListError::Unsupported, ListError::Failed))
}

/// English dates (bsdtar writes month names) while names are still decoded in the user's
/// character set.
fn listing_locale(command: &mut Command) {
  if let Some(all) = std::env::var_os("LC_ALL").filter(|v| !v.is_empty()) {
    command.env_remove("LC_ALL").env("LC_CTYPE", all);
  }
  command.env("LC_TIME", "C");
}

/// How often a running program is checked on.
const POLL: Duration = Duration::from_millis(50);

/// Waits for `child`, collecting its error output, and kills it once `meter` stops. A
/// pause does not reach the program: it runs on, and the task pauses after it.
fn wait_stoppable(mut child: Child, meter: &mut Meter) -> io::Result<(ExitStatus, Vec<u8>)> {
  // Read on the side, so a program writing a lot of errors cannot block on a full pipe.
  let mut stderr = child.stderr.take();
  let reader = std::thread::spawn(move || {
    let mut buf = Vec::new();
    if let Some(stderr) = &mut stderr {
      let _ = stderr.read_to_end(&mut buf);
    }
    buf
  });
  let status = loop {
    if let Some(status) = child.try_wait()? {
      break status;
    }
    if let Err(e) = meter.check() {
      let _ = child.kill();
      let _ = child.wait();
      let _ = reader.join();
      return Err(e);
    }
    std::thread::sleep(POLL);
  };
  Ok((status, reader.join().unwrap_or_default()))
}

/// Extracts `path` into the empty directory `dir` with the first of `tools` installed.
/// Programs run in the user's locale, so names come out in their character set. A
/// program still running when `meter` stops is killed.
pub fn extract(path: &Path, tools: &[Tool], dir: &Path, meter: &mut Meter) -> io::Result<()> {
  for &tool in tools {
    for &program in tool.programs() {
      let mut command = Command::new(program);
      match tool {
        Tool::SevenZip => {
          let mut out = std::ffi::OsString::from("-o");
          out.push(dir);
          command
            .args(["x", "-y", "-p", "-bd", "-bso0", "-bsp0"])
            .arg(out)
            .arg(path)
        }
        // A trailing `/` makes the destination a directory.
        Tool::Unrar => command
          .args(["x", "-p-", "-y", "-idq"])
          .arg(path)
          .arg(dir.join("")),
        Tool::Bsdtar => command.arg("-xf").arg(path).arg("-C").arg(dir),
      };
      command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
      let child = match command.spawn() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
        Err(e) => return Err(io::Error::other(format!("{program}: {e}"))),
        Ok(child) => child,
      };
      let (status, stderr) = wait_stoppable(child, meter)?;
      if status.success() {
        return Ok(());
      }
      let stderr = String::from_utf8_lossy(&stderr);
      let reason = stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with("bsdtar: Error exit delayed"))
        .map_or_else(|| status.to_string(), str::to_string);
      return Err(io::Error::other(format!("{program}: {reason}")));
    }
  }
  let needed = tools
    .iter()
    .map(|t| match t {
      Tool::SevenZip => "7-Zip",
      Tool::Unrar => "unrar",
      Tool::Bsdtar => "bsdtar",
    })
    .collect::<Vec<_>>()
    .join(", ");
  Err(io::Error::other(format!("needs one of {needed}")))
}

/// `YYYY-MM-DD HH:MM` from the longer timestamps 7-Zip and unrar print.
fn minutes(stamp: &str) -> Option<String> {
  let stamp = stamp.trim();
  (stamp.len() >= 16)
    .then(|| stamp.get(..16))
    .flatten()
    .map(str::to_string)
}

/// Blocks of `key = value` (7-Zip) or `key: value` (unrar) lines, separated by blank lines.
fn blocks<'a>(text: &'a str, separator: &'a str) -> impl Iterator<Item = Vec<(&'a str, &'a str)>> {
  text.split("\n\n").map(move |block| {
    block
      .lines()
      .filter_map(|line| {
        let (key, value) = line.trim().split_once(separator.trim_end())?;
        Some((key.trim(), value.trim()))
      })
      .collect()
  })
}

fn get<'a>(block: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
  block.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// `7z l -slt`: the archive's own details, a `----------` line, then one block per member.
pub fn parse_7z(text: &str, contents: &mut Contents, max: usize) -> Option<()> {
  let text = text.replace("\r\n", "\n");
  let (head, body) = text.split_once("\n----------\n")?;
  // The last block of the head describes the archive.
  if let Some(info) = blocks(head, " = ").last()
    && let Some(comment) = get(&info, "Comment").filter(|c| !c.is_empty())
  {
    contents.comment = Some(comment.to_string());
  }
  for block in blocks(body, " = ") {
    let Some(path) = get(&block, "Path") else {
      continue;
    };
    let attributes = get(&block, "Attributes").unwrap_or("");
    let unix = attributes.split_whitespace().nth(1).unwrap_or("");
    let kind = if get(&block, "Folder") == Some("+") || attributes.starts_with('D') {
      MemberKind::Dir
    } else if unix.starts_with('l') || get(&block, "Symbolic Link").is_some_and(|l| !l.is_empty()) {
      MemberKind::Link
    } else {
      MemberKind::File
    };
    let member = Member {
      path: path.to_string(),
      kind,
      size: (kind == MemberKind::File)
        .then(|| get(&block, "Size")?.parse().ok())
        .flatten(),
      modified: get(&block, "Modified").and_then(minutes),
      link: get(&block, "Symbolic Link")
        .filter(|l| !l.is_empty())
        .map(str::to_string),
      encrypted: get(&block, "Encrypted") == Some("+"),
    };
    contents.add(member, max);
  }
  Some(())
}

/// `unrar lt`: a banner, then one `Name: ...` block per member.
pub fn parse_unrar(text: &str, contents: &mut Contents, max: usize) {
  let text = text.replace("\r\n", "\n");
  for block in blocks(&text, ": ") {
    let Some(path) = get(&block, "Name") else {
      continue;
    };
    let kind = match get(&block, "Type").unwrap_or("File") {
      "Directory" => MemberKind::Dir,
      t if t.ends_with("symbolic link") || t.ends_with("junction") => MemberKind::Link,
      // Archive comments, recovery records and other data for unrar itself.
      "Service" => continue,
      _ => MemberKind::File,
    };
    let flags = get(&block, "Flags").unwrap_or("");
    let member = Member {
      path: path.replace('\\', "/"),
      kind,
      size: (kind == MemberKind::File)
        .then(|| get(&block, "Size")?.parse().ok())
        .flatten(),
      modified: get(&block, "mtime").and_then(minutes),
      link: get(&block, "Target").map(str::to_string),
      encrypted: flags.contains("encrypted"),
    };
    contents.add(member, max);
  }
}

/// `bsdtar -tv`, laid out like `ls -l`:
/// `-rw-r--r--  0 user group  1234 Nov 17 23:51 path`.
pub fn parse_bsdtar(text: &str, contents: &mut Contents, max: usize) {
  for line in text.lines() {
    let Some((fields, name)) = split_fields(line, 8) else {
      continue;
    };
    let mode = fields[0];
    let (kind, path, link) = match mode.chars().next() {
      Some('d') => (MemberKind::Dir, name, None),
      Some('l') => match name.split_once(" -> ") {
        Some((path, target)) => (MemberKind::Link, path, Some(target.to_string())),
        None => (MemberKind::Link, name, None),
      },
      // Hard links name what they link to.
      Some('-') => (
        MemberKind::File,
        name.split_once(" link to ").map_or(name, |(p, _)| p),
        None,
      ),
      _ => (MemberKind::Other, name, None),
    };
    let member = Member {
      path: path.to_string(),
      kind,
      size: (kind == MemberKind::File)
        .then(|| fields[4].parse().ok())
        .flatten(),
      modified: Some(format!("{} {:>2} {:>5}", fields[5], fields[6], fields[7])),
      link,
      encrypted: false,
    };
    contents.add(member, max);
  }
}

/// The first `n` space-separated fields of `line`, and the rest after the single space
/// that ends the last of them.
fn split_fields(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
  let mut rest = line;
  let mut fields = Vec::with_capacity(n);
  for _ in 0..n {
    rest = rest.trim_start_matches(' ');
    let end = rest.find(' ')?;
    fields.push(&rest[..end]);
    rest = &rest[end..];
  }
  let name = rest.strip_prefix(' ')?;
  (!name.is_empty()).then_some((fields, name))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn summary(contents: &Contents) -> Vec<(String, MemberKind, Option<u64>, Option<String>)> {
    contents
      .members
      .iter()
      .map(|m| (m.path.clone(), m.kind, m.size, m.link.clone()))
      .collect()
  }

  #[test]
  fn reads_7zip_technical_listings() {
    let text = "\n7-Zip (z) 26.04 (x64)\n\nListing archive: t.7z\n\n--\nPath = t.7z\n\
      Type = 7z\nPhysical Size = 5218\nComment = hello\n\n----------\n\
      Path = src\nSize = 0\nPacked Size = 0\nModified = 2026-11-17 23:51:57.1181025\n\
      Attributes = D drwxr-xr-x\nEncrypted = -\n\n\
      Path = src/a.txt\nSize = 6\nPacked Size = 5016\nModified = 2026-11-17 23:51:57.1131020\n\
      Attributes = A -rw-r--r--\nCRC = 363A3020\nEncrypted = +\n\n\
      Path = src/link\nSize = 5\nModified = 2026-11-17 23:51:57.1131020\n\
      Attributes = A lrwxrwxrwx\nSymbolic Link = a.txt\nEncrypted = -\n\n";
    let mut contents = Contents::new("7z");
    parse_7z(text, &mut contents, 10).unwrap();
    assert_eq!(
      summary(&contents),
      [
        ("src".into(), MemberKind::Dir, None, None),
        ("src/a.txt".into(), MemberKind::File, Some(6), None),
        (
          "src/link".into(),
          MemberKind::Link,
          None,
          Some("a.txt".into())
        ),
      ]
    );
    assert_eq!(contents.encrypted, 1);
    assert_eq!(contents.comment.as_deref(), Some("hello"));
    assert_eq!(
      contents.members[1].modified.as_deref(),
      Some("2026-11-17 23:51")
    );
    // No separator line: not a listing.
    assert!(parse_7z("Path = x\n", &mut Contents::new("7z"), 10).is_none());
  }

  #[test]
  fn reads_unrar_technical_listings() {
    let text = "\nUNRAR 7.30 beta 2 freeware\n\nArchive: t.rar\nDetails: RAR 5\n\n\
      \x20       Name: src/sub/b.bin\n        Type: File\n        Size: 5000\n\
      \x20Packed size: 5000\n       mtime: 2026-11-17 23:51:57,118102555\n\
      \x20 Attributes: -rw-r--r--\n\n\
      \x20       Name: src/link\n        Type: Unix symbolic link\n      Target: a.txt\n\
      \x20       Size: 0\n\n\
      \x20       Name: src\n        Type: Directory\n       mtime: 2026-11-17 23:51:57,118\n\n\
      \x20       Name: QO\n        Type: Service\n        Size: 57\n\n";
    let mut contents = Contents::new("rar");
    parse_unrar(text, &mut contents, 10);
    assert_eq!(
      summary(&contents),
      [
        ("src/sub/b.bin".into(), MemberKind::File, Some(5000), None),
        (
          "src/link".into(),
          MemberKind::Link,
          None,
          Some("a.txt".into())
        ),
        ("src".into(), MemberKind::Dir, None, None),
      ]
    );
    assert_eq!(
      contents.members[0].modified.as_deref(),
      Some("2026-11-17 23:51")
    );
    assert_eq!((contents.files, contents.dirs, contents.links), (1, 2, 1));
  }

  #[test]
  fn reads_bsdtar_verbose_listings() {
    let text = "drwxr-xr-x  0 moskas users       0 Nov 17 23:51 src/\n\
      -rw-r--r--  0 0      0           6 Nov 17  2020 src/a file.txt\n\
      lrwxrwxrwx  0 moskas users       0 Nov 17 23:51 src/link -> a file.txt\n\
      hrw-r--r--  0 moskas users       0 Nov 17 23:51 src/hard link to src/a file.txt\n\
      not a listing line\n";
    let mut contents = Contents::new("iso");
    parse_bsdtar(text, &mut contents, 10);
    let found = summary(&contents);
    assert_eq!(found[0], ("src".into(), MemberKind::Dir, None, None));
    assert_eq!(
      found[1],
      ("src/a file.txt".into(), MemberKind::File, Some(6), None)
    );
    assert_eq!(
      found[2],
      (
        "src/link".into(),
        MemberKind::Link,
        None,
        Some("a file.txt".into())
      )
    );
    assert_eq!(found.len(), 4);
    assert_eq!(
      contents.members[1].modified.as_deref(),
      Some("Nov 17  2020")
    );
  }
}
