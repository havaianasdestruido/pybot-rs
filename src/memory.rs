//! The conversation memory: an ordered map of Python-shaped values.
//!
//! `bot/runtime.py` keeps the conversation in a plain `dict` and hands each module a
//! `copy.deepcopy` of it (`_copy`), keeping the winner's copy. Three properties matter for the
//! port and are modelled here:
//!
//! 1. **Order**: Python dicts preserve insertion order, and some modules iterate
//!    `memory.items()`. [`Map`] is insertion-ordered (a `Vec` of pairs) — never a `HashMap`
//!    (GOAL.md: "nunca iterar `HashMap` para decidir saída"). It is deliberately simple: the
//!    memory has a handful of keys, so O(keys) lookup is *faster* than hashing.
//! 2. **Snapshot isolation**: a module works on its own copy; if it loses the arbitration its
//!    writes vanish. [`Memory::snapshot`] is that copy (a deep clone, like `deepcopy`), and
//!    [`Memory::restore`] installs the winner's copy back into the conversation, replacing the
//!    contents **in place** (`memory.clear(); memory.update(copy)`).
//! 3. **Sharing of `turns`**: `_copy` keeps `memory["turns"]` shared by reference — only the
//!    runtime writes it. In Rust the log is *not* a map value at all: it lives in
//!    [`Session::turns`](crate::session::Session), which is the same guarantee expressed with
//!    types instead of a convention.
//!
//! Values are a small JSON-shaped enum ([`Value`]). That is enough for every memory key the
//! corpus produces, it has an exact canonical serialisation (for `memory_digest`), and it
//! keeps deep copies cheap and panic-free. Fase 5 replaces this with typed session state; the
//! `Value` layer stays as the (de)serialisation format.

use std::borrow::Cow;

use crate::limits;
use crate::pyutil;
use std::fmt::Write as _;

