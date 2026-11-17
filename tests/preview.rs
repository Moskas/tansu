//! Previewers that go beyond plain data: syntax highlighting through an external command and
//! image decoding and encoding.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tansu::fs::Entry;
use tansu::graphics::ImageEncoder;
use tansu::preview::{
  HighlightConfig, Preview, PreviewContext, PreviewLimits, PreviewRegistry, TermColor,
};
use tansu::runtime::run_preview;
use tansu::worker::CancelToken;

fn entry(path: &Path) -> Entry {
  let dir = path.parent().unwrap();
  let listing = tansu::fs::list_dir(dir, 1000, &|| false).unwrap();
  listing
    .entries
    .into_iter()
    .find(|e| e.path == path)
    .expect("entry listed")
}

fn preview(registry: &PreviewRegistry, path: &Path, limits: PreviewLimits) -> Preview {
  let ctx = PreviewContext {
    limits,
    cancelled: &|| false,
    thumbnails: None,
  };
  registry.preview(&entry(path), &ctx).unwrap()
}

fn highlight(command: &str) -> HighlightConfig {
  HighlightConfig {
    enabled: true,
    command: command.to_string(),
    theme: String::new(),
  }
}

/// A stand-in for bat: colors everything it reads red.
fn fake_bat(dir: &Path) -> String {
  let path = dir.join("fake-bat");
  fs::write(&path, "#!/bin/sh\nprintf '\\033[31m'\nexec cat\n").unwrap();
  fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
  path.to_string_lossy().into_owned()
}

#[test]
fn text_is_highlighted_by_the_command_and_capped() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("main.rs");
  fs::write(&file, "fn main() {}\n\tlet x;\nthird\n").unwrap();
  let registry = PreviewRegistry::configured(&highlight(&fake_bat(tmp.path())), false);
  let limits = PreviewLimits {
    max_lines: 2,
    ..PreviewLimits::default()
  };
  match preview(&registry, &file, limits) {
    Preview::Highlighted { lines, truncated } => {
      assert!(truncated);
      assert_eq!(lines.len(), 2);
      assert_eq!(lines[0][0].text, "fn main() {}");
      assert_eq!(lines[0][0].style.fg, TermColor::Indexed(1));
      assert_eq!(lines[1][0].text, "    let x;", "tabs expanded");
    }
    other => panic!("expected highlighted text, got {other:?}"),
  }
}

#[test]
fn missing_highlighter_falls_back_to_plain_text() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("notes.txt");
  fs::write(&file, "hello\n").unwrap();
  let registry = PreviewRegistry::configured(&highlight("tansu-no-such-command"), false);
  for _ in 0..2 {
    assert_eq!(
      preview(&registry, &file, PreviewLimits::default()),
      Preview::Text {
        lines: vec!["hello".to_string()],
        truncated: false,
      }
    );
  }
}

#[test]
fn images_are_decoded_and_encoded_for_the_preview_size() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("dot.png");
  image::RgbImage::from_pixel(40, 20, image::Rgb([200, 30, 30]))
    .save(&file)
    .unwrap();
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  match preview(&registry, &file, PreviewLimits::default()) {
    Preview::Image(img) => assert_eq!((img.width, img.height), (40, 20)),
    other => panic!("expected an image, got {other:?}"),
  }

  let encoder = ImageEncoder::halfblocks();
  let ev = run_preview(
    &registry,
    PreviewLimits::default(),
    Some(&encoder),
    7,
    &entry(&file),
    Some((30, 10)),
    &CancelToken::never(),
  );
  match ev {
    Some(tansu::event::Event::PreviewReady {
      id: 7,
      result: Ok(Preview::Image(_)),
      image: Some(encoded),
      ..
    }) => assert_eq!(encoded.cells, (30, 10)),
    other => panic!("expected an encoded image, got {other:?}"),
  }
}

#[test]
fn undecodable_images_fall_through_to_other_previewers() {
  let tmp = tempfile::tempdir().unwrap();
  let svg = tmp.path().join("logo.svg");
  fs::write(&svg, "<svg/>\n").unwrap();
  let broken = tmp.path().join("broken.png");
  fs::write(&broken, [0x89, b'P', b'N', b'G', 0, 1, 2, 3]).unwrap();
  let registry = PreviewRegistry::configured(&highlight("tansu-no-such-command"), true);
  assert!(matches!(
    preview(&registry, &svg, PreviewLimits::default()),
    Preview::Text { .. }
  ));
  assert!(matches!(
    preview(&registry, &broken, PreviewLimits::default()),
    Preview::Metadata(_)
  ));
}

#[test]
fn images_are_scaled_once_to_fit_and_never_up() {
  use tansu::graphics::scale_to_fit;
  let wide = image::DynamicImage::new_rgb8(4000, 2000);
  let scaled = scale_to_fit(&wide, 600, 600);
  assert_eq!((scaled.width(), scaled.height()), (600, 300));
  // Large enough to take the box-filter shortcut first.
  let huge = image::DynamicImage::new_rgb8(1000, 500);
  let scaled = scale_to_fit(&huge, 40, 40);
  assert_eq!((scaled.width(), scaled.height()), (40, 20));
  let small = image::DynamicImage::new_rgb8(30, 20);
  let kept = scale_to_fit(&small, 600, 600);
  assert_eq!((kept.width(), kept.height()), (30, 20));
}

#[test]
fn encoded_previews_keep_only_the_scaled_image() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("big.png");
  image::RgbImage::new(2000, 1000).save(&file).unwrap();
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let ev = run_preview(
    &registry,
    PreviewLimits::default(),
    Some(&ImageEncoder::halfblocks()),
    1,
    &entry(&file),
    Some((30, 10)),
    &CancelToken::never(),
  );
  match ev {
    Some(tansu::event::Event::PreviewReady {
      result: Ok(Preview::Image(img)),
      image: Some(_),
      ..
    }) => {
      assert_eq!(
        (img.width, img.height),
        (2000, 1000),
        "original size reported"
      );
      // Half blocks assume 10x20 pixel cells: 30x10 cells are 300x200 pixels.
      assert_eq!((img.image.width(), img.image.height()), (300, 150));
    }
    other => panic!("expected an encoded image, got {other:?}"),
  }
}

