//! Archive contents: the format, counts and sizes, where extracting puts things, then the
//! members with their dates and sizes.

use super::listing::{Listing, Row, clean};
use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::archive::{self, Contents, Format, ListError, MemberKind};
use crate::fs::meta::human_size;
use crate::fs::{Entry, EntryKind};

pub struct ArchivePreviewer;

impl Previewer for ArchivePreviewer {
  fn name(&self) -> &str {
    "archive"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    entry.kind == EntryKind::File && Format::from_name(&entry.name).is_some()
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    match archive::list(&entry.path, ctx.limits.max_entries, ctx.cancelled) {
      Ok(contents) => Ok(Some(Preview::Listing(clean(listing(entry, &contents))))),
      Err(ListError::Cancelled) => Err(PreviewError::Cancelled),
      // Damaged, or nothing installed to read it: the metadata says what can be said.
      Err(ListError::Unsupported | ListError::Failed(_)) => Ok(None),
    }
  }
}

fn listing(entry: &Entry, contents: &Contents) -> Listing {
  let size = |n: u64| human_size(n, false);
  let mut info = Vec::new();
  let mut add = |key: &str, value: String| info.push((key.to_string(), value));
  let stem = Format::from_name(&entry.name).map_or("", |(_, stem)| stem);
  if contents.single {
    add("Compression", contents.format.clone());
    if let Some(member) = contents.members.first() {
      add("Contains", member.path.clone());
    }
    let unpacked = match contents.members.first().and_then(|m| m.size) {
      Some(n) => format!("{} ({})", size(n), ratio(entry.size, n)),
      None => format!("over {}", size(archive::MAX_SCAN)),
    };
    add("Unpacked", unpacked);
    return Listing {
      kind: "compressed file",
      info,
      rows: Vec::new(),
    };
  }
  add("Archive", contents.format.clone());
  let mut counts = vec![count(contents.files, "file", "files")];
  if contents.dirs > 0 {
    counts.push(count(contents.dirs, "folder", "folders"));
  }
  if contents.links > 0 {
    counts.push(count(contents.links, "link", "links"));
  }
  let at_least = if contents.complete { "" } else { "at least " };
  add("Contents", format!("{at_least}{}", counts.join(", ")));
  let unpacked = if contents.complete && contents.size > 0 {
    format!(
      "{} ({})",
      size(contents.size),
      ratio(entry.size, contents.size)
    )
  } else {
    format!("{at_least}{}", size(contents.size))
  };
  add("Unpacked", unpacked);
  if contents.encrypted > 0 {
    add("Encrypted", count(contents.encrypted, "member", "members"));
  }
  if let Some(to) = contents.extracts_to(stem) {
    add("Extracts to", to);
  }
  if let Some(comment) = &contents.comment {
    add("Comment", comment.clone());
  }
  if !contents.complete {
    add("Shown", format!("the first {}", contents.members.len()));
  }
  let rows = contents
    .members
    .iter()
    .map(|m| {
      let name = match (m.kind, &m.link) {
        (MemberKind::Dir, _) => format!("{}/", m.path),
        (MemberKind::Link, Some(target)) => format!("{} -> {target}", m.path),
        _ => m.path.clone(),
      };
      Row::Member {
        name,
        kind: m.kind,
        size: m.size.map(size),
        modified: m.modified.clone(),
      }
    })
    .collect();
  Listing {
    kind: "archive",
    info,
    rows,
  }
}

/// The packed size as a share of the unpacked one: `packed to 38%`.
fn ratio(packed: u64, unpacked: u64) -> String {
  if unpacked == 0 {
    return "empty".to_string();
  }
  let percent = packed as f64 / unpacked as f64 * 100.0;
  if percent < 1.0 {
    return "packed to <1%".to_string();
  }
  format!("packed to {percent:.0}%")
}

fn count(n: usize, one: &str, many: &str) -> String {
  format!("{n} {}", if n == 1 { one } else { many })
}
