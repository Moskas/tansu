//! Playlists (`.m3u`, `.m3u8`, including HLS streaming playlists) and cue sheets (`.cue`) as
//! listings: details, then numbered entries.

use super::listing::{Listing, Row, clock, lines};

/// A playlist: its entries with `#EXTINF` titles and durations when present, otherwise the
/// file name without folders or extension, the folder going in the detail.
pub fn parse_m3u(text: &str) -> Listing {
  if text.contains("#EXT-X-") {
    return parse_hls(text);
  }
  let mut name = None;
  let mut rows = Vec::new();
  let mut pending: Option<(Option<f64>, String)> = None;
  let (mut total, mut timed) = (0.0, 0);
  for line in lines(text).map(str::trim).filter(|l| !l.is_empty()) {
    if let Some(rest) = strip_prefix_ci(line, "#EXTINF:") {
      let (duration, title) = rest.split_once(',').unwrap_or((rest, ""));
      // Attributes (`tvg-id="..."`) may follow the duration.
      let duration = duration
        .split_whitespace()
        .next()
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);
      pending = Some((duration, title.trim().to_string()));
      continue;
    }
    if let Some(rest) = strip_prefix_ci(line, "#PLAYLIST:") {
      name = Some(rest.trim().to_string()).filter(|n| !n.is_empty());
      continue;
    }
    if line.starts_with('#') {
      continue;
    }
    let (file_title, folder) = location_parts(line);
    let (duration, ext_title) = pending.take().unwrap_or((None, String::new()));
    let title = if ext_title.is_empty() {
      file_title
    } else {
      ext_title
    };
    if let Some(d) = duration {
      total += d;
      timed += 1;
    }
    rows.push(Row::Entry {
      number: (rows.len() + 1).to_string(),
      title,
      detail: folder,
      time: duration.map(clock),
    });
  }
  let mut info = Vec::new();
  if let Some(name) = name {
    info.push(("Name".to_string(), name));
  }
  info.push(("Entries".to_string(), rows.len().to_string()));
  if timed > 0 {
    let mut length = clock(total);
    if timed < rows.len() {
      length.push_str(&format!(" ({timed} of {} entries timed)", rows.len()));
    }
    info.push(("Duration".to_string(), length));
  }
  Listing {
    kind: "playlist",
    info,
    rows,
  }
}

/// An HLS playlist (`#EXT-X-...`): the variant streams of a master playlist, or a summary
/// of a media playlist's segments.
fn parse_hls(text: &str) -> Listing {
  let mut info = Vec::new();
  let mut rows = Vec::new();
  let (mut segments, mut total) = (0usize, 0.0);
  let (mut target, mut kind, mut ended) = (None, None, false);
  for line in lines(text).map(str::trim) {
    if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
      let attr = |name: &str| hls_attr(attrs, name);
      let title = attr("RESOLUTION").unwrap_or_else(|| "audio".to_string());
      let mut detail = attr("BANDWIDTH")
        .and_then(|b| b.parse::<u64>().ok())
        .map(super::video::bit_rate)
        .unwrap_or_default();
      if let Some(codecs) = attr("CODECS") {
        detail = super::video::join(&detail, &codecs);
      }
      rows.push(Row::Entry {
        number: (rows.len() + 1).to_string(),
        title,
        detail: Some(detail).filter(|d| !d.is_empty()),
        time: attr("FRAME-RATE").map(|f| format!("{f} fps")),
      });
    } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
      segments += 1;
      total += rest
        .split(',')
        .next()
        .and_then(|d| d.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    } else if let Some(rest) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
      target = rest.trim().parse::<f64>().ok();
    } else if let Some(rest) = line.strip_prefix("#EXT-X-PLAYLIST-TYPE:") {
      kind = Some(rest.trim().to_string());
    } else if line == "#EXT-X-ENDLIST" {
      ended = true;
    }
  }
  if rows.is_empty() {
    info.push(("Type".to_string(), "HLS media playlist".to_string()));
    info.push(("Segments".to_string(), segments.to_string()));
    if total > 0.0 {
      info.push(("Duration".to_string(), clock(total)));
    }
    if let Some(target) = target {
      info.push(("Segment length".to_string(), format!("{target} s at most")));
    }
    let state = match (kind.as_deref(), ended) {
      (Some("VOD"), _) | (_, true) => "complete",
      (Some("EVENT"), false) => "growing (event)",
      _ => "live",
    };
    info.push(("Stream".to_string(), state.to_string()));
  } else {
    info.push(("Type".to_string(), "HLS master playlist".to_string()));
    info.push(("Variants".to_string(), rows.len().to_string()));
  }
  Listing {
    kind: "playlist",
    info,
    rows,
  }
}

/// An attribute of an HLS tag: `BANDWIDTH=5000000,CODECS="avc1.64001f,mp4a.40.2"`.
fn hls_attr(attrs: &str, name: &str) -> Option<String> {
  let mut rest = attrs;
  while !rest.is_empty() {
    let (key, after) = rest.split_once('=')?;
    let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
      let end = quoted.find('"')?;
      (&quoted[..end], quoted[end + 1..].trim_start_matches(','))
    } else {
      after.split_once(',').unwrap_or((after, ""))
    };
    if key.trim() == name {
      return Some(value.to_string());
    }
    rest = next;
  }
  None
}