/// A 40x20 JPEG whose EXIF says it is turned (orientation 6: rotate 90° clockwise to view),
/// with photographic details.
fn photo(path: &Path) {
  use exif::{Field, In, Rational, Tag, Value};
  let ascii = |s: &str| Value::Ascii(vec![s.as_bytes().to_vec()]);
  let rational = |n: u32, d: u32| Value::Rational(vec![Rational { num: n, denom: d }]);
  let field = |tag, value| Field {
    tag,
    ifd_num: In::PRIMARY,
    value,
  };
  let fields = [
    field(Tag::Make, ascii("Canon")),
    field(Tag::Model, ascii("Canon EOS R6")),
    field(Tag::Orientation, Value::Short(vec![6])),
    field(Tag::DateTimeOriginal, ascii("2024:05:01 12:34:56")),
    field(Tag::ExposureTime, rational(1, 250)),
    field(Tag::FNumber, rational(28, 10)),
    field(Tag::PhotographicSensitivity, Value::Short(vec![100])),
    field(Tag::FocalLength, rational(42, 10)),
    field(Tag::FocalLengthIn35mmFilm, Value::Short(vec![28])),
    field(Tag::GPSLatitudeRef, ascii("N")),
    field(
      Tag::GPSLatitude,
      Value::Rational(vec![
        Rational { num: 52, denom: 1 },
        Rational { num: 13, denom: 1 },
        Rational { num: 30, denom: 1 },
      ]),
    ),
    field(Tag::GPSLongitudeRef, ascii("E")),
    field(
      Tag::GPSLongitude,
      Value::Rational(vec![
        Rational { num: 21, denom: 1 },
        Rational { num: 0, denom: 1 },
        Rational { num: 36, denom: 1 },
      ]),
    ),
  ];
  let mut writer = exif::experimental::Writer::new();
  for f in &fields {
    writer.push_field(f);
  }
  let mut tiff = std::io::Cursor::new(Vec::new());
  writer.write(&mut tiff, false).unwrap();

  use image::ImageEncoder as _;
  let mut jpeg = Vec::new();
  let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut jpeg);
  encoder.set_exif_metadata(tiff.into_inner()).unwrap();
  let pixels = image::RgbImage::from_pixel(40, 20, image::Rgb([30, 200, 30]));
  encoder
    .write_image(&pixels, 40, 20, image::ExtendedColorType::Rgb8)
    .unwrap();
  fs::write(path, jpeg).unwrap();
}

#[test]
fn photos_are_turned_upright_and_show_their_exif_details() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("photo.jpg");
  photo(&file);
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let Preview::Image(img) = preview(&registry, &file, PreviewLimits::default()) else {
    panic!("expected an image");
  };
  assert_eq!((img.width, img.height), (20, 40), "rotated a quarter turn");
  assert_eq!((img.image.width(), img.image.height()), (20, 40));
  let info: Vec<(&str, &str)> = img
    .info
    .iter()
    .map(|(k, v)| (k.as_str(), v.as_str()))
    .collect();
  assert_eq!(
    info,
    vec![
      ("Camera", "Canon EOS R6"),
      ("Taken", "2024-05-01 12:34:56"),
      ("Exposure", "1/250 s"),
      ("Aperture", "f/2.8"),
      ("ISO", "100"),
      ("Focal length", "4.2 mm (28 mm full-frame)"),
      ("Location", "52.22500° N, 21.01000° E"),
    ]
  );
}

#[test]
fn exif_details_take_room_below_the_image_only_in_a_tall_column() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("photo.jpg");
  photo(&file);
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let encoder = ImageEncoder::halfblocks();
  let encoded_cells = |cells| match run_preview(
    &registry,
    PreviewLimits::default(),
    Some(&encoder),
    1,
    &entry(&file),
    Some(cells),
    &CancelToken::never(),
  ) {
    Some(tansu::event::Event::PreviewReady {
      image: Some(encoded),
      ..
    }) => encoded.cells,
    other => panic!("expected an encoded image, got {other:?}"),
  };
  // 7 detail rows and a blank row fit under the image while it keeps two thirds.
  assert_eq!(encoded_cells((30, 40)), (30, 32));
  // Too short: the image gets the whole column and the details stay in the `i` view.
  assert_eq!(encoded_cells((30, 20)), (30, 20));
}

#[test]
fn the_metadata_view_lists_exif_details_after_the_dimensions() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("photo.jpg");
  photo(&file);
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let content = preview(&registry, &file, PreviewLimits::default());
  let status = tansu::app::PreviewStatus::Ready(std::sync::Arc::new(content));
  let rows = tansu::ui::columns::metadata_rows(&entry(&file), &status);
  let labels: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
  let dims = labels.iter().position(|l| *l == "Dimensions").unwrap();
  assert_eq!(rows[dims].1, "20x40");
  assert!(labels.ends_with(&[
    "Camera",
    "Taken",
    "Exposure",
    "Aperture",
    "ISO",
    "Focal length",
    "Location"
  ]));
}

#[test]
fn an_encoded_image_reports_the_rows_it_covers() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("photo.jpg");
  photo(&file);
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let encoder = ImageEncoder::halfblocks();
  let ev = run_preview(
    &registry,
    PreviewLimits::default(),
    Some(&encoder),
    1,
    &entry(&file),
    Some((30, 40)),
    &CancelToken::never(),
  );
  let Some(tansu::event::Event::PreviewReady {
    image: Some(encoded),
    ..
  }) = ev
  else {
    panic!("expected an encoded image");
  };
  // 32 rows are reserved, but the small upright photo (20x40 px, never enlarged) covers
  // only a few of them; the details are drawn right after those, not at the bottom.
  assert_eq!(encoded.cells, (30, 32));
  assert!(
    (1..32).contains(&encoded.rows()),
    "covers {} rows",
    encoded.rows()
  );
}

#[test]
fn exposure_times_read_like_a_camera_shows_them() {
  use exif::{Field, In, Rational, Tag, Value};
  let exposure = |num: u32, denom: u32| {
    let field = Field {
      tag: Tag::ExposureTime,
      ifd_num: In::PRIMARY,
      value: Value::Rational(vec![Rational { num, denom }]),
    };
    let mut writer = exif::experimental::Writer::new();
    writer.push_field(&field);
    let mut tiff = std::io::Cursor::new(Vec::new());
    writer.write(&mut tiff, false).unwrap();
    tansu::preview::exif::summary(tiff.into_inner())
      .into_iter()
      .find(|(k, _)| k == "Exposure")
      .map(|(_, v)| v)
      .unwrap()
  };
  // What a Pixel 6 records: 1/177.999... s.
  assert_eq!(exposure(1_000_000, 177_999_288), "1/178 s");
  assert_eq!(exposure(1, 250), "1/250 s");
  assert_eq!(exposure(5, 2), "2.5 s");
  assert_eq!(exposure(30, 1), "30 s");
}

