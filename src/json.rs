//! A small, strict, allocation-light JSON reader/writer — std only.
//!
//! The differential harness reads `tests/golden/corpus.jsonl` (one JSON object per line,
//! 3.9 MB, UTF-8, no escapes beyond `\"`, `\\`, `\n`, `\t`, `\uXXXX`) and the manifest.
//! Fase 3 will also read exported knowledge tables, so the reader is written as a reusable
//! module rather than a corpus-specific hack.
//!
//! Semantics match `CPython`'s `json` module where it matters for us:
//!
//! * numbers keep *which kind they were* (an `int` stays exact `i64`/`u64`/`f64` by need;
//!   a float keeps its IEEE bits) — the corpus stores priorities as `-999.5` and `300` and
//!   `-1000.0`, and `-1000` must stay an int;
//! * `\uXXXX` surrogates are combined into one scalar; a lone surrogate is an error (Python
//!   accepts lone surrogates when reading into `str`, but the corpus never has them, and a
//!   strict reader is the safer foundation — documented divergence);
//! * the writer emits `true`/`false`/`null`, escapes `"`/`\\`/control characters and uses
//!   Python's `repr` for floats ([`crate::pyutil::float_repr`]) and `str` for integers,
//!   so digests and reports serialise identically to Python tooling.
//!
//! No recursion on untrusted depth without bound: nesting is capped at
//! [`MAX_DEPTH`](self::MAX_DEPTH) so a hostile file cannot overflow the stack.

use std::fmt::Write as _;

use crate::error::{Error, Result};
use crate::pyutil;

/// Maximum nesting depth the parser accepts. The corpus nests 4 levels; 64 leaves room for
/// later phases while making stack overflow impossible to reach from a file.
pub const MAX_DEPTH: usize = 64;

/// A parsed JSON value.
///
/// Integers beyond `i64` fall back to `f64` (JSON numbers are unbounded; the corpus never
/// needs more, and saturating would silently corrupt data).
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// An integer literal that fits `i64`.
    Int(i64),
    /// A number literal with a fraction or exponent, or an integer too large for `i64`.
    Float(f64),
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Json>),
    /// An object, in document order (Python dicts preserve insertion order; reports rely on
    /// it only for readability, never for decisions).
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The value as a string, if it is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The value as a bool, if it is one.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as an `i64` int, if it is an integer literal in range.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as an `f64` (ints convert exactly like Python's `float(int)` for our sizes).
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            Self::Int(value) => Some(*value as f64),
            _ => None,
        }
    }

    /// One object field by name (first occurrence, like `dict` lookup after `json.loads`).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Object(fields) => fields.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value as an array slice, if it is one.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The value as object fields, if it is one.
    #[must_use]
    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Self::Object(fields) => Some(fields),
            _ => None,
        }
    }

    /// A JSON type name, for error messages.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "string",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
        }
    }
}

/// Parse one complete JSON document from UTF-8 bytes.
///
/// Trailing whitespace is allowed; trailing *content* is an error (strict, like
/// `json.loads` with no extra data).
pub fn parse(bytes: &[u8]) -> Result<Json> {
    let text = std::str::from_utf8(bytes).map_err(|error| Error::InvalidUtf8 {
        valid_up_to: error.valid_up_to(),
    })?;
    let mut parser = Parser {
        bytes: text.as_bytes(),
        position: 0,
    };
    let value = parser.value(0)?;
    parser.skip_whitespace();
    if parser.position != parser.bytes.len() {
        return Err(Error::json(
            parser.position,
            "trailing data after the JSON value",
        ));
    }
    Ok(value)
}

