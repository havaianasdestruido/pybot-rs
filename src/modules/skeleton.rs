//! The Fase 1 skeleton modules: real behaviour where it is cheap, honest placeholders
//! everywhere else.
//!
//! These handlers exist so the pipeline (observe → respond → arbiter → after) has something to
//! run end to end, and so the differential harness measures a *running* bot instead of a stub.
//! Nothing here claims to reproduce `bot/modules/`: each handler documents what it does and
//! what the Python original does, and the report shows the gap.
//!
//! Rules followed by all of them:
//!
//! * never panic (no indexing, no `unwrap`): input is adversarial by definition;
//! * never allocate beyond the memory budget ([`crate::limits`]);
//! * write only through [`Snapshot::set`], which reports budget violations instead of failing;
//! * deterministic: no clocks, no randomness, no iteration over unordered containers.

use crate::memory::{Snapshot, Value};
use crate::modules::registry::{find, Hook, ModuleDef};
use crate::modules::{Answer, Module};
use crate::pyutil;
use crate::types::{Reply, Turn};

/// A definition lookup for a skeleton handler. The handler names itself and the lookup happens
/// on first use, so nothing is copied into the module.
fn definition(name: &str) -> &'static ModuleDef {
    // Unreachable in practice: `tests::every_skeleton_name_exists` pins every name the
    // handlers below use. Falling back to the placeholder arbiter rather than panicking keeps
    // the crate's "no panics outside tests" promise, and a wrong row fails that test first.
    find(name).unwrap_or(&super::registry::PLACEHOLDER_ARBITER)
}

// ---------------------------------------------------------------------------- understand

/// The observer, `bot/modules/understand.py` (priority 10 000).
///
/// The Python module is the enterprise: tokenisation, chat-speak folding, coreference and
/// topic detection over 10 000+ lines of tables (Fase 4). This skeleton keeps only what the
/// memory pipeline needs to be exercised honestly:
///
/// * `memory["understood"]` — the normalised text and its word count;
/// * `memory["said_kind"]` — one of `greeting`, `thanks`, `bye`, `recall`, `question`,
///   `statement` (the Python module also records a `turn` index);
/// * `memory["user"]["name"]` — the name in `my name is X` / `i'm X` / `call me X`.
#[derive(Debug)]
pub struct Understanding;

/// The singleton registered in [`super::HANDLERS`] (as `skeleton::UNDERSTANDING`).
pub static UNDERSTANDING: Understanding = Understanding;

impl Module for Understanding {
    fn def(&self) -> &'static ModuleDef {
        definition("understand")
    }

    fn observe(&self, turn: &Turn, mut snapshot: Snapshot) -> Snapshot {
        let text = turn.text();
        let normalised = pyutil::collapse_spaces(text).into_owned();
        let lower = normalised.to_lowercase();
        let word_count = normalised
            .split(pyutil::is_space)
            .filter(|word| !word.is_empty())
            .count();

        snapshot.set(
            "understood",
            Value::Object(vec![
                ("text".to_string(), Value::Str(normalised)),
                (
                    "words".to_string(),
                    Value::Int(i64::try_from(word_count).unwrap_or(i64::MAX)),
                ),
            ]),
        );

        let kind = classify(&lower);
        snapshot.set(
            "said_kind",
            Value::Object(vec![
                ("kind".to_string(), Value::Str(kind.to_string())),
                (
                    "turn".to_string(),
                    Value::Int(i64::try_from(turn_count(&snapshot)).unwrap_or(i64::MAX)),
                ),
            ]),
        );

        if let Some(name) = extract_name(text) {
            snapshot.set(
                "user",
                Value::Object(vec![("name".to_string(), Value::Str(name))]),
            );
        }
        snapshot
    }
}

/// The number of turns the runtime already logged (mirrored in the snapshot).
fn turn_count(snapshot: &Snapshot) -> usize {
    snapshot
        .get("turns")
        .and_then(Value::as_list)
        .map_or(0, <[Value]>::len)
}

