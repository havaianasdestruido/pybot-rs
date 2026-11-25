//! Python-compatible primitives used across the port.
//!
//! Rust's `str` and Python's `str` disagree on some everyday operations that the
//! differential harness cannot afford to get wrong:
//!
//! | operation | Python | Rust `std` |
//! |---|---|---|
//! | whitespace | 29 characters, incl. `\x1c`-`\x1f` and U+0085 | 25 characters, no `\x1c`-`\x1f` |
//! | `" ".join(s.split())` | splits on runs of Python whitespace | `split_whitespace` uses the Rust set |
//! | `repr(float)` | shortest round-trip; `300.0`, `1e+16`, `1e-05` | `{}` → `300`, `1e16`, `0.00001` |
//! | `str.isspace()` | includes `\x1c`-`\x1f` | differs |
//! | `str.strip(chars)` | strips the given characters | `trim_matches` with a closure |
//!
//! This module gives the port the Python behaviour, with tests that pin every case. It is the
//! foundation the Fase 2 (`util/`) work grows from: adding an operation here is cheaper than
//! discovering the difference through a corpus disagreement.

/// The 29 code points for which Python's `str.isspace()` is true.
///
/// Generated from `CPython`: `[i for i in range(0x110000) if chr(i).isspace()]`.
pub const PY_WHITESPACE: [char; 29] = [
    '\u{0009}', '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{001C}', '\u{001D}', '\u{001E}',
    '\u{001F}', '\u{0020}', '\u{0085}', '\u{00A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}',
    '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}',
    '\u{2028}', '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}',
];

/// Python's `str.isspace()` for one character.
#[must_use]
pub fn is_space(character: char) -> bool {
    // Fast path for ASCII, where only `\t\n\v\f\r` and space count.
    if character.is_ascii() {
        return matches!(
            character,
            '\t' | '\n'
                | '\u{000B}'
                | '\u{000C}'
                | '\r'
                | '\u{001C}'
                | '\u{001D}'
                | '\u{001E}'
                | '\u{001F}'
                | ' '
        );
    }
    PY_WHITESPACE.binary_search(&character).is_ok()
}