/// A value stored in the conversation memory.
///
/// Floats keep their bits; ints are `i64` (the bot never stores a bigger integer); the
/// insertion order of [`Value::Object`] matters for serialisation and is preserved.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Python `None`.
    None,
    /// Python `bool` (before `int` when matching, like `isinstance`).
    Bool(bool),
    /// Python `int`.
    Int(i64),
    /// Python `float` (may be `-0.0`; the corpus never stores `nan`).
    Float(f64),
    /// Python `str`.
    Str(String),
    /// Python `list` / `tuple` (tuples serialise as lists, like `json.dumps`).
    List(Vec<Value>),
    /// Python `dict`/`set`-as-sorted-list: ordered key/value pairs.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// A string value.
    #[must_use]
    pub fn string(text: impl Into<String>) -> Self {
        Self::Str(text.into())
    }

    /// The value as a string, if it is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The value as an integer (`True`/`False` are not ints here: Python code that cares uses
    /// `isinstance`, and the port keeps the distinction explicit).
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a list, if it is one.
    #[must_use]
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    /// The value as an object, if it is one.
    #[must_use]
    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Self::Object(fields) => Some(fields),
            _ => None,
        }
    }

    /// One object field by name.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(fields) => fields.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Python's truthiness: `None`, `False`, `0`, `0.0`, `""`, `[]`, `{}` are falsy.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::None => false,
            Self::Bool(value) => *value,
            Self::Int(value) => *value != 0,
            Self::Float(value) => *value != 0.0,
            Self::Str(text) => !text.is_empty(),
            Self::List(items) => !items.is_empty(),
            Self::Object(fields) => !fields.is_empty(),
        }
    }

    /// A JSON type name, for diagnostics.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::None => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "str",
            Self::List(_) => "list",
            Self::Object(_) => "dict",
        }
    }

    /// The deepest nesting level of this value (a scalar is 1), bounded by construction.
    #[must_use]
    pub fn depth(&self) -> usize {
        match self {
            Self::List(items) => 1 + items.iter().map(Value::depth).max().unwrap_or(0),
            Self::Object(fields) => {
                1 + fields
                    .iter()
                    .map(|(_, value)| value.depth())
                    .max()
                    .unwrap_or(0)
            }
            _ => 1,
        }
    }

    /// True when the value fits the memory budget ([`limits`]).
    #[must_use]
    pub fn within_limits(&self) -> bool {
        if self.depth() > limits::MAX_MEMORY_DEPTH {
            return false;
        }
        match self {
            Self::Str(text) => text.len() <= limits::MAX_MEMORY_STRING_BYTES,
            Self::List(items) => {
                items.len() <= limits::MAX_MEMORY_ITEMS && items.iter().all(Value::within_limits)
            }
            Self::Object(fields) => {
                fields.len() <= limits::MAX_MEMORY_ITEMS
                    && fields.iter().all(|(key, value)| {
                        key.len() <= limits::MAX_MEMORY_STRING_BYTES && value.within_limits()
                    })
            }
            _ => true,
        }
    }

    /// Canonical JSON: keys sorted, `ensure_ascii=False`, compact separators, floats rendered
    /// by [`pyutil::float_repr`].
    ///
    /// This mirrors `tools/recon/gen_corpus.py::_canonical` for the value types above, so
    /// SHA-1 over this text can be compared with the corpus' `memory_digest`.
    #[must_use]
    pub fn canonical_json(&self) -> String {
        let mut out = String::new();
        self.write_canonical(&mut out);
        out
    }

    fn write_canonical(&self, out: &mut String) {
        match self {
            Self::None => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Int(value) => out.push_str(&pyutil::int_repr(*value)),
            Self::Float(value) => out.push_str(&pyutil::float_repr(*value)),
            Self::Str(text) => crate::json::write_string(out, text),
            Self::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write_canonical(out);
                }
                out.push(']');
            }
            Self::Object(fields) => {
                let mut ordered: Vec<&(String, Value)> = fields.iter().collect();
                ordered.sort_by(|left, right| left.0.cmp(&right.0));
                out.push('{');
                for (index, (key, value)) in ordered.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    crate::json::write_string(out, key);
                    out.push(':');
                    value.write_canonical(out);
                }
                out.push('}');
            }
        }
    }

    /// SHA-1 of [`Self::canonical_json`] — the format of `memory_digest` in the corpus.
    #[must_use]
    pub fn digest(&self) -> crate::hash::Sha1Digest {
        crate::hash::sha1(&[self.canonical_json().as_bytes()])
    }
}

/// An insertion-ordered string-keyed map — the conversation memory and any per-turn copy.
///
/// Operations that keep the order (`set` on an existing key updates in place) follow Python
/// `dict` semantics exactly: `d["a"] = 1; d["a"] = 2` keeps `"a"` first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    /// An empty map.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Number of keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when there are no keys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert or update. Updating keeps the original position (Python `dict`).
    ///
    /// Returns `Err` and leaves the map untouched when the write would break the memory
    /// budget: [`limits::MAX_MEMORY_KEYS`] for a new key, or a value that is too deep/large.
    pub fn set(&mut self, key: impl Into<String>, value: Value) -> std::result::Result<(), Value> {
        let key = key.into();
        if !value.within_limits() {
            return Err(value);
        }
        if let Some(slot) = self.entries.iter_mut().find(|(name, _)| *name == key) {
            slot.1 = value;
            return Ok(());
        }
        if self.entries.len() >= limits::MAX_MEMORY_KEYS {
            return Err(value);
        }
        self.entries.push((key, value));
        Ok(())
    }

    /// Only the key's existence matters to callers that pre-check.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(name, _)| name == key)
    }

    /// Borrow a value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// Mutate a value in place, if present.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// Remove a key, returning its value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let position = self.entries.iter().position(|(name, _)| name == key)?;
        Some(self.entries.remove(position).1)
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.entries.iter().map(|(name, _)| name.as_str())
    }

    /// `(key, value)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> + '_ {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }

    /// Keys in sorted order — the order the corpus stores `memory_keys`.
    #[must_use]
    pub fn sorted_keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.keys().collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// Replace the contents with another map's, in place (Python `memory.clear(); update()`).
    pub fn replace_with(&mut self, other: &Map) {
        self.entries.clear();
        self.entries.extend(other.entries.iter().cloned());
    }

    /// A "relevant" view for reports: every key except `turns`/`understood`, each value as
    /// canonical JSON, truncated exactly like `gen_corpus.py` (400 chars → digest stub).
    #[must_use]
    pub fn relevant_json(&self) -> String {
        // Python builds a dict and hands it to `json.dumps(..., sort_keys=True)`: the keys come
        // out sorted, whatever order the memory holds them in. `len(blob)` there counts code
        // points, so the truncation threshold and `chars` are counted in `char`s too.
        let mut out = String::from("{");
        let mut first = true;
        for key in self.sorted_keys() {
            let Some(value) = self.get(key) else { continue };
            if key == "turns" || key == "understood" {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            crate::json::write_string(&mut out, key);
            out.push(':');
            let blob = value.canonical_json();
            let chars = blob.chars().count();
            if chars > 400 {
                // `write!` into the `String` avoids an intermediate allocation; the `Result`
                // cannot fail for a `String`.
                let _ = write!(
                    out,
                    "{{\"__truncated__\":true,\"sha1\":\"{}\",\"chars\":{chars}}}",
                    crate::hash::sha1(&[blob.as_bytes()]).hex(),
                );
            } else {
                out.push_str(&blob);
            }
        }
        out.push('}');
        out
    }
}

