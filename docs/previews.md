# Previews

The right column previews the selected entry: a directory listing, syntax-highlighted
text, an image, or a metadata block for everything else.

- Text is highlighted by piping the first `max_lines` lines through
  [bat](https://github.com/sharkdp/bat) with `--style=plain --theme=ansi`, matching
  ranger's scope.sh. Without bat on `PATH`, previews fall back to plain text.
- Text in any encoding is shown: UTF-8 (with or without a byte order mark), UTF-16 (as
  Windows rippers write their logs) and legacy encodings such as Shift_JIS or
  Windows-1252, detected with Mozilla's chardetng. UTF-8 files with a few broken bytes stay
  UTF-8.
- Images (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA, PNM) are drawn with the graphics
  protocol the terminal supports (kitty, sixel or iTerm2), detected at startup. Other
  terminals get unicode half blocks. Formats that cannot be decoded, such as SVG, fall back
  to the text or metadata preview. Images are scaled once with Lanczos to the column's pixel
  size, never enlarged, and turned upright according to their EXIF orientation.
- Photos list their EXIF details below the image, under a separating rule: camera, lens, date
  taken, exposure, aperture, ISO, focal length and GPS location. They only take room while
  the image keeps two thirds of the column; in a shorter terminal they move to the metadata
  view (`i`).
- Videos show a frame from 10% into the video (the first frame is often black), rendered
  by `ffmpegthumbnailer` or `ffmpeg`, with the duration, resolution, video and audio codecs
  and bitrate from `ffprobe` below it, laid out like photo details. Without the frame
  programs (or without graphics), the details are shown in a metadata block.
- PDFs show their first page, rendered by `pdftoppm`, with the title, author, page count,
  page size and creation date from `pdfinfo` below it. Without poppler (or without
  graphics) the details are shown in a metadata block; a file poppler cannot read falls back
  to the text or metadata preview.
- Music and other audio files list their tags (title, artist, album, track, date, genre)
  and stream details (duration, codec, channels, sample rate, bit depth, bitrate) from
  `ffprobe`. An embedded cover is shown above them as a small picture (at most 400 pixels a
  side, extracted by `ffmpeg`); the details stay below it down to a 4-row cover, and files
  without a cover show them in a metadata block.
- Playlists (`.m3u`, `.m3u8`) list their entries, numbered, with `#EXTINF` titles and
  durations (or file names, with the folder dimmed after them) and the total length. HLS
  streaming playlists are summarised (segments, duration, variants). Cue sheets show the
  album, performer, date, genre and file, then the tracks with their performers and, when
  the tracks share one file, their lengths. CD rip logs from EAC, XLD, CUETools and whipper
  get a summary (ripper, date, album, drive, read mode, tracks, AccurateRip result, errors,
  log checksum) above the log; other `.log` files are plain text. Files not in UTF-8 name
  their encoding. All of these scroll with `<C-j>` / `<C-k>`.
- Ebooks and comics show their cover with the title, author, publisher and publication
  date (or the page count, for comics), read by tansu itself: EPUB covers from the package
  file's cover entry, MOBI/AZW3 covers from the Kindle metadata, and the first page image
  of a CBZ. Damaged files fall back to the metadata preview.
- Archives list their format, how many files, folders and links they hold, the unpacked
  size and how far it is packed, encrypted members, the zip comment and where `x` would
  extract them (`Extracts to`), then every member with its date and size in columns (the
  date is left out in a narrow column). zip and tar, plain or compressed with gzip, bzip2,
  xz, lzip or zstd, are read by tansu itself; `.gz` and the like show the file inside and
  its size, or the tar archive inside. 7z, rar, iso, cab, cpio, deb, rpm and others are
  listed by 7-Zip, unrar (rar only) or bsdtar, whichever is installed first; without one
  they get the metadata preview. At most `max_entries` members are listed, and a compressed
  tar is read for at most 256 MB of data; the details then say `at least`.
- Rendered previews (video frames, PDF pages, covers) are scaled to at most 2048 pixels a
  side and cached in `$XDG_CACHE_HOME/tansu/thumbnails` (usually
  `~/.cache/tansu/thumbnails`, readable only by the user), keyed by path, size and
  modification time, so a file is only rendered again after it changes. The cache keeps the
  1000 most recent thumbnails, up to 256 MiB. External programs get 10 seconds before they
  are stopped.
- `i` switches file previews to a metadata block (size, mode, owner, modification time,
  and dimensions for images) and back. The mode stays on while moving; directories keep
  showing their contents.
- Text and metadata previews scroll independently of the cursor with `<C-j>` / `<C-k>` (one
  line) or the mouse wheel (three lines). Moving to another entry starts at the top again.
  Scrolling near the end of a truncated text preview loads more of the file in the
  background: each step reads four times as many bytes as the last, with no line limit,
  up to 16 MiB. The shown text keeps its position until the longer version arrives.