/// Python's `str.strip()` with no arguments: drop leading and trailing whitespace.
#[must_use]
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// Python's `"\n".splitlines()` for a single line: a trailing `\r` is removed.
///
/// `std::io::BufRead::lines()` strips `\n` but keeps `\r`; Python's `for line in sys.stdin`
/// (text mode, universal newlines) removes both. Used by the piped/REPL input paths.
#[must_use]
pub fn chomp_line(line: &str) -> &str {
    let without_lf = line.strip_suffix('\n').unwrap_or(line);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

/// Python's `" ".join(text.split())`: collapse every run of Python whitespace into one
/// space and drop the ends. Used by `pybot.py` on every reply (`" ".join(str(reply).split())`).
///
/// Returns a borrow when the text is already normalised (the common case for the corpus,
/// whose replies are stored post-normalisation).
#[must_use]
pub fn collapse_spaces(text: &str) -> std::borrow::Cow<'_, str> {
    if is_normalised(text) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if is_space(character) {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(character);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// True when `text` is already what [`collapse_spaces`] would return — its only whitespace
/// is single U+0020 spaces, with none at either end (so `"hi there"` and `""` are normalised,
/// `"hi\nthere"` and `" hi"` are not).
///
/// This is a cheap fast-path test for the corpus' replies, which `pybot.py` already
/// normalised before recording them.
#[must_use]
pub fn is_normalised(text: &str) -> bool {
    let mut previous_was_space = false;
    for (index, character) in text.char_indices() {
        let space = is_space(character);
        if space {
            let at_end = text.len() == index + character.len_utf8();
            let leading = index == 0;
            if leading || at_end || previous_was_space || character != ' ' {
                return false;
            }
        }
        previous_was_space = space;
    }
    true
}

/// Python's `repr(float)` / `str(float)` for finite and infinite values, matching `CPython`
/// 3.11 (`float_repr_style` is `short`):
///
/// * shortest string that round-trips (`0.1`, `1.5`, `0.3333333333333333`);
/// * always a decimal point or exponent (`300.0`, not `300`);
/// * scientific notation when the decimal exponent is `< -4` or `>= 16`
///   (`1e-05`, `1e+16`, `5e-324`, `2.5e-07`);
/// * the exponent carries a sign and at least two digits (`1e+16`, `1e-05`);
/// * signed zero prints its sign (`-0.0`).
///
/// Rust's `Display for f64` produces shortest digits too, but its tie-breaking differs from
/// `CPython`'s: when two `n`-digit strings both round-trip, `CPython` chooses the one nearest
/// the exact binary value, breaking an exact tie towards the **even** last digit (`dtoa`'s
/// rule), while Rust's `Display` rounds the exact decimal half-up. `112171935477118.12` sits
/// exactly between two such strings (the value is `…118.125`): `CPython` prints
/// `112171935477118.12`, Rust prints `112171935477118.13`. [`shortest_digits`] reproduces
/// `CPython`'s choice; the layout below is unchanged. `NaN` and `inf` are handled defensively
/// (`nan`, `inf`, `-inf`) even though the port never feeds them here.
#[must_use]
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return String::from("nan");
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            String::from("inf")
        } else {
            String::from("-inf")
        };
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            String::from("-0.0")
        } else {
            String::from("0.0")
        };
    }

    // Rust's `Display` yields the shortest round-tripping digits. Two layouts:
    //   * plain decimal: "123", "1.5", "0.0001", "-2.5"
    //   * exponent:      "1e16", "1e-5", "-2.5e-7", "1.7976931348623157e308"
    let rendered = shortest_digits(value);
    let (negative, unsigned) = match rendered.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, rendered.as_str()),
    };

    let mut out = String::with_capacity(rendered.len() + 4);
    if negative {
        out.push('-');
    }

    if let Some((mantissa, exponent)) = split_exponent(unsigned) {
        // Scientific notation: `d.ddde±XX` with at least two exponent digits.
        out.push_str(&py_exponent_form(mantissa, exponent));
        return out;
    }

    let (integer, fraction) = match unsigned.split_once('.') {
        Some((integer, fraction)) => (integer, fraction),
        None => (unsigned, ""),
    };

    let decimal_exponent = if integer == "0" {
        // 0.000123 -> first significant digit is at 10^-4
        let zeros = fraction.bytes().take_while(|byte| *byte == b'0').count();
        -(i32::try_from(zeros).unwrap_or(i32::MAX)) - 1
    } else {
        i32::try_from(integer.len()).unwrap_or(i32::MAX) - 1
    };

    if !(-4..16).contains(&decimal_exponent) {
        // Re-express as d.ddd × 10^exp.
        let (mantissa, exponent) = strip_to_mantissa(integer, fraction);
        out.push_str(&py_exponent_form(&mantissa, exponent));
    } else if fraction.is_empty() {
        out.push_str(integer);
        out.push_str(".0");
    } else {
        out.push_str(integer);
        out.push('.');
        out.push_str(fraction);
    }
    out
}

/// The shortest decimal digits that round-trip to `value`, spelled the way `CPython` spells
/// them (`dtoa` mode 0), in Rust's `Display` *layout* (plain decimal or `d`/`de` exponent form)
/// so the caller can keep rewriting the layout only.
///
/// For every precision `1..=17` the exact decimal value is rounded half-even to that many
/// significant digits; the first precision whose result parses back to `value` wins. Choosing
/// the correctly rounded candidate at each precision reproduces `dtoa`'s tie-break: nearest to
/// the exact value, ties to even.
fn shortest_digits(value: f64) -> String {
    let negative = value.is_sign_negative();
    let magnitude = value.abs();
    let exact = exact_decimal(magnitude);
    for precision in 1..=17 {
        let rounded = round_significant(&exact, precision);
        let literal = rounded.to_literal(negative);
        if literal.parse::<f64>() == Ok(value) {
            return literal;
        }
    }
    // Unreachable for a finite `f64` (17 significant digits always round-trip); the fallback
    // keeps the function total instead of panicking.
    format!("{value}")
}

