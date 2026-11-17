//! Previews rendered by external programs (a video frame, a PDF page, an ebook cover):
//! running those programs with a timeout and cancellation, and caching what they render on
//! disk so a file is only converted once per version.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use image::{DynamicImage, ImageReader};

use super::{Preview, PreviewContext, PreviewError, PreviewImage};
use crate::fs::Entry;

/// How long an external program may take before it is killed.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// How often a running program is checked for being done, timed out or cancelled.
const POLL: Duration = Duration::from_millis(20);
/// The cache keeps at most this many thumbnails; the oldest go first.
const MAX_ENTRIES: usize = 1000;
/// The cache takes at most this many bytes; again the oldest go first.
const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// Rendered images are kept no larger than this on either side: more than any preview
/// column needs, and it bounds what a 4K video frame costs in the cache.
pub const MAX_SIDE: u32 = 2048;

/// What a program printed, when it ran to completion.
pub struct Output {
  pub success: bool,
  pub stdout: Vec<u8>,
}

/// Runs `program` with `args` and waits for it, capturing standard output. Returns
/// `Ok(None)` when the program is not installed, so the caller can decline the entry. A
/// program that runs past [`TIMEOUT`] is killed and counts as failed; cancellation kills it
/// and returns [`PreviewError::Cancelled`].
pub fn run(
  program: &str,
  args: &[&OsStr],
  ctx: &PreviewContext,
) -> Result<Option<Output>, PreviewError> {
  let mut command = Command::new(program);
  command.args(args);
  run_command(command, ctx.cancelled)
}

/// [`run`] for a prepared command, which may set environment variables or a directory.
pub fn run_command(
  mut command: Command,
  cancelled: &dyn Fn() -> bool,
) -> Result<Option<Output>, PreviewError> {
  let mut child = match command
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
  {
    Ok(child) => child,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
    // Not executable and the like: as good as missing.
    Err(_) => return Ok(None),
  };
  // Read on another thread so a chatty program never blocks on a full pipe.
  let mut stdout = child.stdout.take().expect("stdout is piped");
  let reader = std::thread::spawn(move || {
    let mut out = Vec::new();
    let _ = stdout.read_to_end(&mut out);
    out
  });
  let started = Instant::now();
  let status = loop {
    if cancelled() {
      let _ = child.kill();
      let _ = child.wait();
      let _ = reader.join();
      return Err(PreviewError::Cancelled);
    }
    match child.try_wait() {
      Ok(Some(status)) => break Some(status),
      Ok(None) if started.elapsed() > TIMEOUT => {
        let _ = child.kill();
        let _ = child.wait();
        break None;
      }
      Ok(None) => std::thread::sleep(POLL),
      Err(_) => break None,
    }
  };
  let stdout = reader.join().unwrap_or_default();
  Ok(Some(Output {
    success: status.is_some_and(|s| s.success()),
    stdout,
  }))
}

/// Rendered thumbnails on disk (`$XDG_CACHE_HOME/tansu/thumbnails`), keyed by the file's
/// path, size and modification time, so a changed file is rendered again.
#[derive(Debug, Clone)]
pub struct ThumbnailCache {
  dir: PathBuf,
}

impl ThumbnailCache {
  pub fn new(dir: PathBuf) -> Self {
    ThumbnailCache { dir }
  }

  pub fn dir(&self) -> &Path {
    &self.dir
  }

  /// File name stem for `entry` rendered as `kind` (`video`, `pdf`, ...).
  fn key(entry: &Entry, kind: &str) -> String {
    use std::os::unix::ffi::OsStrExt;
    // FNV-1a: stable across Rust versions, unlike the standard library's hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
      for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
      }
      // A separator, so ("ab", "c") and ("a", "bc") differ.
      hash ^= 0xff;
      hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    feed(kind.as_bytes());
    feed(entry.path.as_os_str().as_bytes());
    feed(&entry.size.to_le_bytes());
    feed(&entry.mtime.to_le_bytes());
    format!("{kind}-{hash:016x}")
  }

  fn image_path(&self, key: &str) -> PathBuf {
    self.dir.join(format!("{key}.png"))
  }

  fn info_path(&self, key: &str) -> PathBuf {
    self.dir.join(format!("{key}.txt"))
  }

  /// The cached image and details for `key`, if both are there.
  fn get(&self, key: &str) -> Option<(DynamicImage, Vec<(String, String)>)> {
    let info = fs::read_to_string(self.info_path(key)).ok()?;
    let image = decode(&self.image_path(key))?;
    Some((image, parse_info(&info)))
  }

  /// Stores a rendered image with its details. Failures only mean the next visit renders
  /// again.
  fn put(&self, key: &str, image: &DynamicImage, info: &[(String, String)]) {
    // Written under temporary names and renamed, so a half-written entry is never read.
    let partial = self.dir.join(format!("{key}.png.part"));
    let saved = image.save_with_format(&partial, image::ImageFormat::Png);
    if saved.is_err() || fs::rename(&partial, self.image_path(key)).is_err() {
      let _ = fs::remove_file(&partial);
      return;
    }
    // The details are written last: an entry counts as cached once they exist.
    let partial = self.dir.join(format!("{key}.txt.part"));
    if fs::write(&partial, format_info(info)).is_ok() {
      let _ = fs::rename(&partial, self.info_path(key));
    }
    self.prune();
  }

  /// Removes the oldest thumbnails while there are more than [`MAX_ENTRIES`] or they take
  /// more than [`MAX_BYTES`], down to 80% of either.
  fn prune(&self) {
    let Ok(dir) = fs::read_dir(&self.dir) else {
      return;
    };
    let size = |p: &Path| fs::metadata(p).map_or(0, |m| m.len());
    let mut entries: Vec<(std::time::SystemTime, PathBuf, u64)> = dir
      .filter_map(Result::ok)
      .filter(|e| e.path().extension() == Some(OsStr::new("txt")))
      .filter_map(|e| {
        let info = e.path();
        let modified = e.metadata().ok()?.modified().ok()?;
        let bytes = size(&info) + size(&info.with_extension("png"));
        Some((modified, info, bytes))
      })
      .collect();
    let mut count = entries.len();
    let mut total: u64 = entries.iter().map(|(_, _, b)| b).sum();
    if count <= MAX_ENTRIES && total <= MAX_BYTES {
      return;
    }
    entries.sort();
    for (_, info, bytes) in entries {
      if count <= MAX_ENTRIES * 4 / 5 && total <= MAX_BYTES / 5 * 4 {
        break;
      }
      let _ = fs::remove_file(info.with_extension("png"));
      let _ = fs::remove_file(&info);
      count -= 1;
      total = total.saturating_sub(bytes);
    }
  }
}

