//! The turn engine: `observe → respond → arbitrate → after → log`, with panics contained.
//!
//! This is `bot/runtime.py::turn` in Rust, with three deliberate differences that GOAL.md
//! demands and the tests pin:
//!
//! 1. **Panics are faults, not crashes.** Python's `except Exception` cannot save a segfault,
//!    but it does swallow a module's exception; Rust's equivalent is `catch_unwind`. Every hook
//!    call of every module runs inside one, and any panic is counted in [`Faults`] with the
//!    module name. Nothing propagates to the caller: a turn always returns a [`TurnReport`].
//! 2. **Limits are explicit.** The input is truncated to [`limits::MAX_INPUT_CHARS`], the
//!    candidate list to [`limits::MAX_CANDIDATES`], the memory to its key/value budget. Each
//!    clamp is counted in [`Faults`].
//! 3. **The pipeline is testable in pieces.** [`Engine::observe_all`], [`Engine::respond_all`],
//!    [`Engine::finish`] are public so the differential harness can compare intermediate
//!    stages in later phases (`GOAL.md` Fase 4 asks for exactly that), and so the unit tests
//!    can drive each phase in isolation.
//!
//! Ordering is fully deterministic: the registry order (`-priority, name`) is the iteration
//! order of every phase, candidates are kept in that order, and the arbiter is a pure
//! function of (text, memory, candidates).

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::arbiter::{Arbiter, ArbiterInput};
use crate::limits;
use crate::memory::Snapshot;
use crate::modules::{self, registry, Module};
use crate::runtime::{Candidate, Faults, TurnReport};
use crate::session::{LoggedTurn, Session};
use crate::types::{ModuleId, ModuleName, Priority, Reply, Turn};

/// The outcome of the `observe` phase.
#[derive(Debug, Default)]
pub struct Observed {
    /// The memory after every observer ran, in registry order.
    pub memory: Snapshot,
}

/// A panic payload turned into a short, allocation-free description.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        format!("panic: {text}")
    } else if let Some(text) = payload.downcast_ref::<String>() {
        format!("panic: {text}")
    } else {
        String::from("panic: non-string payload")
    }
}

/// Apply one hook inside `catch_unwind`, recording a fault on panic.
fn guarded<T, F>(
    module: &'static registry::ModuleDef,
    hook: registry::Hook,
    faults: &mut Faults,
    work: F,
) -> Option<T>
where
    F: FnOnce() -> T,
{
    match catch_unwind(AssertUnwindSafe(work)) {
        Ok(value) => Some(value),
        Err(payload) => {
            faults.push(crate::runtime::Fault {
                module: module.name,
                hook: hook.as_str(),
                message: panic_message(payload.as_ref()),
            });
            None
        }
    }
}

/// The engine configuration: which arbiter, and the limits already baked into [`limits`].
#[derive(Debug)]
pub struct Engine {
    arbiter: &'static dyn Arbiter,
}