/// A decimal number as `digits × 10^exponent`, with no leading zeros in `digits` and at most
/// `FLOAT_SCALE_DIGITS + 1` of them (`f64` values are dyadic rationals with at most 767
/// significant decimal digits, and the scale needed is at most ~1 075).
///
#[derive(Clone, Debug)]
struct Decimal {
    digits: Vec<u8>,
    /// The decimal exponent of `digits`: `value ≈ 0.digits × 10^exponent`, i.e. `digits` as an
    /// integer scaled by `10^(exponent - digits.len())`.
    exponent: i32,
}

impl Decimal {
    /// Render as a Rust-`Display`-shaped literal: `123`, `1.5`, `0.0001`, `1e16`, `-2.5e-7`.
    fn to_literal(&self, negative: bool) -> String {
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        let digits = &self.digits;
        // `0.digits × 10^exponent` puts the first significant digit at `10^(exponent - 1)`, so
        // `Decimal { digits: [1], exponent: 1 }` is 1 and `exponent: 0` is 0.1.
        let first_power = self.exponent - 1;
        if first_power >= 16 {
            // Rust prints `1e16`, `1.234e20`.
            out.push(char::from(b'0' + digits.first().copied().unwrap_or(0)));
            if digits.len() > 1 {
                out.push('.');
                out.push_str(&digits_string(digits.get(1..).unwrap_or_default()));
            }
            out.push('e');
            out.push_str(&first_power.to_string());
        } else if first_power < -4 {
            // Rust prints `0.00005`, `1e-7` for `0.0000001`: a negative power below -4 becomes
            // exponent form once the literal would need more than ~4 leading zeros.
            out.push(char::from(b'0' + digits.first().copied().unwrap_or(0)));
            if digits.len() > 1 {
                out.push('.');
                out.push_str(&digits_string(digits.get(1..).unwrap_or_default()));
            }
            out.push('e');
            out.push_str(&first_power.to_string());
        } else if first_power >= 0 {
            let integer_len = usize::try_from(first_power).unwrap_or(0) + 1;
            if digits.len() <= integer_len {
                out.push_str(&digits_string(digits));
                for _ in digits.len()..integer_len {
                    out.push('0');
                }
            } else {
                out.push_str(&digits_string(
                    digits.get(..integer_len).unwrap_or_default(),
                ));
                out.push('.');
                out.push_str(&digits_string(
                    digits.get(integer_len..).unwrap_or_default(),
                ));
            }
        } else {
            out.push_str("0.");
            for _ in 0..(-first_power - 1) {
                out.push('0');
            }
            out.push_str(&digits_string(digits));
        }
        out
    }
}

fn digits_string(digits: &[u8]) -> String {
    digits
        .iter()
        .map(|digit| char::from(b'0' + digit))
        .collect()
}

/// The exact decimal expansion of a positive finite `f64`, as `Decimal`.
///
/// Any `f64` is `mantissa × 2^binary_exponent`. A negative binary exponent is turned into a
/// *finite* decimal by scaling: `m / 2^k == m × 5^k / 10^k`, and `m × 5^k` is an integer. So
/// the digits are the decimal digits of `m × 5^k` and the scale is `-k`; a positive binary
/// exponent stays an integer (`m × 2^k`, built by repeated doubling). Both are exact — no
/// rounding happens here.
fn exact_decimal(value: f64) -> Decimal {
    let bits = value.to_bits();
    let raw_exponent = ((bits >> 52) & 0x7FF) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    // Normal numbers carry an implicit leading bit; subnormals do not.
    let (mantissa, binary_exponent) = if raw_exponent == 0 {
        (u128::from(fraction), -1_074)
    } else {
        (u128::from(fraction | (1u64 << 52)), raw_exponent - 1075)
    };
    if mantissa == 0 {
        return Decimal {
            digits: vec![0],
            exponent: 1,
        };
    }

    // Built least-significant-first (cheap to multiply), then stored most-significant-first.
    let mut digits = decimal_digits(mantissa);
    if binary_exponent >= 0 {
        for _ in 0..binary_exponent {
            multiply_in_place(&mut digits, 2);
        }
        trim_trailing_zeros(&mut digits);
        let digits_len = i32::try_from(digits.len()).unwrap_or(i32::MAX);
        digits.reverse();
        Decimal {
            digits,
            exponent: digits_len,
        }
    } else {
        let shift = binary_exponent.unsigned_abs();
        for _ in 0..shift {
            multiply_in_place(&mut digits, 5);
        }
        trim_trailing_zeros(&mut digits);
        let digits_len = i32::try_from(digits.len()).unwrap_or(i32::MAX);
        digits.reverse();
        // `value = digits × 10^-shift`, and `digits` has `digits_len` places.
        Decimal {
            digits,
            exponent: digits_len - i32::try_from(shift).unwrap_or(i32::MAX),
        }
    }
}

