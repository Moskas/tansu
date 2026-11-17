//! The photographic details of an image's EXIF data, as label/value rows: camera, lens, date
//! taken, exposure, aperture, ISO, focal length and location.

use exif::{Exif, Field, In, Reader, Tag, Value};

/// Rows for the EXIF data in `raw` (a TIFF structure, as image decoders return it). Missing
/// fields are left out; unreadable data gives no rows.
pub fn summary(raw: Vec<u8>) -> Vec<(String, String)> {
  let Ok(exif) = Reader::new().read_raw(raw) else {
    return Vec::new();
  };
  let mut rows = Vec::new();
  let mut push = |label: &str, value: Option<String>| {
    if let Some(value) = value.filter(|v| !v.is_empty()) {
      rows.push((label.to_string(), value));
    }
  };
  push("Camera", camera(&exif));
  push("Lens", text(&exif, Tag::LensModel));
  push(
    "Taken",
    text(&exif, Tag::DateTimeOriginal)
      .or_else(|| text(&exif, Tag::DateTime))
      .map(|d| date(&d)),
  );
  push("Exposure", exposure(&exif));
  push("Aperture", shown(&exif, Tag::FNumber));
  push("ISO", shown(&exif, Tag::PhotographicSensitivity));
  push("Focal length", focal_length(&exif));
  push("Location", location(&exif));
  rows
}

fn field(exif: &Exif, tag: Tag) -> Option<&Field> {
  exif.get_field(tag, In::PRIMARY)
}

/// An ASCII field's text, trimmed (cameras pad with spaces and NULs).
fn text(exif: &Exif, tag: Tag) -> Option<String> {
  let Value::Ascii(parts) = &field(exif, tag)?.value else {
    return None;
  };
  let joined: Vec<String> = parts
    .iter()
    .map(|p| {
      String::from_utf8_lossy(p)
        .trim_matches(['\0', ' '])
        .to_string()
    })
    .filter(|p| !p.is_empty())
    .collect();
  Some(joined.join(" "))
}

/// A field as the EXIF library formats it, with its unit (`1/250 s`, `f/2.8`, `4.2 mm`).
fn shown(exif: &Exif, tag: Tag) -> Option<String> {
  let field = field(exif, tag)?;
  Some(field.display_value().with_unit(exif).to_string())
}

/// Make and model, without the make repeated when the model already starts with it
/// (`Canon` + `Canon EOS R6`).
fn camera(exif: &Exif) -> Option<String> {
  let make = text(exif, Tag::Make);
  let model = text(exif, Tag::Model);
  match (make, model) {
    (Some(make), Some(model))
      if model.to_lowercase().starts_with(&make.to_lowercase())
        || make.to_lowercase().starts_with(&model.to_lowercase()) =>
    {
      Some(model)
    }
    (Some(make), Some(model)) => Some(format!("{make} {model}")),
    (make, model) => make.or(model),
  }
}

/// `2024:05:01 12:34:56` (EXIF's form) as `2024-05-01 12:34:56`.
fn date(raw: &str) -> String {
  match raw.split_once(' ') {
    Some((day, time)) => format!("{} {time}", day.replace(':', "-")),
    None => raw.to_string(),
  }
}

/// `26 mm`, followed by the full-frame equivalent when the camera records it.
fn focal_length(exif: &Exif) -> Option<String> {
  let actual = shown(exif, Tag::FocalLength)?;
  let equivalent = field(exif, Tag::FocalLengthIn35mmFilm).and_then(|f| f.value.get_uint(0));
  Some(match equivalent {
    Some(mm) if mm > 0 => format!("{actual} ({mm} mm full-frame)"),
    _ => actual,
  })
}

/// GPS position in decimal degrees: `52.22970° N, 21.01220° E`.
fn location(exif: &Exif) -> Option<String> {
  let lat = degrees(exif, Tag::GPSLatitude)?;
  let lon = degrees(exif, Tag::GPSLongitude)?;
  let lat_ref = text(exif, Tag::GPSLatitudeRef).unwrap_or_else(|| "N".to_string());
  let lon_ref = text(exif, Tag::GPSLongitudeRef).unwrap_or_else(|| "E".to_string());
  Some(format!("{lat:.5}° {lat_ref}, {lon:.5}° {lon_ref}"))
}

/// Degrees, minutes and seconds as one decimal number.
fn degrees(exif: &Exif, tag: Tag) -> Option<f64> {
  let Value::Rational(parts) = &field(exif, tag)?.value else {
    return None;
  };
  let part = |i: usize| parts.get(i).map_or(0.0, |r| r.to_f64());
  let value = part(0) + part(1) / 60.0 + part(2) / 3600.0;
  value.is_finite().then_some(value)
}

/// Exposure time the way cameras show it: `1/250 s` below a second (rounded to a whole
/// fraction, since phones record values like 1/177.999...), `2.5 s` from a second up.
fn exposure(exif: &Exif) -> Option<String> {
  let Value::Rational(parts) = &field(exif, Tag::ExposureTime)?.value else {
    return None;
  };
  let seconds = parts.first()?.to_f64();
  if !seconds.is_finite() || seconds <= 0.0 {
    return None;
  }
  Some(if seconds < 1.0 {
    format!("1/{} s", (1.0 / seconds).round())
  } else {
    let shown = format!("{seconds:.1}");
    format!("{} s", shown.trim_end_matches(".0"))
  })
}
