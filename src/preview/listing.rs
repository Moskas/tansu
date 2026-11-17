//! Structured views of text files about music: playlists (`.m3u`, `.m3u8`), cue sheets
//! (`.cue`) and CD rip logs (`.log` from EAC, XLD, CUETools or whipper). Each becomes a
//! [`Listing`]: detail rows, then numbered entries or text. Other `.log` files are left to
//! the text previewer.

use std::fs::File;
use std::io::Read;

use super::encoding::decode_text;
use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, EntryKind};

/// Playlists, cue sheets and logs are small; this much is read at most.
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
  /// What the file is (`playlist`, `cue sheet`, `rip log`), for the metadata view.
  pub kind: &'static str,
  /// Label/value rows shown first.
  pub info: Vec<(String, String)>,
  pub rows: Vec<Row>,
}

impl Listing {
  /// Lines the listing takes: the details, a rule when both parts are there, the rows.
  pub fn len(&self) -> usize {
    let rule = usize::from(!self.info.is_empty() && !self.rows.is_empty());
    self.info.len() + rule + self.rows.len()
  }

  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
  /// A track or playlist entry: `01  Title · detail      3:45`.
  Entry {
    number: String,
    title: String,
    detail: Option<String>,
    time: Option<String>,
  },
  /// A line of text, as in the file.
  Text(String),
}

pub struct ListingPreviewer;

impl Previewer for ListingPreviewer {
  fn name(&self) -> &str {
    "listing"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    entry.kind == EntryKind::File
      && matches!(
        entry.extension().as_deref(),
        Some("m3u" | "m3u8" | "cue" | "log")
      )
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    let mut data = Vec::new();
    File::open(&entry.path)
      .and_then(|f| f.take(MAX_BYTES).read_to_end(&mut data))
      .map_err(|e| PreviewError::from_io(&entry.path, e))?;
    if (ctx.cancelled)() {
      return Err(PreviewError::Cancelled);
    }
    let Some(decoded) = decode_text(&data) else {
      return Ok(None);
    };
    let text = decoded.text.trim_start_matches('\u{feff}');
    let listing = match entry.extension().as_deref() {
      Some("m3u" | "m3u8") => Some(super::playlist::parse_m3u(text)),
      Some("cue") => Some(super::playlist::parse_cue(text)),
      Some("log") => super::riplog::parse(text),
      _ => None,
    };
    Ok(listing.map(|mut listing| {
      // Worth knowing for files from other systems; UTF-8 goes without saying.
      if decoded.encoding != "UTF-8" {
        listing
          .info
          .push(("Encoding".to_string(), decoded.encoding.to_string()));
      }
      Preview::Listing(clean(listing))
    }))
  }
}

/// Makes every string safe to draw: tabs expanded, control characters replaced.
fn clean(mut listing: Listing) -> Listing {
  let clean = |s: &mut String| *s = super::text::clean_line(s);
  for (key, value) in &mut listing.info {
    clean(key);
    clean(value);
  }
  for row in &mut listing.rows {
    match row {
      Row::Text(text) => clean(text),
      Row::Entry {
        number,
        title,
        detail,
        time,
      } => {
        clean(number);
        clean(title);
        detail.iter_mut().for_each(clean);
        time.iter_mut().for_each(clean);
      }
    }
  }
  listing
}

/// Splits text into lines, without line ending characters (`\r\n`, `\n`, `\r`).
pub(super) fn lines(text: &str) -> impl Iterator<Item = &str> {
  text.split('\n').map(|l| l.trim_end_matches('\r'))
}

/// `m:ss` or `h:mm:ss` for a number of seconds.
pub(super) fn clock(seconds: f64) -> String {
  super::video::duration(seconds)
}