/// Whether `program` is installed. Tests needing an external tool skip without it, so
/// `cargo test` works outside the Nix dev shell too.
fn installed(program: &str) -> bool {
  std::process::Command::new(program)
    .arg("-version")
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status()
    .is_ok()
}

/// A 2 s, 64x48, 10 fps test clip with a mono 44.1 kHz tone.
fn clip(path: &Path) -> bool {
  if !installed("ffmpeg") || !installed("ffprobe") {
    eprintln!("skipped: ffmpeg/ffprobe not installed");
    return false;
  }
  let status = std::process::Command::new("ffmpeg")
    .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i"])
    .arg("testsrc=duration=2:size=64x48:rate=10")
    .args(["-f", "lavfi", "-i", "sine=duration=2:sample_rate=44100"])
    .args(["-shortest", "-c:v", "mpeg4", "-c:a", "aac", "-y"])
    .arg(path)
    .status()
    .unwrap();
  assert!(status.success());
  true
}

fn info_of(preview: &Preview) -> Vec<(String, String)> {
  match preview {
    Preview::Image(img) => img.info.clone(),
    Preview::Metadata(rows) => rows.clone(),
    other => panic!("unexpected {other:?}"),
  }
}

fn row<'a>(info: &'a [(String, String)], label: &str) -> Option<&'a str> {
  info
    .iter()
    .find(|(k, _)| k == label)
    .map(|(_, v)| v.as_str())
}

#[test]
fn ffprobe_output_becomes_video_details() {
  let probe = tansu::preview::video::Probe::parse(
    "streams.stream.0.codec_name=\"h264\"\n\
     streams.stream.0.codec_type=\"video\"\n\
     streams.stream.0.width=1920\n\
     streams.stream.0.height=1080\n\
     streams.stream.0.avg_frame_rate=\"24000/1001\"\n\
     streams.stream.1.codec_name=\"opus\"\n\
     streams.stream.1.codec_type=\"audio\"\n\
     streams.stream.1.sample_rate=\"48000\"\n\
     streams.stream.1.channels=6\n\
     format.duration=\"3725.400000\"\n\
     format.bit_rate=\"4512000\"\n",
  );
  let rows = probe.rows();
  let rows: Vec<(&str, &str)> = rows.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
  assert_eq!(
    rows,
    vec![
      ("Duration", "1:02:05"),
      ("Resolution", "1920x1080"),
      ("Video", "h264, 23.98 fps"),
      ("Audio", "opus, 6 channels, 48 kHz"),
      ("Bitrate", "4.51 Mb/s"),
    ]
  );
}

#[test]
fn videos_show_a_frame_with_their_details() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("clip.mp4");
  if !clip(&file) {
    return;
  }
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let preview = preview(&registry, &file, PreviewLimits::default());
  let Preview::Image(img) = &preview else {
    panic!("expected a frame, got {preview:?}");
  };
  assert_eq!((img.width, img.height), (64, 48));
  let info = info_of(&preview);
  assert_eq!(row(&info, "Duration"), Some("0:02"));
  assert_eq!(row(&info, "Resolution"), Some("64x48"));
  assert_eq!(row(&info, "Video"), Some("mpeg4, 10 fps"));
  assert_eq!(row(&info, "Audio"), Some("aac, mono, 44.1 kHz"));
}

#[test]
fn without_graphics_videos_show_only_their_details() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("clip.mp4");
  if !clip(&file) {
    return;
  }
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), false);
  let preview = preview(&registry, &file, PreviewLimits::default());
  let Preview::Metadata(rows) = &preview else {
    panic!("expected metadata, got {preview:?}");
  };
  assert_eq!(row(rows, "Type"), Some("video"));
  assert_eq!(row(rows, "Resolution"), Some("64x48"));
}

#[test]
fn rendered_frames_are_cached_on_disk_until_the_file_changes() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("clip.mp4");
  if !clip(&file) {
    return;
  }
  let cache = tmp.path().join("cache");
  let mut registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  registry.set_thumbnails(tansu::preview::ThumbnailCache::new(cache.clone()));
  let first = preview(&registry, &file, PreviewLimits::default());
  assert!(matches!(first, Preview::Image(_)));
  let mut names: Vec<String> = fs::read_dir(&cache)
    .unwrap()
    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
    .collect();
  names.sort();
  assert_eq!(
    names.len(),
    2,
    "a PNG and its details, no leftovers: {names:?}"
  );
  assert!(names[0].starts_with("video-") && names[0].ends_with(".png"));
  assert!(names[1].ends_with(".txt"));

  // Same size and mtime, unreadable content: only the cache can still produce a frame.
  let mtime = fs::metadata(&file).unwrap().modified().unwrap();
  let len = fs::metadata(&file).unwrap().len();
  fs::write(&file, vec![0u8; len as usize]).unwrap();
  fs::File::options()
    .write(true)
    .open(&file)
    .unwrap()
    .set_modified(mtime)
    .unwrap();
  let cached = preview(&registry, &file, PreviewLimits::default());
  assert!(matches!(cached, Preview::Image(_)), "served from the cache");
  assert_eq!(info_of(&cached), info_of(&first));

  // A new modification time is a new version: rendered again, which now fails.
  fs::File::options()
    .write(true)
    .open(&file)
    .unwrap()
    .set_modified(mtime + std::time::Duration::from_secs(60))
    .unwrap();
  let changed = preview(&registry, &file, PreviewLimits::default());
  assert!(!matches!(changed, Preview::Image(_)));
}