/// Details as `label<TAB>value` lines; tabs and newlines in values become spaces.
fn format_info(info: &[(String, String)]) -> String {
  let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
  info
    .iter()
    .map(|(k, v)| format!("{}\t{}\n", clean(k), clean(v)))
    .collect()
}

fn parse_info(text: &str) -> Vec<(String, String)> {
  text
    .lines()
    .filter_map(|l| l.split_once('\t'))
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Decodes an image file, within the decoder's default memory limits.
fn decode(path: &Path) -> Option<DynamicImage> {
  ImageReader::open(path)
    .ok()?
    .with_guessed_format()
    .ok()?
    .decode()
    .ok()
}

/// A path for a program to render into: in the cache directory (on the same disk as the
/// cache) or the system temporary directory. Unique per process and call.
fn scratch_path(cache: Option<&ThumbnailCache>) -> PathBuf {
  static COUNTER: AtomicU64 = AtomicU64::new(0);
  let n = COUNTER.fetch_add(1, Ordering::Relaxed);
  let name = format!(".tansu-render-{}-{n}.png", std::process::id());
  match cache {
    Some(cache) => cache.dir.join(name),
    None => std::env::temp_dir().join(name),
  }
}

/// What a renderer produced for an entry.
pub enum Rendered {
  /// An image was written to the given path, with these details.
  Image(Vec<(String, String)>),
  /// No image (the program is missing or failed), but details are known.
  InfoOnly(Vec<(String, String)>),
  /// Nothing; let another previewer try.
  Nothing,
}

/// The preview for `entry` from the thumbnail cache, or rendered by `render` (which writes
/// an image to the path it is given) and then cached. `kind` names the renderer in the
/// cache key. Without an image, known details still make a metadata preview labelled
/// `label`.
pub fn cached_preview(
  entry: &Entry,
  kind: &str,
  label: &'static str,
  ctx: &PreviewContext,
  render: impl FnOnce(&Path) -> Result<Rendered, PreviewError>,
) -> Result<Option<Preview>, PreviewError> {
  let cache = ctx.thumbnails;
  let key = ThumbnailCache::key(entry, kind);
  if let Some((image, info)) = cache.and_then(|c| c.get(&key)) {
    return Ok(Some(image_preview(image, info, label)));
  }
  if let Some(cache) = cache {
    // Thumbnails show the user's files: private, as the freedesktop.org thumbnail
    // specification requires for its cache.
    use std::os::unix::fs::DirBuilderExt;
    let _ = fs::DirBuilder::new()
      .recursive(true)
      .mode(0o700)
      .create(&cache.dir);
  }
  let out = scratch_path(cache);
  let rendered = render(&out);
  let result = match rendered {
    Ok(Rendered::Image(info)) => match decode(&out) {
      Some(image) => {
        let image = crate::graphics::scale_to_fit(&image, MAX_SIDE, MAX_SIDE);
        if let Some(cache) = cache {
          cache.put(&key, &image, &info);
        }
        Ok(Some(image_preview(image, info, label)))
      }
      None => Ok(info_only(entry, label, info)),
    },
    Ok(Rendered::InfoOnly(info)) => Ok(info_only(entry, label, info)),
    Ok(Rendered::Nothing) => Ok(None),
    Err(e) => Err(e),
  };
  // The raw rendering is scratch either way.
  let _ = fs::remove_file(&out);
  result
}

fn image_preview(image: DynamicImage, info: Vec<(String, String)>, label: &'static str) -> Preview {
  let (width, height) = (image.width(), image.height());
  Preview::Image(PreviewImage {
    image: Arc::new(image),
    width,
    height,
    info,
    label: Some(label),
    details_first: false,
  })
}

/// A metadata preview with the renderer's details appended, or nothing without details.
fn info_only(entry: &Entry, label: &str, info: Vec<(String, String)>) -> Option<Preview> {
  if info.is_empty() {
    return None;
  }
  let mut rows = super::metadata_lines(entry, Some(label));
  rows.extend(info);
  Some(Preview::Metadata(rows))
}