impl Engine {
    /// The default engine (the [`arbiter::PriorityFloor`](crate::arbiter::PriorityFloor) policy).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            arbiter: &crate::arbiter::PriorityFloor::INSTANCE,
        }
    }

    /// An engine with a custom arbitration policy.
    #[must_use]
    pub const fn with_arbiter(arbiter: &'static dyn Arbiter) -> Self {
        Self { arbiter }
    }

    /// The arbitration policy in use.
    #[must_use]
    pub const fn arbiter(&self) -> &'static dyn Arbiter {
        self.arbiter
    }

    /// Clamp `turn` to the input budget, recording whether anything was cut.
    ///
    /// A free function rather than a method: it depends only on the turn and the limits.
    fn bounded_turn<'a>(turn: &'a Turn, faults: &mut Faults) -> std::borrow::Cow<'a, Turn> {
        if turn.text().chars().count() <= limits::MAX_INPUT_CHARS {
            return std::borrow::Cow::Borrowed(turn);
        }
        faults.input_truncated = true;
        let text = limits::truncate_chars(turn.text(), limits::MAX_INPUT_CHARS);
        std::borrow::Cow::Owned(Turn::new(text))
    }

    /// Phase 1 — `observe`: every observer, in registry order, on the session's snapshot.
    ///
    /// A panicking observer is skipped (fault recorded); the next one still sees the memory
    /// as it was before the panicking one started, because each call works on its own copy.
    pub fn observe_all(&self, turn: &Turn, session: &Session, faults: &mut Faults) -> Snapshot {
        let mut snapshot = session.snapshot();
        for module in registry::with_hook(registry::Hook::Observe) {
            let Some(handler) = handler_or_fault(module, faults) else {
                continue;
            };
            let input = snapshot.clone();
            if let Some(result) = guarded(module, registry::Hook::Observe, faults, || {
                handler.observe(turn, input)
            }) {
                snapshot = result;
            }
        }
        snapshot
    }

    /// Phase 2 — `respond`: every proposer, each on its own copy of the observed memory.
    ///
    /// Returns `(candidates, snapshots)`: parallel vectors, the snapshots being what each
    /// module would install if it won. Blank or absent answers are not candidates (Python:
    /// `isinstance(answer, str) and answer.strip()`).
    pub fn respond_all(
        &self,
        turn: &Turn,
        memory: &Snapshot,
        faults: &mut Faults,
    ) -> (Vec<Candidate>, Vec<Snapshot>) {
        let mut candidates = Vec::new();
        let mut snapshots = Vec::new();
        for module in registry::with_hook(registry::Hook::Respond) {
            if candidates.len() >= limits::MAX_CANDIDATES {
                faults.candidates_dropped = faults.candidates_dropped.saturating_add(1);
                continue;
            }
            let Some(handler) = handler_or_fault(module, faults) else {
                continue;
            };
            let mine = memory.clone();
            let answer = guarded(module, registry::Hook::Respond, faults, || {
                handler.respond(turn, mine)
            });
            let (Some(answer), Some(snapshot)) = (answer, Some(clone_snapshot(memory))) else {
                continue;
            };
            let Some(reply) = answer else {
                continue;
            };
            if reply.is_blank() {
                continue;
            }
            candidates.push(Candidate {
                id: registry::lookup(module.name),
                module: module.module_name(),
                priority: module.priority,
                reply,
            });
            snapshots.push(snapshot);
        }
        (candidates, snapshots)
    }

    /// Phase 3 — `after`: the winning memory, every `after` hook, in registry order.
    pub fn run_after(
        &self,
        turn: &Turn,
        reply: &Reply,
        mut snapshot: Snapshot,
        faults: &mut Faults,
    ) -> Snapshot {
        for module in registry::with_hook(registry::Hook::After) {
            let Some(handler) = handler_or_fault(module, faults) else {
                continue;
            };
            let input = snapshot.clone();
            if let Some(result) = guarded(module, registry::Hook::After, faults, || {
                handler.after(turn, reply, input)
            }) {
                snapshot = result;
            }
        }
        snapshot
    }

    /// One full turn: everything `runtime.turn(text, memory)` does, minus the part that
    /// mutates a caller-owned dict — here the [`Session`] is updated and a [`TurnReport`]
    /// returned.
    ///
    /// Never fails and never panics: panics inside modules become faults, an empty candidate
    /// list becomes the `"Hello!"` fallback, and the function's return type says so.
    pub fn step(&self, turn: &Turn, session: &mut Session) -> TurnReport {
        let mut faults = Faults::default();
        let turn = Self::bounded_turn(turn, &mut faults);
        let turn = turn.as_ref();

        let observed = self.observe_all(turn, session, &mut faults);
        let (mut candidates, snapshots) = self.respond_all(turn, &observed, &mut faults);

        // The arbiter's view of the candidates must be the arrival order.
        if !crate::arbiter::is_in_arrival_order(&candidates) {
            candidates.sort_by(|left, right| {
                crate::arbiter::candidate_key(left)
                    .partial_cmp(&crate::arbiter::candidate_key(right))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            faults.candidates_reordered = true;
        }

        let (reply, winner) = if candidates.is_empty() {
            (Reply::fallback(), None)
        } else {
            let input = ArbiterInput {
                turn,
                session,
                candidates: &candidates,
            };
            let picked = guarded(
                registry::find("arbiter").unwrap_or(&registry::PLACEHOLDER_ARBITER),
                registry::Hook::Choose,
                &mut faults,
                || self.arbiter.choose(&input),
            );
            let index = match picked {
                Some(Some(index)) if index < candidates.len() => index,
                _ => 0,
            };
            let candidate = candidates.get(index);
            let reply = candidate.map_or_else(Reply::fallback, |c| c.reply.clone());
            let module = candidate.map(|c| c.module.clone());
            faults.arbiter_choice = Some(index);
            (reply, module)
        };

        // Only the winner's memory survives.
        let winning_memory = winner
            .as_ref()
            .and_then(super::types::ModuleName::id)
            .and_then(|id| {
                snapshots
                    .get(candidates.iter().position(|c| c.id == Some(id))?)
                    .cloned()
            })
            .unwrap_or(observed.clone());
        let after = self.run_after(turn, &reply, winning_memory, &mut faults);
        session.apply(&after);
        faults.memory_dropped_writes = session
            .memory
            .dropped_writes()
            .saturating_add(after.dropped_writes());

        session.log_turn(LoggedTurn {
            user: turn.text().to_string(),
            bot: reply.as_str().to_string(),
            module: winner.as_ref().map(|name| name.as_str().to_string()),
        });

        TurnReport {
            reply,
            module: winner,
            candidates,
            faults,
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// The handler of a registry row, or `None` when the module is a skeleton without behaviour.
///
/// A module that declares a hook but has no handler is *not* a fault: it simply has not been
/// ported yet (Fase 1). This is the "skip silently" half of Python's `except Exception:
/// continue`, made explicit.
fn handler_or_fault(
    module: &'static registry::ModuleDef,
    _faults: &mut Faults,
) -> Option<&'static dyn Module> {
    let id = registry::lookup(module.name)?;
    modules::handler_for(id)
}

fn clone_snapshot(snapshot: &Snapshot) -> Snapshot {
    snapshot.clone()
}

/// Convenience: the module ids that define a hook, in order (for reports and tests).
#[must_use]
pub fn hook_modules(hook: registry::Hook) -> Vec<ModuleId> {
    registry::with_hook(hook)
        .filter_map(|module| registry::lookup(module.name))
        .collect()
}

/// Convenience: a candidate's priority as the report prints it.
#[must_use]
pub fn priority_of(module: &ModuleName<'_>) -> Option<Priority> {
    module
        .id()
        .and_then(registry::get)
        .map(|module| module.priority)
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
    use crate::memory::Value;
    use crate::modules::registry::ModuleDef;

    // ------------------------------------------------------------------ a controllable module

    #[derive(Debug)]
    struct Scripted {
        def: &'static ModuleDef,
        observe: Option<fn(&Turn, Snapshot) -> Snapshot>,
        respond: Option<fn(&Turn, Snapshot) -> Option<Reply>>,
        after: Option<fn(&Turn, &Reply, Snapshot) -> Snapshot>,
    }

    impl Module for Scripted {
        fn def(&self) -> &'static ModuleDef {
            self.def
        }

        fn observe(&self, turn: &Turn, snapshot: Snapshot) -> Snapshot {
            match self.observe {
                Some(function) => function(turn, snapshot),
                None => snapshot,
            }
        }

        fn respond(&self, turn: &Turn, snapshot: Snapshot) -> Option<Reply> {
            self.respond.and_then(|function| function(turn, snapshot))
        }

        fn after(&self, turn: &Turn, reply: &Reply, snapshot: Snapshot) -> Snapshot {
            match self.after {
                Some(function) => function(turn, reply, snapshot),
                None => snapshot,
            }
        }
    }

    static PANICKING: ModuleDef = ModuleDef::new(
        "panicky",
        Priority::Int(0),
        &[registry::Hook::Observe, registry::Hook::Respond],
    );

    #[test]
    fn panics_inside_a_hook_are_contained_and_counted() {
        let module = Scripted {
            def: &PANICKING,
            observe: Some(|_turn, _snapshot| -> Snapshot { panic!("observer exploded") }),
            respond: Some(|_turn, _snapshot| -> Option<Reply> { panic!("responder exploded") }),
            after: None,
        };
        let boxed: &'static dyn Module = Box::leak(Box::new(module));
        assert_eq!(boxed.def().name, "panicky");
        // The `guarded` helper is the unit under test: it must swallow and record.
        let mut faults = Faults::default();
        let outcome = guarded(&PANICKING, registry::Hook::Respond, &mut faults, || {
            boxed.respond(&Turn::new("x"), Snapshot::default())
        });
        assert_eq!(outcome, None);
        assert_eq!(faults.len(), 1);
        let fault = faults.faults().first().unwrap();
        assert_eq!(fault.module, "panicky");
        assert_eq!(fault.hook, "respond");
        assert!(fault.message.contains("responder exploded"));
    }

    #[test]
    fn panic_payloads_of_every_shape_get_a_message() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("static str");
        assert_eq!(panic_message(payload.as_ref()), "panic: static str");
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("owned"));
        assert_eq!(panic_message(payload.as_ref()), "panic: owned");
        let payload: Box<dyn std::any::Any + Send> = Box::new(42_u8);
        assert_eq!(panic_message(payload.as_ref()), "panic: non-string payload");
    }

    // ------------------------------------------------------------------ phases on real modules

    #[test]
    fn observe_runs_the_real_observers_in_registry_order() {
        let engine = Engine::new();
        let session = Session::new();
        let mut faults = Faults::default();
        let observed = engine.observe_all(&Turn::new("hi, my name is Sam"), &session, &mut faults);
        assert!(faults.is_empty(), "{faults:?}");
        assert_eq!(
            observed.get("user").and_then(|user| user.get("name")),
            Some(&Value::string("Sam")),
            "understand ran"
        );
        assert_eq!(
            observed.get("intent_wants"),
            Some(&Value::None),
            "intent ran"
        );
        let order: Vec<&str> = registry::with_hook(registry::Hook::Observe)
            .map(|module| module.name)
            .collect();
        assert_eq!(order.first(), Some(&"understand"));
        assert_eq!(order.get(1), Some(&"intent"));
    }

    #[test]
    fn respond_collects_candidates_in_registry_order_with_their_snapshots() {
        let engine = Engine::new();
        let session = Session::new();
        let mut faults = Faults::default();
        let observed = engine.observe_all(&Turn::new("hello"), &session, &mut faults);
        let (candidates, snapshots) =
            engine.respond_all(&Turn::new("hello"), &observed, &mut faults);
        assert!(faults.is_empty());
        assert_eq!(candidates.len(), snapshots.len());
        assert_eq!(
            candidates.len(),
            1,
            "only greeting answers 'hello' in this phase"
        );
        let first = candidates.first().unwrap();
        assert_eq!(first.module.as_str(), "greeting");
        assert_eq!(first.priority, Priority::Int(-1000));
        assert!(crate::arbiter::is_in_arrival_order(&candidates));
        assert!(first.reply.as_str().contains("Hi there"));
    }

    #[test]
    fn respond_adds_no_candidate_for_a_non_greeting_in_this_phase() {
        let engine = Engine::new();
        let session = Session::new();
        let mut faults = Faults::default();
        let text = Turn::new("how do i descale a kettle");
        let observed = engine.observe_all(&text, &session, &mut faults);
        let (candidates, snapshots) = engine.respond_all(&text, &observed, &mut faults);
        assert!(candidates.is_empty());
        assert!(snapshots.is_empty());
    }

    #[test]
    fn after_hooks_run_on_the_winning_memory() {
        let engine = Engine::new();
        let mut session = Session::new();
        let report = engine.step(&Turn::new("hi"), &mut session);
        assert_eq!(
            report.module.as_ref().map(ModuleName::as_str),
            Some("greeting")
        );
        assert_eq!(
            session.memory.get("frequency_reply"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            session.memory.get("frequency_answer"),
            Some(&Value::string(""))
        );
        assert_eq!(session.memory.get("user"), Some(&Value::Object(vec![])));
    }

    // ------------------------------------------------------------------ the whole turn

    #[test]
    fn a_turn_with_no_candidates_falls_back_to_hello() {
        let engine = Engine::new();
        let mut session = Session::new();
        let report = engine.step(&Turn::new("how do i descale a kettle"), &mut session);
        assert_eq!(report.reply.as_str(), "Hello!");
        assert_eq!(report.module, None);
        assert!(report.candidates.is_empty());
        assert_eq!(report.faults.arbiter_choice, None);
        assert_eq!(session.turns().len(), 1);
        assert_eq!(
            session.turns().first().map(|t| t.bot.as_str()),
            Some("Hello!")
        );
        assert_eq!(
            session.turns().first().and_then(|t| t.module.as_deref()),
            None
        );
    }

    #[test]
    fn a_greeting_turn_is_answered_by_greeting_and_logged() {
        let engine = Engine::new();
        let mut session = Session::new();
        let report = engine.step(&Turn::new("hello"), &mut session);
        assert_eq!(report.reply.as_str(), "Hi there — what do you need?");
        assert_eq!(
            report.module.as_ref().map(ModuleName::as_str),
            Some("greeting")
        );
        assert_eq!(report.faults.arbiter_choice, Some(0));
        let logged = session.turns().first().unwrap();
        assert_eq!(logged.user, "hello");
        assert_eq!(logged.bot, "Hi there — what do you need?");
        assert_eq!(logged.module.as_deref(), Some("greeting"));
    }

    #[test]
    fn a_second_identical_greeting_has_no_alternative_so_it_repeats() {
        let engine = Engine::new();
        let mut session = Session::new();
        let first = engine
            .step(&Turn::new("hello"), &mut session)
            .reply
            .into_string();
        let second = engine
            .step(&Turn::new("hello"), &mut session)
            .reply
            .into_string();
        assert_eq!(
            first, second,
            "one candidate: the repeat rule cannot avoid it"
        );
        assert_eq!(session.turns().len(), 2);
    }

    #[test]
    fn memory_carries_across_turns_and_name_recall_works() {
        let engine = Engine::new();
        let mut session = Session::new();
        let intro = engine.step(&Turn::new("hi, i'm Sam"), &mut session);
        assert_eq!(
            intro.reply.as_str(),
            "Nice to meet you, Sam. What can I help with?"
        );
        let recall = engine.step(&Turn::new("what's my name?"), &mut session);
        assert_eq!(recall.reply.as_str(), "You're Sam.");
        assert_eq!(session.turns().len(), 2);
    }

    #[test]
    fn unknown_input_still_updates_observers_but_answers_fallback() {
        let engine = Engine::new();
        let mut session = Session::new();
        let report = engine.step(&Turn::new("   "), &mut session);
        assert_eq!(report.reply.as_str(), "Hello!");
        assert_eq!(
            session
                .memory
                .get("understood")
                .and_then(|value| value.get("text")),
            Some(&Value::string("")),
            "observers ran even though nobody replied"
        );
    }

    #[test]
    fn oversized_input_is_truncated_and_counted() {
        let engine = Engine::new();
        let mut session = Session::new();
        let huge = "a".repeat(limits::MAX_INPUT_CHARS + 100);
        let report = engine.step(&Turn::new(huge), &mut session);
        assert!(report.faults.input_truncated);
        // The turn is cut to the input budget first; the log then applies its own, smaller,
        // text budget and counts that truncation separately.
        let logged = session.turns().first().unwrap();
        assert!(logged.user.chars().count() <= limits::MAX_INPUT_CHARS);
        assert_eq!(logged.user.len(), limits::MAX_LOGGED_TEXT_BYTES);
        assert_eq!(session.truncated(), 1, "the log counted its own clamp");
    }

    #[test]
    fn input_exactly_at_the_budget_is_not_flagged() {
        let engine = Engine::new();
        let mut session = Session::new();
        let exact = "b".repeat(limits::MAX_INPUT_CHARS);
        let report = engine.step(&Turn::new(exact), &mut session);
        assert!(!report.faults.input_truncated);
    }

    #[test]
    fn the_engine_names_its_arbiter_and_defaults_to_it() {
        let engine = Engine::default();
        assert_eq!(engine.arbiter().name(), "priority_floor");
        assert_eq!(Engine::new().arbiter().name(), "priority_floor");
    }

    #[test]
    fn hook_modules_and_priorities_are_reported() {
        let observers = hook_modules(registry::Hook::Observe);
        assert_eq!(observers.len(), 16);
        assert_eq!(
            observers
                .first()
                .and_then(|id| registry::get(*id))
                .map(|m| m.name),
            Some("understand")
        );
        let name = ModuleName::from_static("safety");
        assert_eq!(priority_of(&name), Some(Priority::Int(9000)));
        assert_eq!(priority_of(&ModuleName::from_static("nope")), None);
        assert_eq!(
            priority_of(&ModuleName::from_static("arbiter")),
            Some(Priority::Int(0))
        );
    }

    #[test]
    fn turns_are_deterministic_across_sessions() {
        let engine = Engine::new();
        let script = [
            "hello",
            "hi, i'm Sam",
            "what's my name?",
            "how do i boil an egg",
            "thanks",
        ];
        let run = || {
            let mut session = Session::new();
            let replies: Vec<String> = script
                .iter()
                .map(|text| {
                    engine
                        .step(&Turn::new(*text), &mut session)
                        .reply
                        .into_string()
                })
                .collect();
            (replies, session.memory_digest().hex())
        };
        assert_eq!(run(), run());
    }
}
