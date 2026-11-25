//! Python-shaped string, ordered-map, and similarity utilities used by the port.
//!
//! The routines here are deliberately small, deterministic, and dependency-free. They cover
//! the operations observed in the Phase 0 inventory rather than pretending to implement all
//! of Python's object model or format mini-language.

use std::fmt;

/// A Python-dict-like map: replacing a key preserves its original position; deleting and
/// reinserting appends it to the end. Lookup is linear, appropriate for the small maps used by
/// PyBot and deterministic by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for OrderedMap<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: Eq, V> OrderedMap<K, V> {
    /// Construct an empty map.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Borrow a value by key.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.entries
            .iter()
            .find(|(k, _)| k.borrow() == key)
            .map(|(_, v)| v)
    }
    /// Borrow a mutable value by key.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.entries
            .iter_mut()
            .find(|(k, _)| k.borrow() == key)
            .map(|(_, v)| v)
    }
    /// Set a value, returning the previous value if the key was already present.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some((_, old)) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            return Some(std::mem::replace(old, value));
        }
        self.entries.push((key, value));
        None
    }
    /// Remove a value; later entries retain their relative order.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + ?Sized,
    {
        let position = self.entries.iter().position(|(k, _)| k.borrow() == key)?;
        Some(self.entries.remove(position).1)
    }
    /// Iterate in insertion order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + ExactSizeIterator {
        self.entries.iter().map(|(k, v)| (k, v))
    }
    /// Iterate mutably in insertion order.
    pub fn iter_mut(
        &mut self,
    ) -> impl DoubleEndedIterator<Item = (&K, &mut V)> + ExactSizeIterator {
        self.entries.iter_mut().map(|(k, v)| (&*k, v))
    }
    /// Remove every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl<K: Eq, V> FromIterator<(K, V)> for OrderedMap<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = Self::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

/// Python `str.split()` with no separator: split on Unicode whitespace and collapse runs.
#[must_use]
pub fn split_whitespace(text: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut start = None;
    for (offset, ch) in text.char_indices() {
        if crate::pyutil::is_space(ch) {
            if let Some(begin) = start.take() {
                if let Some(piece) = text.get(begin..offset) {
                    pieces.push(piece);
                }
            }
        } else if start.is_none() {
            start = Some(offset);
        }
    }
    if let Some(begin) = start {
        if let Some(piece) = text.get(begin..) {
            pieces.push(piece);
        }
    }
    pieces
}

/// Python's `str.split(sep, maxsplit)` for a non-empty literal separator. Negative maxsplit
/// means unlimited; zero leaves the input unchanged in a one-element vector.
#[must_use]
pub fn split<'a>(text: &'a str, separator: &str, maxsplit: isize) -> Vec<&'a str> {
    if separator.is_empty() || maxsplit == 0 {
        return vec![text];
    }
    let limit = if maxsplit < 0 {
        usize::MAX
    } else {
        maxsplit as usize
    };
    let mut out = Vec::new();
    let mut rest = text;
    while out.len() < limit {
        let Some(index) = rest.find(separator) else {
            break;
        };
        if let (Some(head), Some(tail)) = (
            rest.get(..index),
            rest.get(index.saturating_add(separator.len())..),
        ) {
            out.push(head);
            rest = tail;
        } else {
            break;
        }
    }
    out.push(rest);
    out
}

/// Strip characters from both ends, matching `str.strip(chars)`; `None` uses Python whitespace.
#[must_use]
pub fn strip_chars<'a>(text: &'a str, chars: Option<&str>) -> &'a str {
    match chars {
        None => crate::pyutil::strip(text),
        Some(characters) => text.trim_matches(|ch| characters.contains(ch)),
    }
}

/// Strip characters from the left edge only, matching `str.lstrip(chars)`.
#[must_use]
pub fn lstrip_chars<'a>(text: &'a str, chars: Option<&str>) -> &'a str {
    match chars {
        None => text.trim_start_matches(crate::pyutil::is_space),
        Some(characters) => text.trim_start_matches(|ch| characters.contains(ch)),
    }
}

