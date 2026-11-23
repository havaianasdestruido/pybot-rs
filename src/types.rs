//! Central types shared by the whole crate: [`Turn`], [`Session`], [`Reply`], [`ModuleId`],
//! [`ModuleName`], [`Priority`], [`SessionId`].
//!
//! Two design decisions come straight from GOAL.md:
//!
//! * **`ModuleId` is a small integer assigned statically** ([`modules::registry`]) so no
//!   `&'static str` comparisons and no `HashMap<&str, _>` ever decide output ordering; the id
//!   order is the canonical `(-priority, name)` order of `bot/runtime.py`.
//! * **`Priority` mirrors Python's heterogeneous `PRIORITY` numbers**: modules declare
//!   `int` or `float` (`-999.5` exists, and `0.0` must stay distinguishable from `-0.0`).
//!   It is parsed from the Python module table with Python's *exact* semantics, including
//!   "`bool` or non-number → 0" and "`nan` → 0".

use std::borrow::Cow;
use std::fmt;

/// Identifier of a registered module: an index into [`crate::modules::registry`].
///
/// Ids are dense, `u16`-sized (the port has ~110 modules) and **stable**: they are assigned
/// by the static registry, never by hashing or by address (`GOAL.md` forbids `id()`-style
/// keys: "Fase 3: IDs `u32` atribuídos em `build.rs`/tabelas, nunca endereços").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId(u16);

impl ModuleId {
    /// Build an id from its dense index. Only the registry should call this.
    #[must_use]
    pub const fn from_index(index: u16) -> Self {
        Self(index)
    }

    /// The dense index behind the id (also its position in the registry table).
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// The raw `u16`, for serialisation.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// A module name, as Python spells it (`"greeting"`, `"the_shape_of_the_answer"`).
///
/// `Cow` because module names are usually the static registry names, but the differential
/// runner must also accept a name that is *not* registered (a Python module the Rust port
/// has not reached yet): reporting "expected `stray_module`, got `chat`" must not fall back
/// to a fake "unknown" module, or the report would lose exactly the information it exists for.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleName<'a>(Cow<'a, str>);

impl<'a> ModuleName<'a> {
    /// Borrow a static name (no allocation).
    #[must_use]
    pub const fn from_static(name: &'a str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// Take ownership of a name read from a file.
    #[must_use]
    pub fn from_owned(name: String) -> ModuleName<'static> {
        ModuleName(Cow::Owned(name))
    }

    /// The name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolve against the registry, if this name is a registered module.
    #[must_use]
    pub fn id(&self) -> Option<ModuleId> {
        crate::modules::registry::lookup(self.as_str())
    }
}

impl fmt::Display for ModuleName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<str> for ModuleName<'_> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ModuleName<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// A module's `PRIORITY`, with Python's `int`/`float` semantics preserved.
///
/// Python sorts the candidate list with `key=(-priority, name)`; ints and floats compare
/// numerically and `-999.5 < -999 < -998`. Keeping the variant (rather than normalising
/// everything to `f64`) preserves signed zero and lets the differential report normalise
/// priorities exactly as Python's `repr` would (`300` vs `300.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Priority {
    /// A Python `int` (only those that fit `i64`; larger is capped, see [`Self::from_i128`]).
    Int(i64),
    /// A Python `float`. May be `-0.0`; `nan` never occurs (see [`Self::from_f64`]).
    Float(f64),
}

