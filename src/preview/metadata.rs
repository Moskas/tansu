use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::meta::{format_mtime, format_permissions, human_size};
use crate::fs::{Entry, EntryKind};

/// Catch-all previewer: a metadata block for binaries, devices, sockets and the like.
pub struct MetadataPreviewer;

impl Previewer for MetadataPreviewer {
  fn name(&self) -> &str {
    "metadata"
  }

  fn can_handle(&self, _entry: &Entry) -> bool {
    true
  }

  fn preview(&self, entry: &Entry, _ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    Ok(Some(Preview::Metadata(metadata_lines(entry, None))))
  }
}

/// Key/value metadata rows for an entry. `kind_label` overrides the derived type name.
pub fn metadata_lines(entry: &Entry, kind_label: Option<&str>) -> Vec<(String, String)> {
  let kind = kind_label.unwrap_or(match entry.kind {
    EntryKind::Dir => "directory",
    EntryKind::File if entry.is_executable() => "executable file",
    EntryKind::File => "binary file",
    EntryKind::Fifo => "named pipe",
    EntryKind::Socket => "socket",
    EntryKind::CharDevice => "character device",
    EntryKind::BlockDevice => "block device",
    EntryKind::Unknown if entry.is_symlink() => "broken symlink",
    EntryKind::Unknown => "unknown",
  });
  let mut rows = vec![
    ("Name".to_string(), entry.name.clone()),
    ("Type".to_string(), kind.to_string()),
    (
      "Size".to_string(),
      format!("{} ({} bytes)", human_size(entry.size, false), entry.size),
    ),
    (
      "Mode".to_string(),
      format!(
        "{} ({:04o})",
        format_permissions(entry.mode),
        entry.mode & 0o7777
      ),
    ),
    (
      "Owner".to_string(),
      format!("{}:{}", entry.owner, entry.group),
    ),
    ("Modified".to_string(), format_mtime(entry.mtime)),
  ];
  if let Some(target) = &entry.link_target {
    rows.push(("Target".to_string(), target.display().to_string()));
  }
  rows
}
