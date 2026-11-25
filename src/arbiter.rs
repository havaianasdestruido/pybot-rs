//! Arbitration: which candidate reply wins.
//!
//! `bot/modules/arbiter.py` is 5 532 lines of documented tie-breaking rules (specialist answers
//! over generalists, recall band, never repeat the last reply, emergencies never passed over,
//! sudden grief, …). GOAL.md (Fase 7) ports the whole scoring function; Fase 1 delivers the
//! *contract* the pipeline needs, and the type the port will fill in.
//!
//! The Fase 1 arbiter is the reference bot's own documented **floor**: candidates arrive in
//! `(-priority, name)` order and, without a rule firing, the first (highest priority) wins.
//! That is exactly `runtime._choose`'s fallback (`return 0`) and the documented behaviour when
//! the arbiter "fails or returns nonsense". Every divergence from Python's richer choice is
//! visible in the differential report as a module disagreement.
//!
//! What the contract guarantees today, and what the tests pin:
//!
//! * the arbiter **cannot** see or modify the memory (it takes `&Session`-free inputs); a rule
//!   that needs memory later gets it through [`ArbiterInput::memory_view`];
//! * it never panics and never returns an out-of-range index ([`Arbiter::choose`] returns an
//!   `Option<usize>` and the runtime resolves `None` to the first candidate);
//! * it is deterministic: same inputs, same index, no hashing, no clocks.

use crate::modules::registry;
use crate::session::Session;
use crate::types::{ModuleName, Priority, Turn};
use crate::Candidate;

/// What the arbiter is allowed to look at.
///
/// Mirrors `arbiter.choose(text, memory, candidates)`: the text, a read-only view of the
/// memory, and the candidate list in arrival order.
#[derive(Debug)]
pub struct ArbiterInput<'a> {
    /// The user's message.
    pub turn: &'a Turn,
    /// The conversation, read-only (the Python arbiter receives the pre-turn memory).
    pub session: &'a Session,
    /// Candidates in `(-priority, name)` order, never empty.
    pub candidates: &'a [Candidate],
}

impl ArbiterInput<'_> {
    /// The memory as the Python arbiter sees it.
    #[must_use]
    pub fn memory_view(&self) -> &crate::memory::Memory {
        &self.session.memory
    }

    /// The last logged turn, when there is one (`memory["turns"][-1]`).
    #[must_use]
    pub fn last_turn(&self) -> Option<&crate::session::LoggedTurn> {
        self.session.turns().last()
    }
}

/// The arbitration policy.
///
/// Implementations must be deterministic and total. `None` means "no rule applies" and the
/// runtime applies the documented fallback (first candidate), exactly like `_choose`.
pub trait Arbiter: std::fmt::Debug + Send + Sync {
    /// A human-readable name of the policy, for reports and `-v`.
    fn name(&self) -> &'static str;

    /// The winning index into `input.candidates`, or `None` for "the first candidate".
    fn choose(&self, input: &ArbiterInput<'_>) -> Option<usize>;
}

/// The Fase 1 policy: arrival order (highest `PRIORITY`, then name), with the one rule that is
/// both cheap and clearly right — **never pick a reply identical to the last reply of the same
/// conversation when another candidate differs** (the Python arbiter documents this rule; the
/// corpus shows repeated greetings getting varied answers).
#[derive(Debug, Default, Clone, Copy)]
pub struct PriorityFloor;

impl PriorityFloor {
    /// The instance the runtime uses by default.
    pub const INSTANCE: Self = Self;
}

impl Arbiter for PriorityFloor {
    fn name(&self) -> &'static str {
        "priority_floor"
    }

    fn choose(&self, input: &ArbiterInput<'_>) -> Option<usize> {
        let candidates = input.candidates;
        if candidates.is_empty() {
            return None;
        }
        let last_reply = input
            .last_turn()
            .map(|turn| crate::pyutil::collapse_spaces(&turn.bot).into_owned());
        if let Some(last_reply) = last_reply {
            if let Some(chosen) = candidates.iter().position(|candidate| {
                crate::pyutil::collapse_spaces(candidate.reply.as_str()) != last_reply
            }) {
                return Some(chosen);
            }
        }
        None
    }
}

/// A candidate's sort key, `(-priority, module name)` — the order `runtime.turn` builds.
#[must_use]
pub fn candidate_key(candidate: &Candidate) -> (f64, &str) {
    (-candidate.priority.as_f64(), candidate.module.as_str())
}

/// True when `candidates` is in the canonical arrival order.
#[must_use]
pub fn is_in_arrival_order(candidates: &[Candidate]) -> bool {
    candidates
        .windows(2)
        .all(|pair| match (pair.first(), pair.get(1)) {
            (Some(left), Some(right)) => candidate_key(left) < candidate_key(right),
            _ => true,
        })
}

