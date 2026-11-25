//! Minimal, version-pinned Unicode behavior required by PyBot.
//!
//! Rust's standard library intentionally does not expose normalization or full Unicode
//! case-folding tables. The checked-in tables are generated from CPython's `unicodedata` via
//! [`tools/recon/gen_unicode_tables.py`](../tools/recon/gen_unicode_tables.py), and can be
//! regenerated explicitly during development. No runtime dependency or file I/O is involved.

mod tables { include!("unicode_tables.rs"); }

/// Unicode data version used to generate the bundled tables.
pub const UNICODE_VERSION: &str = tables::UNICODE_VERSION;

fn mapping(table: &'static [(u32, &'static str)], ch: char) -> Option<&'static str> {
    table.binary_search_by_key(&(ch as u32), |(codepoint, _)| *codepoint)
        .ok().and_then(|index| table.get(index).map(|(_, value)| *value))
}

/// Full Unicode default case folding (the behavior of Python `str.casefold`).
#[must_use]
pub fn casefold(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        if let Some(replacement) = mapping(tables::CASE_FOLD, ch) { result.push_str(replacement); }
        else { result.push(ch); }
    }
    result
}

/// Latin-oriented NFKD accent removal followed by case folding.
///
/// Latin letters with compatibility/canonical decompositions are reduced to their base
/// spelling (`é` → `e`, `Æ` → `ae`, `ﬃ` is not folded because it is not a Latin letter in
/// Unicode's name). All combining marks are removed; other scripts and symbols are preserved.
#[must_use]
pub fn fold_diacritics(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        if tables::COMBINING_MARKS.binary_search(&(ch as u32)).is_ok() { continue; }
        if let Some(replacement) = mapping(tables::LATIN_FOLD, ch) { result.push_str(replacement); }
        else if let Some(replacement) = mapping(tables::CASE_FOLD, ch) { result.push_str(replacement); }
        else { result.push(ch); }
    }
    result
}

/// Python regular-expression `\w`: Unicode alphanumerics and underscore (combining marks are
/// not word characters by themselves).
#[must_use]
pub fn is_word(ch: char) -> bool { ch == '_' || ch.is_alphanumeric() }

/// Python regular-expression Unicode `\d`: decimal digits (General Category `Nd`), not all
/// Unicode numeric symbols such as fractions or superscripts.
#[must_use]
pub fn is_decimal_digit(ch: char) -> bool {
    // `char::to_digit` intentionally recognizes ASCII syntax only. Every Unicode Nd run is
    // represented by a zero through nine contiguous block, with a handful of adjacent sets.
    let cp = ch as u32;
    const STARTS: &[u32] = &[
        0x30,0x660,0x6f0,0x7c0,0x966,0x9e6,0xa66,0xae6,0xb66,0xbe6,0xc66,0xce6,0xd66,0xde6,
        0xe50,0xed0,0xf20,0x1040,0x1090,0x17e0,0x1810,0x1946,0x19d0,0x1a80,0x1a90,0x1b50,
        0x1bb0,0x1c40,0x1c50,0xa620,0xa8d0,0xa900,0xa9d0,0xa9f0,0xaa50,0xabf0,0xff10,0x104a0,
        0x10d30,0x11066,0x110f0,0x11136,0x111d0,0x112f0,0x11450,0x114d0,0x11650,0x116c0,
        0x11730,0x118e0,0x11950,0x11c50,0x11d50,0x11da0,0x16a60,0x16ac0,0x16b50,0x1d7ce,
        0x1e140,0x1e2f0,0x1e4f0,0x1e950,0x1fbf0,
    ];
    STARTS.iter().any(|start| cp >= *start && cp < *start + 10)
}

/// Python-compatible Unicode case-insensitive comparison key.
#[must_use]
pub fn casefold_eq(left: &str, right: &str) -> bool { casefold(left) == casefold(right) }

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;
    #[test] fn full_casefold_expands_and_handles_non_latin() {
        assert_eq!(casefold("Straße"), "strasse");
        assert_eq!(casefold("İΣς"), "i\u{307}σσ");
        assert_eq!(casefold("Kﬃ"), "kffi");
        assert!(casefold_eq("ΜΆΙΟΣ", "μάιοσ"));
    }
    #[test] fn accent_fold_matches_latin_nfkd_examples() {
        assert_eq!(fold_diacritics("Crème brûlée"), "creme brulee");
        assert_eq!(fold_diacritics("Æther Straße"), "aether strasse");
        assert_eq!(fold_diacritics("Cafe\u{301}"), "cafe");
        assert_eq!(fold_diacritics("東京"), "東京");
    }
    #[test] fn python_regex_character_classes_are_distinct() {
        assert!(is_word('é')); assert!(is_word('٣')); assert!(is_word('_'));
        assert!(!is_word('\u{301}')); assert!(!is_word('-'));
        assert!(is_decimal_digit('٣')); assert!(is_decimal_digit('𝟡'));
        assert!(!is_decimal_digit('²')); assert!(!is_decimal_digit('Ⅷ'));
    }
    #[test] fn table_version_is_explicit() { assert_eq!(UNICODE_VERSION, "14.0.0"); }
}
