//! Archives: listing, previews and extraction, on fixtures built in the test.

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tansu::archive::{self, Compression, Contents, Format, MemberKind};
use tansu::fs::Entry;
use tansu::preview::{Preview, PreviewContext, PreviewLimits, PreviewRegistry, Row};
use zip::write::SimpleFileOptions;

fn list(path: &Path) -> Contents {
  archive::list(path, 1000, &|| false).unwrap()
}

fn paths(contents: &Contents) -> Vec<String> {
  contents.members.iter().map(|m| m.path.clone()).collect()
}

/// Names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
  let mut names: Vec<String> = fs::read_dir(dir)
    .unwrap()
    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
    .collect();
  names.sort();
  names
}

/// A zip with `docs/a.txt`, `docs/sub/b.txt` (no directory entries) and `top.txt`.
fn zip_fixture(path: &Path, top_level_file: bool) {
  let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
  let options = SimpleFileOptions::default()
    .last_modified_time(zip::DateTime::from_date_and_time(2021, 3, 4, 5, 6, 8).unwrap());
  zip.start_file("docs/a.txt", options).unwrap();
  zip.write_all(b"alpha\n").unwrap();
  zip.start_file("docs/sub/b.txt", options).unwrap();
  zip.write_all(&[b'b'; 1000]).unwrap();
  if top_level_file {
    zip.start_file("top.txt", options).unwrap();
    zip.write_all(b"top\n").unwrap();
  }
  zip.set_comment("made by the tests").unwrap();
  zip.finish().unwrap();
}

/// tar data with `project/`, `project/main.rs` and a link `project/latest -> main.rs`.
fn tar_bytes() -> Vec<u8> {
  let mut builder = tar::Builder::new(Vec::new());
  let mut dir = tar::Header::new_gnu();
  dir.set_entry_type(tar::EntryType::Directory);
  dir.set_mode(0o755);
  dir.set_size(0);
  dir.set_mtime(1_600_000_000);
  builder
    .append_data(&mut dir, "project/", std::io::empty())
    .unwrap();
  let body = b"fn main() {}\n";
  let mut file = tar::Header::new_gnu();
  file.set_mode(0o644);
  file.set_size(body.len() as u64);
  file.set_mtime(1_600_000_000);
  builder
    .append_data(&mut file, "project/main.rs", &body[..])
    .unwrap();
  let mut link = tar::Header::new_gnu();
  link.set_entry_type(tar::EntryType::Symlink);
  link.set_size(0);
  builder
    .append_link(&mut link, "project/latest", "main.rs")
    .unwrap();
  builder.into_inner().unwrap()
}

fn compress(data: &[u8], c: Compression) -> Vec<u8> {
  match c {
    Compression::None => data.to_vec(),
    Compression::Gzip => {
      let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
      e.write_all(data).unwrap();
      e.finish().unwrap()
    }
    Compression::Bzip2 => {
      let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
      e.write_all(data).unwrap();
      e.finish().unwrap()
    }
    Compression::Xz => {
      let mut e = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::default()).unwrap();
      e.write_all(data).unwrap();
      e.finish().unwrap()
    }
    Compression::Zstd => {
      ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
    }
    Compression::Lzip => unimplemented!("no lzip encoder in the tests"),
  }
}

#[test]
fn formats_are_told_by_name() {
  let cases = [
    ("a.zip", Some(Format::Zip), "a"),
    (
      "Photos.TAR.GZ",
      Some(Format::Tar(Compression::Gzip)),
      "Photos",
    ),
    ("x.tgz", Some(Format::Tar(Compression::Gzip)), "x"),
    ("x.tar.zst", Some(Format::Tar(Compression::Zstd)), "x"),
    ("x.tar", Some(Format::Tar(Compression::None)), "x"),
    (
      "notes.txt.xz",
      Some(Format::Compressed(Compression::Xz)),
      "notes.txt",
    ),
    ("v1.2.zip", Some(Format::Zip), "v1.2"),
  ];
  for (name, format, stem) in cases {
    let found = Format::from_name(name);
    assert_eq!(found.map(|(f, _)| f), format, "{name}");
    assert_eq!(found.map(|(_, s)| s), Some(stem), "{name}");
  }
  assert!(matches!(
    Format::from_name("music.rar"),
    Some((Format::External(_), "music"))
  ));
  assert_eq!(Format::from_name(".tar"), None);
  assert_eq!(Format::from_name("tarball"), None);
  assert_eq!(Format::from_name("main.rs"), None);
}

