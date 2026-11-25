//! The runtime façade: the types a turn produces, and the `Runtime` that owns an [`Engine`].
//!
//! This is the Rust counterpart of `bot/runtime.py`'s public surface (`turn(text, memory)`,
//! `reply(text, memory)`) and of `pybot.py`'s `answer()` normalisation. It is the layer the CLI,
//! the differential runner and the tests all use, so "what a turn is" is defined exactly once.

use crate::engine::Engine;
use crate::session::{LoggedTurn, Session};
use crate::types::{ModuleId, ModuleName, Priority, Reply, Turn};

/// The reply Python's runtime uses when no module proposes anything (`FALLBACK = "Hello!"`).
pub const FALLBACK: &str = "Hello!";

/// One proposed reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// The registry id, when the module is registered (always true for real modules).
    pub id: Option<ModuleId>,
    /// The module name (kept as text so an unregistered Python module can still be reported).
    pub module: ModuleName<'static>,
    /// The module's `PRIORITY`.
    pub priority: Priority,
    /// The proposed reply (never blank: blank proposals are not candidates).
    pub reply: Reply,
}

/// A module fault: a panic inside a hook, or a missing handler.
///
/// Python swallows module exceptions (`except Exception: continue`); the port records them
/// instead of hiding them, so a bug in a ported rule is *visible* in the report without ever
/// reaching the user as a crash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    /// The module whose hook failed.
    pub module: &'static str,
    /// The hook that failed (`observe`, `respond`, `after`, `choose`, `lookup`).
    pub hook: &'static str,
    /// What went wrong (a panic message, usually).
    pub message: String,
}

/// Everything that went wrong or was clamped during one turn.
///
/// One public flag per incident the pipeline can record; they are independent by nature, so
/// they are not grouped into sub-structs.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Faults {
    recorded: Vec<Fault>,
    /// The input exceeded [`crate::limits::MAX_INPUT_CHARS`] and was truncated.
    pub input_truncated: bool,
    /// Number of candidate replies dropped for exceeding [`crate::limits::MAX_CANDIDATES`].
    pub candidates_dropped: usize,
    /// The candidate list had to be re-sorted into arrival order (should never happen).
    pub candidates_reordered: bool,
    /// The index the arbiter returned (`None` when there was nothing to arbitrate).
    pub arbiter_choice: Option<usize>,
    /// Writes dropped because the memory budget was exceeded.
    pub memory_dropped_writes: usize,
}

impl Faults {
    /// Record a fault.
    pub fn push(&mut self, fault: Fault) {
        if self.recorded.len() < 64 {
            self.recorded.push(fault);
        }
    }

    /// The recorded faults.
    #[must_use]
    pub fn faults(&self) -> &[Fault] {
        &self.recorded
    }

    /// Number of recorded faults.
    #[must_use]
    pub fn len(&self) -> usize {
        self.recorded.len()
    }

    /// True when nothing failed and nothing was clamped.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recorded.is_empty()
            && !self.input_truncated
            && self.candidates_dropped == 0
            && !self.candidates_reordered
            && self.memory_dropped_writes == 0
    }
}

/// The result of one turn, mirroring Python's `{"reply", "module", "candidates"}`.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnReport {
    /// The winning reply (the fallback when no candidate existed).
    pub reply: Reply,
    /// The winning module name (`None` for the fallback).
    pub module: Option<ModuleName<'static>>,
    /// Every candidate, in arrival order.
    pub candidates: Vec<Candidate>,
    /// Faults and clamps of this turn.
    pub faults: Faults,
}

impl TurnReport {
    /// The reply as the CLI prints it: `" ".join(str(reply).split()) or ""`.
    #[must_use]
    pub fn normalised_reply(&self) -> String {
        crate::pyutil::collapse_spaces(self.reply.as_str()).into_owned()
    }

    /// The winning module's name, for display.
    #[must_use]
    pub fn module_name(&self) -> &str {
        self.module.as_ref().map_or("none", ModuleName::as_str)
    }

    /// `(module, priority)` pairs, for comparisons against the corpus' `candidates`.
    #[must_use]
    pub fn candidate_pairs(&self) -> Vec<(&str, Priority)> {
        self.candidates
            .iter()
            .map(|candidate| (candidate.module.as_str(), candidate.priority))
            .collect()
    }
}

/// The runtime: one engine + the operations the CLI and harness need.
#[derive(Debug)]
pub struct Runtime {
    engine: Engine,
}

impl Runtime {
    /// A runtime with the default engine.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            engine: Engine::new(),
        }
    }

    /// A runtime with a custom engine.
    #[must_use]
    pub const fn with_engine(engine: Engine) -> Self {
        Self { engine }
    }

    /// One turn, mutating the session (Python: `turn(text, memory)`).
    pub fn turn(&self, turn: &Turn, session: &mut Session) -> TurnReport {
        self.engine.step(turn, session)
    }

    /// One turn from text.
    pub fn say(&self, text: &str, session: &mut Session) -> TurnReport {
        self.turn(&Turn::new(text), session)
    }

    /// Python's `runtime.reply(text, memory)`: just the reply.
    pub fn reply(&self, text: &str, session: &mut Session) -> Reply {
        self.say(text, session).reply
    }

    /// The CLI's `answer()`: the reply normalised with `" ".join(split())`, or `"Hello!"` when
    /// the runtime produced nothing (which cannot happen, but the CLI's contract says so).
    pub fn answer(&self, text: &str, session: &mut Session) -> Answer {
        let report = self.say(text, session);
        let normalised = report.normalised_reply();
        Answer {
            reply: if normalised.is_empty() {
                Reply::fallback()
            } else {
                Reply::new(normalised)
            },
            module: report.module.clone(),
        }
    }

    /// A conversation as a sequence of turns; returns the reports and the final session.
    #[must_use]
    pub fn conversation(&self, inputs: &[&str]) -> (Vec<TurnReport>, Session) {
        let mut session = Session::new();
        let reports = inputs
            .iter()
            .map(|text| self.say(text, &mut session))
            .collect();
        (reports, session)
    }

    /// The last logged turn of a session.
    #[must_use]
    pub fn last_turn(session: &Session) -> Option<&LoggedTurn> {
        session.turns().last()
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

/// What the CLI prints for one input line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The normalised reply.
    pub reply: Reply,
    /// The winning module.
    pub module: Option<ModuleName<'static>>,
}