/// Multiply a little-endian decimal digit vector by a small factor, in place.
fn multiply_in_place(digits: &mut Vec<u8>, factor: u8) {
    let mut carry = 0u16;
    for digit in digits.iter_mut() {
        let product = u16::from(*digit) * u16::from(factor) + carry;
        *digit = (product % 10) as u8;
        carry = product / 10;
    }
    while carry > 0 {
        digits.push((carry % 10) as u8);
        carry /= 10;
    }
}

/// Remove trailing zeros (most significant end), returning how many were removed.
fn trim_trailing_zeros(digits: &mut Vec<u8>) -> i32 {
    let mut removed = 0;
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
        removed += 1;
    }
    removed
}

/// Decimal digits of `value` (which fits in 16), least significant first.
fn decimal_digits(mut value: u128) -> Vec<u8> {
    let mut digits = Vec::with_capacity(16);
    while value > 0 {
        digits.push((value % 10) as u8);
        value /= 10;
    }
    digits
}

/// Round `value` to `precision` significant digits, half to even (`CPython`'s `dtoa` rule).
fn round_significant(value: &Decimal, precision: i32) -> Decimal {
    let available = i32::try_from(value.digits.len()).unwrap_or(i32::MAX);
    if precision >= available {
        return value.clone();
    }
    let keep = usize::try_from(precision).unwrap_or(0);
    let mut kept: Vec<u8> = value
        .digits
        .get(..keep)
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    let next = value.digits.get(keep).copied().unwrap_or(0);
    let rest_nonzero = value
        .digits
        .iter()
        .skip(keep.saturating_add(1))
        .any(|digit| *digit != 0);
    // Nearest; an exact tie goes to the even last digit (this is where CPython and Rust's
    // `Display` disagree, e.g. `112171935477118.125` at 17 digits).
    let round_up = next > 5
        || (next == 5 && (rest_nonzero || kept.last().is_some_and(|digit| digit % 2 == 1)));
    if round_up {
        let mut index = kept.len();
        loop {
            if index == 0 {
                kept.insert(0, 1);
                break;
            }
            index -= 1;
            let digit = kept.get(index).copied().unwrap_or(0);
            if digit == 9 {
                if let Some(slot) = kept.get_mut(index) {
                    *slot = 0;
                }
            } else {
                if let Some(slot) = kept.get_mut(index) {
                    *slot = digit + 1;
                }
                break;
            }
        }
    }
    while kept.len() > 1 && kept.last() == Some(&0) {
        kept.pop();
    }
    if kept.is_empty() {
        kept.push(0);
    }
    Decimal {
        exponent: value.exponent,
        digits: kept,
    }
}

/// Split `"1.25e-7"`-style Rust output into `("1.25", -7)`; `None` for plain decimals.
fn split_exponent(unsigned: &str) -> Option<(&str, i32)> {
    let (mantissa, exponent) = unsigned.split_once('e')?;
    let exponent: i32 = exponent.parse().ok()?;
    Some((mantissa, exponent))
}

