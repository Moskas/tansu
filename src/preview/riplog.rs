//! CD rip logs from Exact Audio Copy, XLD, CUETools and whipper: a summary of the rip
//! (ripper, date, album, drive, mode, tracks, AccurateRip result, errors, checksum) above
//! the log itself. Other logs are not recognised and stay plain text.

use super::listing::{Listing, Row, lines};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ripper {
  Eac,
  Xld,
  CueTools,
  Whipper,
}

/// The listing for a rip log, or `None` when `text` is not one.
pub fn parse(text: &str) -> Option<Listing> {
  let all: Vec<&str> = lines(text).collect();
  let first = all.iter().map(|l| l.trim()).find(|l| !l.is_empty())?;
  let ripper = if first.starts_with("Exact Audio Copy V") {
    Ripper::Eac
  } else if first.starts_with("X Lossless Decoder version") {
    Ripper::Xld
  } else if first.starts_with("[CUETools log;") {
    Ripper::CueTools
  } else if first.starts_with("Log created by: whipper") {
    Ripper::Whipper
  } else {
    return None;
  };
  let mut info = Vec::new();
  let mut push = |label: &str, value: Option<String>| {
    if let Some(value) = value.filter(|v| !v.is_empty()) {
      info.push((label.to_string(), value));
    }
  };
  match ripper {
    Ripper::Eac | Ripper::Xld => {
      let (ripper_line, date_prefix, mode_key) = match ripper {
        Ripper::Eac => (
          first.split(" from ").next().unwrap_or(first),
          "EAC extraction logfile from ",
          "Read mode",
        ),
        _ => (first, "XLD extraction logfile from ", "Ripper mode"),
      };
      push("Ripper", Some(ripper_line.to_string()));
      let date_at = all.iter().position(|l| l.trim().starts_with(date_prefix));
      push(
        "Ripped",
        date_at.map(|i| all[i].trim()[date_prefix.len()..].trim().to_string()),
      );
      // `Artist / Album` follows the date line.
      push(
        "Album",
        date_at.and_then(|i| {
          all[i + 1..]
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .filter(|l| !l.contains(" : "))
            .map(str::to_string)
        }),
      );
      push("Drive", value(&all, "Used drive").map(|d| drive(&d)));
      push("Read mode", value(&all, mode_key));
      let tracks = all.iter().filter(|l| is_track_header(l)).count();
      push("Tracks", (tracks > 0).then(|| tracks.to_string()));
      push("AccurateRip", accuraterip(&all));
      push("Errors", errors(&all));
      let signed = all
        .iter()
        .any(|l| l.starts_with("==== Log checksum") || l.starts_with("-----BEGIN XLD SIGNATURE"));
      push(
        "Checksum",
        Some(if signed { "present" } else { "none" }.to_string()),
      );
    }
    Ripper::CueTools => {
      // `[CUETools log; Date: 1/2/2020 3:04:05 AM; Version: 2.1.6]`
      let field = |name: &str| {
        first
          .trim_matches(['[', ']'])
          .split(';')
          .find_map(|part| part.trim().strip_prefix(name))
          .map(|v| v.trim().to_string())
      };
      push("Ripper", field("Version:").map(|v| format!("CUETools {v}")));
      push("Date", field("Date:"));
      push(
        "AccurateRip",
        all
          .iter()
          .map(|l| l.trim())
          .find(|l| l.starts_with("[AccurateRip ID:"))
          .map(str::to_string),
      );
    }
    Ripper::Whipper => {
      push(
        "Ripper",
        first
          .strip_prefix("Log created by:")
          .map(|v| v.trim().to_string()),
      );
      push("Ripped", yaml_value(&all, "Log creation date"));
      push("Health", yaml_value(&all, "Health status"));
      push("AccurateRip", yaml_value(&all, "AccurateRip summary"));
    }
  }
  let mut rows: Vec<Row> = Vec::new();
  let mut blank = false;
  for line in &all {
    let line = line.trim_end();
    // Runs of blank lines (EAC and XLD use many) are kept to one.
    if line.is_empty() {
      if !blank && !rows.is_empty() {
        rows.push(Row::Text(String::new()));
      }
      blank = true;
      continue;
    }
    blank = false;
    rows.push(Row::Text(line.to_string()));
  }
  Some(Listing {
    kind: "rip log",
    info,
    rows,
  })
}

/// The value of an aligned `Key   : value` line.
fn value(all: &[&str], key: &str) -> Option<String> {
  all.iter().find_map(|line| {
    let (k, v) = line.split_once(':')?;
    (k.trim() == key).then(|| v.trim().to_string())
  })
}

/// The value of a `Key: value` line in whipper's YAML.
fn yaml_value(all: &[&str], key: &str) -> Option<String> {
  value(all, key).map(|v| v.trim_matches('"').to_string())
}

/// `PIONEER BD-RW   BDR-209D   Adapter: 0  ID: 0` as `PIONEER BD-RW BDR-209D`.
fn drive(raw: &str) -> String {
  let name = raw.split("Adapter:").next().unwrap_or(raw);
  name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Track  1` (EAC) or `Track 01` (XLD), alone on its line.
fn is_track_header(line: &str) -> bool {
  let line = line.trim();
  line
    .strip_prefix("Track")
    .is_some_and(|n| n.starts_with(' ') && n.trim().parse::<u32>().is_ok())
}

/// The overall AccurateRip result: EAC's status lines (not indented, unlike the per-track
/// ones), or XLD's `->` summary lines.
fn accuraterip(all: &[&str]) -> Option<String> {
  let eac = all.iter().rev().find(|l| {
    !l.starts_with(' ') && (l.contains("accurately ripped") || l.contains("AccurateRip database"))
  });
  if let Some(line) = eac {
    return Some(line.trim().to_string());
  }
  let xld: Vec<&str> = all
    .iter()
    .filter_map(|l| l.trim().strip_prefix("->"))
    .map(str::trim)
    .collect();
  (!xld.is_empty()).then(|| xld.join("; "))
}

/// Whether the rip reports errors, with EAC's suspicious positions counted.
fn errors(all: &[&str]) -> Option<String> {
  let suspicious = all
    .iter()
    .filter(|l| l.trim_start().starts_with("Suspicious position"))
    .count();
  let clean = all.iter().any(|l| l.trim() == "No errors occurred");
  let failed = all
    .iter()
    .any(|l| l.trim() == "There were errors" || l.trim() == "Some inconsistencies found");
  match (clean, failed, suspicious) {
    (_, _, n) if n > 0 => Some(format!(
      "{n} suspicious position{}",
      if n == 1 { "" } else { "s" }
    )),
    (_, true, _) => Some("there were errors".to_string()),
    (true, _, _) => Some("none".to_string()),
    _ => None,
  }
}