/// Strip characters from the right edge only, matching `str.rstrip(chars)`.
#[must_use]
pub fn rstrip_chars<'a>(text: &'a str, chars: Option<&str>) -> &'a str {
    match chars {
        None => text.trim_end_matches(crate::pyutil::is_space),
        Some(characters) => text.trim_end_matches(|ch| characters.contains(ch)),
    }
}

/// Python `str.replace(old, new, count)` for a literal substring. Negative count is unlimited;
/// an empty `old` inserts at Unicode character boundaries, including both ends.
#[must_use]
pub fn replace(text: &str, old: &str, new: &str, count: isize) -> String {
    if count == 0 {
        return text.to_owned();
    }
    let limit = if count < 0 {
        usize::MAX
    } else {
        count as usize
    };
    if old.is_empty() {
        let mut out = String::with_capacity(
            text.len().saturating_add(
                new.len()
                    .saturating_mul(text.chars().count().saturating_add(1)),
            ),
        );
        let mut inserted = 0usize;
        for ch in text.chars() {
            if inserted < limit {
                out.push_str(new);
                inserted += 1;
            }
            out.push(ch);
        }
        if inserted < limit {
            out.push_str(new);
        }
        return out;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut replaced = 0usize;
    while replaced < limit {
        let Some(position) = rest.find(old) else {
            break;
        };
        let Some(prefix) = rest.get(..position) else {
            break;
        };
        let Some(suffix) = rest.get(position.saturating_add(old.len())..) else {
            break;
        };
        out.push_str(prefix);
        out.push_str(new);
        rest = suffix;
        replaced += 1;
    }
    out.push_str(rest);
    out
}

/// Python's `str.title()` shape: first cased character of each run is uppercase, remainder
/// lowercase. Rust's Unicode casing may expand characters (for example ß -> SS).
#[must_use]
pub fn title(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_cased = false;
    for ch in text.chars() {
        if ch.is_alphabetic() {
            if previous_cased {
                out.extend(ch.to_lowercase());
            } else {
                out.extend(ch.to_uppercase());
            }
            previous_cased = true;
        } else {
            out.push(ch);
            previous_cased = false;
        }
    }
    out
}

/// Python-compatible broad Unicode alphanumeric check for a whole string (empty is false).
#[must_use]
pub fn is_alnum(text: &str) -> bool {
    !text.is_empty() && text.chars().all(crate::unicode::is_alphanumeric)
}

/// Levenshtein edit distance, O(min(m,n)) memory. The calculation saturates at `usize::MAX`
/// instead of overflowing on theoretical adversarial inputs.
#[must_use]
pub fn edit_distance(left: &str, right: &str) -> usize {
    let mut a: Vec<char> = left.chars().collect();
    let mut b: Vec<char> = right.chars().collect();
    if a.len() > b.len() {
        std::mem::swap(&mut a, &mut b);
    }
    let mut previous: Vec<usize> = (0..=a.len()).collect();
    let mut current = vec![0; a.len() + 1];
    for (row, rb) in b.iter().enumerate() {
        if let Some(first) = current.first_mut() {
            *first = row + 1;
        }
        for (col, ca) in a.iter().enumerate() {
            let cost = usize::from(ca != rb);
            let above = previous.get(col + 1).copied().unwrap_or(usize::MAX);
            let left = current.get(col).copied().unwrap_or(usize::MAX);
            let diagonal = previous.get(col).copied().unwrap_or(usize::MAX);
            if let Some(cell) = current.get_mut(col + 1) {
                *cell = above
                    .saturating_add(1)
                    .min(left.saturating_add(1))
                    .min(diagonal.saturating_add(cost));
            }
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous.get(a.len()).copied().unwrap_or(usize::MAX)
}

/// Ratcliff–Obershelp ratio matching `difflib.SequenceMatcher(None, a, b).ratio()` for
/// character sequences. The work budget bounds pathological input; `None` means refused.
pub fn sequence_ratio(left: &str, right: &str, max_work: usize) -> Option<f64> {
    let left_len = left.chars().count();
    let right_len = right.chars().count();
    if left_len.saturating_add(right_len) > max_work {
        return None;
    }
    let a: Vec<char> = left.chars().collect();
    let b: Vec<char> = right.chars().collect();
    let mut work = 0usize;
    let mut frequencies = std::collections::BTreeMap::<char, usize>::new();
    for ch in &b {
        *frequencies.entry(*ch).or_default() += 1;
    }
    // Python SequenceMatcher's default autojunk heuristic suppresses popular symbols in the
    // second sequence once it has at least 200 items.
    let popular = if b.len() >= 200 {
        let threshold = b.len() / 100 + 1;
        frequencies
            .into_iter()
            .filter_map(|(ch, count)| (count > threshold).then_some(ch))
            .collect::<std::collections::BTreeSet<_>>()
    } else {
        std::collections::BTreeSet::new()
    };
    struct MatchBudget<'a> {
        popular: &'a std::collections::BTreeSet<char>,
        work: &'a mut usize,
        max: usize,
    }
    fn matches(
        left_chars: &[char],
        a_range: std::ops::Range<usize>,
        right_chars: &[char],
        b_range: std::ops::Range<usize>,
        budget: &mut MatchBudget<'_>,
    ) -> Option<usize> {
        let mut stack = vec![(a_range, b_range)];
        let mut total = 0usize;
        while let Some((a_part, b_part)) = stack.pop() {
            let mut best = (0, 0, 0);
            for left_index in a_part.start..a_part.end {
                for right_index in b_part.start..b_part.end {
                    *budget.work = budget.work.checked_add(1)?;
                    if *budget.work > budget.max {
                        return None;
                    }
                    if right_chars
                        .get(right_index)
                        .is_some_and(|ch| budget.popular.contains(ch))
                    {
                        continue;
                    }
                    if left_chars.get(left_index) == right_chars.get(right_index) {
                        let mut size = 0;
                        while left_index + size < a_part.end
                            && right_index + size < b_part.end
                            && left_chars.get(left_index + size)
                                == right_chars.get(right_index + size)
                        {
                            size += 1;
                        }
                        if size > best.2 {
                            best = (left_index, right_index, size);
                        }
                    }
                }
            }
            if best.2 > 0 {
                total += best.2;
                if a_part.start < best.0 && b_part.start < best.1 {
                    stack.push((a_part.start..best.0, b_part.start..best.1));
                }
                let a_end = best.0 + best.2;
                let b_end = best.1 + best.2;
                if a_end < a_part.end && b_end < b_part.end {
                    stack.push((a_end..a_part.end, b_end..b_part.end));
                }
            }
        }
        Some(total)
    }
    if a.is_empty() && b.is_empty() {
        return Some(1.0);
    }
    let mut budget = MatchBudget {
        popular: &popular,
        work: &mut work,
        max: max_work,
    };
    let count = matches(&a, 0..a.len(), &b, 0..b.len(), &mut budget)?;
    Some((2.0 * (count as f64)) / ((a.len() + b.len()) as f64))
}

/// Python-like `str.format` for the supported subset `{}`, `{0}`, `{name}` and `{{`/`}}`.
/// Missing fields and malformed braces are errors, never silently retained.
pub fn format_fields(
    template: &str,
    positional: &[&str],
    named: &[(&str, &str)],
) -> Result<String, FormatError> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();
    let mut next = 0;
    while let Some((i, ch)) = chars.next() {
        if ch == '{' {
            if chars.peek().is_some_and(|(_, c)| *c == '{') {
                chars.next();
                out.push('{');
                continue;
            }
            let remainder = template
                .get(i.saturating_add(1)..)
                .ok_or(FormatError::UnmatchedBrace)?;
            let Some((end, _)) = remainder.char_indices().find(|(_, c)| *c == '}') else {
                return Err(FormatError::UnmatchedBrace);
            };
            let key = remainder.get(..end).ok_or(FormatError::UnmatchedBrace)?;
            let value = if key.is_empty() {
                let v = positional.get(next).ok_or(FormatError::MissingField)?;
                next += 1;
                *v
            } else if let Ok(index) = key.parse::<usize>() {
                *positional.get(index).ok_or(FormatError::MissingField)?
            } else {
                named
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| *value)
                    .ok_or(FormatError::MissingField)?
            };
            out.push_str(value);
            while chars
                .peek()
                .is_some_and(|(offset, _)| *offset <= i.saturating_add(1).saturating_add(end))
            {
                chars.next();
            }
        } else if ch == '}' {
            if chars.peek().is_some_and(|(_, c)| *c == '}') {
                chars.next();
                out.push('}');
            } else {
                return Err(FormatError::UnmatchedBrace);
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

/// Error returned by [`format_fields`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// An opening or closing brace was not escaped or paired.
    UnmatchedBrace,
    /// A referenced positional or named argument is absent.
    MissingField,
}
impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for FormatError {}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;
    #[test]
    fn ordered_map_python_update_delete_reinsert_order() {
        let mut m = OrderedMap::new();
        m.insert("a", 1);
        m.insert("b", 2);
        assert_eq!(m.insert("a", 3), Some(1));
        assert_eq!(
            m.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
            [("a", 3), ("b", 2),]
        );
        assert_eq!(m.remove("a"), Some(3));
        m.insert("a", 4);
        assert_eq!(m.iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["b", "a"]);
    }
    #[test]
    fn strip_and_replace_follow_python_edges() {
        assert_eq!(strip_chars("..hello!?", Some(".!?")), "hello");
        assert_eq!(lstrip_chars("  hello  ", None), "hello  ");
        assert_eq!(rstrip_chars("  hello  ", None), "  hello");
        assert_eq!(replace("one one one", "one", "two", 2), "two two one");
        assert_eq!(replace("abc", "", "-", 2), "-a-bc");
        assert_eq!(replace("abc", "a", "x", 0), "abc");
    }
    #[test]
    fn split_cases_match_python_examples() {
        assert_eq!(split_whitespace("  a\u{85} b \t"), ["a", "b"]);
        assert_eq!(split("a,,b,", ",", -1), ["a", "", "b", ""]);
        assert_eq!(split("a,,b,", ",", 2), ["a", "", "b,"]);
        assert_eq!(split("a", "", 1), ["a"]);
    }
    #[test]
    fn title_and_alnum_cover_unicode_and_empty() {
        assert_eq!(title("hELLO o'NEIL ǳABC"), "Hello O'Neil ǲabc");
        assert!(is_alnum("café٣"));
        assert!(!is_alnum(""));
        assert!(!is_alnum("a-b"));
    }
    #[test]
    fn edit_distance_uses_scalars_and_handles_empty() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("é", "e"), 1);
        assert_eq!(edit_distance("same", "same"), 0);
    }
    #[test]
    fn difflib_ratio_known_cases_and_budget() {
        assert_eq!(sequence_ratio("", "", 0), Some(1.0));
        assert_eq!(sequence_ratio("abcd", "abXd", 100), Some(0.75));
        assert_eq!(sequence_ratio("abcd", "wxyz", 1), None);
    }
    #[test]
    fn format_named_positional_escapes_and_errors() {
        assert_eq!(
            format_fields("{{{0}}} {name}", &["x"], &[("name", "y")]),
            Ok("{x} y".into())
        );
        assert_eq!(
            format_fields("{missing}", &[], &[]),
            Err(FormatError::MissingField)
        );
        assert_eq!(
            format_fields("{", &[], &[]),
            Err(FormatError::UnmatchedBrace)
        );
        assert_eq!(
            format_fields("}", &[], &[]),
            Err(FormatError::UnmatchedBrace)
        );
    }
}
