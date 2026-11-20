//! Encoding decoded images for the terminal's graphics protocol (kitty, sixel, iTerm2, or
//! unicode half blocks as the fallback). Encoding runs on the preview worker, sized for the
//! preview column, so drawing a frame never does image work.

use std::io::Write;
use std::time::{Duration, Instant};

use fast_image_resize::{self as fir, ResizeAlg, ResizeOptions, Resizer};
use image::DynamicImage;
use image::imageops::FilterType;
use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::widgets::Widget;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

/// Encodes images with the protocol detected for this terminal.
#[derive(Clone)]
pub struct ImageEncoder {
  picker: Picker,
}

impl ImageEncoder {
  /// Queries the terminal for its graphics protocol and font size. Must run after raw mode
  /// and the alternate screen are entered, and before anything else reads terminal input.
  /// Falls back to half blocks when the terminal does not answer.
  pub fn detect() -> Self {
    // ratatui-image's query reads stdin until the terminal answers a status report; on a
    // terminal that never answers, its reader thread outlives the timeout and swallows
    // keystrokes. So only query terminals known to answer.
    // Both talk to stdio, which may be redirected; the terminal is lent to them.
    let picker = crate::terminal::with_tty_stdio(|| {
      if answers_status_report(QUERY_TIMEOUT) {
        Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
      } else {
        Picker::halfblocks()
      }
    });
    ImageEncoder { picker }
  }

  /// Unicode half blocks only; needs no terminal.
  pub fn halfblocks() -> Self {
    ImageEncoder {
      picker: Picker::halfblocks(),
    }
  }

  /// Scales `image` down to fit `cells` (columns, rows) and encodes it. Also returns the
  /// scaled image, which is all that needs to be kept once encoded.
  pub fn encode(
    &self,
    image: &DynamicImage,
    cells: (u16, u16),
  ) -> Option<(EncodedImage, DynamicImage)> {
    let font = self.picker.font_size();
    let max_w = u32::from(cells.0) * u32::from(font.width);
    let max_h = u32::from(cells.1) * u32::from(font.height);
    let scaled = scale_to_fit(image, max_w, max_h);
    // The image already fits: `Crop` only pads it to whole cells, where `Fit` would resample
    // it again (and stretch it to the cell grid).
    let protocol = self
      .picker
      .new_protocol(
        scaled.clone(),
        Size::new(cells.0, cells.1),
        Resize::Crop(None),
      )
      .ok()?;
    Some((EncodedImage { protocol, cells }, scaled))
  }
}

/// Downscales `image` to fit `max_w` x `max_h` pixels, keeping its aspect ratio; smaller
/// images are left alone, as ranger does. A single Lanczos pass from the source, like
/// ranger's kitty displayer, done with SIMD so large photos stay fast.
pub fn scale_to_fit(image: &DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
  let (w, h) = (image.width(), image.height());
  if max_w == 0 || max_h == 0 || (w <= max_w && h <= max_h) {
    return image.clone();
  }
  let ratio = f64::min(max_w as f64 / w as f64, max_h as f64 / h as f64);
  let nw = ((w as f64 * ratio).round() as u32).clamp(1, max_w);
  let nh = ((h as f64 * ratio).round() as u32).clamp(1, max_h);
  let mut scaled = DynamicImage::new(nw, nh, image.color());
  let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(fir::FilterType::Lanczos3));
  match Resizer::new().resize(image, &mut scaled, &options) {
    Ok(()) => scaled,
    // Pixel formats the SIMD resizer does not support (e.g. 32-bit float).
    Err(_) => image.resize(max_w, max_h, FilterType::Lanczos3),
  }
}

const QUERY_TIMEOUT: Duration = Duration::from_secs(1);

/// Sends a device status report request (`ESC [ 5 n`) and waits up to `timeout` for the
/// answer (`ESC [ <digits> n`), consuming it. Reads the descriptor directly so no bytes end
/// up in std's stdin buffer, out of reach of the input thread.
fn answers_status_report(timeout: Duration) -> bool {
  let mut out = std::io::stdout();
  if out
    .write_all(b"\x1b[5n")
    .and_then(|()| out.flush())
    .is_err()
  {
    return false;
  }
  let deadline = Instant::now() + timeout;
  let mut seen = Vec::new();
  loop {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
      return false;
    }
    let mut pfd = libc::pollfd {
      fd: libc::STDIN_FILENO,
      events: libc::POLLIN,
      revents: 0,
    };
    let millis = left.as_millis().clamp(1, i32::MAX as u128) as i32;
    // SAFETY: `pfd` is a valid pollfd and the count matches.
    let ready = unsafe { libc::poll(&mut pfd, 1, millis) };
    if ready < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
      continue;
    }
    if ready <= 0 {
      return false;
    }
    let mut buf = [0u8; 64];
    // SAFETY: the pointer and length describe `buf`.
    let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
    if n <= 0 {
      return false;
    }
    seen.extend_from_slice(&buf[..n as usize]);
    if contains_status_answer(&seen) {
      return true;
    }
  }
}

fn contains_status_answer(bytes: &[u8]) -> bool {
  bytes.windows(2).enumerate().any(|(i, w)| {
    w == b"\x1b[" && {
      let rest = &bytes[i + 2..];
      let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
      digits > 0 && rest.get(digits) == Some(&b'n')
    }
  })
}

/// An image ready to draw, encoded for an area of `cells`.
#[derive(Clone)]
pub struct EncodedImage {
  protocol: Protocol,
  pub cells: (u16, u16),
}

impl EncodedImage {
  pub fn render(&self, area: Rect, buf: &mut Buffer) {
    Image::new(&self.protocol).render(area, buf);
  }

  /// Rows the image covers, which can be fewer than the rows it was encoded for (a wide
  /// image in a tall column).
  pub fn rows(&self) -> u16 {
    self.protocol.size().height
  }
}

impl std::fmt::Debug for EncodedImage {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "EncodedImage({}x{} cells)", self.cells.0, self.cells.1)
  }
}
