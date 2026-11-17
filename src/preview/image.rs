use std::sync::Arc;

use image::{DynamicImage, ImageDecoder, ImageReader};

use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, EntryKind, FileClass};

/// A decoded image. Terminal encoding happens later, once the preview area is known.
#[derive(Clone)]
pub struct PreviewImage {
  /// Full resolution as decoded; replaced by the scaled-down copy once encoded.
  pub image: Arc<DynamicImage>,
  /// Dimensions of the original file as displayed (after EXIF rotation), before
  /// downscaling.
  pub width: u32,
  pub height: u32,
  /// Details shown under the image and in the metadata view (EXIF data, for photos).
  pub info: Vec<(String, String)>,
  /// What the image shows when it is a rendering of another kind of file (`video`, `PDF
  /// document`); `None` for image files.
  pub label: Option<&'static str>,
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

/// Decodes files with an image extension, turned the way their EXIF orientation says, with
/// their EXIF details. Formats the decoder does not support (SVG, PSD, ...) are declined so
/// the text or metadata previewer can take them.
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
    let Ok(mut decoder) = reader.into_decoder() else {
      return Ok(None);
    };
    // Metadata problems never stop the image from showing.
    let exif = decoder.exif_metadata().ok().flatten();
    let orientation = decoder.orientation().ok();
    // `ImageReader::decode` does this check, `from_decoder` does not: without it a huge
    // image would be allocated in full instead of falling back to the metadata preview.
    if image::Limits::default()
      .reserve(decoder.total_bytes())
      .is_err()
    {
      return Ok(None);
    }
    let Ok(mut image) = DynamicImage::from_decoder(decoder) else {
      return Ok(None);
    };
    if (ctx.cancelled)() {
      return Err(PreviewError::Cancelled);
    }
    if let Some(orientation) = orientation {
      image.apply_orientation(orientation);
    }
    let (width, height) = (image.width(), image.height());
    Ok(Some(Preview::Image(PreviewImage {
      image: Arc::new(image),
      width,
      height,
      info: exif.map(super::exif::summary).unwrap_or_default(),
      label: None,
    })))
  }
}
