//! Preview generation. Previewers are tried in priority order; the first one that can handle
//! an entry and produces content wins. Free of UI types: previews are plain data.

pub mod ansi;
mod directory;
pub mod exif;
pub mod highlight;
mod image;
mod metadata;
mod text;
pub mod thumbnail;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;

use crate::fs::{DirListing, Entry};

pub use ansi::{SpanStyle, StyledLine, StyledSpan, TermColor};
pub use directory::DirectoryPreviewer;
pub use highlight::{HighlightConfig, Highlighter};
pub use image::{ImagePreviewer, PreviewImage};
pub use metadata::{MetadataPreviewer, metadata_lines};
pub use text::{TextPreviewer, looks_binary};
pub use thumbnail::ThumbnailCache;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
  Text {
    lines: Vec<String>,
    truncated: bool,
  },
  /// Syntax highlighted text.
  Highlighted {
    lines: Vec<StyledLine>,
    truncated: bool,
  },
  Image(PreviewImage),
  Directory(Arc<DirListing>),
  Metadata(Vec<(String, String)>),
}

/// Caps that keep preview generation bounded no matter how large the input is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PreviewLimits {
  pub max_bytes: usize,
  pub max_lines: usize,
  pub max_entries: usize,
}

impl Default for PreviewLimits {
  fn default() -> Self {
    PreviewLimits {
      max_bytes: 64 * 1024,
      max_lines: 300,
      max_entries: 10_000,
    }
  }
}

pub struct PreviewContext<'a> {
  pub limits: PreviewLimits,
  pub cancelled: &'a dyn Fn() -> bool,
  /// Where rendered thumbnails are kept; set by the registry from its own cache.
  pub thumbnails: Option<&'a ThumbnailCache>,
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
  #[error("permission denied: {}", .0.display())]
  PermissionDenied(PathBuf),
  #[error("{}: {source}", path.display())]
  Io {
    path: PathBuf,
    source: std::io::Error,
  },
  #[error("cancelled")]
  Cancelled,
}

impl PreviewError {
  pub fn from_io(path: &std::path::Path, e: std::io::Error) -> Self {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
      PreviewError::PermissionDenied(path.to_path_buf())
    } else {
      PreviewError::Io {
        path: path.to_path_buf(),
        source: e,
      }
    }
  }
}

pub trait Previewer: Send + Sync {
  fn name(&self) -> &str;

  /// Cheap check based on entry metadata only.
  fn can_handle(&self, entry: &Entry) -> bool;

  /// Produces content, or `Ok(None)` to let a lower-priority previewer try.
  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError>;
}

/// Priority-ordered previewer list (higher priority first; ties keep registration order).
#[derive(Default)]
pub struct PreviewRegistry {
  previewers: Vec<(i32, Box<dyn Previewer>)>,
  thumbnails: Option<ThumbnailCache>,
}

impl PreviewRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  /// Directory, text, then metadata as the catch-all.
  pub fn with_defaults() -> Self {
    let mut r = Self::new();
    r.register(100, Box::new(DirectoryPreviewer));
    r.register(50, Box::new(TextPreviewer::default()));
    r.register(0, Box::new(MetadataPreviewer));
    r
  }

  /// The defaults plus syntax highlighting (when enabled) and, when `images` is set, an
  /// image previewer ahead of text.
  pub fn configured(highlight: &HighlightConfig, images: bool) -> Self {
    let mut r = Self::new();
    r.register(100, Box::new(DirectoryPreviewer));
    if images {
      r.register(75, Box::new(ImagePreviewer));
    }
    let text = if highlight.enabled {
      TextPreviewer::with_highlighter(Highlighter::new(highlight.clone()))
    } else {
      TextPreviewer::default()
    };
    r.register(50, Box::new(text));
    r.register(0, Box::new(MetadataPreviewer));
    r
  }

  pub fn register(&mut self, priority: i32, previewer: Box<dyn Previewer>) {
    let pos = self
      .previewers
      .iter()
      .position(|(p, _)| *p < priority)
      .unwrap_or(self.previewers.len());
    self.previewers.insert(pos, (priority, previewer));
  }

  pub fn names(&self) -> Vec<&str> {
    self.previewers.iter().map(|(_, p)| p.name()).collect()
  }

  /// Keeps rendered thumbnails (video frames, PDF pages, covers) in `cache`.
  pub fn set_thumbnails(&mut self, cache: ThumbnailCache) {
    self.thumbnails = Some(cache);
  }

  pub fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Preview, PreviewError> {
    let ctx = &PreviewContext {
      limits: ctx.limits,
      cancelled: ctx.cancelled,
      thumbnails: self.thumbnails.as_ref().or(ctx.thumbnails),
    };
    for (_, p) in &self.previewers {
      if (ctx.cancelled)() {
        return Err(PreviewError::Cancelled);
      }
      if !p.can_handle(entry) {
        continue;
      }
      if let Some(preview) = p.preview(entry, ctx)? {
        return Ok(preview);
      }
    }
    Ok(Preview::Metadata(metadata_lines(entry, None)))
  }
}