impl Priority {
    /// Numeric view, for ordering and for the arbiter's arithmetic.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        match self {
            Self::Int(value) => value as f64,
            Self::Float(value) => value,
        }
    }

    /// Python's `isinstance(priority, (int, float)) and not isinstance(priority, bool)`.
    ///
    /// Returns `None` for a bool (Python `True`/`False` are ints but the runtime rejects
    /// them explicitly) and for any non-number.
    #[must_use]
    pub fn from_int_or_bool(value: i64, is_bool: bool) -> Option<Self> {
        if is_bool {
            None
        } else {
            Some(Self::Int(value))
        }
    }

    /// Float constructor with Python semantics: `nan` is not a valid priority and yields
    /// `None`; infinities are kept (they order sensibly); `-0.0` is kept.
    #[must_use]
    pub fn from_f64(value: f64) -> Option<Self> {
        if value.is_nan() {
            None
        } else {
            Some(Self::Float(value))
        }
    }

    /// An `i128` from a JSON/`.py` integer literal, saturated into `i64` (with its sign).
    /// Python ints are unbounded; no real `PRIORITY` comes close to `i64`, so saturation is
    /// exact in practice and total (never panics).
    #[must_use]
    pub fn from_i128(value: i128) -> Self {
        let clamped = value.clamp(i128::from(i64::MIN), i128::from(i64::MAX));
        // `clamped` is within i64 bounds by construction.
        Self::Int(clamped as i64)
    }

    /// Python's `repr()` of the priority (`300`, `-999.5`, `-1000.0`).
    ///
    /// Written by hand (not `{}` on f64) because Rust prints `300` for `300.0` and Python
    /// prints `300.0`; the differential report compares these strings.
    #[must_use]
    pub fn repr(self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Float(value) => crate::pyutil::float_repr(value),
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.repr())
    }
}

impl PartialOrd for Priority {
    /// Numeric order across the int and float variants, like Python's `int`/`float`
    /// comparison. `PartialOrd` (not `Ord`) because two *different* values may compare equal:
    /// `Int(300)` and `Float(300.0)` are the same number, just as in Python — and `-0.0`
    /// vs `0.0` differs only through `total_cmp`, so ties are broken deterministically.
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(match (self, other) {
            (Self::Int(left), Self::Int(right)) => left.cmp(right),
            (Self::Float(left), Self::Float(right)) => left.total_cmp(right),
            (Self::Int(left), Self::Float(right)) => (*left as f64).total_cmp(right),
            (Self::Float(left), Self::Int(right)) => left.total_cmp(&(*right as f64)),
        })
    }
}

/// One user message: the text exactly as it reached `runtime.turn(text, memory)`.
///
/// The corpus stores `str`; the CLI folds an invalid-UTF-8 stdin line to a lossy string and
/// remembers that it did — see [`Turn::from_bytes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    text: String,
    lossy_input: bool,
}

impl Turn {
    /// A turn from valid text.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            lossy_input: false,
        }
    }

    /// A turn from raw bytes (a line of stdin, which need not be UTF-8).
    ///
    /// Never fails and never panics: invalid sequences are replaced with U+FFFD, exactly like
    /// `String::from_utf8_lossy`, and `lossy_input` becomes `true` so callers can report it.
    /// Python raises `UnicodeDecodeError` on the same input (the reference CLI crashes); the
    /// Rust port documents this as an accepted divergence in `docs/fase1.md`.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self::new(text),
            Err(_) => Self {
                text: String::from_utf8_lossy(bytes).into_owned(),
                lossy_input: true,
            },
        }
    }

    /// The message text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// True when [`Turn::from_bytes`] had to replace invalid UTF-8.
    #[must_use]
    pub fn is_lossy(&self) -> bool {
        self.lossy_input
    }

    /// The text `trim()`ed with Python's whitespace set (Rust's `trim` uses a different one:
    /// Python strips `\x1c`-`\x1f` and U+0085, Rust does not).
    #[must_use]
    pub fn trimmed(&self) -> &str {
        crate::pyutil::strip(&self.text)
    }
}

