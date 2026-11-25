//! The handlers: a static trait table, one entry per registered module.
//!
//! `bot/modules/` is 109 Python files the runtime discovers dynamically. Here every module is
//! a value in [`HANDLERS`] — same index space as [`registry::MODULES`] — implementing
//! [`Module`]. The runtime walks that slice; nothing is looked up by reflection.
//!
//! # What is implemented in Fase 1
//!
//! The *architecture* is complete (hooks, snapshot isolation, the arbiter, panics contained),
//! the *content* is not: knowledge tables, understanding and the composer arrive in Fases 2–7.
//! The skeleton handlers here are honest about it:
//!
//! * [`skeleton::Understanding`] — a real observer: normalises the text with Python's
//!   whitespace semantics, remembers the user's name and the kind of line (greeting, thanks…),
//!   so the memory pipeline has something genuine to observe;
//! * [`skeleton::Greeter`] — replies to greetings and recalls the name (priority −1000, the
//!   Python module's own):
//! * [`skeleton::AnswersStub`] — the low-priority placeholder for `answers` (the module that
//!   wins 1 571 of the corpus' 3 062 turns): it never answers, which is exactly the gap the
//!   differential report must show;
//! * [`skeleton::ChatAfter`] / [`skeleton::MemoryAfter`] — the two `after` hooks of the
//!   reference bot, implemented for their pipeline effect (frequency accounting and
//!   `memory["user"]` upkeep);
//! * [`skeleton::NoopArbiter`] — the wrapper the arbiter hook needs (logic in [`crate::arbiter`]);
//! * every other module is `None`: registered, not yet implemented.
//!
//! Consequences for the differential runner: candidate lists and replies *will* disagree with
//! Python on most turns — that is the measurement Fase 1 exists to produce, and the report
//! keeps the two signals (module, reply) separate so they cannot mask each other.

pub mod registry;
pub mod skeleton;

use crate::memory::Snapshot;
use crate::types::{ModuleId, ModuleName, Priority, Reply, Turn};
use registry::ModuleDef;

/// What a module returns from `respond`: a reply, or nothing (`None` in Python).
pub type Answer = Option<Reply>;

/// One module's behaviour.
///
/// Every method receives the module's own [`Snapshot`] and returns the snapshot to keep: only
/// the winner's snapshot is installed, exactly like `_copy`/`copies[index]` in
/// `bot/runtime.py`. Handlers never see the session's real memory, so a losing module's writes
/// are discarded by construction.
///
/// Nothing here may panic. A handler that panics anyway is contained by the runtime
/// (`catch_unwind`) and counted as a fault — see [`crate::runtime::Faults`].
pub trait Module: std::fmt::Debug + Send + Sync {
    /// The registry entry this handler implements.
    fn def(&self) -> &'static ModuleDef;

    /// `observe(text, memory)` — remember, never reply. Default: no observer (93 of 109).
    fn observe(&self, _turn: &Turn, snapshot: Snapshot) -> Snapshot {
        snapshot
    }

    /// `respond(text, memory)` — propose a reply or pass. Default: pass.
    fn respond(&self, _turn: &Turn, _snapshot: Snapshot) -> Answer {
        None
    }

    /// `after(text, reply, memory)` — post-turn bookkeeping on the winning memory.
    /// Only `chat` and `memory_store` define it in Python.
    fn after(&self, _turn: &Turn, _reply: &Reply, snapshot: Snapshot) -> Snapshot {
        snapshot
    }

    /// The module name as a [`ModuleName`].
    fn name(&self) -> ModuleName<'static> {
        self.def().module_name()
    }

    /// The module id in the registry.
    fn id(&self) -> Option<ModuleId> {
        registry::lookup(self.def().name)
    }

    /// The module's priority.
    fn priority(&self) -> Priority {
        self.def().priority
    }
}

/// A handler table row.
#[derive(Debug)]
pub struct Entry {
    /// The registry row of the module at this index.
    pub def: &'static ModuleDef,
    /// The behaviour; `None` when the module has nothing worth calling in this phase.
    pub handler: Option<&'static dyn Module>,
}

/// A placeholder `ModuleDef`; the real definition is always obtained by index from the
/// registry ([`definition_for`]), never from `Entry::def`.
static PLACEHOLDER: ModuleDef = ModuleDef::new("<unset>", Priority::Int(0), &[]);

const fn entry(_name: &'static str, handler: Option<&'static dyn Module>) -> Entry {
    Entry {
        def: &PLACEHOLDER,
        handler,
    }
}

