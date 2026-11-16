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
