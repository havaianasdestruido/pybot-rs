//! Minimal, version-pinned Unicode behavior required by PyBot.
//!
//! Rust's standard library intentionally does not expose normalization or full Unicode
//! case-folding tables. The checked-in tables are generated from CPython's `unicodedata` via
//! [`tools/recon/gen_unicode_tables.py`](../tools/recon/gen_unicode_tables.py), and can be
//! regenerated explicitly during development. No runtime dependency or file I/O is involved.

mod tables {
    include!("unicode_tables.rs");
}

/// Unicode data version used to generate the bundled tables.
pub const UNICODE_VERSION: &str = tables::UNICODE_VERSION;

fn mapping(table: &'static [(u32, &'static str)], ch: char) -> Option<&'static str> {
    table
        .binary_search_by_key(&u32::from(ch), |(codepoint, _)| *codepoint)
        .ok()
        .and_then(|index| table.get(index).map(|(_, value)| *value))
}

/// Full Unicode default case folding (the behavior of Python `str.casefold`).
#[must_use]
pub fn casefold(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        if let Some(replacement) = mapping(tables::CASE_FOLD, ch) {
            result.push_str(replacement);
        } else {
            result.push(ch);
        }
    }
    result
}

fn append_without_marks(output: &mut String, text: &str) {
    for ch in text.chars() {
        if tables::COMBINING_MARKS
            .binary_search(&u32::from(ch))
            .is_err()
        {
            output.push(ch);
        }
    }
}

/// PyBot's explicit transliterations for Latin letters that do not decompose under NFKD.
fn explicit_latin_fold(ch: char) -> Option<&'static str> {
    match ch {
        'æ' | 'Æ' => Some("ae"),
        'œ' | 'Œ' => Some("oe"),
        'ø' | 'Ø' => Some("o"),
        'ł' | 'Ł' => Some("l"),
        'đ' | 'Đ' | 'ð' | 'Ð' => Some("d"),
        'þ' | 'Þ' => Some("th"),
        'ß' => Some("ss"),
        'ı' => Some("i"),
        _ => None,
    }
}

/// Latin-oriented NFKD accent removal followed by case folding.
///
/// NFKD decompositions and marks with non-zero canonical combining class are handled from
/// generated Unicode tables. PyBot's explicit Latin transliterations (`Æ` → `ae`, `ø` → `o`)
/// are applied before decomposition, matching its Python normalization helper.
#[must_use]
pub fn fold_diacritics(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        if tables::COMBINING_MARKS
            .binary_search(&u32::from(ch))
            .is_ok()
        {
            continue;
        }
        if let Some(replacement) = explicit_latin_fold(ch) {
            result.push_str(replacement);
        } else if let Some(replacement) = mapping(tables::NFKD_FOLD, ch) {
            append_without_marks(&mut result, replacement);
        } else if let Some(replacement) = mapping(tables::CASE_FOLD, ch) {
            append_without_marks(&mut result, replacement);
        } else {
            result.push(ch);
        }
    }
    result
}

/// Python's Unicode alphanumeric predicate, pinned to the generated Unicode version.
#[must_use]
pub fn is_alphanumeric(ch: char) -> bool {
    let codepoint = ch as u32;
    let ranges = tables::ALNUM_RANGES;
    let position = ranges.partition_point(|(start, _)| *start <= codepoint);
    position
        .checked_sub(1)
        .and_then(|index| ranges.get(index))
        .is_some_and(|(_, end)| codepoint <= *end)
}

/// Python regular-expression `\w`: Unicode alphanumerics and underscore (combining marks are
/// not word characters by themselves).
#[must_use]
pub fn is_word(ch: char) -> bool {
    ch == '_' || is_alphanumeric(ch)
}

/// Python regular-expression Unicode `\d`: decimal digits (General Category `Nd`), not all
/// Unicode numeric symbols such as fractions or superscripts.
#[must_use]
pub fn is_decimal_digit(ch: char) -> bool {
    let codepoint = ch as u32;
    tables::DECIMAL_DIGIT_STARTS
        .iter()
        .any(|start| codepoint >= *start && codepoint < *start + 10)
}

/// Python-compatible Unicode case-insensitive comparison key.
#[must_use]
pub fn casefold_eq(left: &str, right: &str) -> bool {
    casefold(left) == casefold(right)
}

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
    fn full_casefold_expands_and_handles_non_latin() {
        assert_eq!(casefold("Straße"), "strasse");
        assert_eq!(casefold("İΣς"), "i\u{307}σσ");
        assert_eq!(casefold("Kﬃ"), "kffi");
        assert!(casefold_eq("ΜΆΙΟΣ", "μάιοσ"));
    }
    #[test]
    fn accent_fold_matches_latin_nfkd_examples() {
        assert_eq!(fold_diacritics("Crème brûlée"), "creme brulee");
        assert_eq!(fold_diacritics("Æther Straße"), "aether strasse");
        assert_eq!(fold_diacritics("Œuvre Ø Ł Đ Þ ı"), "oeuvre o l d th i");
        assert_eq!(fold_diacritics("Cafe\u{301}"), "cafe");
        assert_eq!(fold_diacritics("İ ① Ａ ﬃ"), "i 1 a ffi");
        assert_eq!(fold_diacritics("a\u{34f}"), "a\u{34f}");
        assert_eq!(fold_diacritics("東京"), "東京");
    }
    #[test]
    fn python_regex_character_classes_are_distinct() {
        assert!(is_word('é'));
        assert!(is_word('٣'));
        assert!(is_word('_'));
        assert!(!is_word('\u{301}'));
        assert!(!is_word('-'));
        assert!(is_decimal_digit('٣'));
        assert!(is_decimal_digit('𝟡'));
        assert!(is_decimal_digit('𝟬'));
        assert!(is_decimal_digit('𝟿'));
        assert!(!is_decimal_digit('²'));
        assert!(!is_decimal_digit('Ⅷ'));
    }
    #[test]
    fn table_version_is_explicit() {
        assert_eq!(UNICODE_VERSION, "14.0.0");
    }
}