#[test]
fn renderings_are_scaled_down_and_cached_privately() {
  use std::os::unix::fs::PermissionsExt as _;
  use tansu::preview::thumbnail::{MAX_SIDE, Rendered, cached_preview};

  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("huge.bin");
  fs::write(&file, "x").unwrap();
  let cache_dir = tmp.path().join("cache/tansu/thumbnails");
  let cache = tansu::preview::ThumbnailCache::new(cache_dir.clone());
  let ctx = PreviewContext {
    limits: PreviewLimits::default(),
    cancelled: &|| false,
    thumbnails: Some(&cache),
  };
  let info = vec![("Pages".to_string(), "3".to_string())];
  let preview = cached_preview(&entry(&file), "test", "document", &ctx, |out| {
    image::RgbImage::from_pixel(2400, 1200, image::Rgb([1, 2, 3]))
      .save(out)
      .unwrap();
    Ok(Rendered::Image(info.clone()))
  })
  .unwrap()
  .unwrap();
  let Preview::Image(img) = preview else {
    panic!("expected an image");
  };
  assert_eq!((img.width, img.height), (MAX_SIDE, MAX_SIDE / 2));
  assert_eq!(img.info, info);

  let mode = fs::metadata(&cache_dir).unwrap().permissions().mode();
  assert_eq!(mode & 0o777, 0o700);
  let mut names: Vec<String> = fs::read_dir(&cache_dir)
    .unwrap()
    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
    .collect();
  names.sort();
  assert_eq!(names.len(), 2, "no scratch files left: {names:?}");
  let cached = image::open(cache_dir.join(&names[0])).unwrap();
  assert_eq!((cached.width(), cached.height()), (MAX_SIDE, MAX_SIDE / 2));

  // The second time comes from the cache; the renderer is not called.
  let again = cached_preview(&entry(&file), "test", "document", &ctx, |_| {
    panic!("rendered again")
  })
  .unwrap();
  assert!(matches!(again, Some(Preview::Image(_))));
}

/// A two-page A4 PDF with a title and author, its cross-reference table computed so poppler
/// reads it without repairing it.
fn pdf_document(path: &Path) {
  let content = "0 0 1 rg 50 50 200 300 re f";
  let objects = [
    "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
    "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.276 841.89] /Contents 5 0 R >>".to_string(),
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.276 841.89] >>".to_string(),
    format!(
      "<< /Length {} >>\nstream\n{content}\nendstream",
      content.len()
    ),
    "<< /Title (Test Book) /Author (Jane Doe) /CreationDate (D:20240501123456Z) >>".to_string(),
  ];
  let mut pdf = String::from("%PDF-1.4\n");
  let mut offsets = Vec::new();
  for (i, body) in objects.iter().enumerate() {
    offsets.push(pdf.len());
    pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
  }
  let xref = pdf.len();
  pdf.push_str(&format!(
    "xref\n0 {}\n0000000000 65535 f \n",
    objects.len() + 1
  ));
  for offset in offsets {
    pdf.push_str(&format!("{offset:010} 00000 n \n"));
  }
  pdf.push_str(&format!(
    "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n",
    objects.len() + 1
  ));
  fs::write(path, pdf).unwrap();
}

#[test]
fn pdfinfo_reports_become_document_details() {
  let rows = tansu::preview::pdf::rows(
    "Title:           Annual Report\n\
     Author:          \n\
     Producer:        LibreOffice\n\
     CreationDate:    Wed May  1 14:34:56 2024 CEST\n\
     Pages:           12\n\
     Encrypted:       yes (print:yes copy:no)\n\
     Page size:       612 x 792 pts (letter)\n",
  );
  let rows: Vec<(&str, &str)> = rows.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
  assert_eq!(
    rows,
    vec![
      ("Title", "Annual Report"),
      ("Pages", "12"),
      ("Page size", "letter (216 x 279 mm)"),
      ("Created", "Wed May  1 14:34:56 2024 CEST"),
      ("Encrypted", "yes"),
    ]
  );
}

#[test]
fn pdfs_show_their_first_page_with_details() {
  if !installed("pdftoppm") || !installed("pdfinfo") {
    eprintln!("skipped: poppler not installed");
    return;
  }
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("book.pdf");
  pdf_document(&file);
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let preview = preview(&registry, &file, PreviewLimits::default());
  let Preview::Image(img) = &preview else {
    panic!("expected the first page, got {preview:?}");
  };
  // A4 rendered to fit 2048 px.
  assert_eq!(img.height, tansu::preview::thumbnail::MAX_SIDE);
  assert!((1440..1460).contains(&img.width), "width {}", img.width);
  let info = info_of(&preview);
  assert_eq!(row(&info, "Title"), Some("Test Book"));
  assert_eq!(row(&info, "Author"), Some("Jane Doe"));
  assert_eq!(row(&info, "Pages"), Some("2"));
  assert_eq!(row(&info, "Page size"), Some("A4 (210 x 297 mm)"));
  assert!(row(&info, "Created").is_some_and(|c| c.contains("2024")));

  // Without graphics: the details in a metadata block.
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), false);
  let Preview::Metadata(rows) = preview_of(&registry, &file) else {
    panic!("expected metadata");
  };
  assert_eq!(row(&rows, "Type"), Some("PDF document"));
  assert_eq!(row(&rows, "Pages"), Some("2"));
}

fn preview_of(registry: &PreviewRegistry, path: &Path) -> Preview {
  preview(registry, path, PreviewLimits::default())
}

#[test]
fn damaged_pdfs_fall_back_to_other_previewers() {
  if !installed("pdftoppm") || !installed("pdfinfo") {
    eprintln!("skipped: poppler not installed");
    return;
  }
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("broken.pdf");
  fs::write(&file, "%PDF-1.4\nnot really\n").unwrap();
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  // Neither poppler tool can read it, so another previewer (text, here) takes over.
  let preview = preview_of(&registry, &file);
  assert!(!matches!(preview, Preview::Image(_)), "got {preview:?}");
}

/// A 6x9 PNG of one color, to tell covers apart.
fn png(color: [u8; 3]) -> Vec<u8> {
  let mut out = std::io::Cursor::new(Vec::new());
  image::RgbImage::from_pixel(6, 9, image::Rgb(color))
    .write_to(&mut out, image::ImageFormat::Png)
    .unwrap();
  out.into_inner()
}

const RED: [u8; 3] = [200, 0, 0];
const GREEN: [u8; 3] = [0, 200, 0];
const BLUE: [u8; 3] = [0, 0, 200];

/// A zip of `(name, content)` members, compressed like real EPUBs and comics.
fn zip_file(path: &Path, members: &[(&str, &[u8])]) {
  use std::io::Write as _;
  let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
  let options =
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
  for (name, content) in members {
    zip.start_file(*name, options).unwrap();
    zip.write_all(content).unwrap();
  }
  zip.finish().unwrap();
}

/// The color of a preview's image, to check which cover was picked.
fn cover_color(preview: &Preview) -> [u8; 3] {
  let Preview::Image(img) = preview else {
    panic!("expected a cover, got {preview:?}");
  };
  img.image.to_rgb8().get_pixel(0, 0).0
}

const CONTAINER: &str = r#"<?xml version="1.0"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

