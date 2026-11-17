//! Ebook and comic previews, read without external programs: the cover and title/author of
//! EPUB (a zip with an XML package file), MOBI/AZW3 (Kindle's PalmDB container) and CBZ
//! (a zip of page images).

use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::thumbnail::{Rendered, cached_preview};
use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, EntryKind};

/// Covers larger than this are not read (a cover is rarely more than a few MiB).
const MAX_COVER_BYTES: u64 = 32 * 1024 * 1024;
/// Package and container files larger than this are not read.
const MAX_XML_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
  Epub,
  Mobi,
  Cbz,
}

impl Format {
  fn of(entry: &Entry) -> Option<Format> {
    match entry.extension()?.as_str() {
      "epub" => Some(Format::Epub),
      "mobi" | "azw" | "azw3" => Some(Format::Mobi),
      "cbz" => Some(Format::Cbz),
      _ => None,
    }
  }

  fn label(self) -> &'static str {
    match self {
      Format::Epub => "EPUB ebook",
      Format::Mobi => "Kindle ebook",
      Format::Cbz => "comic archive",
    }
  }
}

/// What was read from a book: its cover image (encoded, as stored) and detail rows.
#[derive(Debug, Default)]
pub struct Book {
  pub cover: Option<Vec<u8>>,
  pub info: Vec<(String, String)>,
}

pub struct EbookPreviewer {
  /// Show the cover; without graphics only the details are shown.
  pub covers: bool,
}

impl Previewer for EbookPreviewer {
  fn name(&self) -> &str {
    "ebook"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    entry.kind == EntryKind::File && Format::of(entry).is_some()
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    let Some(format) = Format::of(entry) else {
      return Ok(None);
    };
    let kind = if self.covers { "ebook" } else { "ebook-info" };
    cached_preview(entry, kind, format.label(), ctx, |out| {
      // A damaged or unexpected file only means no cover and details.
      let book = read(&entry.path, format).unwrap_or_default();
      if (ctx.cancelled)() {
        return Err(PreviewError::Cancelled);
      }
      match book.cover {
        Some(cover) if self.covers && std::fs::write(out, &cover).is_ok() => {
          Ok(Rendered::Image(book.info))
        }
        _ => Ok(Rendered::InfoOnly(book.info)),
      }
    })
  }
}

fn read(path: &Path, format: Format) -> Option<Book> {
  match format {
    Format::Epub => read_epub(path),
    Format::Mobi => read_mobi(path),
    Format::Cbz => read_cbz(path),
  }
}

// ---- zip based: EPUB and CBZ ----

type Zip = zip::ZipArchive<File>;

fn open_zip(path: &Path) -> Option<Zip> {
  zip::ZipArchive::new(File::open(path).ok()?).ok()
}

/// The bytes of the archive member `name`, if it is no larger than `limit`.
fn member(zip: &mut Zip, name: &str, limit: u64) -> Option<Vec<u8>> {
  let file = zip.by_name(name).ok()?;
  if file.size() > limit {
    return None;
  }
  let mut data = Vec::with_capacity(file.size() as usize);
  file.take(limit).read_to_end(&mut data).ok()?;
  Some(data)
}