impl From<&str> for Turn {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for Turn {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

/// One bot reply before CLI normalisation.
///
/// `reply` is never empty in a [`crate::runtime::TurnReport`]: a blank module answer is not a
/// candidate (`runtime.py`: `isinstance(answer, str) and answer.strip()`), and the runtime's
/// fallback is `"Hello!"`. An empty [`Reply`] therefore means "no reply" — the CLI drops an
/// empty piped line without calling the bot at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    text: String,
}

impl Reply {
    /// A reply from any string.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The fallback reply of `runtime.py` (`FALLBACK = "Hello!"`).
    #[must_use]
    pub fn fallback() -> Self {
        Self::new(crate::runtime::FALLBACK)
    }

    /// The reply text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// True when the text has no visible content (Python: `not reply.strip()`).
    #[must_use]
    pub fn is_blank(&self) -> bool {
        crate::pyutil::strip(&self.text).is_empty()
    }

    /// Take the text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.text
    }
}

impl fmt::Display for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<&str> for Reply {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for Reply {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

/// The identifier of a conversation in the golden corpus (`s0000` … `s2849`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId<'a>(Cow<'a, str>);

impl<'a> SessionId<'a> {
    /// Borrow a session id.
    #[must_use]
    pub const fn from_static(id: &'a str) -> Self {
        Self(Cow::Borrowed(id))
    }

    /// Own a session id read from a file.
    #[must_use]
    pub fn from_owned(id: String) -> SessionId<'static> {
        SessionId(Cow::Owned(id))
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The numeric part of an `sNNNN` id, when it has the canonical shape.
    #[must_use]
    pub fn number(&self) -> Option<u32> {
        let digits = self.0.strip_prefix('s')?;
        if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    }
}

impl fmt::Display for SessionId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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

    // ------------------------------------------------------------------ ModuleId

    #[test]
    fn module_ids_are_dense_ordered_and_round_trip() {
        let low = ModuleId::from_index(0);
        let high = ModuleId::from_index(u16::MAX);
        assert!(low < high);
        assert_eq!(low.index(), 0);
        assert_eq!(high.index(), usize::from(u16::MAX));
        assert_eq!(high.get(), u16::MAX);
        assert_eq!(low.to_string(), "#0");
    }

    #[test]
    fn module_ids_are_usable_as_order_keys() {
        let mut ids = [
            ModuleId::from_index(7),
            ModuleId::from_index(2),
            ModuleId::from_index(5),
        ];
        ids.sort_unstable();
        assert_eq!(
            ids.iter().map(|id| id.index()).collect::<Vec<_>>(),
            vec![2, 5, 7]
        );
    }

    // ------------------------------------------------------------------ ModuleName

    #[test]
    fn module_names_display_and_compare_like_strings() {
        let borrowed = ModuleName::from_static("greeting");
        let owned = ModuleName::from_owned("greeting".to_string());
        assert_eq!(borrowed, owned);
        assert_eq!(borrowed.as_str(), "greeting");
        assert_eq!(borrowed, "greeting");
        assert_eq!(borrowed, *"greeting");
        assert_ne!(borrowed, "chat");
        assert_eq!(borrowed.to_string(), "greeting");
        assert_eq!(format!("{borrowed:?}"), "ModuleName(\"greeting\")");
    }

    #[test]
    fn module_names_order_alphabetically() {
        let mut names = [
            ModuleName::from_static("opsubject"),
            ModuleName::from_static("chat"),
            ModuleName::from_static("answers"),
        ];
        names.sort();
        assert_eq!(
            names.iter().map(ModuleName::as_str).collect::<Vec<_>>(),
            vec!["answers", "chat", "opsubject"]
        );
    }

    #[test]
    fn module_names_resolve_through_the_registry() {
        // The registry is real (109 modules from bot/modules/): `chat` and `greeting` exist.
        let chat = ModuleName::from_static("chat");
        assert!(chat.id().is_some());
        let ghost = ModuleName::from_static("module_that_does_not_exist");
        assert_eq!(ghost.id(), None);
    }

    // ------------------------------------------------------------------ Priority