#[test]
fn epub3_covers_and_details_are_read_from_the_package() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("book.epub");
  let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Tom &amp; Jerry&#x2019;s Tale</dc:title>
    <dc:creator id="a1">Jane Doe</dc:creator>
    <dc:creator id="a2">John Roe</dc:creator>
    <dc:publisher>Small Press</dc:publisher>
    <dc:date>2020-03-04T00:00:00Z</dc:date>
  </metadata>
  <manifest>
    <item id="decoy" href="images/cover-small.png" media-type="image/png"/>
    <item id="img" href="images/front%20art.png" media-type="image/png" properties="cover-image"/>
  </manifest>
</package>"#;
  zip_file(
    &file,
    &[
      ("mimetype", b"application/epub+zip"),
      ("META-INF/container.xml", CONTAINER.as_bytes()),
      ("OEBPS/content.opf", opf.as_bytes()),
      ("OEBPS/images/cover-small.png", &png(RED)),
      ("OEBPS/images/front art.png", &png(GREEN)),
    ],
  );
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let preview = preview_of(&registry, &file);
  assert_eq!(
    cover_color(&preview),
    GREEN,
    "the cover-image property wins"
  );
  let info = info_of(&preview);
  assert_eq!(row(&info, "Title"), Some("Tom & Jerry\u{2019}s Tale"));
  assert_eq!(row(&info, "Author"), Some("Jane Doe, John Roe"));
  assert_eq!(row(&info, "Publisher"), Some("Small Press"));
  assert_eq!(row(&info, "Published"), Some("2020-03-04"));
}

#[test]
fn epub2_covers_are_found_through_the_cover_meta() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("old.epub");
  let opf = r#"<package version="2.0"><metadata>
    <dc:title>Old Book</dc:title><meta name="cover" content="front"/></metadata>
    <manifest>
      <item id="front" href="../art/front.png" media-type="image/png"/>
      <item id="text" href="text.xhtml" media-type="application/xhtml+xml"/>
    </manifest></package>"#;
  zip_file(
    &file,
    &[
      ("META-INF/container.xml", CONTAINER.as_bytes()),
      ("OEBPS/content.opf", opf.as_bytes()),
      ("art/front.png", &png(BLUE)),
    ],
  );
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let preview = preview_of(&registry, &file);
  assert_eq!(cover_color(&preview), BLUE);
  assert_eq!(row(&info_of(&preview), "Title"), Some("Old Book"));

  // Without graphics: the details in a metadata block.
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), false);
  let Preview::Metadata(rows) = preview_of(&registry, &file) else {
    panic!("expected metadata");
  };
  assert_eq!(row(&rows, "Type"), Some("EPUB ebook"));
  assert_eq!(row(&rows, "Title"), Some("Old Book"));
}

#[test]
fn cbz_covers_are_the_first_page_in_natural_order() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("comic.cbz");
  zip_file(
    &file,
    &[
      ("__MACOSX/._1.png", b"junk"),
      ("pages/10.png", &png(RED)),
      ("pages/2.png", &png(RED)),
      ("pages/1.png", &png(GREEN)),
      ("ComicInfo.xml", b"<ComicInfo/>"),
    ],
  );
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let preview = preview_of(&registry, &file);
  assert_eq!(cover_color(&preview), GREEN);
  assert_eq!(row(&info_of(&preview), "Pages"), Some("3"));
}

/// A MOBI file: PalmDB header and record table, record 0 with the PalmDOC and MOBI headers,
/// EXTH metadata and the full name, a text record, and two image records. The EXTH cover
/// offset (1) picks the second image.
fn mobi_file(path: &Path) {
  let mut exth_records = Vec::new();
  let mut exth = |kind: u32, data: &[u8]| {
    exth_records.extend_from_slice(&kind.to_be_bytes());
    exth_records.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
    exth_records.extend_from_slice(data);
  };
  exth(100, b"Jane Doe");
  exth(100, b"John Roe");
  exth(101, b"Kindle Press");
  exth(106, b"2019-01-02T00:00:00+00:00");
  exth(503, b"Updated Title");
  exth(201, &1u32.to_be_bytes());
  let mut exth_block = b"EXTH".to_vec();
  exth_block.extend_from_slice(&(exth_records.len() as u32 + 12).to_be_bytes());
  exth_block.extend_from_slice(&6u32.to_be_bytes());
  exth_block.extend_from_slice(&exth_records);

  let mobi_len: usize = 232;
  let name = b"Original Name";
  let name_at = 16 + mobi_len + exth_block.len();
  let mut rec0 = vec![0u8; 16 + mobi_len];
  rec0[16..20].copy_from_slice(b"MOBI");
  rec0[20..24].copy_from_slice(&(mobi_len as u32).to_be_bytes());
  rec0[84..88].copy_from_slice(&(name_at as u32).to_be_bytes());
  rec0[88..92].copy_from_slice(&(name.len() as u32).to_be_bytes());
  rec0[108..112].copy_from_slice(&2u32.to_be_bytes()); // first image record
  rec0[128..132].copy_from_slice(&0x40u32.to_be_bytes()); // EXTH present
  rec0.extend_from_slice(&exth_block);
  rec0.extend_from_slice(name);

  let records = [rec0, b"text".to_vec(), png(RED), png(GREEN)];
  let mut pdb = vec![0u8; 78];
  pdb[..8].copy_from_slice(b"testbook");
  pdb[60..68].copy_from_slice(b"BOOKMOBI");
  pdb[76..78].copy_from_slice(&(records.len() as u16).to_be_bytes());
  let mut offset = 78 + records.len() * 8 + 2;
  for (i, r) in records.iter().enumerate() {
    pdb.extend_from_slice(&(offset as u32).to_be_bytes());
    pdb.extend_from_slice(&(i as u32).to_be_bytes());
    offset += r.len();
  }
  pdb.extend_from_slice(&[0, 0]);
  for r in &records {
    pdb.extend_from_slice(r);
  }
  fs::write(path, pdb).unwrap();
}

#[test]
fn mobi_covers_and_details_come_from_the_exth_records() {
  let tmp = tempfile::tempdir().unwrap();
  for name in ["book.mobi", "book.azw3"] {
    let file = tmp.path().join(name);
    mobi_file(&file);
    let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
    let preview = preview_of(&registry, &file);
    assert_eq!(
      cover_color(&preview),
      GREEN,
      "{name}: the EXTH cover offset"
    );
    let info = info_of(&preview);
    assert_eq!(row(&info, "Title"), Some("Updated Title"));
    assert_eq!(row(&info, "Author"), Some("Jane Doe, John Roe"));
    assert_eq!(row(&info, "Publisher"), Some("Kindle Press"));
    assert_eq!(row(&info, "Published"), Some("2019-01-02"));
  }
}