fn is_image_name(name: &str) -> bool {
  let lower = name.to_lowercase();
  [".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp"]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// EPUB: `META-INF/container.xml` names the package (OPF) file, whose metadata has the
/// title and authors and whose manifest marks the cover image.
pub fn read_epub(path: &Path) -> Option<Book> {
  let mut zip = open_zip(path)?;
  let container = member(&mut zip, "META-INF/container.xml", MAX_XML_BYTES)?;
  let container = String::from_utf8_lossy(&container);
  let opf_path = tags(&container)
    .find(|t| t.name == "rootfile")
    .and_then(|t| t.attr("full-path").map(str::to_string))?;
  let opf = member(&mut zip, &opf_path, MAX_XML_BYTES)?;
  let opf = String::from_utf8_lossy(&opf);
  let package = Package::parse(&opf);
  let cover = package.cover_href().and_then(|href| {
    // Hrefs are relative to the package file and may be percent-encoded.
    let dir = opf_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let full = normalize_zip_path(dir, &percent_decode(href));
    member(&mut zip, &full, MAX_COVER_BYTES)
  });
  Some(Book {
    cover,
    info: package.rows(),
  })
}

/// CBZ: the page images in natural order; the first is the cover.
pub fn read_cbz(path: &Path) -> Option<Book> {
  let mut zip = open_zip(path)?;
  let mut pages: Vec<String> = zip
    .file_names()
    .filter_map(Result::ok)
    .filter(|n| is_image_name(n) && !n.starts_with("__MACOSX/"))
    .map(Cow::into_owned)
    .collect();
  pages.sort_by(|a, b| crate::fs::sort::natural_cmp(a, b));
  let cover = pages
    .first()
    .and_then(|first| member(&mut zip, first, MAX_COVER_BYTES));
  Some(Book {
    cover,
    info: vec![("Pages".to_string(), pages.len().to_string())],
  })
}

/// `dir` joined with a relative `href`, with `.` and `..` resolved, as a zip member name.
fn normalize_zip_path(dir: &str, href: &str) -> String {
  let mut parts: Vec<&str> = if href.starts_with('/') {
    Vec::new()
  } else {
    dir.split('/').filter(|p| !p.is_empty()).collect()
  };
  for part in href.split('/') {
    match part {
      "" | "." => {}
      ".." => {
        parts.pop();
      }
      p => parts.push(p),
    }
  }
  parts.join("/")
}

fn percent_decode(text: &str) -> String {
  let bytes = text.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'%'
      && let Some(hex) = text.get(i + 1..i + 3)
      && let Ok(b) = u8::from_str_radix(hex, 16)
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

/// The parts of an EPUB package file that matter here.
#[derive(Debug, Default)]
struct Package {
  title: Option<String>,
  authors: Vec<String>,
  publisher: Option<String>,
  date: Option<String>,
  /// `<meta name="cover" content="ID">` (EPUB 2).
  cover_id: Option<String>,
  /// Manifest items: id, href, media type, properties.
  items: Vec<(String, String, String, String)>,
}

impl Package {
  fn parse(xml: &str) -> Package {
    let mut package = Package::default();
    for tag in tags(xml) {
      let text = || Some(unescape(tag.text.trim())).filter(|t| !t.is_empty());
      match tag.name {
        "title" if package.title.is_none() => package.title = text(),
        "creator" => package.authors.extend(text()),
        "publisher" if package.publisher.is_none() => package.publisher = text(),
        "date" if package.date.is_none() => package.date = text(),
        "meta" if tag.attr("name") == Some("cover") => {
          package.cover_id = tag.attr("content").map(str::to_string);
        }
        "item" => {
          let attr = |name| tag.attr(name).unwrap_or_default().to_string();
          package.items.push((
            attr("id"),
            attr("href"),
            attr("media-type"),
            attr("properties"),
          ));
        }
        _ => {}
      }
    }
    package
  }

  /// The cover: the item with the `cover-image` property (EPUB 3), the one `meta
  /// name="cover"` points at (EPUB 2), or an image whose id or name says cover.
  fn cover_href(&self) -> Option<&str> {
    let image = |(_, _, media, _): &&(String, String, String, String)| media.starts_with("image/");
    let by_property = self
      .items
      .iter()
      .find(|(_, _, _, props)| props.split_whitespace().any(|p| p == "cover-image"));
    let by_meta = || {
      let id = self.cover_id.as_deref()?;
      self.items.iter().filter(image).find(|(i, ..)| i == id)
    };
    let by_name = || {
      self.items.iter().filter(image).find(|(id, href, ..)| {
        id.to_lowercase().contains("cover") || href.to_lowercase().contains("cover")
      })
    };
    by_property
      .or_else(by_meta)
      .or_else(by_name)
      .map(|(_, href, ..)| href.as_str())
  }

  fn rows(&self) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(title) = &self.title {
      rows.push(("Title".to_string(), title.clone()));
    }
    if !self.authors.is_empty() {
      rows.push(("Author".to_string(), self.authors.join(", ")));
    }
    if let Some(publisher) = &self.publisher {
      rows.push(("Publisher".to_string(), publisher.clone()));
    }
    if let Some(date) = &self.date {
      // Dates are ISO 8601; the day is enough.
      rows.push(("Published".to_string(), date.chars().take(10).collect()));
    }
    rows
  }
}

/// A start (or empty) tag, with the text that follows it up to the next tag.
struct Tag<'a> {
  /// Without a namespace prefix: `dc:title` is `title`.
  name: &'a str,
  attrs: &'a str,
  text: &'a str,
}

