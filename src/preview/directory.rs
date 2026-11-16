use std::sync::Arc;

use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, ListError, list_dir};

/// Lists the selected directory, capped at `max_entries`.
pub struct DirectoryPreviewer;

impl Previewer for DirectoryPreviewer {
  fn name(&self) -> &str {
    "directory"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    entry.is_dir()
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    match list_dir(&entry.path, ctx.limits.max_entries, ctx.cancelled) {
      Ok(listing) => Ok(Some(Preview::Directory(Arc::new(listing)))),
      Err(ListError::Cancelled) => Err(PreviewError::Cancelled),
      Err(ListError::PermissionDenied(p)) => Err(PreviewError::PermissionDenied(p)),
      Err(ListError::NotFound(p)) => Err(PreviewError::Io {
        path: p,
        source: std::io::ErrorKind::NotFound.into(),
      }),
      Err(ListError::Io { path, source }) => Err(PreviewError::Io { path, source }),
    }
  }
}