/// A coarse classification of the line, mirroring the names the corpus' `said_kind` uses.
fn classify(lower: &str) -> &'static str {
    // The first word without punctuation: "howdy!" and "hi," are still greetings, the way the
    // recognisers in `bot/modules` compare a stripped token rather than the raw slice.
    let first = lower
        .split(pyutil::is_space)
        .find(|word| !word.is_empty())
        .map(|word| word.trim_matches(|character: char| !character.is_alphabetic()));
    let is = |words: &[&str]| first.is_some_and(|word| words.contains(&word));
    // The marker may follow a greeting: "hi, i'm Sam" is still a self-introduction. Require a
    // word boundary in front so "aim " or "slim " do not count.
    let introduces_self = |marker: &str| {
        lower.match_indices(marker).any(|(index, _)| {
            index == 0
                || !lower[..index]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphabetic)
        })
    };
    if lower.contains("my name is")
        || lower.contains("call me ")
        || introduces_self("i'm ")
        || introduces_self("im ")
        || introduces_self("i am ")
    {
        return "identity";
    }
    if lower.contains("what's my name")
        || lower.contains("what is my name")
        || lower.contains("who am i")
    {
        return "recall";
    }
    if is(&["hi", "hello", "hey", "hiya", "yo", "howdy"]) {
        return "greeting";
    }
    if is(&["thanks", "thank", "cheers", "ta"]) || lower.contains("thank you") {
        return "thanks";
    }
    if is(&["bye", "goodbye", "see"]) || lower == "goodbye" {
        return "bye";
    }
    if lower.ends_with('?') {
        return "question";
    }
    if lower.is_empty() {
        return "silence";
    }
    "statement"
}

/// `my name is Sam` → `Sam`; `call me Ana` → `Ana`; `i'm Tom` → `Tom`.
///
/// Capitalises the first letter like the Python modules do, and rejects candidates that are
/// too long, contain digits or are one of the words that follow `i'm` in normal speech
/// (`i'm tired`, `i'm sorry`).
fn extract_name(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let rest = ["my name is ", "call me ", "i'm ", "i am ", "im "]
        .iter()
        .find_map(|marker| lower.find(marker).map(|index| index + marker.len()))?;
    let candidate = text.get(rest..)?;
    let candidate = candidate.split(pyutil::is_space).next().unwrap_or("");
    let candidate = candidate.trim_matches(|character: char| {
        !character.is_alphanumeric() && character != '-' && character != '\''
    });
    if candidate.len() < 2 || candidate.len() > 32 {
        return None;
    }
    if !candidate
        .chars()
        .all(|character| character.is_alphabetic() || character == '-' || character == '\'')
    {
        return None;
    }
    const NOT_NAMES: &[&str] = &[
        "sorry", "tired", "sad", "happy", "fine", "okay", "ok", "good", "great", "angry", "bored",
        "stressed", "anxious", "lonely", "here", "back", "sure", "not", "so", "very", "really",
        "just", "still", "afraid", "worried", "excited", "confused", "lost", "hungry", "cold",
        "warm", "hot", "better", "worse", "done", "trying", "going", "looking", "thinking",
        "feeling", "working", "about", "on", "at", "in", "from", "a", "an", "the", "and", "but",
        "so", "then", "now", "today", "tomorrow",
    ];
    let lower_candidate = candidate.to_lowercase();
    if NOT_NAMES.contains(&lower_candidate.as_str()) {
        return None;
    }
    let mut characters = candidate.chars();
    let first = characters.next()?;
    let mut name = String::with_capacity(candidate.len());
    name.extend(first.to_uppercase());
    name.push_str(characters.as_str());
    Some(name)
}

// ---------------------------------------------------------------------------- intent

/// The second observer, `bot/modules/intent.py` (priority 9 500).
///
/// The Python module extracts what the user wants done ("intent"). The skeleton records the
/// leading verb for questions that start with one, and a boolean for the imperative mood,
/// under `memory["intent_wants"]`, which is the key the corpus already shows.
#[derive(Debug)]
pub struct ObserveIntent;

/// The singleton registered in [`super::HANDLERS`] (as `skeleton::OBSERVE_INTENT`).
pub static OBSERVE_INTENT: ObserveIntent = ObserveIntent;

impl Module for ObserveIntent {
    fn def(&self) -> &'static ModuleDef {
        definition("intent")
    }

    fn observe(&self, turn: &Turn, mut snapshot: Snapshot) -> Snapshot {
        let lower = turn.text().to_lowercase();
        let words: Vec<&str> = lower
            .split(pyutil::is_space)
            .filter(|w| !w.is_empty())
            .collect();
        let first = words.first().copied().unwrap_or("");
        let wants = match first {
            "how" => Some("method"),
            "why" => Some("reason"),
            "what" | "which" => Some("definition"),
            "who" => Some("person"),
            "when" => Some("time"),
            "where" => Some("place"),
            _ => None,
        };
        let value = match wants {
            Some(want) => Value::Str(want.to_string()),
            None => Value::None,
        };
        snapshot.set("intent_wants", value);
        snapshot
    }
}