impl Tag<'_> {
  fn attr(&self, name: &str) -> Option<&str> {
    let mut rest = self.attrs;
    while let Some(eq) = rest.find('=') {
      let key = rest[..eq].trim();
      let key = key.rsplit(':').next().unwrap_or(key);
      let after = rest[eq + 1..].trim_start();
      let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
      let value_end = after[1..].find(quote)?;
      let value = &after[1..1 + value_end];
      if key == name {
        return Some(value);
      }
      rest = &after[1 + value_end + 1..];
    }
    None
  }
}

/// The start and empty tags of `xml` in order. Comments, declarations and end tags are
/// skipped. Enough for package files; not a general XML parser.
fn tags(xml: &str) -> impl Iterator<Item = Tag<'_>> {
  let mut rest = xml;
  std::iter::from_fn(move || {
    loop {
      let start = rest.find('<')?;
      let after = &rest[start + 1..];
      let end = after.find('>')?;
      let inner = &after[..end];
      let next = &after[end + 1..];
      let text_end = next.find('<').unwrap_or(next.len());
      rest = next;
      if inner.starts_with(['/', '?', '!']) {
        continue;
      }
      let inner = inner.trim_end_matches('/');
      let (name, attrs) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
      let name = name.rsplit(':').next().unwrap_or(name);
      return Some(Tag {
        name,
        attrs,
        text: &next[..text_end],
      });
    }
  })
}

/// Replaces the predefined and numeric XML entities.
fn unescape(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let mut rest = text;
  while let Some(amp) = rest.find('&') {
    out.push_str(&rest[..amp]);
    let after = &rest[amp + 1..];
    let Some(semi) = after.find(';').filter(|&s| s <= 10) else {
      out.push('&');
      rest = after;
      continue;
    };
    let entity = &after[..semi];
    let decoded = match entity {
      "amp" => Some('&'),
      "lt" => Some('<'),
      "gt" => Some('>'),
      "quot" => Some('"'),
      "apos" => Some('\''),
      e => e
        .strip_prefix("#x")
        .map(|h| u32::from_str_radix(h, 16))
        .or_else(|| e.strip_prefix('#').map(str::parse))
        .and_then(Result::ok)
        .and_then(char::from_u32),
    };
    match decoded {
      Some(c) => out.push(c),
      None => out.push_str(&rest[amp..amp + 1 + semi + 1]),
    }
    rest = &after[semi + 1..];
  }
  out.push_str(rest);
  out
}

// ---- MOBI / AZW3: PalmDB records ----