/// Render `mantissa × 10^exponent` the way `CPython` does: `1.5e+16`, `5e-324`, `1e-05`.
fn py_exponent_form(mantissa: &str, exponent: i32) -> String {
    let sign = if exponent < 0 { '-' } else { '+' };
    let magnitude = exponent.unsigned_abs();
    if magnitude < 10 {
        format!("{mantissa}e{sign}0{magnitude}")
    } else {
        format!("{mantissa}e{sign}{magnitude}")
    }
}

/// Convert a plain decimal (`"123"`, `"0.000123"`, `"123.45"`) to `(mantissa, exponent)`
/// with `mantissa` in `d.ddd` or single-`d` form.
fn strip_to_mantissa(integer: &str, fraction: &str) -> (String, i32) {
    let digits: String = format!("{integer}{fraction}");
    let trimmed_leading = digits.trim_start_matches('0');
    let significant: &str = if trimmed_leading.is_empty() {
        "0"
    } else {
        trimmed_leading
    };
    let significant = significant.trim_end_matches('0');
    let significant = if significant.is_empty() {
        "0"
    } else {
        significant
    };

    let leading_zeros = digits.len() - trimmed_leading.len();
    let exponent = if integer == "0" {
        -(i32::try_from(leading_zeros).unwrap_or(i32::MAX))
    } else {
        i32::try_from(integer.len()).unwrap_or(i32::MAX) - 1
    };

    let mut mantissa = String::with_capacity(significant.len() + 1);
    let mut characters = significant.chars();
    match characters.next() {
        Some(first) => mantissa.push(first),
        None => mantissa.push('0'),
    }
    let rest: String = characters.collect();
    if !rest.is_empty() {
        mantissa.push('.');
        mantissa.push_str(&rest);
    }
    (mantissa, exponent)
}

