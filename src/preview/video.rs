//! Video previews: a frame rendered by `ffmpegthumbnailer` (or `ffmpeg`), and the duration,
//! resolution and codecs reported by `ffprobe`. Each program is optional; without them the
//! entry falls back to what the others can show.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

use super::thumbnail::{Rendered, cached_preview, run};
use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, EntryKind, FileClass};

/// How far into the video the frame is taken. The very first frame is often black (a fade
/// in), so like ranger (through `ffmpegthumbnailer`'s default) a frame a little way in.
const FRAME_AT_PERCENT: u32 = 10;

pub struct VideoPreviewer {
  /// Render a frame; without graphics only the details are shown.
  pub frames: bool,
}

impl Previewer for VideoPreviewer {
  fn name(&self) -> &str {
    "video"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    entry.kind == EntryKind::File && entry.class() == FileClass::Video
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    let kind = if self.frames { "video" } else { "video-info" };
    cached_preview(entry, kind, "video", ctx, |out| {
      let probe = probe(&entry.path, ctx)?;
      let info = probe.as_ref().map(Probe::rows).unwrap_or_default();
      if !self.frames {
        return Ok(Rendered::InfoOnly(info));
      }
      let duration = probe.as_ref().and_then(|p| p.duration);
      if frame(&entry.path, out, duration, ctx)? {
        Ok(Rendered::Image(info))
      } else {
        Ok(Rendered::InfoOnly(info))
      }
    })
  }
}

/// Renders a frame of `video` into `out` (PNG). False when no program could.
fn frame(
  video: &Path,
  out: &Path,
  duration: Option<f64>,
  ctx: &PreviewContext,
) -> Result<bool, PreviewError> {
  let percent = format!("{FRAME_AT_PERCENT}%");
  let args: [&OsStr; 10] = [
    "-i".as_ref(),
    video.as_os_str(),
    "-o".as_ref(),
    out.as_os_str(),
    "-s".as_ref(),
    "0".as_ref(), // full size; tansu scales it to the column itself
    "-c".as_ref(),
    "png".as_ref(),
    "-t".as_ref(),
    percent.as_ref(),
  ];
  if let Some(output) = run("ffmpegthumbnailer", &args, ctx)?
    && output.success
    && out.exists()
  {
    return Ok(true);
  }
  // ffmpeg seeks by time, so the percentage needs the duration.
  let seek = duration.map_or(0.0, |d| d * f64::from(FRAME_AT_PERCENT) / 100.0);
  let seek = format!("{seek:.3}");
  let args: [&OsStr; 11] = [
    "-v".as_ref(),
    "error".as_ref(),
    "-nostdin".as_ref(),
    "-ss".as_ref(),
    seek.as_ref(),
    "-i".as_ref(),
    video.as_os_str(),
    "-frames:v".as_ref(),
    "1".as_ref(),
    "-y".as_ref(),
    out.as_os_str(),
  ];
  Ok(run("ffmpeg", &args, ctx)?.is_some_and(|output| output.success && out.exists()))
}

/// What `ffprobe` reports about a video.
#[derive(Debug, Default, PartialEq)]
pub struct Probe {
  pub duration: Option<f64>,
  pub bit_rate: Option<u64>,
  /// Streams in file order, each a map of the fields asked for.
  pub streams: Vec<BTreeMap<String, String>>,
}

fn probe(video: &Path, ctx: &PreviewContext) -> Result<Option<Probe>, PreviewError> {
  let args: [&OsStr; 7] = [
    "-v".as_ref(),
    "error".as_ref(),
    "-of".as_ref(),
    "flat".as_ref(),
    "-show_entries".as_ref(),
    "format=duration,bit_rate:stream=codec_type,codec_name,width,height,avg_frame_rate,\
     channels,sample_rate"
      .as_ref(),
    video.as_os_str(),
  ];
  Ok(
    run("ffprobe", &args, ctx)?
      .filter(|o| o.success)
      .map(|o| Probe::parse(&String::from_utf8_lossy(&o.stdout))),
  )
}