/// The conversation memory plus the accounting the runtime needs.
///
/// `Memory` is the map a module sees; [`Snapshot`] is the private copy a module works on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Memory {
    map: Map,
    /// Keys a module tried to write after the map hit [`limits::MAX_MEMORY_KEYS`].
    pub(crate) dropped_writes: usize,
}

impl Memory {
    /// An empty memory (a new conversation).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The map itself.
    #[must_use]
    pub fn map(&self) -> &Map {
        &self.map
    }

    /// The map, mutably (runtime bookkeeping only; modules get a [`Snapshot`]).
    #[must_use]
    pub fn map_mut(&mut self) -> &mut Map {
        &mut self.map
    }

    /// Borrow a value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    /// Set a value, counting drops instead of failing.
    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        if self.map.set(key, value).is_err() {
            self.dropped_writes = self.dropped_writes.saturating_add(1);
        }
    }

    /// A private deep copy (Python `copy.deepcopy`).
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            map: self.map.clone(),
            dropped_writes: self.dropped_writes,
        }
    }

    /// Install a snapshot's contents in place (Python `memory.clear(); memory.update(copy)`).
    pub fn restore(&mut self, snapshot: &Snapshot) {
        self.map.replace_with(&snapshot.map);
        self.dropped_writes = snapshot.dropped_writes;
    }

    /// The canonical JSON of the whole memory.
    ///
    /// The runtime keeps the turn log *outside* the map, so the corpus' `memory["turns"]`
    /// is mirrored here by a virtual `turns` entry supplied by the caller — see
    /// [`crate::session::Session::memory_digest`], which is the function the differential
    /// test compares.
    #[must_use]
    pub fn canonical_json(&self) -> String {
        Value::Object(
            self.map
                .iter()
                .map(|(key, value)| (key.to_string(), value.clone()))
                .collect(),
        )
        .canonical_json()
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.map.keys()
    }

    /// Sorted keys (the corpus' `memory_keys`).
    #[must_use]
    pub fn sorted_keys(&self) -> Vec<&str> {
        self.map.sorted_keys()
    }

    /// How many writes were dropped for exceeding the memory budget.
    #[must_use]
    pub const fn dropped_writes(&self) -> usize {
        self.dropped_writes
    }
}

/// A module's private copy of the memory. Writes stay here unless the module wins.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    map: Map,
    dropped_writes: usize,
}

impl Snapshot {
    /// Borrow a value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    /// Set a value; returns `false` (and drops the write) when the budget is exceeded.
    pub fn set(&mut self, key: impl Into<String>, value: Value) -> bool {
        if self.map.set(key, value).is_ok() {
            true
        } else {
            self.dropped_writes = self.dropped_writes.saturating_add(1);
            false
        }
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.map.keys()
    }

