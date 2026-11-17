//! zip and tar, read in-process.

use std::cell::Cell;
use std::fs::{self, File, FileTimes};
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use ruzstd::decoding::{FrameDecoder, StreamingDecoder};
use zip::CompressionMethod;

use super::{Compression, Contents, ListError, Member, MemberKind};
use crate::fs::meta::format_mtime;

/// Listing a compressed tar means decompressing it; this much is read at most, so a huge
/// archive does not keep a core busy for minutes.
pub const MAX_SCAN: u64 = 256 * 1024 * 1024;

/// Why a [`Guarded`] reader stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
  Cancelled,
  Limit,
}

/// Passes reads through until `cancelled` returns true or `limit` bytes were read; then it
/// fails, and records why in `stop`.
struct Guarded<'a, R> {
  inner: R,
  read: u64,
  limit: u64,
  cancelled: &'a dyn Fn() -> bool,
  stop: &'a Cell<Option<Stop>>,
}

impl<R: Read> Read for Guarded<'_, R> {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    let stop = if (self.cancelled)() {
      Some(Stop::Cancelled)
    } else if self.read >= self.limit {
      Some(Stop::Limit)
    } else {
      None
    };
    if let Some(stop) = stop {
      self.stop.set(Some(stop));
      return Err(io::Error::other("stopped"));
    }
    let n = self.inner.read(buf)?;
    self.read += n as u64;
    Ok(n)
  }
}

/// A reader that decompresses `file`.
fn decoder<'a>(file: impl Read + 'a, c: Compression) -> io::Result<Box<dyn Read + 'a>> {
  let buf = BufReader::new(file);
  Ok(match c {
    Compression::None => Box::new(buf),
    Compression::Gzip => Box::new(flate2::bufread::MultiGzDecoder::new(buf)),
    Compression::Bzip2 => Box::new(bzip2::bufread::MultiBzDecoder::new(buf)),
    Compression::Xz => Box::new(lzma_rust2::XzReader::new(buf, true)),
    Compression::Lzip => Box::new(lzma_rust2::LzipReader::new(buf)),
    Compression::Zstd => Box::new(Zstd::new(buf)?),
  })
}

/// zstd that goes on past the end of a frame, as files made by concatenation have several.
struct Zstd<R: BufRead> {
  inner: Option<StreamingDecoder<R, FrameDecoder>>,
}

impl<R: BufRead> Zstd<R> {
  fn new(source: R) -> io::Result<Self> {
    let inner = StreamingDecoder::new(source).map_err(io::Error::other)?;
    Ok(Zstd { inner: Some(inner) })
  }
}

impl<R: BufRead> Read for Zstd<R> {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    loop {
      let Some(inner) = &mut self.inner else {
        return Ok(0);
      };
      let n = inner.read(buf)?;
      if n > 0 || buf.is_empty() {
        return Ok(n);
      }
      let (mut source, decoder) = self.inner.take().expect("checked above").into_parts();
      if source.fill_buf()?.is_empty() {
        return Ok(0);
      }
      let next = StreamingDecoder::new_with_decoder(source, decoder).map_err(io::Error::other)?;
      self.inner = Some(next);
    }
  }
}

fn tar_label(c: Compression) -> String {
  match c {
    Compression::None => "tar".to_string(),
    c => format!("tar, {}", c.name()),
  }
}

/// Whether a block of decompressed data starts a tar archive (POSIX and GNU tar mark the
/// header with `ustar` at offset 257).
fn looks_like_tar(block: &[u8]) -> bool {
  block.get(257..262) == Some(b"ustar")
}

pub fn list_zip(
  path: &Path,
  max: usize,
  cancelled: &dyn Fn() -> bool,
) -> Result<Contents, ListError> {
  let mut zip = zip::ZipArchive::new(File::open(path)?).map_err(failed)?;
  let names = NameDecoder::for_zip(&mut zip);
  let mut contents = Contents::new(match names.0 {
    Some(encoding) => format!("zip, {} names", encoding.name()),
    None => "zip".to_string(),
  });
  let comment = names.decode(zip.comment());
  if let Some(line) = comment.lines().map(str::trim).find(|l| !l.is_empty()) {
    contents.comment = Some(line.to_string());
  }
  for i in 0..zip.len() {
    if i % 256 == 0 && cancelled() {
      return Err(ListError::Cancelled);
    }
    // Raw: only the metadata is wanted, and it reads encrypted members too.
    let file = zip.by_index_raw(i).map_err(failed)?;
    // Windows tools may separate with `\`, which extraction does too.
    let name = names.decode(file.name_raw()).replace('\\', "/");
    let kind = if file.is_dir() || name.ends_with('/') {
      MemberKind::Dir
    } else if file.is_symlink() {
      MemberKind::Link
    } else {
      MemberKind::File
    };
    let modified = file.last_modified().map(|t| {
      format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute()
      )
    });
    let member = Member {
      path: name,
      kind,
      size: (kind != MemberKind::Dir).then_some(file.size()),
      modified,
      link: None,
      encrypted: file.encrypted(),
    };
    contents.add(member, max);
  }
  Ok(contents)
}

