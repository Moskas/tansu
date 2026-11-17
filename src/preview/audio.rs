//! Music and other audio: the tags (title, artist, album, ...) and stream details reported
//! by `ffprobe`, and the embedded cover art, extracted by `ffmpeg` as a small image.

use std::ffi::OsStr;
use std::path::Path;

use super::thumbnail::{Rendered, cached_preview, run};
use super::video::{Probe, bit_rate, duration, probe};
use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, EntryKind, FileClass};

/// Covers are extracted at most this many pixels a side: a small picture next to the
/// details, not a poster.
pub const COVER_SIDE: u32 = 400;

pub struct AudioPreviewer {
  /// Show the embedded cover; without graphics only the details are shown.
  pub covers: bool,
}

impl Previewer for AudioPreviewer {
  fn name(&self) -> &str {
    "audio"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    // Cue sheets are text and belong to the text previewer.
    entry.kind == EntryKind::File
      && entry.class() == FileClass::Audio
      && entry.extension().as_deref() != Some("cue")
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    let kind = if self.covers { "audio" } else { "audio-info" };
    let preview = cached_preview(entry, kind, "audio", ctx, |out| {
      let Some(probe) = probe(&entry.path, ctx)? else {
        return Ok(Rendered::Nothing);
      };
      let info = rows(&probe);
      if self.covers && has_cover(&probe) && cover(&entry.path, out, ctx)? {
        Ok(Rendered::Image(info))
      } else {
        Ok(Rendered::InfoOnly(info))
      }
    })?;
    // The details are what an audio file is about; the cover is a small extra.
    Ok(preview.map(|p| match p {
      Preview::Image(mut image) => {
        image.details_first = true;
        Preview::Image(image)
      }
      other => other,
    }))
  }
}

/// Whether the file carries cover art: a picture stream marked as attached.
fn has_cover(probe: &Probe) -> bool {
  probe
    .streams
    .iter()
    .any(|s| s.get("disposition.attached_pic").map(String::as_str) == Some("1"))
}

/// Extracts the cover of `audio` into `out` (PNG), at most [`COVER_SIDE`] a side.
fn cover(audio: &Path, out: &Path, ctx: &PreviewContext) -> Result<bool, PreviewError> {
  let scale = format!(
    "scale='min({COVER_SIDE},iw)':'min({COVER_SIDE},ih)':force_original_aspect_ratio=decrease"
  );
  let args: [&OsStr; 14] = [
    "-v".as_ref(),
    "error".as_ref(),
    "-nostdin".as_ref(),
    "-i".as_ref(),
    audio.as_os_str(),
    "-map".as_ref(),
    "0:v:0".as_ref(),
    "-an".as_ref(),
    "-frames:v".as_ref(),
    "1".as_ref(),
    "-vf".as_ref(),
    scale.as_ref(),
    "-y".as_ref(),
    out.as_os_str(),
  ];
  Ok(run("ffmpeg", &args, ctx)?.is_some_and(|o| o.success && out.exists()))
}

/// Tags and stream details as rows: title, artist, album, track, date, genre, duration,
/// the audio stream and the bitrate. Empty tags are left out.
pub fn rows(probe: &Probe) -> Vec<(String, String)> {
  let tag = |names: &[&str]| {
    names
      .iter()
      .find_map(|n| probe.tags.get(*n))
      .map(|v| v.trim().to_string())
      .filter(|v| !v.is_empty())
  };
  let mut rows = Vec::new();
  let mut push = |label: &str, value: Option<String>| {
    if let Some(value) = value {
      rows.push((label.to_string(), value));
    }
  };
  push("Title", tag(&["title"]));
  push(
    "Artist",
    tag(&["artist", "album_artist", "albumartist", "performer"]),
  );
  push("Album", tag(&["album"]));
  push("Track", tag(&["track", "tracknumber"]).map(|t| track(&t)));
  push("Date", tag(&["date", "year", "originaldate"]));
  push("Genre", tag(&["genre"]));
  push(
    "Duration",
    probe
      .duration
      .filter(|d| d.is_finite() && *d >= 0.0)
      .map(duration),
  );
  push("Audio", probe.audio_summary());
  push("Bitrate", probe.bit_rate.filter(|r| *r > 0).map(bit_rate));
  rows
}

/// `3/12` as `3 of 12`; anything else as it is.
fn track(raw: &str) -> String {
  match raw.split_once('/') {
    Some((n, total)) if !total.trim().is_empty() => format!("{} of {}", n.trim(), total.trim()),
    Some((n, _)) => n.trim().to_string(),
    None => raw.to_string(),
  }
}