/// MOBI and AZW3: a PalmDB database whose first record holds the MOBI header (with the
/// index of the first image record) and the EXTH metadata (title, authors, publisher and
/// the cover's offset among the images). Only the records needed are read.
pub fn read_mobi(path: &Path) -> Option<Book> {
  let mut file = File::open(path).ok()?;
  let len = file.metadata().ok()?.len();
  let mut header = [0u8; 78];
  file.read_exact(&mut header).ok()?;
  if &header[60..68] != b"BOOKMOBI" {
    return None;
  }
  let count = usize::from(u16::from_be_bytes([header[76], header[77]]));
  let mut table = vec![0u8; count * 8];
  file.read_exact(&mut table).ok()?;
  let offsets: Vec<u64> = table
    .chunks_exact(8)
    .map(|r| u64::from(u32::from_be_bytes([r[0], r[1], r[2], r[3]])))
    .collect();
  // Record i spans from its offset to the next record's (or the end of the file).
  let record = |file: &mut File, i: usize, limit: u64| -> Option<Vec<u8>> {
    let start = *offsets.get(i)?;
    let end = offsets.get(i + 1).copied().unwrap_or(len).min(len);
    let size = end.checked_sub(start)?;
    if size > limit {
      return None;
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut data = vec![0u8; size as usize];
    file.read_exact(&mut data).ok()?;
    Some(data)
  };
  let rec0 = record(&mut file, 0, MAX_XML_BYTES)?;
  let u32_at = |data: &[u8], at: usize| -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
  };
  // The MOBI header follows the 16-byte PalmDOC header.
  if rec0.get(16..20) != Some(b"MOBI".as_slice()) {
    return None;
  }
  let mobi_len = u32_at(&rec0, 20)? as usize;
  let first_image = u32_at(&rec0, 108);
  let full_name = u32_at(&rec0, 84)
    .zip(u32_at(&rec0, 88))
    .and_then(|(at, n)| {
      let bytes = rec0.get(at as usize..(at as usize).checked_add(n as usize)?)?;
      Some(String::from_utf8_lossy(bytes).into_owned())
    });
  let has_exth = u32_at(&rec0, 128).is_some_and(|flags| flags & 0x40 != 0);
  let exth = if has_exth {
    rec0
      .get(16 + mobi_len..)
      .map(parse_exth)
      .unwrap_or_default()
  } else {
    Vec::new()
  };
  let text = |kind: u32| {
    exth
      .iter()
      .filter(|(k, _)| *k == kind)
      .map(|(_, v)| String::from_utf8_lossy(v).trim().to_string())
      .filter(|v| !v.is_empty())
      .collect::<Vec<_>>()
  };
  let mut info = Vec::new();
  if let Some(title) = text(503).into_iter().next().or(full_name) {
    info.push(("Title".to_string(), title));
  }
  let authors = text(100);
  if !authors.is_empty() {
    info.push(("Author".to_string(), authors.join(", ")));
  }
  if let Some(publisher) = text(101).into_iter().next() {
    info.push(("Publisher".to_string(), publisher));
  }
  if let Some(date) = text(106).into_iter().next() {
    info.push(("Published".to_string(), date.chars().take(10).collect()));
  }
  // EXTH 201: the cover's index among the image records; 0xffffffff means none.
  let cover_offset = exth
    .iter()
    .find(|(k, _)| *k == 201)
    .and_then(|(_, v)| Some(u32::from_be_bytes(v.get(..4)?.try_into().ok()?)))
    .filter(|&o| o != u32::MAX);
  let cover = first_image
    .filter(|&i| i != u32::MAX)
    .zip(cover_offset)
    .and_then(|(first, offset)| {
      let index = usize::try_from(first.checked_add(offset)?).ok()?;
      record(&mut file, index, MAX_COVER_BYTES)
    });
  Some(Book { cover, info })
}

/// EXTH records as (type, data): `EXTH`, header length, record count, then records of
/// type, length (header included) and data.
fn parse_exth(data: &[u8]) -> Vec<(u32, Vec<u8>)> {
  let u32_at =
    |at: usize| -> Option<u32> { Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?)) };
  if data.get(..4) != Some(b"EXTH".as_slice()) {
    return Vec::new();
  }
  let count = u32_at(8).unwrap_or(0);
  let mut records = Vec::new();
  let mut at = 12;
  for _ in 0..count {
    let (Some(kind), Some(size)) = (u32_at(at), u32_at(at + 4)) else {
      break;
    };
    let size = size as usize;
    let Some(body) = data.get(at + 8..at + size.max(8)) else {
      break;
    };
    records.push((kind, body.to_vec()));
    at += size.max(8);
  }
  records
}