pub fn list_tar(
  path: &Path,
  c: Compression,
  max: usize,
  cancelled: &dyn Fn() -> bool,
) -> Result<Contents, ListError> {
  let mut contents = Contents::new(tar_label(c));
  let file = File::open(path)?;
  if c == Compression::None {
    // Seeking skips over file data instead of reading it.
    let mut archive = tar::Archive::new(file);
    let entries = archive.entries_with_seek()?;
    read_tar(entries, &mut contents, max, cancelled)?;
    return Ok(contents);
  }
  let stop = Cell::new(None);
  let reader = Guarded {
    inner: decoder(file, c)?,
    read: 0,
    limit: MAX_SCAN,
    cancelled,
    stop: &stop,
  };
  let mut archive = tar::Archive::new(reader);
  let result = archive
    .entries()
    .map_err(ListError::from)
    .and_then(|entries| read_tar(entries, &mut contents, max, cancelled));
  match (stop.get(), result) {
    (Some(Stop::Cancelled), _) => Err(ListError::Cancelled),
    (Some(Stop::Limit), _) => {
      contents.complete = false;
      Ok(contents)
    }
    (None, result) => result.map(|()| contents),
  }
}

/// Adds the members of a tar archive to `contents`, stopping after `max` of them.
fn read_tar<R: Read>(
  entries: tar::Entries<R>,
  contents: &mut Contents,
  max: usize,
  cancelled: &dyn Fn() -> bool,
) -> Result<(), ListError> {
  use tar::EntryType;
  for entry in entries {
    if cancelled() {
      return Err(ListError::Cancelled);
    }
    let entry = entry?;
    // Past the members kept: counting the rest would mean reading all of it.
    if contents.members.len() >= max {
      contents.complete = false;
      break;
    }
    let header = entry.header();
    let kind = match header.entry_type() {
      EntryType::Directory => MemberKind::Dir,
      EntryType::Symlink => MemberKind::Link,
      EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse | EntryType::Link => {
        MemberKind::File
      }
      // Extended headers describing other entries, or the archive as a whole.
      EntryType::XGlobalHeader
      | EntryType::XHeader
      | EntryType::GNULongName
      | EntryType::GNULongLink => continue,
      _ => MemberKind::Other,
    };
    let member = Member {
      path: String::from_utf8_lossy(&entry.path_bytes()).into_owned(),
      kind,
      size: (kind == MemberKind::File).then(|| entry.size()),
      modified: header.mtime().ok().map(|t| format_mtime(t as i64)),
      link: if kind == MemberKind::Link {
        entry
          .link_name_bytes()
          .map(|l| String::from_utf8_lossy(&l).into_owned())
      } else {
        None
      },
      encrypted: false,
    };
    contents.add(member, max);
  }
  Ok(())
}

/// A file compressed on its own (`notes.txt.gz`), or a tar archive inside: the first block
/// tells. A single file is decompressed (up to [`MAX_SCAN`]) to learn its size.
pub fn list_compressed(
  path: &Path,
  c: Compression,
  name: &str,
  max: usize,
  cancelled: &dyn Fn() -> bool,
) -> Result<Contents, ListError> {
  let stop = Cell::new(None);
  let mut reader = Guarded {
    inner: decoder(File::open(path)?, c)?,
    read: 0,
    limit: MAX_SCAN,
    cancelled,
    stop: &stop,
  };
  let mut block = Vec::with_capacity(512);
  let first = (&mut reader).take(512).read_to_end(&mut block);
  if stop.get() == Some(Stop::Cancelled) {
    return Err(ListError::Cancelled);
  }
  first?;
  if looks_like_tar(&block) {
    drop(reader);
    return list_tar(path, c, max, cancelled);
  }
  let rest = io::copy(&mut reader, &mut io::sink());
  let mut contents = Contents::new(c.name());
  contents.single = true;
  let size = match (stop.get(), rest) {
    (Some(Stop::Cancelled), _) => return Err(ListError::Cancelled),
    (Some(Stop::Limit), _) => {
      contents.complete = false;
      None
    }
    (None, Ok(n)) => Some(block.len() as u64 + n),
    (None, Err(e)) => return Err(e.into()),
  };
  let (_, stem) = super::Format::from_name(name).ok_or(ListError::Unsupported)?;
  let member = Member {
    path: stem.to_string(),
    kind: MemberKind::File,
    size,
    modified: None,
    link: None,
    encrypted: false,
  };
  contents.add(member, max);
  Ok(contents)
}