// ---------------------------------------------------------------------------- greeting

/// `bot/modules/greeting.py` (priority −1000): the last module that still replies.
///
/// Real replies for a real subset: greetings and name recall. Everything else passes, which is
/// what makes the `chat`/`answers` gap show up in the report instead of being papered over by
/// a second-rate imitation.
#[derive(Debug)]
pub struct Greeter;

/// The singleton registered in [`super::HANDLERS`] (as `skeleton::GREETER`).
pub static GREETER: Greeter = Greeter;

impl Module for Greeter {
    fn def(&self) -> &'static ModuleDef {
        definition("greeting")
    }

    fn respond(&self, turn: &Turn, snapshot: Snapshot) -> Answer {
        let lower = pyutil::collapse_spaces(turn.text()).to_lowercase();
        if lower.is_empty() {
            return None;
        }
        let stripped = lower.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'');

        // "what's my name" family: reply with the remembered name, or say we do not know.
        if stripped.contains("what's my name")
            || stripped.contains("what is my name")
            || stripped.contains("who am i")
            || stripped.contains("do you know my name")
        {
            let name = snapshot
                .get("user")
                .and_then(|user| user.get("name"))
                .and_then(Value::as_str);
            return Some(match name {
                Some(name) => Reply::new(format!("You're {name}.")),
                None => Reply::new("I don't know it yet."),
            });
        }

        // Greetings, keyed on the opening word — with its punctuation taken off, so "hi, i'm
        // Sam" and "hello!" still open the chat rather than falling to the fallback.
        let first = stripped
            .split(pyutil::is_space)
            .next()
            .unwrap_or("")
            .trim_matches(|character: char| !character.is_alphanumeric());
        let greeting = match first {
            "hi" | "hello" | "hey" | "hiya" | "yo" | "howdy" | "greetings" => true,
            _ => {
                stripped.starts_with("good morning")
                    || stripped.starts_with("good afternoon")
                    || stripped.starts_with("good evening")
                    || stripped.starts_with("nice to meet you")
            }
        };
        if !greeting {
            return None;
        }
        let name = snapshot
            .get("user")
            .and_then(|user| user.get("name"))
            .and_then(Value::as_str);
        Some(match name {
            Some(name) => Reply::new(format!("Nice to meet you, {name}. What can I help with?")),
            None => Reply::new("Hi there — what do you need?"),
        })
    }
}

// ---------------------------------------------------------------------------- answers (stub)

/// `bot/modules/answers.py` — the module that answers most knowledge questions.
///
/// The placeholder never replies. Its presence in the table (and its absence from candidate
/// lists) is the single largest source of corpus disagreements, and therefore the number the
/// report exists to make visible.
#[derive(Debug)]
pub struct AnswersStub;

/// The singleton registered in [`super::HANDLERS`] (as `skeleton::ANSWERS`).
pub static ANSWERS: AnswersStub = AnswersStub;

impl Module for AnswersStub {
    fn def(&self) -> &'static ModuleDef {
        definition("answers")
    }
}

// ---------------------------------------------------------------------------- chat (after)

/// `bot/modules/chat.py` (priority 300) — only its `after` hook is ported here.
///
/// The Python `after` records whether the previous reply was a "frequency" answer (how often /
/// how long did that take), which later turns read as `memory["frequency_answer"]` /
/// `memory["frequency_reply"]`, keys that already appear in the corpus.
#[derive(Debug)]
pub struct ChatAfter;

/// The singleton registered in [`super::HANDLERS`] (as `skeleton::CHAT`).
pub static CHAT: ChatAfter = ChatAfter;

impl Module for ChatAfter {
    fn def(&self) -> &'static ModuleDef {
        definition("chat")
    }

    fn after(&self, turn: &Turn, reply: &Reply, mut snapshot: Snapshot) -> Snapshot {
        let lower = turn.text().to_lowercase();
        // Python's `after` marks whether the reply just given *was* the frequency pool's answer
        // (`str(reply) in pool`), not merely whether the line asked how often. Without the
        // pools (Fase 2), the skeleton cannot decide that, so it records `false` and says so
        // in the differential report rather than inventing a positive.
        let _ = (&lower, reply);
        snapshot.set("frequency_reply", Value::Bool(false));
        snapshot.set("frequency_answer", Value::Str(String::new()));
        snapshot
    }
}