/// Build a candidate for the skeleton pipeline (used by the runtime and by tests).
#[must_use]
pub fn candidate(
    module: &'static str,
    priority: Priority,
    reply: crate::types::Reply,
) -> Candidate {
    Candidate {
        id: registry::lookup(module),
        module: ModuleName::from_static(module),
        priority,
        reply,
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
    use crate::types::Reply;

    fn candidates(entries: &[(&'static str, Priority, &str)]) -> Vec<Candidate> {
        entries
            .iter()
            .map(|(module, priority, text)| candidate(module, *priority, Reply::new(*text)))
            .collect()
    }

    fn input<'a>(session: &'a Session, turn: &'a Turn, list: &'a [Candidate]) -> ArbiterInput<'a> {
        ArbiterInput {
            turn,
            session,
            candidates: list,
        }
    }

    #[test]
    fn the_floor_keeps_the_first_candidate_when_everything_is_new() {
        let list = candidates(&[
            ("answers", Priority::Int(1200), "First answer."),
            ("chat", Priority::Int(300), "Second answer."),
            ("greeting", Priority::Int(-1000), "Third answer."),
        ]);
        let session = Session::new();
        let turn = Turn::new("hello");
        assert_eq!(
            PriorityFloor::INSTANCE.choose(&input(&session, &turn, &list)),
            None
        );
        assert_eq!(PriorityFloor::INSTANCE.name(), "priority_floor");
    }

    #[test]
    fn the_floor_skips_a_repeat_of_the_last_reply() {
        let mut session = Session::new();
        session.record(
            &Turn::new("hi"),
            &Reply::new("Hi there — what do you need?"),
            None,
        );
        let list = candidates(&[
            ("chat", Priority::Int(300), "Hi there  —  what do you need?"),
            ("greeting", Priority::Int(-1000), "Hello again!"),
        ]);
        let turn = Turn::new("hi");
        assert_eq!(
            PriorityFloor::INSTANCE.choose(&input(&session, &turn, &list)),
            Some(1),
            "the repeat is skipped (whitespace-insensitive comparison)"
        );
    }

    #[test]
    fn the_floor_keeps_a_repeat_when_it_is_the_only_candidate() {
        let mut session = Session::new();
        session.record(&Turn::new("hi"), &Reply::new("Hello!"), None);
        let list = candidates(&[("chat", Priority::Int(300), "Hello!")]);
        let turn = Turn::new("hi");
        assert_eq!(
            PriorityFloor::INSTANCE.choose(&input(&session, &turn, &list)),
            None
        );
    }

    #[test]
    fn the_floor_handles_empty_candidates() {
        let session = Session::new();
        let turn = Turn::new("x");
        assert_eq!(
            PriorityFloor::INSTANCE.choose(&input(&session, &turn, &[])),
            None
        );
    }

    #[test]
    fn candidate_keys_are_priority_desc_then_name() {
        let a = candidate("chat", Priority::Int(300), Reply::new("a"));
        let b = candidate("answers", Priority::Int(300), Reply::new("b"));
        let c = candidate("safety", Priority::Int(9000), Reply::new("c"));
        assert!(candidate_key(&c) < candidate_key(&a));
        assert!(
            candidate_key(&b) < candidate_key(&a),
            "same priority sorts by name"
        );
        let ordered = vec![c.clone(), b.clone(), a.clone()];
        assert!(
            is_in_arrival_order(&ordered),
            "safety@9000, answers@300, chat@300 is the arrival order"
        );
        let unordered = vec![a, c];
        assert!(!is_in_arrival_order(&unordered));
        assert!(is_in_arrival_order(&[]));
        assert!(is_in_arrival_order(&[candidate(
            "chat",
            Priority::Int(0),
            Reply::new("x")
        )]));
    }

    #[test]
    fn candidates_carry_their_registry_id() {
        let chat = candidate("chat", Priority::Int(300), Reply::new("x"));
        assert_eq!(chat.id, registry::lookup("chat"));
        assert_eq!(chat.module.as_str(), "chat");
        let ghost = candidate("not_a_module", Priority::Int(0), Reply::new("x"));
        assert_eq!(ghost.id, None);
        assert_eq!(ghost.module.as_str(), "not_a_module");
    }

    #[test]
    fn the_input_exposes_the_arbiter_view() {
        let mut session = Session::new();
        session
            .memory
            .set("user", crate::memory::Value::Object(vec![]));
        session.record(&Turn::new("hello"), &Reply::new("Hi!"), None);
        let list = candidates(&[("chat", Priority::Int(300), "x")]);
        let turn = Turn::new("y");
        let borrowed = input(&session, &turn, &list);
        assert!(borrowed.memory_view().map().contains_key("user"));
        assert_eq!(
            borrowed.last_turn().map(|entry| entry.user.as_str()),
            Some("hello")
        );
        assert_eq!(borrowed.turn.text(), "y");
    }
}
