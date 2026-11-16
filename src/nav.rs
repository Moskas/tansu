//! Cursor and scroll-offset bookkeeping for a list shown in a fixed-height viewport.

/// Cursor index plus the index of the first visible row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
  pub index: usize,
  pub offset: usize,
}

impl Cursor {
  /// Places the cursor at `index` (clamped) and scrolls the minimum needed to show it.
  pub fn set(&mut self, index: usize, len: usize, height: usize) {
    if len == 0 {
      *self = Cursor::default();
      return;
    }
    self.index = index.min(len - 1);
    self.fix_offset(len, height);
  }

  pub fn move_by(&mut self, delta: isize, len: usize, height: usize) {
    let target = self.index.saturating_add_signed(delta);
    self.set(target, len, height);
  }

  /// Keeps the cursor inside the viewport and avoids empty space below the last entry.
  pub fn fix_offset(&mut self, len: usize, height: usize) {
    if len == 0 {
      *self = Cursor::default();
      return;
    }
    self.index = self.index.min(len - 1);
    if height == 0 {
      self.offset = self.index;
      return;
    }
    if self.index < self.offset {
      self.offset = self.index;
    } else if self.index >= self.offset + height {
      self.offset = self.index + 1 - height;
    }
    self.offset = self.offset.min(len.saturating_sub(height));
  }
}

/// Offset for a list that is displayed but not navigated (parent and preview columns): the
/// selected row stays visible, roughly centered once the list is longer than the viewport.
pub fn passive_offset(selected: usize, len: usize, height: usize) -> usize {
  if height == 0 || len <= height {
    return 0;
  }
  selected
    .saturating_sub(height / 2)
    .min(len.saturating_sub(height))
}

/// Where the viewport sits within the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollPos {
  All,
  Top,
  Bot,
  Percent(u8),
}

impl ScrollPos {
  pub fn compute(offset: usize, len: usize, height: usize) -> Self {
    if len <= height {
      ScrollPos::All
    } else if offset == 0 {
      ScrollPos::Top
    } else if offset + height >= len {
      ScrollPos::Bot
    } else {
      let pct = offset * 100 / (len - height);
      ScrollPos::Percent(pct.min(99) as u8)
    }
  }
}

impl std::fmt::Display for ScrollPos {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ScrollPos::All => f.write_str("All"),
      ScrollPos::Top => f.write_str("Top"),
      ScrollPos::Bot => f.write_str("Bot"),
      ScrollPos::Percent(p) => write!(f, "{p}%"),
    }
  }
}
