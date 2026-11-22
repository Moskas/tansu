//! Bulk rename: names edited one per line in `$EDITOR`, checked, then renamed within one
//! directory. Swaps and cycles (`a` -> `b`, `b` -> `a`) go through temporary names.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One rename within a directory, by entry name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
  pub from: String,
  pub to: String,
}

/// What a bulk rename did: the paths renamed (old, new), and why the others were not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
  pub renamed: Vec<(PathBuf, PathBuf)>,
  pub errors: Vec<String>,
}

/// The text handed to the editor: one name per line.
pub fn edit_text(names: &[String]) -> String {
  let mut text = names.join("\n");
  text.push('\n');
  text
}

/// Pairs the edited `text` with the `old` names line by line and returns the changed ones.
/// Trailing blank lines are ignored. Fails when the line count differs, or a new name is
/// empty, contains `/`, is `.` or `..`, or is given twice.
pub fn parse_edit(old: &[String], text: &str) -> Result<Vec<Rename>, String> {
  let mut new: Vec<&str> = text.lines().collect();
  while new.len() > old.len() && new.last().is_some_and(|l| l.is_empty()) {
    new.pop();
  }
  if new.len() != old.len() {
    let lines = |n: usize| {
      if n == 1 {
        "1 line".to_string()
      } else {
        format!("{n} lines")
      }
    };
    return Err(format!(
      "expected {}, got {}",
      lines(old.len()),
      lines(new.len())
    ));
  }
  let mut seen = HashSet::new();
  for (line, name) in new.iter().enumerate() {
    let line = line + 1;
    if name.is_empty() {
      return Err(format!("line {line} is empty"));
    }
    if name.contains('/') || *name == "." || *name == ".." {
      return Err(format!("line {line}: `{name}` is not a valid name"));
    }
    if !seen.insert(*name) {
      return Err(format!("`{name}` is given twice"));
    }
  }
  Ok(
    old
      .iter()
      .zip(new)
      .filter(|(from, to)| from.as_str() != *to)
      .map(|(from, to)| Rename {
        from: from.clone(),
        to: to.to_string(),
      })
      .collect(),
  )
}

/// Renames entries of `dir`. Nothing is renamed when a new name is taken by an entry that is
/// not itself renamed away. Renames onto a name that another rename frees up go through a
/// temporary name first.
pub fn apply(dir: &Path, renames: &[Rename]) -> Outcome {
  let mut out = Outcome::default();
  let sources: HashSet<&str> = renames.iter().map(|r| r.from.as_str()).collect();
  for r in renames {
    if !sources.contains(r.to.as_str()) && dir.join(&r.to).symlink_metadata().is_ok() {
      out.errors.push(format!("`{}` already exists", r.to));
    }
  }
  if !out.errors.is_empty() {
    return out;
  }
  let (hops, direct): (Vec<&Rename>, Vec<&Rename>) = renames
    .iter()
    .partition(|r| sources.contains(r.to.as_str()));
  // Move the entries whose new name is still taken out of the way first.
  let mut parked = Vec::new();
  for (i, r) in hops.into_iter().enumerate() {
    let tmp = temp_name(dir, i);
    match std::fs::rename(dir.join(&r.from), &tmp) {
      Ok(()) => parked.push((r, tmp)),
      Err(e) => out.errors.push(format!("{}: {e}", r.from)),
    }
  }
  for r in direct {
    let (from, to) = (dir.join(&r.from), dir.join(&r.to));
    move_unless_taken(&from, &to, &r.from, &r.to, &mut out);
  }
  for (r, tmp) in parked {
    let to = dir.join(&r.to);
    let before = out.errors.len();
    move_unless_taken(&tmp, &to, &r.from, &r.to, &mut out);
    if out.errors.len() > before {
      // The name it was to take is still in use: put it back where it was.
      let _ = std::fs::rename(&tmp, dir.join(&r.from));
    } else if let Some(last) = out.renamed.last_mut() {
      last.0 = dir.join(&r.from);
    }
  }
  out
}

/// Renames `from` to `to` unless `to` exists, recording the result under the entry names.
fn move_unless_taken(from: &Path, to: &Path, old: &str, new: &str, out: &mut Outcome) {
  if to.symlink_metadata().is_ok() {
    out.errors.push(format!("`{new}` already exists"));
    return;
  }
  match std::fs::rename(from, to) {
    Ok(()) => out.renamed.push((from.to_path_buf(), to.to_path_buf())),
    Err(e) => out.errors.push(format!("{old}: {e}")),
  }
}

/// A name in `dir` that is not taken, for parking an entry during a swap.
fn temp_name(dir: &Path, i: usize) -> PathBuf {
  let pid = std::process::id();
  (0..)
    .map(|n| dir.join(format!(".tansu-rename-{pid}-{i}-{n}")))
    .find(|p| p.symlink_metadata().is_err())
    .expect("an unused name")
}