#[test]
fn damaged_ebooks_fall_back_without_panicking() {
  let tmp = tempfile::tempdir().unwrap();
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  for (name, content) in [
    ("empty.epub", &b""[..]),
    ("junk.epub", b"PK\x03\x04 not a zip"),
    ("short.mobi", b"BOOKMOBI"),
    ("junk.azw3", &[0xffu8; 200][..]),
    ("empty.cbz", b""),
  ] {
    let file = tmp.path().join(name);
    fs::write(&file, content).unwrap();
    let preview = preview_of(&registry, &file);
    assert!(!matches!(preview, Preview::Image(_)), "{name}: {preview:?}");
  }
  // A MOBI whose header says the cover is in a record that does not exist.
  let file = tmp.path().join("truncated.mobi");
  mobi_file(&file);
  let mut data = fs::read(&file).unwrap();
  data[77] = 3; // claims 3 records; the cover offset points at the 4th
  fs::write(&file, data).unwrap();
  let preview = preview_of(&registry, &file);
  assert!(!matches!(preview, Preview::Image(_)));
}

#[test]
fn the_metadata_view_names_what_a_rendering_shows() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("book.azw3");
  mobi_file(&file);
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let content = preview_of(&registry, &file);
  assert!(matches!(content, Preview::Image(_)));
  let status = tansu::app::PreviewStatus::Ready(std::sync::Arc::new(content));
  let rows = tansu::ui::columns::metadata_rows(&entry(&file), &status);
  assert_eq!(row(&rows, "Type"), Some("Kindle ebook"));
  assert_eq!(
    row(&rows, "Dimensions"),
    None,
    "the cover's size says nothing about the book"
  );
  assert_eq!(row(&rows, "Title"), Some("Updated Title"));
}

/// A 2 s tone tagged like a music file, with a 64x48 cover (FLAC, MP3) or without (Opus).
fn song(path: &Path, cover: bool) -> bool {
  if !installed("ffmpeg") || !installed("ffprobe") {
    eprintln!("skipped: ffmpeg/ffprobe not installed");
    return false;
  }
  let dir = path.parent().unwrap();
  let art = dir.join("art.png");
  image::RgbImage::from_pixel(64, 48, image::Rgb(GREEN))
    .save(&art)
    .unwrap();
  let mut cmd = std::process::Command::new("ffmpeg");
  cmd.args(["-v", "error", "-nostdin", "-f", "lavfi", "-i"]);
  cmd.arg("sine=duration=2:sample_rate=48000");
  if cover {
    cmd.arg("-i").arg(&art);
    cmd.args([
      "-map",
      "0:a",
      "-map",
      "1:v",
      "-disposition:v",
      "attached_pic",
    ]);
  }
  for tag in [
    "title=Song",
    "artist=Band",
    "album=Record",
    "date=2021",
    "genre=Rock",
    "track=3/12",
  ] {
    cmd.args(["-metadata", tag]);
  }
  let status = cmd.arg("-y").arg(path).status().unwrap();
  assert!(status.success(), "ffmpeg failed for {}", path.display());
  true
}

#[test]
fn songs_show_their_tags_and_a_small_cover() {
  let tmp = tempfile::tempdir().unwrap();
  for name in ["song.flac", "song.mp3"] {
    let file = tmp.path().join(name);
    if !song(&file, true) {
      return;
    }
    let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
    let preview = preview_of(&registry, &file);
    let Preview::Image(img) = &preview else {
      panic!("{name}: expected the cover, got {preview:?}");
    };
    assert!(
      img.details_first,
      "{name}: details stay visible in short columns"
    );
    assert_eq!(
      (img.width, img.height),
      (64, 48),
      "{name}: small covers stay small"
    );
    assert!(cover_color(&preview)[1] > 150, "{name}: the green cover");
    let info = info_of(&preview);
    assert_eq!(row(&info, "Title"), Some("Song"), "{name}");
    assert_eq!(row(&info, "Artist"), Some("Band"), "{name}");
    assert_eq!(row(&info, "Album"), Some("Record"), "{name}");
    assert_eq!(row(&info, "Track"), Some("3 of 12"), "{name}");
    assert_eq!(row(&info, "Date"), Some("2021"), "{name}");
    assert_eq!(row(&info, "Genre"), Some("Rock"), "{name}");
    assert_eq!(row(&info, "Duration"), Some("0:02"), "{name}");
    assert!(
      row(&info, "Audio").is_some_and(|a| a.contains("mono, 48 kHz")),
      "{name}"
    );
  }
}

#[test]
fn songs_without_a_cover_show_their_tags_in_a_metadata_block() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("song.opus");
  if !song(&file, false) {
    return;
  }
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let Preview::Metadata(rows) = preview_of(&registry, &file) else {
    panic!("expected metadata");
  };
  assert_eq!(row(&rows, "Type"), Some("audio"));
  // Opus keeps its tags on the stream, not the container.
  assert_eq!(row(&rows, "Title"), Some("Song"));
  assert_eq!(row(&rows, "Artist"), Some("Band"));
  assert!(row(&rows, "Audio").is_some_and(|a| a.starts_with("opus")));
}

#[test]
fn large_covers_are_extracted_small() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("big.flac");
  if !installed("ffmpeg") || !installed("ffprobe") {
    return;
  }
  let art = tmp.path().join("big.png");
  image::RgbImage::from_pixel(1200, 900, image::Rgb(BLUE))
    .save(&art)
    .unwrap();
  let status = std::process::Command::new("ffmpeg")
    .args([
      "-v",
      "error",
      "-nostdin",
      "-f",
      "lavfi",
      "-i",
      "sine=duration=1",
      "-i",
    ])
    .arg(&art)
    .args([
      "-map",
      "0:a",
      "-map",
      "1:v",
      "-disposition:v",
      "attached_pic",
      "-y",
    ])
    .arg(&file)
    .status()
    .unwrap();
  assert!(status.success());
  let registry = PreviewRegistry::configured(&HighlightConfig::default(), true);
  let Preview::Image(img) = preview_of(&registry, &file) else {
    panic!("expected the cover");
  };
  let side = tansu::preview::audio::COVER_SIDE;
  assert_eq!((img.width, img.height), (side, side * 3 / 4));
}

