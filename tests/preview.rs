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