fn failed(e: zip::result::ZipError) -> ListError {
  ListError::Failed(e.to_string())
}

/// Whether the built-in zip support can unpack every member (it has deflate only); if not,
/// an external program has to.
pub fn zip_supported(path: &Path) -> io::Result<bool> {
  let mut zip = zip::ZipArchive::new(File::open(path)?).map_err(io::Error::other)?;
  for i in 0..zip.len() {
    let file = zip.by_index_raw(i).map_err(io::Error::other)?;
    if !matches!(
      file.compression(),
      CompressionMethod::Stored | CompressionMethod::Deflated
    ) {
      return Ok(false);
    }
  }
  Ok(true)
}

/// Extracts a zip archive into the empty directory `dir`, with permissions (without setuid
/// and setgid) and modification times, and returns the names of damaged members: those
/// whose data fails its checksum or does not decompress. They are left out.
///
/// Members are written one by one so a damaged one does not stop the rest. Paths leading
/// outside `dir` are skipped, and links are made last, only when they point inside `dir`:
/// with nothing written after them, nothing can be written through them.
pub fn extract_zip(path: &Path, dir: &Path) -> io::Result<Vec<String>> {
  let mut zip = zip::ZipArchive::new(File::open(path)?).map_err(io::Error::other)?;
  let names = NameDecoder::for_zip(&mut zip);
  let mut damaged = Vec::new();
  let mut links = Vec::new();
  let mut dirs = Vec::new();
  let mut buf = vec![0; 64 * 1024];
  for i in 0..zip.len() {
    let mut file = zip.by_index(i).map_err(io::Error::other)?;
    let name = names.decode(file.name_raw());
    let Some(rel) = enclosed(&name) else {
      continue;
    };
    let target = dir.join(&rel);
    let mode = file.unix_mode().map(|m| m & 0o777);
    let time = file.last_modified().as_ref().and_then(local_time);
    if file.is_dir() || name.ends_with('\\') {
      fs::create_dir_all(&target)?;
      dirs.push((target, mode, time));
      continue;
    }
    if let Some(parent) = target.parent() {
      fs::create_dir_all(parent)?;
    }
    if file.is_symlink() {
      let mut link = Vec::new();
      match (&mut file).take(4096).read_to_end(&mut link) {
        Ok(_) => links.push((rel, target, link)),
        Err(_) => damaged.push(name),
      }
      continue;
    }
    // A name already taken (a member listed twice): the first one stays.
    let mut out = match File::create_new(&target) {
      Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
      r => r?,
    };
    if !copy_member(&mut file, &mut out, &mut buf)? {
      drop(out);
      fs::remove_file(&target)?;
      damaged.push(name);
      continue;
    }
    if let Some(mode) = mode {
      out.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    if let Some(time) = time {
      // Best effort, like `cp -p`: the contents are what matters.
      let _ = out.set_times(FileTimes::new().set_accessed(time).set_modified(time));
    }
  }
  for (rel, target, link) in links {
    let link = Path::new(std::ffi::OsStr::from_bytes(&link));
    if stays_inside(&rel, link) && fs::symlink_metadata(&target).is_err() {
      std::os::unix::fs::symlink(link, &target)?;
    }
  }
  // Last and deepest first, so read-only directories could still be filled.
  dirs.sort_by_key(|(path, ..)| std::cmp::Reverse(path.components().count()));
  for (path, mode, time) in dirs {
    if let Some(time) = time
      && let Ok(f) = File::open(&path)
    {
      let _ = f.set_times(FileTimes::new().set_accessed(time).set_modified(time));
    }
    if let Some(mode) = mode {
      fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
    }
  }
  Ok(damaged)
}

/// Decodes zip member names. Names in UTF-8 (flagged so or not) are taken as they are.
/// Tools on Japanese, Chinese or Korean Windows write names in the system code page
/// (Shift_JIS, GBK, ...) without saying so, where the zip crate would read DOS CP437: for
/// those, the encoding is detected once over all such names of the archive.
pub struct NameDecoder(Option<&'static encoding_rs::Encoding>);

impl NameDecoder {
  pub fn for_zip<R: Read + io::Seek>(zip: &mut zip::ZipArchive<R>) -> Self {
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    let mut legacy = false;
    for i in 0..zip.len() {
      if let Ok(file) = zip.by_index_raw(i)
        && std::str::from_utf8(file.name_raw()).is_err()
      {
        legacy = true;
        detector.feed(file.name_raw(), false);
        // Keeps names apart, as a line break would in text.
        detector.feed(b"\n", false);
      }
    }
    if !legacy {
      return NameDecoder(None);
    }
    detector.feed(b"", true);
    let guess = detector.guess(None, chardetng::Utf8Detection::Deny);
    NameDecoder(Some(guess))
  }

  pub fn decode(&self, raw: &[u8]) -> String {
    match (std::str::from_utf8(raw), self.0) {
      (Ok(name), _) => name.to_string(),
      (Err(_), Some(encoding)) => encoding.decode_without_bom_handling(raw).0.into_owned(),
      (Err(_), None) => String::from_utf8_lossy(raw).into_owned(),
    }
  }
}

/// The relative path a member name stands for, or `None` when it would lead outside the
/// extraction directory (`..`, an absolute path, a drive letter) or names nothing. Both `/`
/// and `\` separate components, as Windows tools write either.
fn enclosed(name: &str) -> Option<std::path::PathBuf> {
  if name.starts_with(['/', '\\']) || name.contains('\0') {
    return None;
  }
  let mut path = std::path::PathBuf::new();
  for component in name.split(['/', '\\']) {
    match component {
      "" | "." => {}
      ".." => return None,
      c if c.len() >= 2 && c.as_bytes()[1] == b':' && path.as_os_str().is_empty() => {
        return None;
      }
      c => path.push(c),
    }
  }
  (!path.as_os_str().is_empty()).then_some(path)
}

/// Copies a member's data to `out`. False when reading it fails (a bad checksum, data that
/// does not decompress); writing errors are errors.
fn copy_member(file: &mut impl Read, out: &mut File, buf: &mut [u8]) -> io::Result<bool> {
  use std::io::Write;
  loop {
    let n = match file.read(buf) {
      Ok(0) => return Ok(true),
      Ok(n) => n,
      Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
      Err(_) => return Ok(false),
    };
    out.write_all(&buf[..n])?;
  }
}

/// Whether a link at `rel` (relative to the extraction directory) to `target` points
/// inside that directory. `..` is only allowed before the first name: the directories
/// leading to `rel` are real ones, so climbing from there is exact, while a name may be
/// another link, and climbing out of one is not. Every link pointing inside, a target
/// going down through links stays inside too.
fn stays_inside(rel: &Path, target: &Path) -> bool {
  use std::path::Component;
  let mut depth = rel.components().count().saturating_sub(1);
  let mut named = false;
  for component in target.components() {
    match component {
      Component::Normal(_) => named = true,
      Component::CurDir => {}
      Component::ParentDir if !named && depth > 0 => depth -= 1,
      Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
    }
  }
  true
}

/// A zip timestamp, which is in local time, as a point in time.
fn local_time(t: &zip::DateTime) -> Option<SystemTime> {
  // SAFETY: `tm` is plain data that mktime reads and normalizes in place.
  let secs = unsafe {
    let mut tm: libc::tm = std::mem::zeroed();
    tm.tm_year = i32::from(t.year()) - 1900;
    tm.tm_mon = i32::from(t.month()) - 1;
    tm.tm_mday = i32::from(t.day());
    tm.tm_hour = i32::from(t.hour());
    tm.tm_min = i32::from(t.minute());
    tm.tm_sec = i32::from(t.second());
    tm.tm_isdst = -1;
    libc::mktime(&mut tm)
  };
  let secs = u64::try_from(secs).ok()?;
  Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

/// Extracts a tar archive into the empty directory `dir`. Entries leading outside it are
/// skipped; setuid and setgid bits are dropped.
pub fn extract_tar(path: &Path, c: Compression, dir: &Path) -> io::Result<()> {
  let mut archive = tar::Archive::new(decoder(File::open(path)?, c)?);
  archive.set_preserve_permissions(false);
  archive.unpack(dir)
}

/// Decompresses a single compressed file to `target`, or, when it turns out to hold a tar
/// archive, extracts that into `dir` (the directory `target` is in).
pub fn extract_compressed(
  path: &Path,
  c: Compression,
  dir: &Path,
  target: &Path,
) -> io::Result<()> {
  let mut reader = decoder(File::open(path)?, c)?;
  let mut block = Vec::with_capacity(512);
  (&mut reader).take(512).read_to_end(&mut block)?;
  if looks_like_tar(&block) {
    drop(reader);
    return extract_tar(path, c, dir);
  }
  let mut out = File::create_new(target)?;
  io::copy(&mut io::Cursor::new(block).chain(reader), &mut out)?;
  Ok(())
}