#[test]
fn ffprobe_tags_become_music_details() {
  let probe = tansu::preview::video::Probe::parse(
    "streams.stream.0.codec_name=\"flac\"\n\
     streams.stream.0.codec_type=\"audio\"\n\
     streams.stream.0.sample_rate=\"44100\"\n\
     streams.stream.0.channels=2\n\
     streams.stream.0.bits_per_raw_sample=\"24\"\n\
     streams.stream.0.tags.ALBUM_ARTIST=\"Stream Artist\"\n\
     format.duration=\"245.2\"\n\
     format.bit_rate=\"1411000\"\n\
     format.tags.TITLE=\"Loud Song\"\n\
     format.tags.ALBUM=\"Big Record\"\n\
     format.tags.TRACKNUMBER=\"7\"\n\
     format.tags.DATE=\"1999-05-01\"\n",
  );
  let rows = tansu::preview::audio::rows(&probe);
  let rows: Vec<(&str, &str)> = rows.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
  assert_eq!(
    rows,
    vec![
      ("Title", "Loud Song"),
      ("Artist", "Stream Artist"),
      ("Album", "Big Record"),
      ("Track", "7"),
      ("Date", "1999-05-01"),
      ("Duration", "4:05"),
      ("Audio", "flac, stereo, 44.1 kHz, 24-bit"),
      ("Bitrate", "1.41 Mb/s"),
    ]
  );
}

fn entries(
  listing: &tansu::preview::Listing,
) -> Vec<(String, String, Option<String>, Option<String>)> {
  listing
    .rows
    .iter()
    .filter_map(|r| match r {
      tansu::preview::Row::Entry {
        number,
        title,
        detail,
        time,
      } => Some((number.clone(), title.clone(), detail.clone(), time.clone())),
      tansu::preview::Row::Text(_) | tansu::preview::Row::Member { .. } => None,
    })
    .collect()
}

fn e(
  n: &str,
  title: &str,
  detail: Option<&str>,
  time: Option<&str>,
) -> (String, String, Option<String>, Option<String>) {
  (
    n.to_string(),
    title.to_string(),
    detail.map(str::to_string),
    time.map(str::to_string),
  )
}

#[test]
fn m3u_playlists_list_titles_folders_and_durations() {
  let listing = tansu::preview::playlist::parse_m3u(
    "#EXTM3U\r\n#PLAYLIST:Road trip\r\n\
     #EXTINF:201,P-Model - Heaven 2000\r\n01 - Heaven 2000.flac\r\n\
     (2010) - dancefloor degrader - (flac)/02 - goreshit - hardcore bisnis.flac\r\n\
     C:\\Music\\Album\\03%20Track.mp3\r\n\
     #EXTINF:-1,Radio\r\nhttps://example.com/stream/live.aac?token=1\r\n",
  );
  assert_eq!(listing.kind, "playlist");
  assert_eq!(
    entries(&listing),
    vec![
      e("1", "P-Model - Heaven 2000", None, Some("3:21")),
      e(
        "2",
        "02 - goreshit - hardcore bisnis",
        Some("(2010) - dancefloor degrader - (flac)"),
        None
      ),
      e("3", "03 Track", Some("Album"), None),
      e("4", "Radio", Some("stream"), None),
    ]
  );
  assert_eq!(
    listing.info,
    vec![
      ("Name".to_string(), "Road trip".to_string()),
      ("Entries".to_string(), "4".to_string()),
      (
        "Duration".to_string(),
        "3:21 (1 of 4 entries timed)".to_string()
      ),
    ]
  );
}

#[test]
fn hls_playlists_are_summarised() {
  let media = tansu::preview::playlist::parse_m3u(
    "#EXTM3U\n#EXT-X-TARGETDURATION:12\n#EXT-X-PLAYLIST-TYPE:VOD\n\
     #EXTINF:10.0,\na.ts\n#EXTINF:12.5,\nb.ts\n#EXT-X-ENDLIST\n",
  );
  let info: Vec<(&str, &str)> = media
    .info
    .iter()
    .map(|(k, v)| (k.as_str(), v.as_str()))
    .collect();
  assert_eq!(
    info,
    vec![
      ("Type", "HLS media playlist"),
      ("Segments", "2"),
      ("Duration", "0:23"),
      ("Segment length", "12 s at most"),
      ("Stream", "complete"),
    ]
  );
  assert!(media.rows.is_empty());

  let master = tansu::preview::playlist::parse_m3u(
    "#EXTM3U\n\
     #EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080,CODECS=\"avc1.640028,mp4a.40.2\",FRAME-RATE=30\n\
     hi.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=128000,CODECS=\"mp4a.40.2\"\naudio.m3u8\n",
  );
  assert_eq!(
    entries(&master),
    vec![
      e(
        "1",
        "1920x1080",
        Some("5 Mb/s, avc1.640028,mp4a.40.2"),
        Some("30 fps")
      ),
      e("2", "audio", Some("128 kb/s, mp4a.40.2"), None),
    ]
  );
}

#[test]
fn cue_sheets_list_tracks_with_lengths_from_one_file() {
  let listing = tansu::preview::playlist::parse_cue(
    "REM GENRE \"New Wave\"\r\nREM DATE 2002\r\nPERFORMER \"P-Model\"\r\n\
     TITLE \"Big Album\"\r\nFILE \"Big Album.flac\" WAVE\r\n\
       TRACK 01 AUDIO\r\n    TITLE \"Opening\"\r\n    PERFORMER \"P-Model\"\r\n    INDEX 01 00:00:00\r\n\
       TRACK 02 AUDIO\r\n    TITLE \"Guest Song\"\r\n    PERFORMER \"Guest\"\r\n    INDEX 00 03:20:00\r\n    INDEX 01 03:21:37\r\n\
       TRACK 03 AUDIO\r\n    INDEX 01 07:00:00\r\n",
  );
  assert_eq!(listing.kind, "cue sheet");
  let info: Vec<(&str, &str)> = listing
    .info
    .iter()
    .map(|(k, v)| (k.as_str(), v.as_str()))
    .collect();
  assert_eq!(
    info,
    vec![
      ("Album", "Big Album"),
      ("Performer", "P-Model"),
      ("Date", "2002"),
      ("Genre", "New Wave"),
      ("File", "Big Album.flac"),
      ("Tracks", "3"),
    ]
  );
  assert_eq!(
    entries(&listing),
    vec![
      e("01", "Opening", None, Some("3:21")),
      e("02", "Guest Song", Some("Guest"), Some("3:39")),
      e("03", "Track 03", None, None),
    ]
  );
}

