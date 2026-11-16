//! Cursor and scroll-offset bookkeeping for a list shown in a fixed-height viewport.

/// Cursor index plus the index of the first visible row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
  pub index: usize,
  pub offset: usize,
}

impl Cursor {
  /// Places the cursor at `index` (clamped) and scrolls as needed, keeping `margin` rows of
  /// context around it (see [`Cursor::fix_offset`]).
  pub fn set(&mut self, index: usize, len: usize, height: usize, margin: usize) {
    if len == 0 {
      *self = Cursor::default();
      return;
    }
    self.index = index.min(len - 1);
    self.fix_offset(len, height, margin);
  }

  pub fn move_by(&mut self, delta: isize, len: usize, height: usize, margin: usize) {
    let target = self.index.saturating_add_signed(delta);
    self.set(target, len, height, margin);
  }

  /// Scrolls like ranger's `scroll_offset`: the viewport moves only when the cursor comes
  /// within `margin` rows of its top or bottom edge, and when the viewport is too short for
  /// that margin on both sides, the cursor is kept centered. The list never scrolls past its
  /// ends, so there is no empty space below the last entry.
  pub fn fix_offset(&mut self, len: usize, height: usize, margin: usize) {
    if len == 0 {
      *self = Cursor::default();
      return;
    }
    self.index = self.index.min(len - 1);
    if height == 0 {
      self.offset = self.index;
      return;
    }
    let max_offset = len.saturating_sub(height);
    if height / 2 < margin {
      self.offset = self.index.saturating_sub(height / 2).min(max_offset);
      return;
    }
    // Rows that must stay visible above and below the cursor.
    let above = margin;
    let below = height - 1 - margin;
    if self.index < self.offset + above {
      self.offset = self.index.saturating_sub(above);
    } else if self.index > self.offset + below {
      self.offset = self.index - below;
    }
    self.offset = self.offset.min(max_offset);
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