#[test]
fn zip_listing_counts_implied_folders() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("bundle.zip");
  zip_fixture(&path, true);
  let contents = list(&path);
  assert_eq!(contents.format, "zip");
  assert_eq!(
    paths(&contents),
    ["docs/a.txt", "docs/sub/b.txt", "top.txt"]
  );
  assert_eq!((contents.files, contents.dirs), (3, 2));
  assert_eq!(contents.size, 6 + 1000 + 4);
  assert!(contents.complete);
  assert_eq!(contents.comment.as_deref(), Some("made by the tests"));
  assert_eq!(
    contents.members[0].modified.as_deref(),
    Some("2021-03-04 05:06")
  );
  assert_eq!(contents.extracts_to("bundle").as_deref(), Some("bundle/"));

  zip_fixture(&path, false);
  assert_eq!(list(&path).extracts_to("bundle").as_deref(), Some("docs/"));
}

#[test]
fn tar_listing_in_every_built_in_compression() {
  let tmp = tempfile::tempdir().unwrap();
  let cases = [
    ("p.tar", Compression::None, "tar"),
    ("p.tar.gz", Compression::Gzip, "tar, gzip"),
    ("p.tbz2", Compression::Bzip2, "tar, bzip2"),
    ("p.tar.xz", Compression::Xz, "tar, xz"),
    ("p.tar.zst", Compression::Zstd, "tar, zstd"),
    // Compressed, but not named as a tar: the content tells.
    ("p.gz", Compression::Gzip, "tar, gzip"),
  ];
  for (name, c, label) in cases {
    let path = tmp.path().join(name);
    fs::write(&path, compress(&tar_bytes(), c)).unwrap();
    let contents = list(&path);
    assert_eq!(contents.format, label, "{name}");
    assert_eq!(
      paths(&contents),
      ["project", "project/main.rs", "project/latest"],
      "{name}"
    );
    let kinds: Vec<MemberKind> = contents.members.iter().map(|m| m.kind).collect();
    assert_eq!(kinds, [MemberKind::Dir, MemberKind::File, MemberKind::Link]);
    assert_eq!(contents.members[2].link.as_deref(), Some("main.rs"));
    assert_eq!(contents.members[1].size, Some(13));
    assert_eq!(contents.extracts_to("p").as_deref(), Some("project/"));
  }
}

#[test]
fn zstd_with_several_frames_is_read_to_the_end() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("notes.txt.zst");
  let mut data = compress(b"first frame\n", Compression::Zstd);
  data.extend(compress(b"second frame\n", Compression::Zstd));
  fs::write(&path, data).unwrap();
  let contents = list(&path);
  assert!(contents.single);
  assert_eq!(contents.members[0].size, Some(25));
}

#[test]
fn single_compressed_file_is_described() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("notes.txt.gz");
  fs::write(&path, compress(&[b'n'; 5000], Compression::Gzip)).unwrap();
  let contents = list(&path);
  assert!(contents.single);
  assert_eq!(contents.format, "gzip");
  assert_eq!(paths(&contents), ["notes.txt"]);
  assert_eq!(contents.members[0].size, Some(5000));
  assert_eq!(
    contents.extracts_to("notes.txt").as_deref(),
    Some("notes.txt")
  );
}

#[test]
fn listing_stops_at_the_member_limit() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("p.tar.gz");
  fs::write(&path, compress(&tar_bytes(), Compression::Gzip)).unwrap();
  let contents = archive::list(&path, 2, &|| false).unwrap();
  assert_eq!(contents.members.len(), 2);
  assert!(!contents.complete);
  assert_eq!(contents.extracts_to("p"), None);
  // Exactly as many members as allowed is still the whole archive.
  assert!(archive::list(&path, 3, &|| false).unwrap().complete);
}

#[test]
fn damaged_archives_and_cancellation_are_errors() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("broken.zip");
  fs::write(&path, b"PK\x03\x04 not really").unwrap();
  assert!(matches!(
    archive::list(&path, 10, &|| false),
    Err(archive::ListError::Failed(_))
  ));
  let path = tmp.path().join("p.tar.gz");
  fs::write(&path, compress(&tar_bytes(), Compression::Gzip)).unwrap();
  assert!(matches!(
    archive::list(&path, 10, &|| true),
    Err(archive::ListError::Cancelled)
  ));
}