    /// Sorted keys.
    #[must_use]
    pub fn sorted_keys(&self) -> Vec<&str> {
        self.map.sorted_keys()
    }

    /// The underlying map (read-only view for handlers that need structure).
    #[must_use]
    pub fn map(&self) -> &Map {
        &self.map
    }

    /// Remove a key from the snapshot, returning its value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key)
    }

    /// The map, mutably (the runtime uses it to strip the read-only `turns` mirror).
    pub fn map_mut(&mut self) -> &mut Map {
        &mut self.map
    }

    /// How many writes this snapshot dropped.
    #[must_use]
    pub const fn dropped_writes(&self) -> usize {
        self.dropped_writes
    }

    /// Canonical JSON of the snapshot's map.
    #[must_use]
    pub fn canonical_json(&self) -> String {
        Memory {
            map: self.map.clone(),
            dropped_writes: self.dropped_writes,
        }
        .canonical_json()
    }
}

/// A borrowed text that may be either a `&str` or an owned `String` — handy for handlers
/// that sometimes echo memory text and sometimes produce new text.
pub type Text<'a> = Cow<'a, str>;

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    fn object(pairs: &[(&str, Value)]) -> Value {
        Value::Object(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect(),
        )
    }

    // ------------------------------------------------------------------ Value

    #[test]
    fn truthiness_matches_python_for_every_variant() {
        assert!(!Value::None.is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Bool(true).is_truthy());
        assert!(!Value::Int(0).is_truthy());
        assert!(Value::Int(-1).is_truthy());
        assert!(!Value::Float(0.0).is_truthy());
        assert!(Value::Float(0.1).is_truthy());
        assert!(!Value::string("").is_truthy());
        assert!(Value::string("0").is_truthy());
        assert!(!Value::List(vec![]).is_truthy());
        assert!(!object(&[]).is_truthy());
        assert!(Value::List(vec![Value::None]).is_truthy());
    }

    #[test]
    fn type_names_mirror_python() {
        assert_eq!(Value::None.type_name(), "NoneType");
        assert_eq!(Value::Bool(true).type_name(), "bool");
        assert_eq!(Value::Int(1).type_name(), "int");
        assert_eq!(Value::Float(1.0).type_name(), "float");
        assert_eq!(Value::string("").type_name(), "str");
        assert_eq!(Value::List(vec![]).type_name(), "list");
        assert_eq!(object(&[]).type_name(), "dict");
    }

    #[test]
    fn accessors_are_total() {
        assert_eq!(Value::None.as_str(), None);
        assert_eq!(Value::Int(2).as_i64(), Some(2));
        assert_eq!(Value::Bool(true).as_i64(), None);
        assert_eq!(Value::Float(2.0).as_i64(), None);
        let list = Value::List(vec![Value::Int(1)]);
        assert_eq!(list.as_list().map(<[Value]>::len), Some(1));
        assert_eq!(list.as_object(), None);
        let obj = object(&[("a", Value::Int(1))]);
        assert_eq!(obj.get("a"), Some(&Value::Int(1)));
        assert_eq!(obj.get("b"), None);
        assert_eq!(Value::None.get("a"), None);
        assert_eq!(Value::None.as_list(), None);
        assert_eq!(Value::None.as_object(), None);
    }

    #[test]
    fn canonical_json_matches_python_json_dumps_canonical_form() {
        // The format produced by gen_corpus._canonical + json.dumps(sort_keys=True).
        assert_eq!(Value::None.canonical_json(), "null");
        assert_eq!(Value::Bool(true).canonical_json(), "true");
        assert_eq!(Value::Int(-7).canonical_json(), "-7");
        assert_eq!(Value::Float(-1000.0).canonical_json(), "-1000.0");
        assert_eq!(
            Value::string("café \"x\"").canonical_json(),
            "\"café \\\"x\\\"\""
        );
        assert_eq!(
            Value::List(vec![Value::Int(1), Value::string("a")]).canonical_json(),
            "[1,\"a\"]"
        );
        // Keys are sorted regardless of insertion order.
        assert_eq!(
            object(&[("z", Value::Int(1)), ("a", Value::Int(2))]).canonical_json(),
            r#"{"a":2,"z":1}"#
        );
        assert_eq!(object(&[]).canonical_json(), "{}");
        assert_eq!(Value::List(vec![]).canonical_json(), "[]");
    }

    #[test]
    fn canonical_json_escapes_like_python() {
        assert_eq!(Value::string("a\nb\tc").canonical_json(), "\"a\\nb\\tc\"");
        assert_eq!(Value::string("\u{1}").canonical_json(), "\"\\u0001\"");
        assert_eq!(Value::string("\u{7f}").canonical_json(), "\"\u{7f}\"");
    }

    #[test]
    fn digest_of_a_corpus_memory_matches_the_recorded_sha1() {
        // Python anchors, computed with the corpus generator's own `_canonical` +
        // `json.dumps(..., sort_keys=True, separators=(",", ":"), ensure_ascii=False)`: the
        // digest covers the **whole** memory, `turns` and `understood` included.
        let anchors: [(&str, Value, &str); 6] = [
            // The empty conversation.
            (
                "empty",
                object(&[]),
                "bf21a9e8fbc5a3846fb05b4fa0859e0917b2202f",
            ),
            // Flat scalars: keys sorted, `true`/`false`/`null` lower case.
            (
                "flat",
                object(&[
                    ("c", Value::Int(3)),
                    ("a", Value::Int(1)),
                    ("b", Value::Int(2)),
                ]),
                "e7ec4a8f2309bdd4c4c57cb2adfb79c91a293597",
            ),
            // Python float reprs: `1.0`, `-0.0`, `5e-324`, `1e+16`, `1e-05`.
            (
                "scalars",
                object(&[
                    ("t", Value::Bool(true)),
                    ("f", Value::Bool(false)),
                    ("n", Value::None),
                    ("i", Value::Int(-7)),
                    ("big", Value::Int(1_234_567_890_123_456)),
                    ("fi", Value::Float(1.0)),
                    ("ff", Value::Float(1.5)),
                    ("neg", Value::Float(-0.0)),
                    ("eps", Value::Float(5e-324)),
                    ("e16", Value::Float(1e16)),
                    ("tiny", Value::Float(1e-05)),
                ]),
                "043c2c5d75af552e3b50e976f914db942e58302f",
            ),
            // Non-ASCII is written raw (`ensure_ascii=False`); control characters are escaped.
            (
                "unicode",
                object(&[
                    ("name", Value::string("café ☕")),
                    ("quote", Value::string("he said \"hi\"")),
                    ("back", Value::string("a\\b")),
                    ("ctrl", Value::string("\u{1}\u{7f}\u{2028}")),
                    ("emoji", Value::string("😀")),
                    ("nul", Value::string("\u{0}")),
                ]),
                "b5becfe028176a5c26b2b2661ec362a5351d8d66",
            ),
            // Nested containers: object keys sorted at every level, arrays keep their order.
            (
                "nested",
                object(&[
                    (
                        "z",
                        Value::List(vec![
                            Value::Int(1),
                            Value::List(vec![
                                Value::Int(2),
                                object(&[("b", Value::List(vec![])), ("a", object(&[]))]),
                            ]),
                        ]),
                    ),
                    (
                        "a",
                        object(&[("d", object(&[("c", Value::List(vec![Value::None]))]))]),
                    ),
                ]),
                "3f330003bfa49677de624fe5177a434e67cfb9de",
            ),
            // The first corpus turn's memory *shape* — em dash in the reply, a turn log entry,
            // `null` for an unanswered intent — with a small `understood` standing in for the
            // real 60-key reading. (The full-corpus replay, `understood` and all, is checked by
            // `golden::tests`: every turn's digest must equal the recorded `memory_digest`.)
            (
                "corpus-shape",
                object(&[
                    ("frequency_answer", Value::string("")),
                    ("frequency_reply", Value::Bool(false)),
                    ("intent_wants", Value::None),
                    (
                        "said_kind",
                        object(&[("kind", Value::string("greeting")), ("turn", Value::Int(0))]),
                    ),
                    (
                        "turns",
                        Value::List(vec![object(&[
                            ("user", Value::string("hello")),
                            ("bot", Value::string("Hi there — what do you need?")),
                            ("module", Value::string("chat")),
                        ])]),
                    ),
                    (
                        "understood",
                        object(&[
                            ("clean", Value::string("hello")),
                            ("intent", Value::string("greeting")),
                            ("kind", Value::string("greeting")),
                            ("plain", Value::string("hello")),
                        ]),
                    ),
                    ("user", object(&[])),
                ]),
                "aa2271fb540c3af24a801956f5b95b88ce52be08",
            ),
        ];
        for (name, memory, expected) in anchors {
            assert_eq!(memory.digest().hex(), expected, "{name}");
        }
    }

    #[test]
    fn the_digest_is_sensitive_to_every_part_of_the_memory() {
        let base = object(&[
            ("a", Value::Int(1)),
            ("turns", Value::List(vec![])),
            ("understood", Value::None),
        ]);
        let digest = base.digest().hex();

        // A different scalar, an added key, a different list and a different `turns` all move it.
        for changed in [
            object(&[
                ("a", Value::Int(2)),
                ("turns", Value::List(vec![])),
                ("understood", Value::None),
            ]),
            object(&[
                ("a", Value::Int(1)),
                ("b", Value::Int(0)),
                ("turns", Value::List(vec![])),
                ("understood", Value::None),
            ]),
            object(&[
                ("a", Value::Int(1)),
                ("turns", Value::List(vec![Value::Int(0)])),
                ("understood", Value::None),
            ]),
            object(&[
                ("a", Value::Int(1)),
                ("turns", Value::List(vec![])),
                ("understood", object(&[("x", Value::None)])),
            ]),
        ] {
            assert_ne!(
                changed.digest().hex(),
                digest,
                "{}",
                changed.canonical_json()
            );
        }

        // Key order is *not* part of the digest: the canonical form sorts keys.
        let reordered = object(&[
            ("understood", Value::None),
            ("turns", Value::List(vec![])),
            ("a", Value::Int(1)),
        ]);
        assert_eq!(reordered.digest().hex(), digest);
    }

    #[test]
    fn integers_and_floats_with_the_same_value_digest_differently() {
        // In JSON `1` and `1.0` are different texts even though they compare equal in Python
        // (`json.dumps(1) == "1"`, `json.dumps(1.0) == "1.0"`), and the digest is over text.
        assert_eq!(
            object(&[("v", Value::Int(1))]).canonical_json(),
            "{\"v\":1}"
        );
        assert_eq!(
            object(&[("v", Value::Float(1.0))]).canonical_json(),
            "{\"v\":1.0}"
        );
        assert_ne!(
            object(&[("v", Value::Int(1))]).digest().hex(),
            object(&[("v", Value::Float(1.0))]).digest().hex()
        );
    }

    #[test]
    fn depth_and_limits_are_enforced_without_recursion_surprises() {
        assert_eq!(Value::Int(1).depth(), 1);
        assert_eq!(Value::List(vec![Value::Int(1)]).depth(), 2);
        assert_eq!(
            Value::List(vec![Value::List(vec![Value::Int(1)])]).depth(),
            3
        );
        let mut deep = Value::Int(1);
        for _ in 0..limits::MAX_MEMORY_DEPTH {
            deep = Value::List(vec![deep]);
        }
        assert!(!deep.within_limits(), "one level too deep");
        let mut shallow = Value::Int(1);
        for _ in 0..limits::MAX_MEMORY_DEPTH - 1 {
            shallow = Value::List(vec![shallow]);
        }
        assert!(shallow.within_limits());
        let huge = Value::string("x".repeat(limits::MAX_MEMORY_STRING_BYTES + 1));
        assert!(!huge.within_limits());
        let fits = Value::string("x".repeat(limits::MAX_MEMORY_STRING_BYTES));
        assert!(fits.within_limits());
        let too_many = Value::List(vec![Value::None; limits::MAX_MEMORY_ITEMS + 1]);
        assert!(!too_many.within_limits());
    }

    // ------------------------------------------------------------------ Map

    #[test]
    fn map_preserves_insertion_order_and_updates_in_place() {
        let mut map = Map::new();
        assert!(map.is_empty());
        assert_eq!(map.set("b", Value::Int(1)), Ok(()));
        assert_eq!(map.set("a", Value::Int(2)), Ok(()));
        assert_eq!(map.set("c", Value::Int(3)), Ok(()));
        assert_eq!(map.set("a", Value::Int(20)), Ok(()));
        assert_eq!(map.len(), 3);
        assert_eq!(map.keys().collect::<Vec<_>>(), vec!["b", "a", "c"]);
        assert_eq!(map.get("a"), Some(&Value::Int(20)));
        assert_eq!(map.sorted_keys(), vec!["a", "b", "c"]);
        assert!(map.contains_key("b"));
        assert!(!map.contains_key("z"));
    }

    #[test]
    fn map_get_mut_and_remove_keep_semantics() {
        let mut map = Map::new();
        map.set("x", Value::Int(1)).unwrap();
        if let Some(slot) = map.get_mut("x") {
            *slot = Value::Int(9);
        }
        assert_eq!(map.get("x"), Some(&Value::Int(9)));
        assert_eq!(map.get_mut("missing"), None);
        assert_eq!(map.remove("x"), Some(Value::Int(9)));
        assert_eq!(map.remove("x"), None);
        assert!(map.is_empty());
    }

    #[test]
    fn map_refuses_writes_beyond_the_key_budget_and_reports_them() {
        let mut map = Map::new();
        for index in 0..limits::MAX_MEMORY_KEYS {
            assert!(map.set(format!("k{index}"), Value::Int(0)).is_ok());
        }
        assert_eq!(map.len(), limits::MAX_MEMORY_KEYS);
        let rejected = map.set("overflow", Value::Int(1));
        assert!(rejected.is_err());
        assert_eq!(map.len(), limits::MAX_MEMORY_KEYS);
        // Updating an existing key still works at the cap.
        assert!(map.set("k0", Value::Int(5)).is_ok());
        // A value that is too large is rejected even for an existing key.
        assert!(map
            .set(
                "k0",
                Value::string("x".repeat(limits::MAX_MEMORY_STRING_BYTES + 1))
            )
            .is_err());
        assert_eq!(map.get("k0"), Some(&Value::Int(5)));
    }

    #[test]
    fn map_replace_with_mirrors_in_place_update() {
        let mut memory = Map::new();
        memory.set("keep", Value::Int(1)).unwrap();
        let mut winner = Map::new();
        winner.set("new", Value::Int(2)).unwrap();
        memory.replace_with(&winner);
        assert_eq!(memory.keys().collect::<Vec<_>>(), vec!["new"]);
    }

    #[test]
    fn relevant_json_drops_internal_keys_and_truncates_long_values() {
        let mut memory = Map::new();
        memory.set("user", object(&[])).unwrap();
        memory.set("turns", Value::List(vec![])).unwrap();
        memory.set("understood", Value::None).unwrap();
        memory.set("frequency_answer", Value::string("")).unwrap();
        let rendered = memory.relevant_json();
        assert!(!rendered.contains("\"turns\""));
        assert!(!rendered.contains("understood"));
        assert!(rendered.contains("\"user\":{}"));
        assert!(rendered.contains("\"frequency_answer\":\"\""));

        // 200 × "word " is 1 000 characters, plus two quotes: 1 002 code points, and the
        // threshold counts `len()` of the JSON text exactly as Python does.
        let long_value = Value::string("word ".repeat(200));
        let blob = long_value.canonical_json();
        let mut long = Map::new();
        long.set("big", long_value).unwrap();
        long.set("also", Value::Int(2)).unwrap();
        let rendered = long.relevant_json();
        assert_eq!(
            rendered,
            format!(
                "{{\"also\":2,\"big\":{{\"__truncated__\":true,\"sha1\":\"{}\",\"chars\":1002}}}}",
                crate::hash::sha1(&[blob.as_bytes()]).hex()
            )
        );
    }

    #[test]
    fn the_truncation_threshold_is_exactly_four_hundred_characters_of_json() {
        // A 398-character string becomes 400 characters of JSON (two quotes) and is kept; one
        // more character crosses the threshold and becomes the stub.
        for (length, truncated) in [(398, false), (399, true)] {
            let mut map = Map::new();
            map.set("k", Value::string("x".repeat(length))).unwrap();
            let rendered = map.relevant_json();
            assert_eq!(
                rendered.contains("__truncated__"),
                truncated,
                "{length}: {rendered}"
            );
            assert_eq!(
                rendered.contains(&format!("\"chars\":{}", length + 2)),
                truncated,
                "{length}: {rendered}"
            );
        }
        // The count is in code points, not bytes: `é` is two UTF-8 bytes but one JSON character,
        // so 398 of them fill the budget exactly and 400 overflow it by two.
        let mut map = Map::new();
        map.set("k", Value::string("é".repeat(398))).unwrap();
        assert!(!map.relevant_json().contains("__truncated__"));
        let mut map = Map::new();
        map.set("k", Value::string("é".repeat(400))).unwrap();
        assert_eq!(
            map.relevant_json(),
            "{\"k\":{\"__truncated__\":true,\"sha1\":\"e58ba6a775c5583481b4176fdadd220d0066c1d4\",\"chars\":402}}"
        );
    }

    // ------------------------------------------------------------------ Memory / Snapshot

    #[test]
    fn snapshots_are_isolated_and_restores_replace_in_place() {
        let mut memory = Memory::new();
        memory.set("user", object(&[]));
        memory.set("said_kind", Value::None);

        let mut losing = memory.snapshot();
        losing.set("loser", Value::Int(1));
        assert_eq!(memory.get("loser"), None, "loser's write must not leak");

        let mut winning = memory.snapshot();
        winning.set("user", object(&[("name", Value::string("Sam"))]));
        memory.restore(&winning);
        assert_eq!(
            memory.get("user").and_then(|user| user.get("name")),
            Some(&Value::string("Sam"))
        );
        assert_eq!(memory.keys().collect::<Vec<_>>(), vec!["user", "said_kind"]);
    }

    #[test]
    fn memory_counts_dropped_writes_instead_of_failing() {
        let mut memory = Memory::new();
        for index in 0..limits::MAX_MEMORY_KEYS {
            memory.set(format!("k{index}"), Value::Int(0));
        }
        assert_eq!(memory.dropped_writes(), 0);
        memory.set("overflow", Value::Int(1));
        assert_eq!(memory.dropped_writes(), 1);
        assert!(!memory.map().contains_key("overflow"));
        let snapshot = memory.snapshot();
        assert_eq!(snapshot.dropped_writes(), 1);
    }

    #[test]
    fn snapshot_set_reports_success_like_python_would_not_need_to() {
        let memory = Memory::new();
        let mut snapshot = memory.snapshot();
        assert!(snapshot.set("a", Value::Int(1)));
        assert!(snapshot.set("a", Value::Int(2)));
        assert_eq!(snapshot.get("a"), Some(&Value::Int(2)));
        assert_eq!(snapshot.sorted_keys(), vec!["a"]);
        assert_eq!(snapshot.map().len(), 1);
    }

    #[test]
    fn canonical_json_of_memory_is_key_ordered_and_stable() {
        let mut memory = Memory::new();
        memory.set("z", Value::Int(1));
        memory.set("a", Value::Int(2));
        assert_eq!(memory.canonical_json(), r#"{"a":2,"z":1}"#);
        let snapshot = memory.snapshot();
        assert_eq!(snapshot.canonical_json(), memory.canonical_json());
    }

    #[test]
    fn text_alias_accepts_both_borrowed_and_owned() {
        let borrowed: Text<'_> = Cow::Borrowed("a");
        let owned: Text<'_> = Cow::Owned(String::from("b"));
        assert_eq!(borrowed.as_ref(), "a");
        assert_eq!(owned.as_ref(), "b");
    }
}