    #[test]
    fn priority_int_and_float_compare_numerically_like_python() {
        let a = Priority::Int(-1000);
        let b = Priority::Float(-999.5);
        let c = Priority::Int(-999);
        assert!(a < b);
        assert!(b < c);
        // The conversions are exact for these values, so `==` is the honest check.
        assert!((a.as_f64() - -1000.0).abs() < f64::EPSILON);
        assert!((b.as_f64() - -999.5).abs() < f64::EPSILON);
    }

    #[test]
    fn priority_keeps_signed_zero_apart() {
        let negative = Priority::from_f64(-0.0).unwrap();
        let positive = Priority::from_f64(0.0).unwrap();
        assert!(negative == positive, "they compare equal numerically");
        assert_eq!(negative.repr(), "-0.0");
        assert_eq!(positive.repr(), "0.0");
        assert_ne!(
            format!("{negative:?}"),
            format!("{positive:?}"),
            "debug sees the sign"
        );
    }

    #[test]
    fn priority_rejects_nan_and_bools_like_the_python_runtime() {
        assert_eq!(Priority::from_f64(f64::NAN), None);
        assert_eq!(Priority::from_int_or_bool(1, true), None);
        assert_eq!(Priority::from_int_or_bool(0, false), Some(Priority::Int(0)));
        assert_eq!(
            Priority::from_f64(f64::INFINITY),
            Some(Priority::Float(f64::INFINITY))
        );
        assert!(Priority::from_f64(f64::INFINITY).unwrap() > Priority::Int(i64::MAX));
    }

    #[test]
    fn priority_from_i128_saturates_instead_of_panicking() {
        assert_eq!(Priority::from_i128(0), Priority::Int(0));
        assert_eq!(Priority::from_i128(i128::MAX), Priority::Int(i64::MAX));
        assert_eq!(Priority::from_i128(i128::MIN), Priority::Int(i64::MIN));
        assert_eq!(Priority::from_i128(-999), Priority::Int(-999));
    }

    #[test]
    fn priority_repr_matches_python_repr_for_the_priorities_the_bot_uses() {
        let cases = [
            (Priority::Int(300), "300"),
            (Priority::Int(-1000), "-1000"),
            (Priority::Int(9000), "9000"),
            (Priority::Int(0), "0"),
            (Priority::Float(-999.5), "-999.5"),
            (Priority::Float(-1000.0), "-1000.0"),
            (Priority::Float(0.0), "0.0"),
            (Priority::Float(-0.0), "-0.0"),
            (Priority::Float(1e16), "1e+16"),
        ];
        for (value, expected) in cases {
            assert_eq!(value.repr(), expected, "{value:?}");
            assert_eq!(value.to_string(), expected);
        }
    }

    #[test]
    fn priority_ordering_is_numeric_and_total() {
        let mut priorities = [
            Priority::Float(0.0),
            Priority::Int(-1000),
            Priority::Float(-999.5),
            Priority::Int(9000),
            Priority::Int(0),
            Priority::Int(300),
            Priority::Float(-1000.0),
        ];
        priorities.sort_by(|left, right| left.partial_cmp(right).unwrap());
        let rendered: Vec<String> = priorities.iter().copied().map(Priority::repr).collect();
        // Numeric order; `0` and `0.0` compare equal and a stable sort keeps insertion order,
        // and so do `-1000` and `-1000.0`.
        assert_eq!(
            rendered,
            vec!["-1000", "-1000.0", "-999.5", "0.0", "0", "300", "9000"]
        );
        // Equal numbers across variants are equal, like Python's int/float comparison.
        assert_eq!(
            Priority::Int(0).partial_cmp(&Priority::Float(0.0)),
            Some(std::cmp::Ordering::Equal)
        );
        assert_eq!(Priority::Int(-1000), Priority::Int(-1000));
        assert_ne!(Priority::Int(-1000), Priority::Float(-1000.0));
    }

    // ------------------------------------------------------------------ Turn

