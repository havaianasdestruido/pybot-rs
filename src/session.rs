//! A conversation: the memory a module sees, the turn log, and the digest accounting.
//!
//! In Python, `memory` is one dict: `runtime.turn` hands modules deep copies of it (sharing
//! `memory["turns"]` by reference behind a convention) and appends
//! `{"user": text, "bot": reply, "module": name}` to the log at the end. In Rust the log is
//! **structurally** outside the module-visible memory ([`Session::turns`] vs [`Session::memory`]),
//! which makes `_copy`'s "only the runtime writes `turns`" a type guarantee. The two are
//! re-joined for canonical hashing and for the corpus comparison ([`Session::memory_value`],
//! [`Session::snapshot_digest`]).
//!
//! Bounds (GOAL.md, Fase 9): the log keeps at most [`limits::MAX_TURN_LOG`] turns, each text
//! at most [`limits::MAX_LOGGED_TEXT_BYTES`]; evictions and truncations are counted in
//! [`Session::evicted`] / [`Session::truncated`] so the harness can report them instead of
//! hiding them.

use crate::hash::Sha1Digest;
use crate::limits;
use crate::memory::{Map, Memory, Snapshot, Value};
use crate::types::{ModuleName, Reply, SessionId, Turn};

/// One turn of the conversation log — `memory["turns"][i]` in the Python bot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedTurn {
    /// What the user said (bounded to [`limits::MAX_LOGGED_TEXT_BYTES`]).
    pub user: String,
    /// What the bot replied (bounded likewise).
    pub bot: String,
    /// The winning module's name, when a module replied (`None` for the fallback).
    pub module: Option<String>,
}

impl LoggedTurn {
    /// The log entry as a memory [`Value`], with the key order `turns.append({"user", "bot",
    /// "module"})` used in Python.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::Object(vec![
            ("user".to_string(), Value::Str(self.user.clone())),
            ("bot".to_string(), Value::Str(self.bot.clone())),
            (
                "module".to_string(),
                match &self.module {
                    Some(name) => Value::Str(name.clone()),
                    None => Value::None,
                },
            ),
        ])
    }

    /// Truncate the texts to the logged-text budget, returning whether anything changed.
    fn enforce_budget(&mut self) -> bool {
        let mut changed = false;
        if self.user.len() > limits::MAX_LOGGED_TEXT_BYTES {
            self.user = truncate_bytes(&self.user, limits::MAX_LOGGED_TEXT_BYTES);
            changed = true;
        }
        if self.bot.len() > limits::MAX_LOGGED_TEXT_BYTES {
            self.bot = truncate_bytes(&self.bot, limits::MAX_LOGGED_TEXT_BYTES);
            changed = true;
        }
        changed
    }
}

/// Truncate to at most `max_bytes`, backing off to a char boundary.
fn truncate_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default().to_string()
}

/// A conversation: memory + turn log.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    /// The module-visible memory (everything except the turn log).
    pub memory: Memory,
    turns: Vec<LoggedTurn>,
    evicted: usize,
    truncated: usize,
}

impl Session {
    /// A new, empty conversation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The turn log, oldest first.
    #[must_use]
    pub fn turns(&self) -> &[LoggedTurn] {
        &self.turns
    }

    /// Forgive everything: memory and log (the `/reset` command).
    pub fn reset(&mut self) {
        self.memory = Memory::new();
        self.turns.clear();
        self.evicted = 0;
        self.truncated = 0;
    }

    /// Append a turn to the log, enforcing the window.
    pub fn log_turn(&mut self, mut entry: LoggedTurn) {
        if entry.enforce_budget() {
            self.truncated = self.truncated.saturating_add(1);
        }
        self.turns.push(entry);
        while self.turns.len() > limits::MAX_TURN_LOG {
            self.turns.remove(0);
            self.evicted = self.evicted.saturating_add(1);
        }
    }

    /// Number of log entries evicted by the window since the last [`Self::reset`].
    #[must_use]
    pub const fn evicted(&self) -> usize {
        self.evicted
    }

    /// Number of log entries truncated to the text budget since the last [`Self::reset`].
    #[must_use]
    pub const fn truncated(&self) -> usize {
        self.truncated
    }