fn entry(path: &Path) -> Entry {
  let listing = tansu::fs::list_dir(path.parent().unwrap(), 1000, &|| false).unwrap();
  listing
    .entries
    .into_iter()
    .find(|e| e.path == path)
    .unwrap()
}

#[test]
fn preview_lists_details_then_members() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("bundle.zip");
  zip_fixture(&path, true);
  let ctx = PreviewContext {
    limits: PreviewLimits::default(),
    cancelled: &|| false,
    thumbnails: None,
  };
  let preview = PreviewRegistry::with_defaults()
    .preview(&entry(&path), &ctx)
    .unwrap();
  let Preview::Listing(listing) = preview else {
    panic!("expected a listing, got {preview:?}");
  };
  assert_eq!(listing.kind, "archive");
  let info: Vec<(&str, &str)> = listing
    .info
    .iter()
    .map(|(k, v)| (k.as_str(), v.as_str()))
    .collect();
  assert_eq!(info[0], ("Archive", "zip"));
  assert_eq!(info[1], ("Contents", "3 files, 2 folders"));
  assert!(info[2].1.starts_with("1.01 k ("), "{:?}", info[2]);
  assert!(info.contains(&("Extracts to", "bundle/")));
  assert!(info.contains(&("Comment", "made by the tests")));
  assert_eq!(
    listing.rows[1],
    Row::Member {
      name: "docs/sub/b.txt".to_string(),
      kind: MemberKind::File,
      size: Some("1 k".to_string()),
      modified: Some("2021-03-04 05:06".to_string()),
    }
  );
}

#[test]
fn preview_falls_back_to_metadata_for_damaged_archives() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("broken.tar.gz");
  // Binary, so the text previewer passes on it too.
  fs::write(&path, b"\x1f\x8b\x00\x00broken").unwrap();
  let ctx = PreviewContext {
    limits: PreviewLimits::default(),
    cancelled: &|| false,
    thumbnails: None,
  };
  let preview = PreviewRegistry::with_defaults()
    .preview(&entry(&path), &ctx)
    .unwrap();
  assert!(matches!(preview, Preview::Metadata(_)), "{preview:?}");
}

/// A directory to extract into, holding only the archive.
fn dest_with(name: &str, data: &[u8]) -> (tempfile::TempDir, PathBuf) {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join(name);
  fs::write(&path, data).unwrap();
  (tmp, path)
}

#[test]
fn extracting_several_top_level_entries_makes_a_folder() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("bundle.zip");
  zip_fixture(&path, true);
  let target = extract(&path, tmp.path()).unwrap();
  assert_eq!(target, tmp.path().join("bundle"));
  assert_eq!(names(&target), ["docs", "top.txt"]);
  assert_eq!(
    fs::read(target.join("docs/sub/b.txt")).unwrap(),
    [b'b'; 1000]
  );
  // The zip's modification time, in local time like the zip stores it.
  let modified = fs::metadata(target.join("top.txt"))
    .unwrap()
    .modified()
    .unwrap();
  let shown = tansu::fs::meta::format_mtime(
    modified
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_secs() as i64,
  );
  assert_eq!(shown, "2021-03-04 05:06");
  // Nothing else: no leftover hidden directory.
  assert_eq!(names(tmp.path()), ["bundle", "bundle.zip"]);

  // Again: the name is taken, so it gets a suffix and the first stays as it was.
  fs::write(tmp.path().join("bundle/top.txt"), "edited").unwrap();
  let again = extract(&path, tmp.path()).unwrap();
  assert_eq!(again, tmp.path().join("bundle_"));
  assert_eq!(
    fs::read_to_string(tmp.path().join("bundle/top.txt")).unwrap(),
    "edited"
  );
}

#[test]
fn extracting_a_single_top_level_folder_keeps_it() {
  let (tmp, path) = dest_with("p.tar.xz", &compress(&tar_bytes(), Compression::Xz));
  let target = extract(&path, tmp.path()).unwrap();
  assert_eq!(target, tmp.path().join("project"));
  assert_eq!(names(&target), ["latest", "main.rs"]);
  assert_eq!(
    fs::read_link(target.join("latest")).unwrap(),
    Path::new("main.rs")
  );
  assert_eq!(names(tmp.path()), ["p.tar.xz", "project"]);
}