    #[test]
    fn turns_carry_exact_text() {
        let turn = Turn::new("  Hello, World!  ");
        assert_eq!(turn.text(), "  Hello, World!  ");
        assert!(!turn.is_lossy());
        assert_eq!(turn.trimmed(), "Hello, World!");
        assert_eq!(Turn::from("a"), Turn::new("a"));
        assert_eq!(Turn::from(String::from("a")), Turn::new("a"));
    }

    #[test]
    fn turn_from_valid_bytes_is_exact() {
        let turn = Turn::from_bytes("olá, tudo bem?".as_bytes());
        assert_eq!(turn.text(), "olá, tudo bem?");
        assert!(!turn.is_lossy());
    }

    #[test]
    fn turn_from_invalid_bytes_is_lossy_but_never_fails() {
        let turn = Turn::from_bytes(&[b'h', b'i', 0xFF, 0xFE, b'!']);
        assert!(turn.is_lossy());
        assert_eq!(turn.text(), "hi\u{FFFD}\u{FFFD}!");
        // NUL bytes are valid UTF-8: not lossy, present in the text.
        let nul = Turn::from_bytes(b"a\0b");
        assert!(!nul.is_lossy());
        assert_eq!(nul.text(), "a\0b");
    }

    #[test]
    fn turn_trimming_uses_the_python_whitespace_set() {
        // U+001C..U+001F are Python whitespace and must be stripped; U+200B is not.
        let turn = Turn::new("\u{1c}\u{1d}\u{1e}\u{1f}hi\u{85}\u{3000}");
        assert_eq!(turn.trimmed(), "hi");
        let zero_width = Turn::new("\u{200b}hi\u{200b}");
        assert_eq!(zero_width.trimmed(), "\u{200b}hi\u{200b}");
    }

    #[test]
    fn turn_text_is_kept_verbatim_with_inner_newlines_and_tabs() {
        let turn = Turn::new("line one\nline two\tcol2");
        assert!(turn.text().contains('\n'));
        assert!(turn.text().contains('\t'));
        assert_eq!(turn.trimmed(), "line one\nline two\tcol2");
    }

    // ------------------------------------------------------------------ Reply

    #[test]
    fn reply_knows_whether_it_is_blank() {
        assert!(Reply::new("").is_blank());
        assert!(Reply::new("  \n\t\u{2003}").is_blank());
        assert!(!Reply::new("Hello!").is_blank());
        assert_eq!(Reply::fallback().as_str(), "Hello!");
        assert_eq!(Reply::default().as_str(), "");
        assert_eq!(Reply::from("x").to_string(), "x");
        assert_eq!(Reply::from(String::from("x")).into_string(), "x");
    }

    #[test]
    fn reply_is_blank_agrees_with_python_strip_semantics() {
        // "\x1c".strip() == "" in Python; Rust's `trim` would keep it.
        assert!(Reply::new("\u{1c}").is_blank());
        assert!(!Reply::new("a").is_blank());
    }

    // ------------------------------------------------------------------ SessionId

    #[test]
    fn session_ids_parse_their_number_only_when_canonical() {
        assert_eq!(SessionId::from_static("s0000").number(), Some(0));
        assert_eq!(SessionId::from_static("s2849").number(), Some(2849));
        assert_eq!(SessionId::from_static("s000").number(), None);
        assert_eq!(SessionId::from_static("x0001").number(), None);
        assert_eq!(SessionId::from_static("s12345").number(), None);
        assert_eq!(SessionId::from_static("s00a1").number(), None);
        assert_eq!(SessionId::from_static("s\u{e9}000").number(), None);
        assert_eq!(
            SessionId::from_owned(String::from("s0007")).number(),
            Some(7)
        );
    }

    #[test]
    fn session_ids_display_verbatim_even_when_odd() {
        assert_eq!(SessionId::from_static("weird id!").to_string(), "weird id!");
        assert_eq!(SessionId::from_static("weird id!").number(), None);
    }
}
