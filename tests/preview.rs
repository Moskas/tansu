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
