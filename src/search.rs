//! Matching entry names against a search pattern: shell-style wildcards when the pattern has
//! any, fuzzy matching otherwise. Both use vim's smart case: case is ignored unless the
//! pattern contains an uppercase letter.

/// A parsed search pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
  kind: Kind,
  case_sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
  /// Matched against the whole name.
  Glob(Vec<Token>),
  /// The chars must appear in order, not necessarily next to each other.
  Fuzzy(Vec<char>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
  /// `*`: any run of chars, including none.
  Any,
  /// `?`: exactly one char.
  One,
  Char(char),
}

// Fuzzy scoring, after fzf: every matched char scores, more when it continues the previous
// match or starts a word; skipped chars between matches cost a little.
const SCORE_MATCH: i64 = 16;
const BONUS_CONSECUTIVE: i64 = 8;
const BONUS_WORD_START: i64 = 8;
const PENALTY_GAP: i64 = 1;

impl Pattern {
  /// Parses `text`. It is a wildcard pattern when it contains an unescaped `*` or `?`;
  /// `\` makes the next char literal. Returns `None` for an empty pattern.
  pub fn parse(text: &str) -> Option<Pattern> {
    if text.is_empty() {
      return None;
    }
    let case_sensitive = text.chars().any(char::is_uppercase);
    let fold = |c: char| {
      if case_sensitive { c } else { fold_case(c) }
    };
    let mut tokens = Vec::new();
    let mut wildcard = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
      tokens.push(match c {
        '*' => {
          wildcard = true;
          Token::Any
        }
        '?' => {
          wildcard = true;
          Token::One
        }
        '\\' => Token::Char(fold(chars.next().unwrap_or('\\'))),
        c => Token::Char(fold(c)),
      });
    }
    let kind = if wildcard {
      Kind::Glob(tokens)
    } else {
      // Without wildcards every token is a literal char.
      let chars = tokens
        .into_iter()
        .filter_map(|t| match t {
          Token::Char(c) => Some(c),
          _ => None,
        })
        .collect();
      Kind::Fuzzy(chars)
    };
    Some(Pattern {
      kind,
      case_sensitive,
    })
  }

  /// Whether this is a wildcard pattern (as opposed to a fuzzy one).
  pub fn is_glob(&self) -> bool {
    matches!(self.kind, Kind::Glob(_))
  }

  /// How well `name` matches; higher is better, `None` is no match. Every wildcard match
  /// scores the same.
  pub fn score(&self, name: &str) -> Option<i64> {
    let name: Vec<char> = if self.case_sensitive {
      name.chars().collect()
    } else {
      name.chars().map(fold_case).collect()
    };
    match &self.kind {
      Kind::Glob(tokens) => glob_match(tokens, &name).then_some(0),
      Kind::Fuzzy(pattern) => fuzzy_score(pattern, &name),
    }
  }
}

/// Lowercases a char that lowercases to a single char (all that matter for names).
fn fold_case(c: char) -> char {
  let mut lower = c.to_lowercase();
  match (lower.next(), lower.next()) {
    (Some(l), None) => l,
    _ => c,
  }
}

/// Iterative wildcard matching with backtracking to the last `*`; linear in practice.
fn glob_match(tokens: &[Token], name: &[char]) -> bool {
  let (mut t, mut n) = (0, 0);
  // Position after the last `*` and the name position it is currently matched up to.
  let mut star: Option<(usize, usize)> = None;
  while n < name.len() {
    match tokens.get(t) {
      Some(Token::Any) => {
        star = Some((t + 1, n));
        t += 1;
      }
      Some(Token::One) => {
        t += 1;
        n += 1;
      }
      Some(Token::Char(c)) if *c == name[n] => {
        t += 1;
        n += 1;
      }
      _ => match star {
        // Let the last `*` swallow one more char and retry from there.
        Some((after, matched)) => {
          t = after;
          n = matched + 1;
          star = Some((after, matched + 1));
        }
        None => return false,
      },
    }
  }
  tokens[t..].iter().all(|t| *t == Token::Any)
}

/// Best score of `pattern` as a subsequence of `name`, over every way of placing it.
fn fuzzy_score(pattern: &[char], name: &[char]) -> Option<i64> {
  if pattern.len() > name.len() {
    return None;
  }
  // best[j]: best score with the current pattern char matched at name[j].
  let mut prev: Vec<Option<i64>> = Vec::new();
  for (i, &pc) in pattern.iter().enumerate() {
    let mut best = vec![None; name.len()];
    // Best of prev[k] - gap penalty for k < j, carried along as j advances.
    let mut carry: Option<i64> = None;
    for j in 0..name.len() {
      if i > 0 && j > 0 {
        carry = carry.map(|s| s - PENALTY_GAP);
        if let Some(s) = prev[j - 1] {
          carry = Some(carry.map_or(s, |c| c.max(s)));
        }
      }
      if name[j] != pc {
        continue;
      }
      let bonus = SCORE_MATCH
        + if word_start(name, j) {
          BONUS_WORD_START
        } else {
          0
        };
      best[j] = if i == 0 {
        Some(bonus)
      } else {
        let consecutive = (j > 0)
          .then(|| prev[j - 1])
          .flatten()
          .map(|s| s + bonus + BONUS_CONSECUTIVE);
        // `carry` already holds prev[j - 1] with no gap; the consecutive bonus is on top.
        let gapped = carry.map(|s| s + bonus);
        match (consecutive, gapped) {
          (Some(a), Some(b)) => Some(a.max(b)),
          (a, b) => a.or(b),
        }
      };
    }
    prev = best;
  }
  prev.into_iter().flatten().max()
}

/// Whether `name[j]` starts a word: the name's first char, a char after a separator, or an
/// uppercase letter after a lowercase one (camelCase). Case is already folded unless the
/// pattern is case sensitive, so camelCase only counts then.
fn word_start(name: &[char], j: usize) -> bool {
  let Some(&before) = j.checked_sub(1).and_then(|k| name.get(k)) else {
    return true;
  };
  let c = name[j];
  matches!(before, ' ' | '_' | '-' | '.' | '/') || (before.is_lowercase() && c.is_uppercase())
}