    /// Append a turn with the parts the runtime has.
    pub fn record(&mut self, turn: &Turn, reply: &Reply, module: Option<&ModuleName<'_>>) {
        self.log_turn(LoggedTurn {
            user: turn.text().to_string(),
            bot: reply.as_str().to_string(),
            module: module.map(|name| name.as_str().to_string()),
        });
    }

    /// The full memory as one [`Value`] **including** the `turns` list, exactly as Python's
    /// `memory` dict would hold it. The `turns` entry is placed first: it is validated only
    /// through the canonical digest, whose object keys are sorted anyway.
    #[must_use]
    pub fn memory_value(&self) -> Value {
        let mut fields: Vec<(String, Value)> = vec![(
            "turns".to_string(),
            Value::List(self.turns.iter().map(LoggedTurn::to_value).collect()),
        )];
        // The log is authoritative: a stored `turns` value (a restorer, a test, a future
        // importer) must never shadow or duplicate it, so the ordinary fields skip that key.
        fields.extend(
            self.memory
                .map()
                .iter()
                .filter(|(key, _)| *key != "turns")
                .map(|(key, value)| (key.to_string(), value.clone())),
        );
        Value::Object(fields)
    }

    /// Sorted keys of the full memory (`memory_keys` in the corpus).
    #[must_use]
    pub fn memory_keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.memory.sorted_keys();
        keys.push("turns");
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// SHA-1 of the canonical full memory (`memory_digest` in the corpus).
    #[must_use]
    pub fn memory_digest(&self) -> Sha1Digest {
        self.memory_value().digest()
    }

    /// The "relevant" memory view the corpus stores (everything but `turns`/`understood`).
    #[must_use]
    pub fn relevant_memory_json(&self) -> String {
        self.memory.map().relevant_json()
    }