// ---------------------------------------------------------------------------- memory_store (after)

/// `bot/modules/memory_store.py` (priority 0) — only its `after` hook is ported here.
///
/// The Python `after` keeps `memory["user"]` a dict (creating it if a module removed it) so
/// later turns can store facts about the user. The skeleton does the same, minimally.
#[derive(Debug)]
pub struct MemoryAfter;

/// The singleton registered in [`super::HANDLERS`] (as `skeleton::MEMORY_STORE`).
pub static MEMORY_STORE: MemoryAfter = MemoryAfter;

impl Module for MemoryAfter {
    fn def(&self) -> &'static ModuleDef {
        definition("memory_store")
    }

    fn after(&self, _turn: &Turn, _reply: &Reply, mut snapshot: Snapshot) -> Snapshot {
        if !matches!(snapshot.get("user"), Some(Value::Object(_))) {
            snapshot.set("user", Value::Object(Vec::new()));
        }
        snapshot
    }
}

// ---------------------------------------------------------------------------- arbiter wrapper

/// The registry face of the arbiter.
///
/// `bot/modules/arbiter.py` defines `choose(text, memory, candidates)` and no `respond`; the
/// runtime calls it directly (not through the [`Module`] trait, which has no `choose`). This
/// marker keeps the table aligned and the registry honest.
#[derive(Debug)]
pub struct NoopArbiter;

/// The singleton registered in [`super::HANDLERS`] (as `skeleton::ARBITER_MODULE`).
pub static ARBITER_MODULE: NoopArbiter = NoopArbiter;

impl Module for NoopArbiter {
    fn def(&self) -> &'static ModuleDef {
        definition("arbiter")
    }
}