/// Python's `repr(int)` is `str(int)`; this exists so call sites read symmetrically with
/// [`float_repr`] and later phases can swap in a bignum if ever needed.
#[must_use]
pub fn int_repr(value: i64) -> String {
    value.to_string()
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

    // ------------------------------------------------------------------ whitespace

    #[test]
    fn the_whitespace_table_is_sorted_unique_and_has_29_entries() {
        assert_eq!(PY_WHITESPACE.len(), 29);
        for pair in PY_WHITESPACE.windows(2) {
            assert!(pair[0] < pair[1], "table must be strictly increasing");
        }
    }

    #[test]
    fn is_space_matches_python_for_every_code_point_we_can_afford_to_scan() {
        // CPython's exact set, restated independently of PY_WHITESPACE.
        const REFERENCE: &[u32] = &[
            0x9, 0xA, 0xB, 0xC, 0xD, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x85, 0xA0, 0x1680, 0x2000,
            0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200A, 0x2028,
            0x2029, 0x202F, 0x205F, 0x3000,
        ];
        let reference_set: std::collections::BTreeSet<u32> = REFERENCE.iter().copied().collect();
        // Scan the whole BMP plus the ranges where Python whitespace lives above it.
        let mut code_points: Vec<u32> = (0..=0xFFFF).collect();
        code_points.extend(0x1_0000..=0x10_FFFF);
        for code_point in code_points {
            let Some(character) = char::from_u32(code_point) else {
                continue;
            };
            let expected = reference_set.contains(&code_point);
            assert_eq!(is_space(character), expected, "U+{code_point:04X}");
        }
    }

    #[test]
    fn rust_trim_and_python_strip_really_differ_on_the_control_characters() {
        // Rust's `char::is_whitespace` does not include U+001C..U+001F; Python's does.
        let text = "\u{1c}\u{1d}\u{1e}\u{1f}x";
        assert_eq!(text.trim(), text, "Rust's trim keeps \\x1c-\\x1f");
        assert_eq!(strip(text), "x", "Python's strip removes them");
        assert_eq!(strip("x"), "x");
        assert_eq!(strip(""), "");
        assert_eq!(strip("\u{3000}\u{a0}\u{85}"), "");
        assert_eq!("\u{85}x".trim(), "x", "U+0085 is whitespace for both");
    }

    #[test]
    fn chomp_line_removes_universal_newlines_like_python_text_mode() {
        assert_eq!(chomp_line("hi\n"), "hi");
        assert_eq!(chomp_line("hi\r\n"), "hi");
        assert_eq!(chomp_line("hi\r"), "hi");
        assert_eq!(chomp_line("hi"), "hi");
        assert_eq!(chomp_line("\n"), "");
        assert_eq!(chomp_line(""), "");
        assert_eq!(chomp_line("a\rb"), "a\rb", "interior CR is kept");
        assert_eq!(chomp_line("hi\n\n"), "hi\n", "only one newline");
    }

    // ------------------------------------------------------------------ collapse_spaces

    #[test]
    fn collapse_spaces_is_exactly_python_join_split() {
        let cases = [
            ("", ""),
            (" ", ""),
            ("  hello   world  ", "hello world"),
            ("\thello\tworld", "hello world"),
            ("line one\nline two", "line one line two"),
            ("\u{1c}a\u{1c}b\u{1c}", "a b"),
            ("a\u{a0}b", "a b"),
            ("a\u{2003}\u{2003}b", "a b"),
            ("word ", "word"),
            ("no\u{200b}break", "no\u{200b}break"),
        ];
        for (input, expected) in cases {
            assert_eq!(collapse_spaces(input), expected, "{input:?}");
        }
    }

    #[test]
    fn collapse_spaces_borrows_untouched_text() {
        match collapse_spaces("Nothing \"to see\" here") {
            std::borrow::Cow::Borrowed(text) => assert_eq!(text, "Nothing \"to see\" here"),
            std::borrow::Cow::Owned(text) => panic!("unexpected allocation: {text:?}"),
        }
        match collapse_spaces("double  space") {
            std::borrow::Cow::Owned(text) => assert_eq!(text, "double space"),
            std::borrow::Cow::Borrowed(text) => panic!("expected an owned copy: {text:?}"),
        }
    }

    #[test]
    fn collapse_spaces_keeps_corpus_replies_byte_identical() {
        // Every reply in the golden corpus is already `" ".join(reply.split())`.
        let reply = "Half fill the kettle with equal parts water and white vinegar, or a \
                     sachet of citric acid, boil it and leave it half an hour.";
        assert_eq!(collapse_spaces(reply), reply);
        // A naive `split_whitespace().join(" ")` differs from Python on \x1c; ours must not.
        assert_eq!(collapse_spaces("a\u{1c}b"), "a b");
    }

    #[test]
    fn is_normalised_agrees_with_collapse_spaces_on_random_text() {
        // Deterministic pseudo-fuzz over a nasty alphabet.
        let alphabet = [
            'a', 'b', ' ', ' ', '\t', '\n', '\u{1c}', '\u{a0}', '\u{3000}', '\u{200b}',
        ];
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..5_000 {
            let length = usize::try_from(next() % 12).unwrap();
            let text: String = (0..length)
                .map(|_| {
                    let index = usize::try_from(next() % alphabet.len() as u64).unwrap();
                    alphabet[index]
                })
                .collect();
            let collapsed = collapse_spaces(&text);
            assert_eq!(is_normalised(&text), collapsed == text, "{text:?}");
        }
    }

    // ------------------------------------------------------------------ float repr

    #[test]
    fn float_repr_matches_cpython_for_hand_picked_values() {
        let cases: [(f64, &str); 24] = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (-1.0, "-1.0"),
            (300.0, "300.0"),
            (-1000.0, "-1000.0"),
            (0.5, "0.5"),
            (1.5, "1.5"),
            (-999.5, "-999.5"),
            (0.1, "0.1"),
            (1.0 / 3.0, "0.3333333333333333"),
            (2.0 / 3.0, "0.6666666666666666"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1e-5, "1e-05"),
            (-2.5e-7, "-2.5e-07"),
            (1_234_567_890_123_456.0, "1234567890123456.0"),
            (1e15_f64, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (1e300, "1e+300"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
        ];
        for (value, expected) in cases {
            assert_eq!(float_repr(value), expected, "{value:e}");
        }
    }

    #[test]
    fn float_repr_breaks_shortest_representation_ties_like_cpython() {
        // `112171935477118.12` is exactly `…118.125`, so both a 17-digit `…12` and `…13`
        // round-trip. CPython's `dtoa` picks nearest-to-exact then even; Rust's `Display`
        // rounds the exact decimal half-up and prints `…13`. The whole-report differential
        // compares `repr`-shaped strings, so the port must follow CPython.
        assert_eq!(float_repr(112_171_935_477_118.12), "112171935477118.12");
        // The same tie at a smaller magnitude: `0.5` at one digit is a tie, and `0` is even.
        assert_eq!(float_repr(0.5), "0.5");
        // A handful of the "17 significant digits" family that the harness samples.
        // `…118.13` is the *same* double as `…118.12`, so it prints the same way.
        for (value, expected) in [
            (112_171_935_477_118.13, "112171935477118.12"),
            (112_171_935_477_118.14, "112171935477118.14"),
            (900_719_925_474_099.2, "900719925474099.2"),
        ] {
            assert_eq!(float_repr(value), expected);
        }
    }

    #[test]
    fn rounded_digits_never_lose_a_round_trip() {
        // Every power-of-two fraction and a sweep of mantissas: the emitted literal must parse
        // back to the same bit pattern, which is the property the whole function exists for.
        let mut values = Vec::new();
        for exponent in -1_070..=1_000i32 {
            values.push(2.0_f64.powi(exponent));
            values.push(-2.0_f64.powi(exponent));
            values.push(2.0_f64.powi(exponent) * 3.0);
        }
        for step in 0..2_000u32 {
            values.push(f64::from(step) / 7.0);
        }
        for value in values {
            if value == 0.0 {
                continue;
            }
            let rendered = float_repr(value);
            let parsed: f64 = rendered.parse().expect("own output parses");
            assert_eq!(
                parsed.to_bits(),
                value.to_bits(),
                "{value:?} rendered as {rendered}"
            );
        }
    }

    #[test]
    fn float_repr_handles_infinity_and_nan_defensively() {
        assert_eq!(float_repr(f64::INFINITY), "inf");
        assert_eq!(float_repr(f64::NEG_INFINITY), "-inf");
        assert_eq!(float_repr(f64::NAN), "nan");
    }

    #[test]
    fn float_repr_round_trips_through_rust_parsing() {
        let values = [
            0.1,
            -999.5,
            1.0 / 3.0,
            1e16,
            1e-5,
            5e-324,
            f64::MAX,
            f64::MIN_POSITIVE,
            1_234_567_890_123_456.0,
            -2.5e-7,
        ];
        for value in values {
            let rendered = float_repr(value);
            let parsed: f64 = rendered.parse().unwrap();
            assert_eq!(parsed.to_bits(), value.to_bits(), "{rendered}");
        }
    }

    #[test]
    fn float_repr_always_has_a_point_or_an_exponent() {
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        for _ in 0..2_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state);
            if !value.is_finite() {
                continue;
            }
            let rendered = float_repr(value);
            assert!(
                rendered.contains('.') || rendered.contains('e'),
                "{value:e} rendered as {rendered}"
            );
            let parsed: f64 = rendered.parse().unwrap();
            assert_eq!(parsed.to_bits(), value.to_bits(), "{rendered}");
        }
    }

    #[test]
    fn float_repr_uses_two_exponent_digits_at_least() {
        assert_eq!(float_repr(1e-5), "1e-05");
        assert_eq!(float_repr(1e5), "100000.0");
        assert_eq!(float_repr(1e-4), "0.0001");
        assert_eq!(float_repr(1e20), "1e+20");
        assert_eq!(float_repr(-1e-9), "-1e-09");
    }

    #[test]
    fn int_repr_is_plain_decimal() {
        assert_eq!(int_repr(0), "0");
        assert_eq!(int_repr(-999), "-999");
        assert_eq!(int_repr(i64::MAX), "9223372036854775807");
    }
}
