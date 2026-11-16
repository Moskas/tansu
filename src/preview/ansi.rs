//! A minimal parser for the colored output of highlighters such as `bat`. SGR sequences
//! become span styles; every other escape sequence is dropped, so file contents can never
//! smuggle terminal control sequences into the UI.

use super::text::TAB_WIDTH;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TermColor {
  /// The terminal (or theme) default.
  #[default]
  Default,
  /// A palette index: 0-15 are the basic and bright colors, 16-255 the extended palette.
  Indexed(u8),
  Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpanStyle {
  pub fg: TermColor,
  pub bg: TermColor,
  pub bold: bool,
  pub dim: bool,
  pub italic: bool,
  pub underline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
  pub style: SpanStyle,
  pub text: String,
}

pub type StyledLine = Vec<StyledSpan>;

/// Splits `input` into lines of styled spans. Tabs are expanded and control characters
/// replaced, as for plain text previews. A trailing newline does not produce an empty line.
pub fn parse(input: &str) -> Vec<StyledLine> {
  let mut lines = Vec::new();
  let mut style = SpanStyle::default();
  let body = input.strip_suffix('\n').unwrap_or(input);
  for raw in body.split('\n') {
    lines.push(parse_line(raw, &mut style));
  }
  lines
}

fn parse_line(raw: &str, style: &mut SpanStyle) -> StyledLine {
  let mut spans: StyledLine = Vec::new();
  let mut text = String::new();
  let mut col = 0;
  let mut chars = raw.chars().peekable();
  while let Some(c) = chars.next() {
    match c {
      '\x1b' => {
        let before = *style;
        match chars.next() {
          // CSI: parameters, then a final byte in 0x40..=0x7e.
          Some('[') => {
            let mut params = String::new();
            for p in chars.by_ref() {
              if ('\x40'..='\x7e').contains(&p) {
                if p == 'm' {
                  apply_sgr(&params, style);
                }
                break;
              }
              params.push(p);
            }
          }
          // OSC: terminated by BEL or ST (ESC \).
          Some(']') => {
            while let Some(p) = chars.next() {
              if p == '\x07' || (p == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                break;
              }
            }
          }
          _ => {}
        }
        if *style != before && !text.is_empty() {
          spans.push(StyledSpan {
            style: before,
            text: std::mem::take(&mut text),
          });
        }
      }
      '\t' => {
        let pad = TAB_WIDTH - col % TAB_WIDTH;
        text.extend(std::iter::repeat_n(' ', pad));
        col += pad;
      }
      '\r' => {}
      c if c.is_control() => {
        text.push('?');
        col += 1;
      }
      c => {
        text.push(c);
        col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
      }
    }
  }
  if !text.is_empty() {
    spans.push(StyledSpan {
      style: *style,
      text,
    });
  }
  spans
}

fn apply_sgr(params: &str, style: &mut SpanStyle) {
  let mut codes = params.split([';', ':']).map(|p| {
    if p.is_empty() {
      0
    } else {
      p.parse::<u16>().unwrap_or(u16::MAX)
    }
  });
  while let Some(code) = codes.next() {
    match code {
      0 => *style = SpanStyle::default(),
      1 => style.bold = true,
      2 => style.dim = true,
      3 => style.italic = true,
      4 => style.underline = true,
      22 => {
        style.bold = false;
        style.dim = false;
      }
      23 => style.italic = false,
      24 => style.underline = false,
      30..=37 => style.fg = TermColor::Indexed((code - 30) as u8),
      39 => style.fg = TermColor::Default,
      40..=47 => style.bg = TermColor::Indexed((code - 40) as u8),
      49 => style.bg = TermColor::Default,
      90..=97 => style.fg = TermColor::Indexed((code - 90 + 8) as u8),
      100..=107 => style.bg = TermColor::Indexed((code - 100 + 8) as u8),
      38 | 48 => {
        let color = match codes.next() {
          Some(5) => codes.next().and_then(byte).map(TermColor::Indexed),
          Some(2) => match (
            codes.next().and_then(byte),
            codes.next().and_then(byte),
            codes.next().and_then(byte),
          ) {
            (Some(r), Some(g), Some(b)) => Some(TermColor::Rgb(r, g, b)),
            _ => None,
          },
          _ => None,
        };
        if let Some(color) = color {
          if code == 38 {
            style.fg = color;
          } else {
            style.bg = color;
          }
        }
      }
      _ => {}
    }
  }
}

fn byte(n: u16) -> Option<u8> {
  u8::try_from(n).ok()
}