#[test]
fn extracting_a_compressed_file_decompresses_it() {
  let (tmp, path) = dest_with(
    "notes.txt.bz2",
    &compress(b"some notes\n", Compression::Bzip2),
  );
  let target = extract(&path, tmp.path()).unwrap();
  assert_eq!(target, tmp.path().join("notes.txt"));
  assert_eq!(fs::read_to_string(&target).unwrap(), "some notes\n");
}

#[test]
fn failed_extraction_leaves_nothing_behind() {
  let (tmp, path) = dest_with("broken.tar.gz", b"not gzip at all");
  let err = extract(&path, tmp.path()).unwrap_err();
  assert!(err.starts_with("extract: broken.tar.gz: "), "{err}");
  assert_eq!(names(tmp.path()), ["broken.tar.gz"]);

  let (tmp, path) = dest_with(
    "empty.tar",
    &tar::Builder::new(Vec::new()).into_inner().unwrap(),
  );
  let err = extract(&path, tmp.path()).unwrap_err();
  assert!(err.ends_with("the archive is empty"), "{err}");
  assert_eq!(names(tmp.path()), ["empty.tar"]);
}

/// A tar header for `name` written as is, bypassing the checks `tar::Builder` makes.
fn raw_header(name: &str, kind: tar::EntryType, size: u64, link: Option<&str>) -> tar::Header {
  let mut header = tar::Header::new_old();
  let raw = header.as_old_mut();
  raw.name[..name.len()].copy_from_slice(name.as_bytes());
  if let Some(link) = link {
    raw.linkname[..link.len()].copy_from_slice(link.as_bytes());
  }
  header.set_entry_type(kind);
  header.set_size(size);
  header.set_mode(0o644);
  header.set_cksum();
  header
}

#[test]
fn extraction_never_writes_outside_the_destination() {
  let tmp = tempfile::tempdir().unwrap();
  let dest = tmp.path().join("dest");
  fs::create_dir(&dest).unwrap();
  let outside = tmp.path().join("outside");
  fs::create_dir(&outside).unwrap();

  // tar: a `..` path, an absolute path, and a link out followed by a file through it.
  let mut builder = tar::Builder::new(Vec::new());
  let escapes = [
    ("../escaped.txt".to_string(), tar::EntryType::Regular, None),
    (
      format!("{}/absolute.txt", outside.display()),
      tar::EntryType::Regular,
      None,
    ),
    (
      "out".to_string(),
      tar::EntryType::Symlink,
      Some(outside.to_string_lossy().into_owned()),
    ),
    ("out/through.txt".to_string(), tar::EntryType::Regular, None),
    ("fine.txt".to_string(), tar::EntryType::Regular, None),
  ];
  for (name, kind, link) in &escapes {
    let size = if *kind == tar::EntryType::Regular {
      4
    } else {
      0
    };
    let header = raw_header(name, *kind, size, link.as_deref());
    builder.append(&header, &b"evil"[..size as usize]).unwrap();
  }
  let path = dest.join("evil.tar");
  fs::write(&path, builder.into_inner().unwrap()).unwrap();
  let _ = extract(&path, &dest);

  // zip: the same with zip paths.
  let path = dest.join("evil.zip");
  let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
  let options = SimpleFileOptions::default();
  zip
    .start_file("..\\escaped-backslash.txt", options)
    .unwrap();
  zip.write_all(b"evil").unwrap();
  zip.start_file("../escaped-zip.txt", options).unwrap();
  zip.write_all(b"evil").unwrap();
  zip
    .add_symlink("zout", outside.to_string_lossy(), options)
    .unwrap();
  zip.start_file("zout/through-zip.txt", options).unwrap();
  zip.write_all(b"evil").unwrap();
  zip.start_file("fine-zip.txt", options).unwrap();
  zip.write_all(b"good").unwrap();
  zip.finish().unwrap();
  let _ = extract(&path, &dest);

  assert_eq!(names(tmp.path()), ["dest", "outside"]);
  assert!(names(&outside).is_empty(), "{:?}", names(&outside));
  assert!(
    names(&dest).iter().all(|n| !n.starts_with('.')),
    "temporary directory left: {:?}",
    names(&dest)
  );
}

