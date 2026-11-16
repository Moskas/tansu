use std::sync::Arc;

use image::{DynamicImage, ImageReader};

use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, EntryKind, FileClass};

/// A decoded image. Terminal encoding happens later, once the preview area is known.
#[derive(Clone)]
pub struct PreviewImage {
  /// Full resolution as decoded; replaced by the scaled-down copy once encoded.
  pub image: Arc<DynamicImage>,
  /// Dimensions of the original file, before downscaling.
  pub width: u32,
  pub height: u32,
}

impl std::fmt::Debug for PreviewImage {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "PreviewImage({}x{})", self.width, self.height)
  }
}

impl PartialEq for PreviewImage {
  fn eq(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.image, &other.image)
  }
}

impl Eq for PreviewImage {}

/// Decodes files with an image extension. Formats the decoder does not support (SVG, PSD,
/// ...) are declined so the text or metadata previewer can take them.
pub struct ImagePreviewer;

impl Previewer for ImagePreviewer {
  fn name(&self) -> &str {
    "image"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    entry.kind == EntryKind::File && entry.class() == FileClass::Image
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    let reader = ImageReader::open(&entry.path)
      .and_then(ImageReader::with_guessed_format)
      .map_err(|e| PreviewError::from_io(&entry.path, e))?;
    if reader.format().is_none() {
      return Ok(None);
    }
    let Ok(image) = reader.decode() else {
      return Ok(None);
    };
    if (ctx.cancelled)() {
      return Err(PreviewError::Cancelled);
    }
    let (width, height) = (image.width(), image.height());
    Ok(Some(Preview::Image(PreviewImage {
      image: Arc::new(image),
      width,
      height,
    })))
  }
}
