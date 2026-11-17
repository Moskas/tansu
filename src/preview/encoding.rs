//! Turning file content into text whatever its encoding: UTF-8 (with or without a byte order
//! mark), UTF-16 (rip logs from Windows tools), or a legacy encoding such as Shift_JIS or
//! Windows-1252, detected with Mozilla's `chardetng`. Binary data is recognised and refused.

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

/// Decoded text and the encoding it was in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
  pub text: String,
  /// The encoding's name, e.g. `UTF-8`, `UTF-16LE`, `Shift_JIS`, `windows-1252`.
  pub encoding: &'static str,
}

/// How much of the start is sampled to recognise BOM-less UTF-16.
const SAMPLE: usize = 4096;

/// Decodes `data`, the start of a file (it may end inside a multibyte character). `None`
/// when it looks like binary data rather than text.
pub fn decode_text(data: &[u8]) -> Option<Decoded> {
  if let Some((encoding, bom)) = Encoding::for_bom(data) {
    return Some(decode_as(encoding, &data[bom..]));
  }
  if let Some(encoding) = utf16_without_bom(data) {
    return Some(decode_as(encoding, data));
  }
  // Text never contains NUL; nearly every binary format does.
  if data.contains(&0) {
    return None;
  }
  match std::str::from_utf8(data) {
    Ok(text) => {
      return Some(Decoded {
        text: text.to_string(),
        encoding: UTF_8.name(),
      });
    }
    // Only cut off at the end of what was read: UTF-8 all the same.
    Err(e) if e.error_len().is_none() => {
      return Some(Decoded {
        text: String::from_utf8_lossy(&data[..e.valid_up_to()]).into_owned(),
        encoding: UTF_8.name(),
      });
    }
    Err(_) => {}
  }
  if mostly_utf8(data) {
    return Some(Decoded {
      text: String::from_utf8_lossy(data).into_owned(),
      encoding: UTF_8.name(),
    });
  }
  // ISO-2022-JP is 7-bit, so it never gets here (valid UTF-8); refusing it is harmless.
  let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
  detector.feed(data, true);
  let encoding = detector.guess(None, chardetng::Utf8Detection::Allow);
  let decoded = decode_as(encoding, data);
  // Every byte means something in a single-byte encoding, so binary data would "decode":
  // refuse text full of control characters or replacement characters.
  let total = decoded.text.chars().count().max(1);
  let odd = decoded
    .text
    .chars()
    .filter(|&c| {
      c == char::REPLACEMENT_CHARACTER
        || (c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0c' | '\x1b'))
    })
    .count();
  (odd * 50 <= total).then_some(decoded)
}

/// UTF-8 with a few broken bytes, such as a playlist where one program wrote a name in
/// another encoding: more valid multibyte characters than invalid bytes. Text in a legacy
/// single-byte encoding almost never forms valid multibyte sequences, so it does not pass.
fn mostly_utf8(data: &[u8]) -> bool {
  let (mut multibyte, mut invalid) = (0usize, 0usize);
  for chunk in data.utf8_chunks() {
    multibyte += chunk.valid().chars().filter(|c| c.len_utf8() > 1).count();
    invalid += chunk.invalid().len();
  }
  multibyte > invalid * 2
}

fn decode_as(encoding: &'static Encoding, data: &[u8]) -> Decoded {
  let (text, _) = encoding.decode_without_bom_handling(data);
  Decoded {
    text: text.into_owned(),
    encoding: encoding.name(),
  }
}

/// UTF-16 without a byte order mark: text in the ASCII range has a NUL in every other byte,
/// the odd ones for little endian, the even ones for big endian.
fn utf16_without_bom(data: &[u8]) -> Option<&'static Encoding> {
  let sample = &data[..data.len().min(SAMPLE) & !1];
  let pairs = sample.len() / 2;
  if pairs < 4 {
    return None;
  }
  let zeros = |offset: usize| {
    sample
      .iter()
      .skip(offset)
      .step_by(2)
      .filter(|&&b| b == 0)
      .count()
  };
  let (even, odd) = (zeros(0), zeros(1));
  // Mostly ASCII-range characters, and no NULs on the other side.
  if odd * 10 >= pairs * 7 && even * 50 <= pairs {
    Some(UTF_16LE)
  } else if even * 10 >= pairs * 7 && odd * 50 <= pairs {
    Some(UTF_16BE)
  } else {
    None
  }
}