/// The skeleton module names and their hooks, for the alignment test above.
#[must_use]
pub fn skeleton_hooks() -> Vec<(&'static str, &'static [Hook])> {
    [
        "understand",
        "intent",
        "chat",
        "greeting",
        "memory_store",
        "answers",
        "arbiter",
    ]
    .iter()
    .filter_map(|name| find(name).map(|module| (module.name, module.hooks)))
    .collect()
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
    use crate::modules::registry;
    use crate::types::ModuleName;

    fn module_for(name: &str) -> &'static dyn Module {
        let id = registry::lookup(name).unwrap_or_else(|| panic!("{name} must exist"));
        crate::modules::handler_for(id).unwrap_or_else(|| panic!("{name} must have a handler"))
    }

    // ------------------------------------------------------------------ names exist

    #[test]
    fn every_skeleton_name_exists() {
        for name in [
            "understand",
            "intent",
            "chat",
            "greeting",
            "memory_store",
            "answers",
            "arbiter",
        ] {
            let module = module_for(name);
            assert_eq!(module.def().name, name);
            assert_eq!(module.name(), ModuleName::from_static(name));
        }
        assert_eq!(skeleton_hooks().len(), 7);
    }

    #[test]
    fn skeleton_handlers_only_use_hooks_their_modules_declare() {
        assert!(module_for("understand").def().has(Hook::Observe));
        assert!(module_for("intent").def().has(Hook::Observe));
        assert!(module_for("greeting").def().has(Hook::Respond));
        assert!(module_for("chat").def().has(Hook::After));
        assert!(module_for("memory_store").def().has(Hook::After));
        assert!(module_for("arbiter").def().has(Hook::Choose));
        assert!(
            module_for("answers").def().proposes(),
            "answers has respond() in Python"
        );
    }

    // ------------------------------------------------------------------ understand

    #[test]
    fn understanding_records_normalised_text_and_kind() {
        let snapshot =
            UNDERSTANDING.observe(&Turn::new("  Hello \u{1c} there  "), Snapshot::default());
        assert_eq!(
            snapshot
                .get("understood")
                .and_then(|value| value.get("text")),
            Some(&Value::string("Hello there"))
        );
        assert_eq!(
            snapshot
                .get("understood")
                .and_then(|value| value.get("words")),
            Some(&Value::Int(2))
        );
        assert_eq!(
            snapshot
                .get("said_kind")
                .and_then(|value| value.get("kind")),
            Some(&Value::string("greeting"))
        );
        assert_eq!(
            snapshot
                .get("said_kind")
                .and_then(|value| value.get("turn")),
            Some(&Value::Int(0))
        );
    }

    #[test]
    fn understanding_classifies_every_kind() {
        let kind_of = |text: &str| {
            UNDERSTANDING
                .observe(&Turn::new(text), Snapshot::default())
                .get("said_kind")
                .and_then(|value| value.get("kind"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        assert_eq!(kind_of("my name is Sam").as_deref(), Some("identity"));
        assert_eq!(kind_of("hi, i'm Sam").as_deref(), Some("identity"));
        assert_eq!(kind_of("what's my name?").as_deref(), Some("recall"));
        assert_eq!(kind_of("who am i").as_deref(), Some("recall"));
        assert_eq!(kind_of("hello").as_deref(), Some("greeting"));
        assert_eq!(kind_of("howdy!").as_deref(), Some("greeting"));
        assert_eq!(kind_of("thanks").as_deref(), Some("thanks"));
        assert_eq!(kind_of("thank you so much").as_deref(), Some("thanks"));
        assert_eq!(kind_of("bye").as_deref(), Some("bye"));
        assert_eq!(kind_of("see you later").as_deref(), Some("bye"));
        assert_eq!(
            kind_of("how long should i boil an egg?").as_deref(),
            Some("question")
        );
        assert_eq!(kind_of("the kettle is loud").as_deref(), Some("statement"));
        assert_eq!(kind_of("   ").as_deref(), Some("silence"));
    }

    #[test]
    fn understanding_extracts_names_but_not_words_that_follow_im() {
        let name_of = |text: &str| {
            UNDERSTANDING
                .observe(&Turn::new(text), Snapshot::default())
                .get("user")
                .and_then(|user| user.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        assert_eq!(name_of("my name is Sam").as_deref(), Some("Sam"));
        assert_eq!(name_of("my name is priya").as_deref(), Some("Priya"));
        assert_eq!(name_of("hi, i'm sam").as_deref(), Some("Sam"));
        assert_eq!(name_of("call me marco").as_deref(), Some("Marco"));
        assert_eq!(name_of("i'm JoAo").as_deref(), Some("JoAo"));
        assert_eq!(name_of("i'm tired"), None);
        assert_eq!(name_of("i'm sorry"), None);
        assert_eq!(name_of("i'm just looking around"), None);
        assert_eq!(name_of("i'm 42"), None);
        assert_eq!(name_of("my name is A"), None, "too short");
        assert_eq!(name_of("my name is"), None);
        assert_eq!(name_of("hello"), None);
    }

    #[test]
    fn understanding_keeps_unicode_names_and_apostrophes() {
        let name_of = |text: &str| {
            UNDERSTANDING
                .observe(&Turn::new(text), Snapshot::default())
                .get("user")
                .and_then(|user| user.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        assert_eq!(name_of("my name is João").as_deref(), Some("João"));
        assert_eq!(name_of("my name is O'Brien").as_deref(), Some("O'Brien"));
        assert_eq!(name_of("call me Anne-Marie").as_deref(), Some("Anne-Marie"));
        assert_eq!(name_of("hi, i'm 李").as_deref(), Some("李"));
    }

    #[test]
    fn understanding_counts_logged_turns_from_the_mirror() {
        let mut session = crate::session::Session::new();
        session.record(&Turn::new("one"), &Reply::new("1"), None);
        session.record(&Turn::new("two"), &Reply::new("2"), None);
        let snapshot = UNDERSTANDING.observe(&Turn::new("three"), session.snapshot());
        assert_eq!(
            snapshot
                .get("said_kind")
                .and_then(|value| value.get("turn")),
            Some(&Value::Int(2))
        );
    }

    // ------------------------------------------------------------------ intent

    #[test]
    fn intent_records_the_kind_of_question() {
        let intent_of = |text: &str| {
            OBSERVE_INTENT
                .observe(&Turn::new(text), Snapshot::default())
                .get("intent_wants")
                .cloned()
        };
        assert_eq!(
            intent_of("how do i descale a kettle"),
            Some(Value::string("method"))
        );
        assert_eq!(
            intent_of("why is the sky blue"),
            Some(Value::string("reason"))
        );
        assert_eq!(
            intent_of("what is photosynthesis"),
            Some(Value::string("definition"))
        );
        assert_eq!(intent_of("who wrote hamlet"), Some(Value::string("person")));
        assert_eq!(
            intent_of("when is my appointment"),
            Some(Value::string("time"))
        );
        assert_eq!(
            intent_of("where is the station"),
            Some(Value::string("place"))
        );
        assert_eq!(intent_of("the kettle is loud"), Some(Value::None));
        assert_eq!(intent_of(""), Some(Value::None));
    }

    // ------------------------------------------------------------------ greeting

    #[test]
    fn greeter_replies_to_greetings() {
        let answer = |text: &str, snapshot: Snapshot| GREETER.respond(&Turn::new(text), snapshot);
        assert_eq!(
            answer("hello", Snapshot::default()).map(crate::types::Reply::into_string),
            Some("Hi there — what do you need?".to_string())
        );
        assert_eq!(
            answer("HI THERE", Snapshot::default()).map(crate::types::Reply::into_string),
            Some("Hi there — what do you need?".to_string())
        );
        assert_eq!(
            answer("good morning!", Snapshot::default()).map(crate::types::Reply::into_string),
            Some("Hi there — what do you need?".to_string())
        );
        assert!(answer("how do i boil an egg", Snapshot::default()).is_none());
        assert!(answer("", Snapshot::default()).is_none());
        assert!(answer("   ", Snapshot::default()).is_none());
        assert!(answer("thanks", Snapshot::default()).is_none());
    }

    #[test]
    fn greeter_uses_the_remembered_name() {
        let mut snapshot = Snapshot::default();
        snapshot.set(
            "user",
            Value::Object(vec![("name".to_string(), Value::string("Sam"))]),
        );
        assert_eq!(
            GREETER
                .respond(&Turn::new("hi"), snapshot.clone())
                .map(crate::types::Reply::into_string),
            Some("Nice to meet you, Sam. What can I help with?".to_string())
        );
    }

    #[test]
    fn greeter_handles_name_recall_with_and_without_a_name() {
        let empty = Snapshot::default();
        assert_eq!(
            GREETER
                .respond(&Turn::new("what's my name?"), empty)
                .map(crate::types::Reply::into_string),
            Some("I don't know it yet.".to_string())
        );
        let mut named = Snapshot::default();
        named.set(
            "user",
            Value::Object(vec![("name".to_string(), Value::string("Priya"))]),
        );
        for text in [
            "what's my name?",
            "What Is My Name",
            "who am i?",
            "do you know my name?",
        ] {
            assert_eq!(
                GREETER
                    .respond(&Turn::new(text), named.clone())
                    .map(crate::types::Reply::into_string),
                Some("You're Priya.".to_string()),
                "{text}"
            );
        }
    }

    // ------------------------------------------------------------------ answers stub

    #[test]
    fn the_answers_stub_never_answers_anything() {
        for text in [
            "hello",
            "what is the capital of france",
            "",
            "how long to boil an egg",
        ] {
            assert!(ANSWERS
                .respond(&Turn::new(text), Snapshot::default())
                .is_none());
        }
    }

    // ------------------------------------------------------------------ after hooks

    #[test]
    fn chat_after_always_records_the_pair_as_unset_in_phase_one() {
        // Python sets `frequency_reply` from "was this reply in the frequency pool?", and the
        // pools arrive in Fase 2. Until then the skeleton records the pair as unset — a
        // documented gap the differential report measures, never a claim it cannot back.
        for (text, reply) in [
            ("how long does that take", "Half an hour."),
            ("the kettle is loud", "Try vinegar."),
            ("", ""),
        ] {
            let snapshot = CHAT.after(&Turn::new(text), &Reply::new(reply), Snapshot::default());
            assert_eq!(snapshot.get("frequency_reply"), Some(&Value::Bool(false)));
            assert_eq!(snapshot.get("frequency_answer"), Some(&Value::string("")));
        }
    }

    #[test]
    fn memory_after_keeps_user_a_dict() {
        let snapshot = MEMORY_STORE.after(&Turn::new("x"), &Reply::new("y"), Snapshot::default());
        assert_eq!(snapshot.get("user"), Some(&Value::Object(vec![])));
        // An existing dict is left alone.
        let mut existing = Snapshot::default();
        existing.set(
            "user",
            Value::Object(vec![("name".to_string(), Value::string("Sam"))]),
        );
        let after = MEMORY_STORE.after(&Turn::new("x"), &Reply::new("y"), existing.clone());
        assert_eq!(after.get("user"), existing.get("user"));
        // A non-dict user (a module scribbled) is replaced by a fresh dict.
        let mut broken = Snapshot::default();
        broken.set("user", Value::string("oops"));
        let after = MEMORY_STORE.after(&Turn::new("x"), &Reply::new("y"), broken);
        assert_eq!(after.get("user"), Some(&Value::Object(vec![])));
    }
}