impl Probe {
  /// Parses `ffprobe -of flat` output: `format.duration="12.5"`,
  /// `streams.stream.0.codec_name="h264"`, `streams.stream.0.width=1920`.
  pub fn parse(text: &str) -> Probe {
    let mut probe = Probe::default();
    let mut streams: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
    for line in text.lines() {
      let Some((key, value)) = line.split_once('=') else {
        continue;
      };
      let value = value.trim().trim_matches('"').to_string();
      if let Some(field) = key.strip_prefix("format.") {
        match field {
          "duration" => probe.duration = value.parse().ok(),
          "bit_rate" => probe.bit_rate = value.parse().ok(),
          _ => {}
        }
      } else if let Some(rest) = key.strip_prefix("streams.stream.")
        && let Some((index, field)) = rest.split_once('.')
        && let Ok(index) = index.parse::<usize>()
      {
        streams
          .entry(index)
          .or_default()
          .insert(field.to_string(), value);
      }
    }
    probe.streams = streams.into_values().collect();
    probe
  }

  fn stream(&self, kind: &str) -> Option<&BTreeMap<String, String>> {
    self
      .streams
      .iter()
      .find(|s| s.get("codec_type").map(String::as_str) == Some(kind))
  }

  /// Duration, resolution, video and audio codecs, and bitrate, as detail rows.
  pub fn rows(&self) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(d) = self.duration.filter(|d| d.is_finite() && *d >= 0.0) {
      rows.push(("Duration".to_string(), duration(d)));
    }
    if let Some(video) = self.stream("video") {
      if let (Some(w), Some(h)) = (video.get("width"), video.get("height")) {
        rows.push(("Resolution".to_string(), format!("{w}x{h}")));
      }
      let mut desc = video.get("codec_name").cloned().unwrap_or_default();
      if let Some(fps) = video.get("avg_frame_rate").and_then(|r| frame_rate(r)) {
        desc = join(&desc, &format!("{fps} fps"));
      }
      if !desc.is_empty() {
        rows.push(("Video".to_string(), desc));
      }
    }
    if let Some(audio) = self.stream("audio") {
      let mut desc = audio.get("codec_name").cloned().unwrap_or_default();
      if let Some(ch) = audio.get("channels").and_then(|c| c.parse::<u32>().ok()) {
        let ch = match ch {
          1 => "mono".to_string(),
          2 => "stereo".to_string(),
          n => format!("{n} channels"),
        };
        desc = join(&desc, &ch);
      }
      if let Some(rate) = audio.get("sample_rate").and_then(|r| r.parse::<f64>().ok()) {
        desc = join(&desc, &format!("{} kHz", trim_float(rate / 1000.0)));
      }
      if !desc.is_empty() {
        rows.push(("Audio".to_string(), desc));
      }
    }
    if let Some(rate) = self.bit_rate.filter(|r| *r > 0) {
      rows.push(("Bitrate".to_string(), bit_rate(rate)));
    }
    rows
  }
}

fn join(a: &str, b: &str) -> String {
  if a.is_empty() {
    b.to_string()
  } else {
    format!("{a}, {b}")
  }
}

/// `1:02:03` or `2:03`.
fn duration(seconds: f64) -> String {
  let total = seconds.round() as u64;
  let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
  if h > 0 {
    format!("{h}:{m:02}:{s:02}")
  } else {
    format!("{m}:{s:02}")
  }
}

/// `24000/1001` as `23.98`; `0/0` (unknown) as nothing.
fn frame_rate(rate: &str) -> Option<String> {
  let (num, den) = rate.split_once('/')?;
  let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
  (den > 0.0 && num > 0.0).then(|| trim_float((num / den * 100.0).round() / 100.0))
}

/// A number with at most two decimals and no trailing zeros: `48`, `44.1`, `23.98`.
fn trim_float(value: f64) -> String {
  let text = format!("{value:.2}");
  text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `4.5 Mb/s`, `320 kb/s`.
fn bit_rate(bits: u64) -> String {
  if bits >= 1_000_000 {
    format!("{} Mb/s", trim_float(bits as f64 / 1e6))
  } else {
    format!("{} kb/s", (bits as f64 / 1e3).round())
  }
}
