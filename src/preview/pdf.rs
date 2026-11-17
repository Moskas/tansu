//! PDF previews: the first page rendered by poppler's `pdftoppm`, and the title, author,
//! page count and page size reported by `pdfinfo`. Both are optional; without them the
//! entry falls back to what the other can show.

use std::ffi::OsStr;
use std::path::Path;

use super::thumbnail::{MAX_SIDE, Rendered, cached_preview, run};
use super::{Preview, PreviewContext, PreviewError, Previewer};
use crate::fs::{Entry, EntryKind};

pub struct PdfPreviewer {
  /// Render the first page; without graphics only the details are shown.
  pub pages: bool,
}

impl Previewer for PdfPreviewer {
  fn name(&self) -> &str {
    "pdf"
  }

  fn can_handle(&self, entry: &Entry) -> bool {
    entry.kind == EntryKind::File && entry.extension().as_deref() == Some("pdf")
  }

  fn preview(&self, entry: &Entry, ctx: &PreviewContext) -> Result<Option<Preview>, PreviewError> {
    let kind = if self.pages { "pdf" } else { "pdf-info" };
    cached_preview(entry, kind, "PDF document", ctx, |out| {
      let info = info(&entry.path, ctx)?.unwrap_or_default();
      if self.pages && first_page(&entry.path, out, ctx)? {
        Ok(Rendered::Image(info))
      } else {
        Ok(Rendered::InfoOnly(info))
      }
    })
  }
}

/// Renders the first page of `pdf` into `out` (PNG), at most [`MAX_SIDE`] pixels a side.
fn first_page(pdf: &Path, out: &Path, ctx: &PreviewContext) -> Result<bool, PreviewError> {
  // pdftoppm adds the `.png` extension to the name it is given.
  let root = out.with_extension("");
  let size = MAX_SIDE.to_string();
  let args: [&OsStr; 10] = [
    "-f".as_ref(),
    "1".as_ref(),
    "-l".as_ref(),
    "1".as_ref(),
    "-singlefile".as_ref(),
    "-png".as_ref(),
    "-scale-to".as_ref(),
    size.as_ref(),
    pdf.as_os_str(),
    root.as_os_str(),
  ];
  Ok(run("pdftoppm", &args, ctx)?.is_some_and(|output| output.success && out.exists()))
}

/// `pdfinfo`'s report as detail rows, or `None` when it is missing or fails (a damaged or
/// password-protected file).
fn info(pdf: &Path, ctx: &PreviewContext) -> Result<Option<Vec<(String, String)>>, PreviewError> {
  let output = run("pdfinfo", &[pdf.as_os_str()], ctx)?;
  Ok(
    output
      .filter(|o| o.success)
      .map(|o| rows(&String::from_utf8_lossy(&o.stdout))),
  )
}

/// Picks the useful lines of `pdfinfo` output (`Key:   value`) as rows.
pub fn rows(report: &str) -> Vec<(String, String)> {
  let field = |key: &str| {
    report.lines().find_map(|line| {
      let (k, v) = line.split_once(':')?;
      (k.trim() == key)
        .then(|| v.trim().to_string())
        .filter(|v| !v.is_empty())
    })
  };
  let mut rows = Vec::new();
  for (key, label) in [("Title", "Title"), ("Author", "Author"), ("Pages", "Pages")] {
    if let Some(value) = field(key) {
      rows.push((label.to_string(), value));
    }
  }
  if let Some(size) = field("Page size") {
    rows.push(("Page size".to_string(), page_size(&size)));
  }
  if let Some(created) = field("CreationDate") {
    rows.push(("Created".to_string(), created));
  }
  if field("Encrypted").is_some_and(|e| e.starts_with("yes")) {
    rows.push(("Encrypted".to_string(), "yes".to_string()));
  }
  rows
}

/// `595.276 x 841.89 pts (A4)` as `A4 (210 x 297 mm)`; sizes without a name as just the
/// millimetres.
fn page_size(size: &str) -> String {
  let (dims, name) = match size.split_once('(') {
    Some((dims, name)) => (dims, Some(name.trim_end_matches(')').trim())),
    None => (size, None),
  };
  let mm: Option<Vec<f64>> = dims
    .trim()
    .trim_end_matches("pts")
    .split('x')
    .map(|n| n.trim().parse::<f64>().ok().map(|pt| pt / 72.0 * 25.4))
    .collect();
  let mm = match mm.as_deref() {
    Some([w, h]) => format!("{} x {} mm", w.round(), h.round()),
    _ => return size.to_string(),
  };
  match name {
    Some(name) if !name.is_empty() => format!("{name} ({mm})"),
    _ => mm,
  }
}