/// Convenience alias so call sites can `use crate::runtime::Finish` for pipeline stages.
pub type Finish = TurnReport;

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
    fn the_fallback_is_the_python_constant() {
        assert_eq!(FALLBACK, "Hello!");
        assert_eq!(Reply::fallback().as_str(), FALLBACK);
    }

    #[test]
    fn a_turn_report_exposes_reply_module_and_candidates() {
        let runtime = Runtime::new();
        let mut session = Session::new();
        let report = runtime.say("hello", &mut session);
        assert_eq!(report.reply.as_str(), "Hi there — what do you need?");
        assert_eq!(report.module_name(), "greeting");
        assert_eq!(report.normalised_reply(), "Hi there — what do you need?");
        assert_eq!(
            report.candidate_pairs(),
            vec![("greeting", Priority::Int(-1000))]
        );
        assert!(report.faults.is_empty());
    }

    #[test]
    fn normalised_reply_collapses_like_python() {
        let report = TurnReport {
            reply: Reply::new("  a \t b  "),
            module: None,
            candidates: vec![],
            faults: Faults::default(),
        };
        assert_eq!(report.normalised_reply(), "a b");
        assert_eq!(report.module_name(), "none");
    }

    #[test]
    fn answer_normalises_and_falls_back() {
        let runtime = Runtime::new();
        let mut session = Session::new();
        let answer = runtime.answer("hello", &mut session);
        assert_eq!(answer.reply.as_str(), "Hi there — what do you need?");
        assert_eq!(
            answer.module.as_ref().map(ModuleName::as_str),
            Some("greeting")
        );
        let answer = runtime.answer("how do i boil an egg", &mut session);
        assert_eq!(answer.reply.as_str(), "Hello!");
        assert_eq!(answer.module, None);
    }

    #[test]
    fn reply_is_just_the_reply() {
        let runtime = Runtime::new();
        let mut session = Session::new();
        assert_eq!(
            runtime.reply("hello", &mut session).as_str(),
            "Hi there — what do you need?"
        );
    }

    #[test]
    fn conversations_accumulate_memory() {
        let runtime = Runtime::new();
        let (reports, session) = runtime.conversation(&["hi, i'm Lucia", "what's my name?"]);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[1].reply.as_str(), "You're Lucia.");
        assert_eq!(session.turns().len(), 2);
        assert_eq!(
            Runtime::last_turn(&session).map(|t| t.user.as_str()),
            Some("what's my name?")
        );
    }

    #[test]
    fn last_turn_of_a_new_session_is_none() {
        assert_eq!(Runtime::last_turn(&Session::new()), None);
    }

    #[test]
    fn faults_account_for_clamps_and_are_ordered() {
        let mut faults = Faults::default();
        assert!(faults.is_empty());
        faults.input_truncated = true;
        assert!(!faults.is_empty());
        let mut faults = Faults::default();
        for index in 0..100 {
            faults.push(Fault {
                module: "chat",
                hook: "respond",
                message: format!("fault {index}"),
            });
        }
        assert_eq!(faults.len(), 64, "bounded");
        assert_eq!(
            faults.faults().first().map(|f| f.message.as_str()),
            Some("fault 0")
        );
        for faults in [
            Faults {
                candidates_dropped: 1,
                ..Faults::default()
            },
            Faults {
                candidates_reordered: true,
                ..Faults::default()
            },
            Faults {
                memory_dropped_writes: 1,
                ..Faults::default()
            },
            Faults {
                input_truncated: true,
                ..Faults::default()
            },
        ] {
            assert!(!faults.is_empty(), "{faults:?} must count as a fault");
        }
        assert!(
            Faults {
                arbiter_choice: Some(0),
                ..Faults::default()
            }
            .is_empty(),
            "an arbitration is not a fault"
        );
    }

    #[test]
    fn the_runtime_is_deterministic() {
        let run = || {
            let runtime = Runtime::new();
            let (reports, session) = runtime.conversation(&[
                "hello",
                "hi, i'm Sam",
                "what's my name?",
                "how long should i boil an egg",
            ]);
            let replies: Vec<String> = reports
                .iter()
                .map(super::TurnReport::normalised_reply)
                .collect();
            (replies, session.memory_digest().hex())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn construction_is_const_friendly() {
        const _RUNTIME: Runtime = Runtime::new();
        static _STATIC: Runtime = Runtime::new();
        let engine = Engine::new();
        let _custom = Runtime::with_engine(engine);
    }
}
