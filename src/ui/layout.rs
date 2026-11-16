use ratatui::layout::Rect;

/// Rows taken by the header, the body's top and bottom borders, and the status bar.
pub const CHROME_ROWS: u16 = 4;

/// Number of list rows available in the body for a terminal of `height` rows.
pub fn body_rows(height: u16) -> usize {
  height.saturating_sub(CHROME_ROWS) as usize
}

/// Splits `inner` (the body without its outer border) into the three columns. Columns with
/// a zero ratio are hidden. Returns the column rects and the x positions of separators.
pub fn split_columns(inner: Rect, ratios: [u16; 3]) -> ([Option<Rect>; 3], Vec<u16>) {
  let shown: Vec<usize> = (0..3).filter(|&i| ratios[i] > 0).collect();
  let mut rects = [None; 3];
  if shown.is_empty() || inner.width == 0 {
    return (rects, Vec::new());
  }
  let seps = (shown.len() - 1) as u16;
  let avail = inner.width.saturating_sub(seps) as u32;
  let total: u32 = shown.iter().map(|&i| ratios[i] as u32).sum();
  let mut x = inner.x;
  let mut separators = Vec::new();
  let mut used = 0u32;
  for (n, &i) in shown.iter().enumerate() {
    let last = n + 1 == shown.len();
    let w = if last {
      avail - used
    } else {
      avail * ratios[i] as u32 / total
    };
    used += w;
    rects[i] = Some(Rect::new(x, inner.y, w as u16, inner.height));
    x += w as u16;
    if !last {
      separators.push(x);
      x += 1;
    }
  }
  (rects, separators)
}