/// An entry's file name without extension, and its folder, from a path (with `/` or
/// Windows `\`) or a URL.
fn location_parts(location: &str) -> (String, Option<String>) {
  let location = location
    .split(['?', '#'])
    .next()
    .unwrap_or(location)
    .trim_end_matches(['/', '\\']);
  let mut parts = location.rsplit(['/', '\\']);
  let file = parts.next().unwrap_or(location);
  let folder = parts.next().filter(|f| !f.is_empty() && !f.ends_with(':'));
  let file = percent_decode(file);
  let stem = match file.rsplit_once('.') {
    Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 => stem.to_string(),
    _ => file,
  };
  (stem, folder.map(percent_decode))
}

fn percent_decode(text: &str) -> String {
  let bytes = text.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'%'
      && let Some(b) = text
        .get(i + 1..i + 3)
        .and_then(|h| u8::from_str_radix(h, 16).ok())
    {
      out.push(b);
      i += 3;
    } else {
      out.push(bytes[i]);
      i += 1;
    }
  }
  String::from_utf8_lossy(&out).into_owned()
}

fn strip_prefix_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
  let head = line.get(..prefix.len())?;
  head
    .eq_ignore_ascii_case(prefix)
    .then(|| &line[prefix.len()..])
}

/// One track of a cue sheet.
#[derive(Default)]
struct CueTrack {
  number: String,
  title: Option<String>,
  performer: Option<String>,
  file: usize,
  /// `INDEX 01`, in CD frames (75 per second) from the start of its file.
  start: Option<u64>,
}

/// A cue sheet: album details, then the tracks with their performer (when it differs from
/// the album's) and length (when the tracks share one file, so it can be computed).
pub fn parse_cue(text: &str) -> Listing {
  let (mut title, mut performer, mut date, mut genre) = (None, None, None, None);
  let mut files: Vec<String> = Vec::new();
  let mut tracks: Vec<CueTrack> = Vec::new();
  for line in lines(text).map(str::trim) {
    let (command, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim();
    match command.to_ascii_uppercase().as_str() {
      "REM" => {
        let (key, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        match key.to_ascii_uppercase().as_str() {
          "DATE" => date = Some(unquote(value)),
          "GENRE" => genre = Some(unquote(value)),
          _ => {}
        }
      }
      "PERFORMER" => match tracks.last_mut() {
        Some(track) => track.performer = Some(unquote(rest)),
        None => performer = Some(unquote(rest)),
      },
      "TITLE" => match tracks.last_mut() {
        Some(track) => track.title = Some(unquote(rest)),
        None => title = Some(unquote(rest)),
      },
      "FILE" => {
        // `FILE "name with spaces.flac" WAVE`: the type follows the name.
        let name = match rest.strip_prefix('"') {
          Some(quoted) => quoted.split('"').next().unwrap_or(quoted).to_string(),
          None => rest
            .rsplit_once(char::is_whitespace)
            .map_or(rest, |(name, _)| name)
            .to_string(),
        };
        files.push(name);
      }
      "TRACK" => tracks.push(CueTrack {
        number: rest.split_whitespace().next().unwrap_or("").to_string(),
        file: files.len().saturating_sub(1),
        ..CueTrack::default()
      }),
      "INDEX" => {
        let mut parts = rest.split_whitespace();
        if parts.next().and_then(|i| i.parse::<u32>().ok()) == Some(1)
          && let Some(track) = tracks.last_mut()
        {
          track.start = parts.next().and_then(cue_frames);
        }
      }
      _ => {}
    }
  }
  let mut info = Vec::new();
  let mut push = |label: &str, value: Option<String>| {
    if let Some(value) = value.filter(|v| !v.is_empty()) {
      info.push((label.to_string(), value));
    }
  };
  push("Album", title);
  push("Performer", performer.clone());
  push("Date", date);
  push("Genre", genre);
  push(
    "File",
    match files.as_slice() {
      [] => None,
      [one] => Some(one.clone()),
      many => Some(format!("{} files", many.len())),
    },
  );
  push("Tracks", Some(tracks.len().to_string()));
  let rows = tracks
    .iter()
    .enumerate()
    .map(|(i, track)| {
      // A track's length is known when the next one starts later in the same file.
      let length = tracks
        .get(i + 1)
        .filter(|next| next.file == track.file)
        .and_then(|next| Some(next.start? as f64 - track.start? as f64))
        .filter(|frames| *frames > 0.0)
        .map(|frames| clock(frames / 75.0));
      Row::Entry {
        number: track.number.clone(),
        title: track
          .title
          .clone()
          .unwrap_or_else(|| format!("Track {}", track.number)),
        detail: track
          .performer
          .clone()
          .filter(|p| Some(p) != performer.as_ref()),
        time: length,
      }
    })
    .collect();
  Listing {
    kind: "cue sheet",
    info,
    rows,
  }
}

/// `mm:ss:ff` as CD frames.
fn cue_frames(time: &str) -> Option<u64> {
  let mut parts = time.split(':').map(|p| p.parse::<u64>().ok());
  let (m, s, f) = (parts.next()??, parts.next()??, parts.next()??);
  Some((m * 60 + s) * 75 + f)
}

/// A cue sheet value: the text between quotes, or the rest of the line.
fn unquote(value: &str) -> String {
  let value = value.trim();
  match value.strip_prefix('"') {
    Some(quoted) => quoted
      .rsplit_once('"')
      .map_or(quoted, |(v, _)| v)
      .to_string(),
    None => value.to_string(),
  }
}