const EAC_LOG: &str = "Exact Audio Copy V1.0 beta 3 from 29. August 2011\r\n\r\n\
EAC extraction logfile from 9. June 2012, 13:25\r\n\r\n\
CAVE / 怒首領蜂最大往生 Original Soundtrack\r\n\r\n\
Used drive  : ASUS    BC-08B1LT   Adapter: 2  ID: 2\r\n\r\n\
Read mode               : Secure\r\n\r\n\r\n\r\n\
Track  1\r\n\r\n     Filename C:\\01.wav\r\n     Suspicious position 0:02:20\r\n\
     Accurately ripped (confidence 5)  [ABCD1234]  (AR v2)\r\n     Copy OK\r\n\r\n\
Track  2\r\n\r\n     Copy OK\r\n\r\n\
All tracks accurately ripped\r\n\r\nThere were errors\r\n\r\nEnd of status report\r\n\r\n\
==== Log checksum FA67A2FD ====\r\n";

#[test]
fn eac_logs_are_summarised_above_the_log() {
  let listing = tansu::preview::riplog::parse(EAC_LOG).unwrap();
  assert_eq!(listing.kind, "rip log");
  let info: Vec<(&str, &str)> = listing
    .info
    .iter()
    .map(|(k, v)| (k.as_str(), v.as_str()))
    .collect();
  assert_eq!(
    info,
    vec![
      ("Ripper", "Exact Audio Copy V1.0 beta 3"),
      ("Ripped", "9. June 2012, 13:25"),
      ("Album", "CAVE / 怒首領蜂最大往生 Original Soundtrack"),
      ("Drive", "ASUS BC-08B1LT"),
      ("Read mode", "Secure"),
      ("Tracks", "2"),
      ("AccurateRip", "All tracks accurately ripped"),
      ("Errors", "1 suspicious position"),
      ("Checksum", "present"),
    ]
  );
  // The log follows, without its runs of blank lines.
  let text: Vec<&str> = listing
    .rows
    .iter()
    .map(|r| match r {
      tansu::preview::Row::Text(t) => t.as_str(),
      _ => panic!("logs are text"),
    })
    .collect();
  assert_eq!(text[0], "Exact Audio Copy V1.0 beta 3 from 29. August 2011");
  assert!(!text.windows(2).any(|w| w[0].is_empty() && w[1].is_empty()));
}

#[test]
fn xld_cuetools_and_whipper_logs_are_recognised_and_others_are_not() {
  let xld = tansu::preview::riplog::parse(
    "X Lossless Decoder version 20230627 (155.2)\n\n\
     XLD extraction logfile from 2023-07-01 12:00:00 +0900\n\nArtist / Album\n\n\
     Used drive : PIONEER BD-RW BDR-XD07\nRipper mode : XLD Secure Ripper\n\n\
     AccurateRip Summary\n    ->All tracks accurately ripped.\n\nTrack 01\n\nNo errors occurred\n\n\
     -----BEGIN XLD SIGNATURE-----\nabc\n-----END XLD SIGNATURE-----\n",
  )
  .unwrap();
  let get = |l: &tansu::preview::Listing, k: &str| {
    l.info
      .iter()
      .find(|(key, _)| key == k)
      .map(|(_, v)| v.clone())
  };
  assert_eq!(get(&xld, "Read mode").as_deref(), Some("XLD Secure Ripper"));
  assert_eq!(
    get(&xld, "AccurateRip").as_deref(),
    Some("All tracks accurately ripped.")
  );
  assert_eq!(get(&xld, "Errors").as_deref(), Some("none"));
  assert_eq!(get(&xld, "Checksum").as_deref(), Some("present"));

  let cuetools = tansu::preview::riplog::parse(
    "[CUETools log; Date: 1/2/2020 3:04:05 AM; Version: 2.1.6]\n[AccurateRip ID: 0012-ab] found.\n",
  )
  .unwrap();
  assert_eq!(get(&cuetools, "Ripper").as_deref(), Some("CUETools 2.1.6"));
  assert_eq!(
    get(&cuetools, "Date").as_deref(),
    Some("1/2/2020 3:04:05 AM")
  );

  let whipper = tansu::preview::riplog::parse(
    "Log created by: whipper 0.10.0 (internal logger)\nLog creation date: 2021-01-01T00:00:00Z\n\
     Health status: No errors occurred\n",
  )
  .unwrap();
  assert_eq!(
    get(&whipper, "Health").as_deref(),
    Some("No errors occurred")
  );

  assert!(tansu::preview::riplog::parse("2024-01-01 server started\n").is_none());
}

#[test]
fn other_logs_stay_plain_text_and_utf16_logs_are_decoded() {
  let tmp = tempfile::tempdir().unwrap();
  let registry = PreviewRegistry::with_defaults();
  let plain = tmp.path().join("server.log");
  fs::write(&plain, "started\nlistening\n").unwrap();
  assert!(matches!(
    preview_of(&registry, &plain),
    Preview::Text { .. }
  ));

  // A UTF-16 EAC log: NUL bytes everywhere, yet text.
  let rip = tmp.path().join("rip.log");
  let mut data = vec![0xff, 0xfe];
  data.extend(EAC_LOG.encode_utf16().flat_map(u16::to_le_bytes));
  fs::write(&rip, data).unwrap();
  let Preview::Listing(listing) = preview_of(&registry, &rip) else {
    panic!("expected a rip log listing");
  };
  assert_eq!(
    listing.info.last(),
    Some(&("Encoding".to_string(), "UTF-16LE".to_string()))
  );

  // A UTF-16 text file that is not a rip log is still text, not binary.
  let notes = tmp.path().join("notes.txt");
  let mut data = vec![0xff, 0xfe];
  data.extend("hello\nworld\n".encode_utf16().flat_map(u16::to_le_bytes));
  fs::write(&notes, data).unwrap();
  match preview_of(&registry, &notes) {
    Preview::Text { lines, .. } => assert_eq!(lines, vec!["hello", "world"]),
    other => panic!("expected text, got {other:?}"),
  }
}

#[test]
fn listings_are_cleaned_of_control_characters() {
  let tmp = tempfile::tempdir().unwrap();
  let file = tmp.path().join("evil.m3u");
  fs::write(&file, "#EXTINF:10,Bad\x1b]0;title\x07Song\tTab\nsong.mp3\n").unwrap();
  let Preview::Listing(listing) = preview_of(&PreviewRegistry::with_defaults(), &file) else {
    panic!("expected a listing");
  };
  let (_, title, _, _) = &entries(&listing)[0];
  assert!(
    !title.chars().any(|c| c.is_control() && c != ' '),
    "{title:?}"
  );
}
