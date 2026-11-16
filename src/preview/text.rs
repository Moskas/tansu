use std::fs::File;
use std::io::Read;

use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, EntryKind};

const TAB_WIDTH: usize = 4;
const CHUNK: usize = 8 * 1024;

/// Shows the first lines of regular files that look like text. Declines binary content.
pub struct TextPreviewer;

impl Previewer for TextPreviewer {
  fn name(&self) -> &str {
    "text"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    entry.kind == EntryKind::File
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    let mut file = File::open(&entry.path).map_err(|e| PreviewError::from_io(&entry.path, e))?;
    let mut data = Vec::new();
    let mut chunk = vec![0u8; CHUNK];
    while data.len() < ctx.limits.max_bytes {
      if (ctx.cancelled)() {
        return Err(PreviewError::Cancelled);
      }
      let want = CHUNK.min(ctx.limits.max_bytes - data.len());
      let n = match file.read(&mut chunk[..want]) {
        Ok(0) => break,
        Ok(n) => n,
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(e) => return Err(PreviewError::from_io(&entry.path, e)),
      };
      data.extend_from_slice(&chunk[..n]);
    }
    if looks_binary(&data) {
      return Ok(None);
    }
    let byte_capped =
      data.len() as u64 >= ctx.limits.max_bytes as u64 && entry.size > data.len() as u64;
    let text = decode(&data);
    let mut lines = Vec::new();
    let mut truncated = byte_capped;
    for line in text.split('\n') {
      if lines.len() >= ctx.limits.max_lines {
        truncated = true;
        break;
      }
      lines.push(clean_line(line));
    }
    // A trailing newline produces one empty final element that is not a real line.
    if text.ends_with('\n') && !truncated {
      lines.pop();
    }
    Ok(Some(Preview::Text { lines, truncated }))
  }
}

/// Heuristic: NUL bytes, or many invalid UTF-8 sequences, mean binary.
pub fn looks_binary(data: &[u8]) -> bool {
  if data.contains(&0) {
    return true;
  }
  let text = String::from_utf8_lossy(data);
  let bad = text
    .chars()
    .filter(|&c| c == char::REPLACEMENT_CHARACTER)
    .count();
  let total = text.chars().count().max(1);
  // A few bad sequences are tolerated (e.g. a multi-byte char cut off at the read cap).
  bad > 4 && bad * 20 > total
}

fn decode(data: &[u8]) -> String {
  match std::str::from_utf8(data) {
    Ok(s) => s.to_string(),
    // Drop an incomplete trailing sequence caused by the byte cap.
    Err(e) if e.error_len().is_none() => String::from_utf8_lossy(&data[..e.valid_up_to()]).into(),
    Err(_) => String::from_utf8_lossy(data).into_owned(),
  }
}

/// Expands tabs and replaces control characters so the line is safe to draw.
fn clean_line(line: &str) -> String {
  let mut out = String::with_capacity(line.len());
  let mut col = 0;
  for c in line.chars() {
    match c {
      '\t' => {
        let pad = TAB_WIDTH - col % TAB_WIDTH;
        out.extend(std::iter::repeat_n(' ', pad));
        col += pad;
      }
      '\r' => {}
      c if c.is_control() => {
        out.push('?');
        col += 1;
      }
      c => {
        out.push(c);
        col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
      }
    }
  }
  out
}