#[test]
fn extraction_drops_setuid_bits() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("tool.zip");
  let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
  zip
    .start_file("run", SimpleFileOptions::default().unix_permissions(0o4755))
    .unwrap();
  zip.write_all(b"#!/bin/sh\n").unwrap();
  zip.finish().unwrap();
  let target = extract(&path, tmp.path()).unwrap();
  let mode = fs::metadata(&target).unwrap().permissions().mode();
  assert_eq!(mode & 0o7777, 0o755);
}

fn installed(program: &str) -> bool {
  std::process::Command::new(program)
    .arg("--help")
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status()
    .is_ok()
}

/// Makes `name` with `tar` data through bsdtar, which writes most formats.
fn bsdtar_create(dir: &Path, name: &str, format: &str) -> PathBuf {
  let src = dir.join("src");
  fs::create_dir_all(src.join("inner")).unwrap();
  fs::write(src.join("inner/x.txt"), "x\n").unwrap();
  fs::write(src.join("y.txt"), "yy\n").unwrap();
  let path = dir.join(name);
  let status = std::process::Command::new("bsdtar")
    .args(["--format", format, "-cf"])
    .arg(&path)
    .arg("-C")
    .arg(&src)
    .args(["inner", "y.txt"])
    .status()
    .unwrap();
  assert!(status.success());
  fs::remove_dir_all(&src).unwrap();
  path
}

#[test]
fn external_formats_through_bsdtar() {
  if !installed("bsdtar") {
    eprintln!("skipped: bsdtar not installed");
    return;
  }
  let tmp = tempfile::tempdir().unwrap();
  let path = bsdtar_create(tmp.path(), "pack.7z", "7zip");
  let contents = list(&path);
  assert_eq!(contents.format, "7z");
  let mut found = paths(&contents);
  found.sort();
  assert_eq!(found, ["inner", "inner/x.txt", "y.txt"]);
  assert_eq!(contents.size, 5);
  let target = extract(&path, tmp.path()).unwrap();
  assert_eq!(target, tmp.path().join("pack"));
  assert_eq!(
    fs::read_to_string(target.join("inner/x.txt")).unwrap(),
    "x\n"
  );
}

#[test]
fn read_only_folders_are_extracted_as_they_are() {
  // As tarballs of Nix store paths have them.
  let mut builder = tar::Builder::new(Vec::new());
  let mut dir = tar::Header::new_gnu();
  dir.set_entry_type(tar::EntryType::Directory);
  dir.set_mode(0o555);
  dir.set_size(0);
  builder
    .append_data(&mut dir, "pkg/", std::io::empty())
    .unwrap();
  let mut file = tar::Header::new_gnu();
  file.set_mode(0o444);
  file.set_size(3);
  builder
    .append_data(&mut file, "pkg/a", &b"aaa"[..])
    .unwrap();
  let (tmp, path) = dest_with("pkg.tar", &builder.into_inner().unwrap());
  let target = extract(&path, tmp.path()).unwrap();
  assert_eq!(target, tmp.path().join("pkg"));
  let mode = fs::metadata(&target).unwrap().permissions().mode();
  assert_eq!(mode & 0o777, 0o555);
  assert_eq!(fs::read(target.join("a")).unwrap(), b"aaa");
  assert_eq!(names(tmp.path()), ["pkg", "pkg.tar"]);
  // Let the temporary directory be removed.
  fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Extracts without skipping damaged members: what was created, or the error message.
fn extract(path: &Path, dest: &Path) -> Result<PathBuf, String> {
  archive::extract(path, dest, false)
    .map(|e| e.path)
    .map_err(|e| e.to_string())
}

/// A stored zip of `album/01.flac`, `album/02.flac` (damaged: a byte of its data changed
/// after the checksum was taken) and `album/03.flac`.
fn damaged_zip(path: &Path) {
  let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
  let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
  for (name, data) in [
    ("album/01.flac", "first track"),
    ("album/02.flac", "SECOND TRACK"),
    ("album/03.flac", "third track"),
  ] {
    zip.start_file(name, stored).unwrap();
    zip.write_all(data.as_bytes()).unwrap();
  }
  zip.finish().unwrap();
  let mut bytes = fs::read(path).unwrap();
  let at = bytes
    .windows(12)
    .position(|w| w == b"SECOND TRACK")
    .unwrap();
  bytes[at] = b'X';
  fs::write(path, bytes).unwrap();
}

#[test]
fn damaged_members_stop_extraction_unless_skipped() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("album.zip");
  damaged_zip(&path);
  assert_eq!(
    archive::extract(&path, tmp.path(), false),
    Err(archive::ExtractError::Damaged {
      archive: "album.zip".to_string(),
      members: vec!["album/02.flac".to_string()],
    })
  );
  assert_eq!(names(tmp.path()), ["album.zip"]);

  let extracted = archive::extract(&path, tmp.path(), true).unwrap();
  assert_eq!(extracted.path, tmp.path().join("album"));
  assert_eq!(extracted.skipped, ["album/02.flac"]);
  assert_eq!(names(&extracted.path), ["01.flac", "03.flac"]);
  assert_eq!(names(tmp.path()), ["album", "album.zip"]);
}

