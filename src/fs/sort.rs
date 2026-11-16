use std::cmp::Ordering;

use super::Entry;

/// Sorts directories first, then by natural, case-insensitive name order.
pub fn sort_entries(entries: &mut [Entry]) {
  entries.sort_by(compare_entries);
}

pub fn compare_entries(a: &Entry, b: &Entry) -> Ordering {
  b.is_dir()
    .cmp(&a.is_dir())
    .then_with(|| natural_cmp(&a.name, &b.name))
}

/// Natural, case-insensitive comparison: digit runs compare by numeric value, so `file2`
/// sorts before `file10`. Ties fall back to a plain comparison for a stable total order.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
  let mut ai = a.chars().peekable();
  let mut bi = b.chars().peekable();
  loop {
    match (ai.peek().copied(), bi.peek().copied()) {
      (None, None) => return a.cmp(b),
      (None, Some(_)) => return Ordering::Less,
      (Some(_), None) => return Ordering::Greater,
      (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
        let na = take_digits(&mut ai);
        let nb = take_digits(&mut bi);
        let ord = cmp_digit_runs(&na, &nb);
        if ord != Ordering::Equal {
          return ord;
        }
      }
      (Some(ca), Some(cb)) => {
        ai.next();
        bi.next();
        let ord = ca.to_lowercase().cmp(cb.to_lowercase());
        if ord != Ordering::Equal {
          return ord;
        }
      }
    }
  }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
  let mut s = String::new();
  while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
    s.push(c);
    it.next();
  }
  s
}

fn cmp_digit_runs(a: &str, b: &str) -> Ordering {
  let ta = a.trim_start_matches('0');
  let tb = b.trim_start_matches('0');
  ta.len()
    .cmp(&tb.len())
    .then_with(|| ta.cmp(tb))
    // Fewer leading zeros first: `1` < `01`.
    .then_with(|| a.len().cmp(&b.len()))
}