/// Parse one JSON document from a string.
pub fn parse_str(text: &str) -> Result<Json> {
    parse(text.as_bytes())
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn expect(&mut self, byte: u8, what: &'static str) -> Result<()> {
        match self.bump() {
            Some(found) if found == byte => Ok(()),
            _ => Err(Error::json(self.position, what)),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json> {
        if depth > MAX_DEPTH {
            return Err(Error::json(self.position, "nesting depth exceeded"));
        }
        self.skip_whitespace();
        match self.peek() {
            None => Err(Error::json(self.position, "unexpected end of input")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => {
                self.literal(b"true")?;
                Ok(Json::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(Json::Bool(false))
            }
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(Json::Null)
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(Error::json(self.position, "unexpected character")),
        }
    }

    fn literal(&mut self, expected: &[u8]) -> Result<()> {
        for &byte in expected {
            self.expect(byte, "invalid literal")?;
        }
        Ok(())
    }

    fn object(&mut self, depth: usize) -> Result<Json> {
        self.expect(b'{', "expected '{'")?;
        let mut fields = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Json::Object(fields));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(b':', "expected ':'")?;
            let value = self.value(depth + 1)?;
            fields.push((key, value));
            self.skip_whitespace();
            match self.bump() {
                Some(b',') => {}
                Some(b'}') => return Ok(Json::Object(fields)),
                _ => return Err(Error::json(self.position, "expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json> {
        self.expect(b'[', "expected '['")?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_whitespace();
            match self.bump() {
                Some(b',') => {}
                Some(b']') => return Ok(Json::Array(items)),
                _ => return Err(Error::json(self.position, "expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<String> {
        self.expect(b'"', "expected '\"'")?;
        let mut out = String::new();
        loop {
            let start = self.position;
            // Fast path: copy the run of plain bytes up to the next quote/escape/control.
            while let Some(byte) = self.peek() {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.position += 1;
            }
            if self.position > start {
                let run = self
                    .bytes
                    .get(start..self.position)
                    .ok_or_else(|| Error::json(start, "invalid string slice"))?;
                let run = std::str::from_utf8(run)
                    .map_err(|_| Error::json(start, "string is not valid UTF-8"))?;
                out.push_str(run);
            }
            match self.bump() {
                Some(b'"') => return Ok(out),
                Some(b'\\') => self.escape(&mut out)?,
                Some(byte) if byte < 0x20 => {
                    return Err(Error::json(
                        self.position - 1,
                        "raw control character in string",
                    ))
                }
                Some(_) => return Err(Error::json(self.position, "unexpected byte in string")),
                None => return Err(Error::json(self.position, "unterminated string")),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<()> {
        let escape = self
            .bump()
            .ok_or_else(|| Error::json(self.position, "unterminated escape"))?;
        let simple = match escape {
            b'"' => Some('"'),
            b'\\' => Some('\\'),
            b'/' => Some('/'),
            b'b' => Some('\u{0008}'),
            b'f' => Some('\u{000C}'),
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            _ => None,
        };
        if let Some(character) = simple {
            out.push(character);
            return Ok(());
        }
        if escape != b'u' {
            return Err(Error::json(self.position - 1, "invalid escape sequence"));
        }
        let first = self.hex4()?;
        match first {
            // High surrogate: a low surrogate must follow, or the pair is rejected.
            0xD800..=0xDBFF => {
                self.expect(b'\\', "expected '\\' before a low surrogate")?;
                self.expect(b'u', "expected 'u' before a low surrogate")?;
                let second = self.hex4()?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err(Error::json(self.position, "invalid low surrogate"));
                }
                let high = u32::from(first - 0xD800);
                let low = u32::from(second - 0xDC00);
                let combined = 0x1_0000 + (high << 10) + low;
                let character = char::from_u32(combined)
                    .ok_or_else(|| Error::json(self.position, "invalid surrogate pair"))?;
                out.push(character);
                Ok(())
            }
            0xDC00..=0xDFFF => Err(Error::json(self.position, "lone low surrogate")),
            _ => {
                let character = char::from_u32(u32::from(first))
                    .ok_or_else(|| Error::json(self.position, "invalid \\u escape"))?;
                out.push(character);
                Ok(())
            }
        }
    }

    fn hex4(&mut self) -> Result<u16> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            let byte = self
                .bump()
                .ok_or_else(|| Error::json(self.position, "truncated \\u escape"))?;
            let digit = match byte {
                b'0'..=b'9' => u16::from(byte - b'0'),
                b'a'..=b'f' => u16::from(byte - b'a') + 10,
                b'A'..=b'F' => u16::from(byte - b'A') + 10,
                _ => {
                    return Err(Error::json(
                        self.position - 1,
                        "non-hex digit in \\u escape",
                    ))
                }
            };
            value = value
                .checked_mul(16)
                .and_then(|current| current.checked_add(digit))
                .ok_or_else(|| Error::json(self.position, "\\u escape overflow"))?;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json> {
        let start = self.position;
        let mut is_float = false;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        // Integer part: 0 | [1-9][0-9]*   (no leading zeros, per the grammar)
        match self.peek() {
            Some(b'0') => {
                self.position += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(Error::json(self.position, "leading zero in number"));
                }
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return Err(Error::json(self.position, "invalid number")),
        }
        if self.peek() == Some(b'.') {
            is_float = true;
            self.position += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(Error::json(self.position, "expected digit after '.'"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(Error::json(self.position, "expected digit in exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        let literal = self
            .bytes
            .get(start..self.position)
            .ok_or_else(|| Error::json(start, "invalid number slice"))?;
        let literal = std::str::from_utf8(literal)
            .map_err(|_| Error::json(start, "number is not valid UTF-8"))?;
        if !is_float {
            if let Ok(integer) = literal.parse::<i64>() {
                return Ok(Json::Int(integer));
            }
        }
        let value: f64 = literal
            .parse()
            .map_err(|_| Error::json(start, "number out of range"))?;
        Ok(Json::Float(value))
    }
}

/// Serialise a value compactly (`separators=(",", ":")`), like Python's
/// `json.dumps(value, ensure_ascii=False, separators=(",", ":"))`.
#[must_use]
pub fn to_string(value: &Json) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

/// Write a value into an existing buffer (avoids re-allocating in loops).
pub fn write_value(out: &mut String, value: &Json) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(integer) => {
            let _ = write!(out, "{integer}");
        }
        Json::Float(number) => {
            // Python's json prints NaN/Infinity for those values; the port never produces
            // them, and `float_repr` handles them defensively anyway.
            out.push_str(&pyutil::float_repr(*number));
        }
        Json::Str(text) => write_string(out, text),
        Json::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Json::Object(fields) => {
            out.push('{');
            for (index, (key, item)) in fields.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(out, item);
            }
            out.push('}');
        }
    }
}

/// Write a JSON string literal, escaping exactly what the JSON grammar requires.
pub fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            control if control < '\u{0020}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// Build an object from borrowed pairs (convenience for report construction).
#[must_use]
pub fn object(fields: impl IntoIterator<Item = (&'static str, Json)>) -> Json {
    Json::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

/// `Json` from anything string-like.
#[must_use]
pub fn string(text: impl Into<String>) -> Json {
    Json::Str(text.into())
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

    fn parse_ok(text: &str) -> Json {
        parse_str(text).unwrap()
    }

    fn parse_err(text: &str) -> Error {
        parse_str(text).expect_err("expected a parse error")
    }

    // ------------------------------------------------------------------ scalars

    #[test]
    fn parses_scalars_with_the_right_kinds() {
        assert_eq!(parse_ok("null"), Json::Null);
        assert_eq!(parse_ok("true"), Json::Bool(true));
        assert_eq!(parse_ok("false"), Json::Bool(false));
        assert_eq!(parse_ok("0"), Json::Int(0));
        assert_eq!(parse_ok("-0"), Json::Int(0));
        assert_eq!(parse_ok("300"), Json::Int(300));
        assert_eq!(parse_ok("-1000"), Json::Int(-1000));
        assert_eq!(parse_ok("9000"), Json::Int(9000));
        assert_eq!(parse_ok("\"\""), Json::Str(String::new()));
        assert_eq!(parse_ok("\"hi\""), Json::Str("hi".to_string()));
    }

    #[test]
    fn keeps_float_markers_like_python_json() {
        // `-999.5`, `-1000.0` and `0.0` are floats; `300` is an int.
        assert_eq!(parse_ok("-999.5"), Json::Float(-999.5));
        assert_eq!(parse_ok("-1000.0"), Json::Float(-1000.0));
        assert_eq!(parse_ok("0.0"), Json::Float(0.0));
        assert_eq!(parse_ok("1e3"), Json::Float(1000.0));
        assert_eq!(parse_ok("1E+3"), Json::Float(1000.0));
        assert_eq!(parse_ok("1e-3"), Json::Float(0.001));
        assert_eq!(parse_ok("1.5e2"), Json::Float(150.0));
        match parse_ok("-0.0") {
            Json::Float(value) => assert!(value.is_sign_negative()),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn huge_integers_degrade_to_floats_instead_of_corrupting() {
        let value = parse_ok("123456789012345678901234567890");
        match value {
            Json::Float(number) => assert!(number > 1e29),
            other => panic!("expected a float, got {other:?}"),
        }
        assert_eq!(parse_ok("9223372036854775807"), Json::Int(i64::MAX));
        assert_eq!(parse_ok("-9223372036854775808"), Json::Int(i64::MIN));
        match parse_ok("9223372036854775808") {
            Json::Float(number) => assert!(number > 9.2e18),
            other => panic!("expected a float, got {other:?}"),
        }
    }

    #[test]
    fn accessors_are_total() {
        assert_eq!(Json::Null.as_str(), None);
        assert_eq!(Json::Null.as_bool(), None);
        assert_eq!(Json::Null.as_i64(), None);
        assert_eq!(Json::Null.as_f64(), None);
        assert_eq!(Json::Null.as_array(), None);
        assert_eq!(Json::Null.as_object(), None);
        assert_eq!(Json::Null.get("x"), None);
        assert_eq!(Json::Int(7).as_f64(), Some(7.0));
        assert_eq!(Json::Int(7).as_i64(), Some(7));
        assert_eq!(Json::Float(7.5).as_i64(), None);
        assert_eq!(Json::Bool(true).as_bool(), Some(true));
        assert_eq!(Json::Str("s".into()).type_name(), "string");
        assert_eq!(Json::Array(vec![]).type_name(), "array");
        assert_eq!(Json::Object(vec![]).type_name(), "object");
    }

    // ------------------------------------------------------------------ strings

    #[test]
    fn parses_every_escape_sequence() {
        assert_eq!(
            parse_ok(r#""a\"b\\c\/d\be\ff\ng\rh\ti""#),
            Json::Str("a\"b\\c/d\u{8}e\u{c}f\ng\rh\ti".to_string())
        );
    }

    #[test]
    fn parses_unicode_escapes_including_surrogate_pairs() {
        assert_eq!(parse_ok(r#""\u00e9""#), Json::Str("é".to_string()));
        assert_eq!(parse_ok(r#""\u00C9""#), Json::Str("É".to_string()));
        assert_eq!(parse_ok(r#""\ud83d\ude00""#), Json::Str("😀".to_string()));
        assert_eq!(parse_ok(r#""\u0000""#), Json::Str("\0".to_string()));
        assert_eq!(parse_ok(r#""\uFFFD""#), Json::Str("\u{FFFD}".to_string()));
    }

    #[test]
    fn rejects_bad_escapes_and_lone_surrogates() {
        assert!(matches!(parse_err(r#""\ud83d""#), Error::Json { .. }));
        assert!(matches!(parse_err(r#""\ud83dx""#), Error::Json { .. }));
        assert!(matches!(parse_err(r#""\ude00""#), Error::Json { .. }));
        assert!(matches!(parse_err(r#""\ud83d\u0041""#), Error::Json { .. }));
        assert!(matches!(parse_err(r#""\q""#), Error::Json { .. }));
        assert!(matches!(parse_err(r#""\u12""#), Error::Json { .. }));
        assert!(matches!(parse_err(r#""\uZZZZ""#), Error::Json { .. }));
        assert!(
            matches!(parse_err("\"a\u{1}b\""), Error::Json { .. }),
            "raw control"
        );
    }

    #[test]
    fn strings_round_trip_through_writer_and_parser() {
        let texts = [
            "",
            "plain",
            "quotes \" and \\ backslash",
            "newline\nand\ttab",
            "unicode: é, ã, 日本, 😀",
            "control \u{1} \u{1f} \u{7f}",
            "nul \0 inside",
        ];
        for text in texts {
            let mut buffer = String::new();
            write_string(&mut buffer, text);
            assert_eq!(parse_ok(&buffer), Json::Str(text.to_string()), "{buffer}");
        }
    }

    // ------------------------------------------------------------------ containers

    #[test]
    fn parses_nested_objects_and_arrays_in_document_order() {
        let value = parse_ok(r#"{"a":1,"b":[true,null,"x"],"c":{"d":-999.5}}"#);
        assert_eq!(value.get("a"), Some(&Json::Int(1)));
        assert_eq!(
            value.get("b"),
            Some(&Json::Array(vec![
                Json::Bool(true),
                Json::Null,
                Json::Str("x".into())
            ]))
        );
        assert_eq!(
            value.get("c").and_then(|inner| inner.get("d")),
            Some(&Json::Float(-999.5))
        );
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(keys, vec!["a", "b", "c"], "insertion order is preserved");
    }

    #[test]
    fn accepts_whitespace_around_everything() {
        assert!(parse_ok(" \n\t{ \"a\" : [ 1 , 2 ] } \r\n")
            .get("a")
            .is_some());
        assert_eq!(parse_ok("[]"), Json::Array(vec![]));
        assert_eq!(parse_ok("{}"), Json::Object(vec![]));
        assert_eq!(parse_ok(" 1 "), Json::Int(1));
    }

    // ------------------------------------------------------------------ errors

    #[test]
    fn rejects_malformed_documents_with_offsets() {
        let cases = [
            "",
            " ",
            "{",
            "[1,",
            "{\"a\"}",
            "{\"a\":}",
            "{\"a\":1,}",
            "[1,]",
            "tru",
            "TRUE",
            "nulll",
            "\"unterminated",
            "1 2",
            "{} {}",
            "01",
            "-",
            "-01",
            "1.",
            "1.e5",
            "1e",
            "1e+",
            "+1",
            ".5",
            "{1:2}",
            "[,]",
        ];
        for case in cases {
            let error = parse_str(case).expect_err(case);
            match error {
                Error::Json { offset, .. } => assert!(offset <= case.len(), "{case:?}"),
                other => panic!("{case:?} produced {other:?}"),
            }
        }
    }

    #[test]
    fn error_offsets_point_at_the_problem() {
        match parse_str("{\"a\": 1, x}").expect_err("bad") {
            Error::Json { offset, message } => {
                assert_eq!(offset, 10, "the offset points just past the offending byte");
                assert_eq!(message, "expected '\"'");
            }
            other => panic!("{other:?}"),
        }
        match parse_str("1 2").expect_err("bad") {
            Error::Json { offset, .. } => assert_eq!(offset, 2),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_utf8_with_the_offset() {
        match parse(&[b'"', 0xFF, b'"']) {
            Err(Error::InvalidUtf8 { valid_up_to }) => assert_eq!(valid_up_to, 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn depth_is_capped_so_a_hostile_file_cannot_blow_the_stack() {
        let mut deep = String::new();
        for _ in 0..=MAX_DEPTH + 1 {
            deep.push('[');
        }
        deep.push('0');
        for _ in 0..=MAX_DEPTH + 1 {
            deep.push(']');
        }
        match parse_str(&deep).expect_err("too deep") {
            Error::Json { message, .. } => assert_eq!(message, "nesting depth exceeded"),
            other => panic!("{other:?}"),
        }
        // One less is fine.
        let mut shallow = String::new();
        for _ in 0..MAX_DEPTH {
            shallow.push('[');
        }
        shallow.push('0');
        for _ in 0..MAX_DEPTH {
            shallow.push(']');
        }
        assert!(parse_str(&shallow).is_ok());
    }

    // ------------------------------------------------------------------ writer

    #[test]
    fn writer_emits_python_compatible_compact_json() {
        let value = Json::Object(vec![
            ("module".into(), Json::Null),
            ("priority".into(), Json::Float(-1000.0)),
            ("index".into(), Json::Int(3)),
            ("ok".into(), Json::Bool(false)),
            ("name".into(), Json::Str("café \"x\"".into())),
        ]);
        assert_eq!(
            to_string(&value),
            r#"{"module":null,"priority":-1000.0,"index":3,"ok":false,"name":"café \"x\""}"#
        );
    }

    #[test]
    fn writer_escapes_controls_as_four_hex_digits() {
        let mut buffer = String::new();
        write_string(&mut buffer, "\u{0}\u{1}\u{1f}\u{7f}\u{9f}");
        assert_eq!(buffer, "\"\\u0000\\u0001\\u001f\u{7f}\u{9f}\"");
    }

    #[test]
    fn writer_and_parser_round_trip_a_realistic_corpus_line() {
        let line = r#"{"session":"s0000","turn":0,"input":"hello","reply":"Hi there — what do you need?","module":"chat","candidates":[{"module":"chat","priority":300},{"module":"greeting","priority":-1000}],"memory_keys":["a","b"],"memory_digest":"bdb11cff87ee24bf42f8f3406e272f18a07b4b8f","memory":{"user":{}}}"#;
        let parsed = parse_ok(line);
        let rendered = to_string(&parsed);
        assert_eq!(
            rendered, line,
            "a canonical corpus line round-trips byte-identically"
        );
    }

    #[test]
    fn object_and_string_helpers_build_what_they_say() {
        let built = object([("a", string("x")), ("b", Json::Int(1))]);
        assert_eq!(to_string(&built), r#"{"a":"x","b":1}"#);
    }
}