/// Every handler, **index-aligned with [`registry::MODULES`]**.
///
/// Entry *i* is the behaviour of the module whose id is `i`; a test walks the whole table and
/// asserts `entry.handler.def() == registry::MODULES[i]`. Entries with `None` are modules whose
/// content has not been ported yet: registered (the table stays a faithful copy of
/// `bot/modules/`) but unable to propose, so they never enter arbitration.
pub static HANDLERS: &[Entry] = &[
    entry("understand", Some(&skeleton::UNDERSTANDING)),
    entry("intent", Some(&skeleton::OBSERVE_INTENT)),
    entry("a_count_of_packets", None),
    entry("safety", None),
    entry("the_month_they_have", None),
    entry("since_it_began", None),
    entry("keeping", None),
    entry("a_question_about_somebody", None),
    entry("an_age_ahead", None),
    entry("steps", None),
    entry("what_they_said", None),
    entry("a_fact_asked_again", None),
    entry("a_return_brings_back_the_advice", None),
    entry("group_ages", None),
    entry("household", None),
    entry("kid_tally", None),
    entry("said_fact_back", None),
    entry("live_times", None),
    entry("a_count_said", None),
    entry("favourites", None),
    entry("recap", None),
    entry("attribute_said_again", None),
    entry("done_ask", None),
    entry("thing_i_did_with", None),
    entry("trip_date_said_again", None),
    entry("name_taken_back", None),
    entry("dreams", None),
    entry("grief", None),
    entry("loss", None),
    entry("who_to_see", None),
    entry("pace", None),
    entry("a_cake_between_its_layers", None),
    entry("words", None),
    entry("my_work", None),
    entry("years_in", None),
    entry("aboutachild", None),
    entry("familyask", None),
    entry("night", None),
    entry("saving_followup", None),
    entry("how_long_a_stay", None),
    entry("no_advice", None),
    entry("a_figure_beside_the_state", None),
    entry("a_measure_asked_again", None),
    entry("a_shock_at_a_bill", None),
    entry("a_will_it_work", None),
    entry("figure_asked_again", None),
    entry("taking_asked_again", None),
    entry("a_limit_over_the_answer", None),
    entry("already", None),
    entry("doubt", None),
    entry("colour_on_the_body", None),
    entry("said_back", None),
    entry("after_a_lead_in", None),
    entry("chat", Some(&skeleton::CHAT)),
    entry("their_pastime", None),
    entry("callback_about", None),
    entry("theirperson", None),
    entry("their_life", None),
    entry("answers", Some(&skeleton::ANSWERS)),
    entry("arbiter", Some(&skeleton::ARBITER_MODULE)),
    entry("chat_replies", None),
    entry("memory_store", Some(&skeleton::MEMORY_STORE)),
    entry("memory_tables", None),
    entry("on_record", None),
    entry("particles", None),
    entry("safety_cards", None),
    entry("safety_phrases", None),
    entry("safety_tables", None),
    entry("ruled_out", None),
    entry("a_debt_said_flat", None),
    entry("invitation", None),
    entry("judge_ask", None),
    entry("record_due", None),
    entry("talkto", None),
    entry("a_letter_about_money", None),
    entry("confusing", None),
    entry("urgent", None),
    entry("nudge", None),
    entry("their_taste", None),
    entry("hardpast", None),
    entry("receiving_end", None),
    entry("ward", None),
    entry("near_me", None),
    entry("a_first_time", None),
    entry("a_figure_of_a_dish", None),
    entry("chasing_a_body", None),
    entry("detail_question", None),
    entry("the_trouble_named", None),
    entry("carers_turn", None),
    entry("live_source", None),
    entry("onsubject", None),
    entry("what_decides_it", None),
    entry("a_check_with_you", None),
    entry("a_day_somebody_else_keeps", None),
    entry("a_habit_is_raised", None),
    entry("a_job_that_is_not_a_diy_job", None),
    entry("a_number_the_bot_knows", None),
    entry("a_plan_the_thread_holds", None),
    entry("a_thing_being_weighed", None),
    entry("a_thing_wants_doing", None),
    entry("bulky", None),
    entry("the_detail_the_thread_adds", None),
    entry("the_kind_one_line_up", None),
    entry("the_trouble_it_has", None),
    entry("the_wait_about_a_body", None),
    entry("where_it_answers", None),
    entry("first_line_open", None),
    entry("the_shape_of_the_answer", None),
    entry("greeting", Some(&skeleton::GREETER)),
];