#[test]
fn zip_links_are_made_only_when_they_stay_inside() {
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("links.zip");
  let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
  let options = SimpleFileOptions::default();
  zip.start_file("pkg/lib/real.so", options).unwrap();
  zip.write_all(b"elf").unwrap();
  zip
    .add_symlink("pkg/lib/short.so", "real.so", options)
    .unwrap();
  zip.add_symlink("pkg/up", "../pkg/lib", options).unwrap();
  zip.add_symlink("pkg/here", ".", options).unwrap();
  // Through `here` this climbs one level more than it seems to: out of the destination.
  zip
    .add_symlink("pkg/sneaky", "here/../..", options)
    .unwrap();
  zip.add_symlink("pkg/out", "../../..", options).unwrap();
  zip.finish().unwrap();
  let target = extract(&path, tmp.path()).unwrap();
  assert_eq!(names(&target), ["here", "lib", "up"]);
  assert_eq!(fs::read(target.join("lib/short.so")).unwrap(), b"elf");
  assert_eq!(fs::read(target.join("up/real.so")).unwrap(), b"elf");
}

#[test]
fn shift_jis_names_without_the_utf8_flag_are_decoded() {
  // As written by zip tools on Japanese Windows: the code page, with no flag saying so.
  let originals = [
    "アルバム\\01 ナクモ - あおいくん.flac",
    "アルバム\\02 知声 - ゆうえんち.flac",
    "アルバム\\表紙.png",
  ];
  let encoded: Vec<Vec<u8>> = originals
    .iter()
    .map(|n| encoding_rs::SHIFT_JIS.encode(n).0.into_owned())
    .collect();
  let tmp = tempfile::tempdir().unwrap();
  let path = tmp.path().join("album.zip");
  let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
  let placeholders: Vec<String> = encoded
    .iter()
    .enumerate()
    .map(|(i, e)| format!("{i}{}", "Z".repeat(e.len() - 1)))
    .collect();
  for name in &placeholders {
    zip.start_file(name, SimpleFileOptions::default()).unwrap();
    zip.write_all(b"data").unwrap();
  }
  zip.finish().unwrap();
  let mut bytes = fs::read(&path).unwrap();
  for (placeholder, real) in placeholders.iter().zip(&encoded) {
    let p = placeholder.as_bytes();
    let mut at = 0;
    while let Some(i) = bytes[at..].windows(p.len()).position(|w| w == p) {
      bytes[at + i..at + i + p.len()].copy_from_slice(real);
      at += i + p.len();
    }
  }
  fs::write(&path, bytes).unwrap();

  let contents = list(&path);
  assert_eq!(contents.format, "zip, Shift_JIS names");
  assert_eq!(
    paths(&contents),
    [
      "アルバム/01 ナクモ - あおいくん.flac",
      "アルバム/02 知声 - ゆうえんち.flac",
      "アルバム/表紙.png",
    ]
  );
  assert_eq!(contents.extracts_to("album").as_deref(), Some("アルバム/"));
  let target = extract(&path, tmp.path()).unwrap();
  assert_eq!(target, tmp.path().join("アルバム"));
  assert_eq!(
    names(&target),
    [
      "01 ナクモ - あおいくん.flac",
      "02 知声 - ゆうえんち.flac",
      "表紙.png"
    ]
  );
}