    /// A snapshot of the module-visible memory with the same convention as Python: the
    /// handle a module writes to. The turn log is deliberately *not* in it — the runtime
    /// appends to it after the arbitration, and modules can only read it through
    /// [`Snapshot::get`] after the runtime injected it.
    ///
    /// The runtime injects a read-only mirror of the log before calling modules, so handlers
    /// that inspect `memory["turns"]` behave like in Python while being unable to corrupt it
    /// (the mirror is overwritten after the turn).
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.memory
            .snapshot_with_mirror("turns", self.turns_value())
    }

    /// The log as a [`Value::List`].
    #[must_use]
    pub fn turns_value(&self) -> Value {
        Value::List(self.turns.iter().map(LoggedTurn::to_value).collect())
    }

    /// Install a winning snapshot's memory back into the session (its `turns` mirror is
    /// ignored: the runtime owns the log).
    pub fn apply(&mut self, snapshot: &Snapshot) {
        let mut cleaned = snapshot.clone();
        // Neither the runtime's mirror nor a module-written `turns` key may enter the memory:
        // the turn log is `Session::turns`, and only `log_turn` appends to it.
        cleaned.clear_mirror();
        cleaned.map_mut().remove("turns");
        self.memory.restore(&cleaned);
    }

    /// The conversation as it would be keyed by the corpus (`session` id), when the caller has
    /// one. `None` for an interactive session.
    #[must_use]
    pub fn id_hint(&self) -> Option<SessionId<'static>> {
        None
    }

    /// Count of memory keys (for the report's summary).
    #[must_use]
    pub fn memory_key_count(&self) -> usize {
        self.memory.map().len()
    }

    /// The memory as a [`Map`] (read-only; tests and the report use it).
    #[must_use]
    pub fn memory_map(&self) -> &Map {
        self.memory.map()
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

    fn session_with_memory() -> Session {
        let mut session = Session::new();
        session.memory.set("user", Value::Object(vec![]));
        session.memory.set("frequency_answer", Value::string(""));
        session
    }

    #[test]
    fn a_new_session_is_empty() {
        let session = Session::new();
        assert!(session.turns().is_empty());
        assert_eq!(session.memory_key_count(), 0);
        assert_eq!(session.evicted(), 0);
        assert_eq!(session.truncated(), 0);
        assert_eq!(session.memory_keys(), vec!["turns"]);
        assert_eq!(session.relevant_memory_json(), "{}");
        assert!(session.id_hint().is_none());
    }

    #[test]
    fn recording_turns_keeps_order_and_content() {
        let mut session = Session::new();
        session.record(
            &Turn::new("hello"),
            &Reply::new("Hi there — what do you need?"),
            Some(&ModuleName::from_static("chat")),
        );
        session.record(&Turn::new("bye"), &Reply::new("Hello!"), None);
        assert_eq!(session.turns().len(), 2);
        assert_eq!(
            session.turns().first().map(|t| t.user.as_str()),
            Some("hello")
        );
        assert_eq!(
            session.turns().get(1).map(|t| t.module.as_deref()),
            Some(None)
        );
        assert_eq!(
            session.turns_value(),
            Value::List(vec![
                Value::Object(vec![
                    ("user".into(), Value::string("hello")),
                    ("bot".into(), Value::string("Hi there — what do you need?")),
                    ("module".into(), Value::string("chat")),
                ]),
                Value::Object(vec![
                    ("user".into(), Value::string("bye")),
                    ("bot".into(), Value::string("Hello!")),
                    ("module".into(), Value::None),
                ]),
            ])
        );
    }

    #[test]
    fn reset_forgets_everything_including_the_accounting() {
        let mut session = session_with_memory();
        session.log_turn(LoggedTurn {
            user: "x".repeat(limits::MAX_LOGGED_TEXT_BYTES + 10),
            bot: "y".into(),
            module: None,
        });
        assert_eq!(session.truncated(), 1);
        session.reset();
        assert!(session.turns().is_empty());
        assert_eq!(session.memory_key_count(), 0);
        assert_eq!(session.truncated(), 0);
        assert_eq!(session.evicted(), 0);
    }

    #[test]
    fn the_turn_log_is_a_bounded_window_that_keeps_the_newest_turns() {
        let mut session = Session::new();
        for index in 0..limits::MAX_TURN_LOG + 5 {
            session.log_turn(LoggedTurn {
                user: format!("u{index}"),
                bot: format!("b{index}"),
                module: None,
            });
        }
        assert_eq!(session.turns().len(), limits::MAX_TURN_LOG);
        assert_eq!(session.evicted(), 5);
        assert_eq!(session.turns().first().map(|t| t.user.as_str()), Some("u5"));
        assert_eq!(
            session.turns().last().map(|t| t.user.as_str()),
            Some(format!("u{}", limits::MAX_TURN_LOG + 4).as_str())
        );
    }

    #[test]
    fn long_texts_are_truncated_on_a_char_boundary_and_counted() {
        let mut session = Session::new();
        let long = "é".repeat(limits::MAX_LOGGED_TEXT_BYTES); // 2 bytes each
        session.log_turn(LoggedTurn {
            user: long.clone(),
            bot: long,
            module: None,
        });
        assert_eq!(session.truncated(), 1);
        let entry = session.turns().first().unwrap();
        assert!(entry.user.len() <= limits::MAX_LOGGED_TEXT_BYTES);
        assert!(entry.user.chars().all(|c| c == 'é'), "no split characters");
        assert_eq!(entry.user.len() % 2, 0);
    }

    #[test]
    fn snapshots_include_a_turns_mirror_but_installing_one_cannot_rewrite_the_log() {
        let mut session = Session::new();
        session.record(&Turn::new("hi"), &Reply::new("Hello!"), None);
        let mut snapshot = session.snapshot();
        assert_eq!(
            snapshot.get("turns"),
            Some(&Value::List(vec![LoggedTurn {
                user: "hi".into(),
                bot: "Hello!".into(),
                module: None
            }
            .to_value()]))
        );
        // A module scribbles over its mirror; the winner's snapshot still cannot change the log.
        snapshot.set("turns", Value::string("corrupted"));
        snapshot.set("kept", Value::Int(1));
        session.apply(&snapshot);
        assert_eq!(session.turns().len(), 1);
        assert_eq!(session.turns().first().map(|t| t.user.as_str()), Some("hi"));
        assert_eq!(session.memory_key_count(), 1);
        assert!(session.memory_map().contains_key("kept"));
        assert!(
            !session.memory_map().contains_key("turns"),
            "mirror never lands in memory"
        );
    }

    #[test]
    fn a_stored_turns_key_cannot_alter_the_log_or_its_digest() {
        // A restorer, a future importer or a buggy module may leave a `turns` value in the
        // ordinary memory. `Session::turns` stays authoritative: the value never shadows the
        // log in `memory_value`, `memory_keys` or the digest.
        let mut session = Session::new();
        session.record(&Turn::new("hi"), &Reply::new("Hello!"), None);
        session.memory.set("turns", Value::string("not the log"));
        let with_stray = session.memory_value();
        let digest_with_stray = session.memory_digest();
        let keys_with_stray: Vec<String> = session
            .memory_keys()
            .iter()
            .map(ToString::to_string)
            .collect();

        session.memory.map_mut().remove("turns");
        assert_eq!(session.memory_value(), with_stray, "the stray is invisible");
        assert_eq!(session.memory_digest(), digest_with_stray);
        assert_eq!(
            session.memory_keys(),
            keys_with_stray
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            session
                .memory_value()
                .get("turns")
                .and_then(Value::as_list)
                .map(<[Value]>::len),
            Some(1),
            "the log, not the stray string, is under `turns`"
        );
        assert_eq!(
            session.memory_keys(),
            vec!["turns"],
            "no duplicated `turns` key"
        );
    }

    #[test]
    fn the_turns_mirror_survives_a_full_memory_budget() {
        // A long conversation can fill all 512 ordinary slots. The runtime owns one slot on
        // top of the budget for the `turns` view, so handlers still see the log, no ordinary
        // key is evicted, and nothing is counted as a dropped write.
        let mut session = Session::new();
        session.record(&Turn::new("hi"), &Reply::new("Hello!"), None);
        for index in 0..limits::MAX_MEMORY_KEYS {
            session.memory.set(format!("key_{index}"), Value::Int(1));
        }
        assert_eq!(session.memory_key_count(), limits::MAX_MEMORY_KEYS);
        let before = session.memory.dropped_writes();

        let snapshot = session.snapshot();
        assert_eq!(snapshot.sorted_keys().len(), limits::MAX_MEMORY_KEYS + 1);
        assert!(
            snapshot.get("turns").and_then(Value::as_list).is_some(),
            "the mirror is present at the cap"
        );
        assert_eq!(
            snapshot.map().keys().count(),
            limits::MAX_MEMORY_KEYS,
            "the mirror is not an ordinary key"
        );
        assert_eq!(
            session.memory.dropped_writes(),
            before,
            "installing the mirror is free"
        );

        // Installing the snapshot back cannot lose an ordinary key nor add the mirror.
        let winner = snapshot.clone();
        session.apply(&winner);
        assert_eq!(session.memory_key_count(), limits::MAX_MEMORY_KEYS);
        assert!(!session.memory_map().contains_key("turns"));
        assert_eq!(session.memory.dropped_writes(), before);
    }

    #[test]
    fn memory_keys_and_digest_include_turns() {
        let mut session = session_with_memory();
        session.record(&Turn::new("hello"), &Reply::new("hi"), None);
        assert_eq!(
            session.memory_keys(),
            vec!["frequency_answer", "turns", "user"]
        );
        let digest = session.memory_digest();
        assert_eq!(
            digest,
            session.memory_value().digest(),
            "digest is a pure function of the full memory value"
        );
        assert_eq!(digest.hex().len(), 40);
    }

    #[test]
    fn memory_values_place_turns_first_and_memory_after_in_insertion_order() {
        let mut session = session_with_memory();
        session.record(&Turn::new("a"), &Reply::new("b"), None);
        let keys: Vec<String> = match session.memory_value() {
            Value::Object(fields) => fields.into_iter().map(|(key, _)| key).collect(),
            other => panic!("{other:?}"),
        };
        assert_eq!(keys, vec!["turns", "user", "frequency_answer"]);
    }

    #[test]
    fn relevant_memory_json_excludes_internals() {
        let mut session = session_with_memory();
        session.memory.set("turns", Value::List(vec![]));
        session.memory.set("understood", Value::None);
        let json = session.relevant_memory_json();
        // Sorted keys, like `json.dumps(..., sort_keys=True)`.
        assert_eq!(json, r#"{"frequency_answer":"","user":{}}"#);
    }

    #[test]
    fn truncate_bytes_never_splits_characters() {
        assert_eq!(truncate_bytes("abcdef", 3), "abc");
        assert_eq!(truncate_bytes("abc", 10), "abc");
        assert_eq!(truncate_bytes("ação", 3), "aç");
        assert_eq!(truncate_bytes("ação", 0), "");
        assert_eq!(truncate_bytes("😀", 3), "");
        assert_eq!(truncate_bytes("😀", 4), "😀");
    }
}