/// The handler of a module id (`None` for skeleton modules without behaviour).
#[must_use]
pub fn handler_for(id: ModuleId) -> Option<&'static dyn Module> {
    HANDLERS.get(id.index()).and_then(|entry| entry.handler)
}

/// The registry definition of a module id.
#[must_use]
pub fn definition_for(id: ModuleId) -> Option<&'static ModuleDef> {
    registry::get(id)
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
    use crate::memory::Snapshot;
    use std::collections::BTreeSet;

    #[test]
    fn handlers_are_index_aligned_with_the_registry() {
        assert_eq!(HANDLERS.len(), registry::MODULES.len());
        assert_eq!(HANDLERS.len(), 109);
        for (index, module) in registry::MODULES.iter().enumerate() {
            let entry = HANDLERS
                .get(index)
                .unwrap_or_else(|| panic!("missing handler entry for {}", module.name));
            let definition = definition_for(ModuleId::from_index(u16::try_from(index).unwrap()));
            assert_eq!(definition, Some(module));
            if let Some(handler) = entry.handler {
                assert_eq!(
                    handler.def().name,
                    module.name,
                    "handler at {index} implements the wrong module"
                );
                let id = handler
                    .id()
                    .unwrap_or_else(|| panic!("{} has no id", module.name));
                assert_eq!(id.index(), index);
                assert_eq!(handler.priority(), module.priority);
                assert_eq!(handler.name(), ModuleName::from_static(module.name));
            }
        }
    }

    #[test]
    fn exactly_the_phase_one_modules_have_handlers() {
        let implemented: BTreeSet<&str> = registry::MODULES
            .iter()
            .filter_map(|module| {
                let id = registry::lookup(module.name)?;
                Some((module.name, handler_for(id).is_some()))
            })
            .filter(|(_, has_handler)| *has_handler)
            .map(|(name, _)| name)
            .collect();
        let expected: BTreeSet<&str> = [
            "answers",
            "arbiter",
            "chat",
            "greeting",
            "intent",
            "memory_store",
            "understand",
        ]
        .into_iter()
        .collect();
        assert_eq!(implemented, expected);
    }

    #[test]
    fn handler_lookup_by_id_is_total_and_matches_the_table() {
        for module in registry::MODULES {
            let id = registry::lookup(module.name).unwrap();
            assert_eq!(
                handler_for(id).is_some(),
                HANDLERS
                    .get(id.index())
                    .and_then(|entry| entry.handler)
                    .is_some(),
                "{}",
                module.name
            );
            assert_eq!(definition_for(id), Some(module));
        }
        let out_of_range = ModuleId::from_index(65_000);
        assert!(handler_for(out_of_range).is_none());
        assert_eq!(definition_for(out_of_range), None);
    }

    #[test]
    fn non_responders_never_propose_and_non_observers_never_write() {
        let text = Turn::new("hello, my name is Sam");
        let reply = Reply::new("Hello!");
        for module in registry::MODULES {
            let Some(handler) = handler_for(registry::lookup(module.name).unwrap()) else {
                continue;
            };
            let snapshot = Snapshot::default();
            let answer = handler.respond(&text, snapshot.clone());
            if !module.proposes() {
                assert!(
                    answer.is_none(),
                    "{} answered without respond()",
                    module.name
                );
            }
            let observed = handler.observe(&text, snapshot.clone());
            if !module.has(registry::Hook::Observe) {
                assert_eq!(
                    observed, snapshot,
                    "{} observed without declaring it",
                    module.name
                );
            }
            let after = handler.after(&text, &reply, snapshot.clone());
            if !module.has(registry::Hook::After) {
                assert_eq!(
                    after, snapshot,
                    "{} ran after() without declaring it",
                    module.name
                );
            }
        }
    }

    #[test]
    fn the_placeholder_definition_is_never_used_in_decisions() {
        assert_eq!(PLACEHOLDER.name, "<unset>");
        for entry in HANDLERS {
            assert!(
                std::ptr::eq(entry.def, &raw const PLACEHOLDER),
                "Entry::def stays a placeholder; definitions come from `definition_for`"
            );
        }
    }

    #[test]
    fn handlers_are_object_safe_and_send_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Module>();
        let handler: &dyn Module = handler_for(registry::lookup("greeting").unwrap()).unwrap();
        assert_eq!(handler.name().as_str(), "greeting");
        assert!(!format!("{handler:?}").is_empty());
    }
}
