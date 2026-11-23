//! The static module registry — the Rust replacement for `pkgutil.iter_modules` +
//! `getattr(module, hook)`.
//!
//! `bot/runtime.py` discovers modules dynamically (one file per module under `bot/modules/`),
//! sorts them by `(-PRIORITY, filename)`, and looks up `observe`/`respond`/`after`/`choose`
//! with `getattr`. GOAL.md replaces that with a **static, ordered table**:
//!
//! * the order of [`MODULES`] *is* the arbitration default order: `(-priority, name)`, exactly
//!   what `load_modules()` produces — so "first candidate wins" is "lowest index wins";
//! * each entry records which hooks the Python module defines, so the pipeline can run the
//!   same three phases without reflection;
//! * [`lookup`] resolves a name with a binary search on a sorted name index (no hashing, no
//!   iteration order anywhere near a decision).
//!
//! # Keeping the table honest
//!
//! The table is a *copy* of `bot/modules/` metadata. Two tests pin it:
//! `tests/registry.rs` re-derives it from the Python sources at test time (AST-lite parse with
//! the std only) and fails on any drift, and `tools/export_registry.py` regenerates the Rust
//! table when the Python side changes. Every module mentioned by the golden corpus must exist
//! here; modules the port has not implemented yet keep their metadata and get a skeleton
//! handler (or none), which is exactly what the differential report exposes.

use crate::types::{ModuleId, ModuleName, Priority};

/// A hook a Python module defines and the runtime calls by name.
///
/// The variants are declared in **alphabetical order of their Python names**
/// (`after < choose < observe < respond`) so that the `Ord` derive agrees with the sorted
/// `hooks` arrays of [`ModuleDef`] (which `tools/export_registry.py` emits alphabetically).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hook {
    /// `after(text, reply, memory)`: post-turn bookkeeping (2 modules in the reference bot).
    After,
    /// `choose(text, memory, candidates)`: the arbiter (`bot/modules/arbiter.py`).
    Choose,
    /// `observe(text, memory)`: remember something, never replies.
    Observe,
    /// `respond(text, memory)`: propose a reply or pass (`None`).
    Respond,
}

impl Hook {
    /// The hook name as Python spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Respond => "respond",
            Self::After => "after",
            Self::Choose => "choose",
        }
    }
}

/// The static description of one module.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModuleDef {
    /// The module name as Python spells it (`bot/modules/<name>.py`).
    pub name: &'static str,
    /// The `PRIORITY` it declares (`0` when it declares none, like `getattr(module,
    /// "PRIORITY", 0)`).
    pub priority: Priority,
    /// The hooks it defines, sorted.
    pub hooks: &'static [Hook],
}

impl ModuleDef {
    /// Const constructor for the table.
    #[must_use]
    pub const fn new(name: &'static str, priority: Priority, hooks: &'static [Hook]) -> Self {
        Self {
            name,
            priority,
            hooks,
        }
    }

    /// True when the module defines the hook.
    #[must_use]
    pub fn has(&self, hook: Hook) -> bool {
        // Four entries at most: a scan is faster than a binary search and cannot be broken by
        // an ordering mistake in the table.
        self.hooks.contains(&hook)
    }

    /// True when the module can propose a reply (`respond`).
    #[must_use]
    pub fn proposes(&self) -> bool {
        self.has(Hook::Respond)
    }

    /// The module's name as a [`ModuleName`].
    #[must_use]
    pub fn module_name(&self) -> ModuleName<'static> {
        ModuleName::from_static(self.name)
    }
}

/// Every module of `bot/modules/`, in the runtime's canonical order: `(-priority, name)`.
///
/// `bot/runtime.py`'s `load_modules()` sorts `(-PRIORITY, entry.name)`; this table is that
/// sort made static. Index = [`ModuleId`].
#[rustfmt::skip]
pub const MODULES: &[ModuleDef] = &[
    ModuleDef::new("understand", Priority::Int(10000), &[Hook::Observe]),
    ModuleDef::new("intent", Priority::Int(9500), &[Hook::Observe]),
    ModuleDef::new("a_count_of_packets", Priority::Int(9000), &[Hook::Respond]),
    ModuleDef::new("safety", Priority::Int(9000), &[Hook::Respond]),
    ModuleDef::new("the_month_they_have", Priority::Int(8000), &[Hook::Respond]),
    ModuleDef::new("since_it_began", Priority::Int(7500), &[Hook::Respond]),
    ModuleDef::new("keeping", Priority::Int(7400), &[Hook::Respond]),
    ModuleDef::new("a_question_about_somebody", Priority::Int(7300), &[Hook::Respond]),
    ModuleDef::new("an_age_ahead", Priority::Int(7150), &[Hook::Respond]),
    ModuleDef::new("steps", Priority::Int(7100), &[Hook::Respond]),
    ModuleDef::new("what_they_said", Priority::Int(7060), &[Hook::Respond]),
    ModuleDef::new("a_fact_asked_again", Priority::Int(7050), &[Hook::Respond]),
    ModuleDef::new("a_return_brings_back_the_advice", Priority::Int(7050), &[Hook::Respond]),
    ModuleDef::new("group_ages", Priority::Int(7050), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("household", Priority::Int(7050), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("kid_tally", Priority::Int(7050), &[Hook::Respond]),
    ModuleDef::new("said_fact_back", Priority::Int(7050), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("live_times", Priority::Int(7001), &[Hook::Respond]),
    ModuleDef::new("a_count_said", Priority::Int(7000), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("favourites", Priority::Int(7000), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("recap", Priority::Int(7000), &[Hook::Respond]),
    ModuleDef::new("attribute_said_again", Priority::Int(6900), &[Hook::Respond]),
    ModuleDef::new("done_ask", Priority::Int(6900), &[Hook::Observe]),
    ModuleDef::new("thing_i_did_with", Priority::Int(6900), &[Hook::Respond]),
    ModuleDef::new("trip_date_said_again", Priority::Int(6900), &[Hook::Observe]),
    ModuleDef::new("name_taken_back", Priority::Int(6600), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("dreams", Priority::Int(3000), &[Hook::Respond]),
    ModuleDef::new("grief", Priority::Int(2100), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("loss", Priority::Int(2000), &[Hook::Respond]),
    ModuleDef::new("who_to_see", Priority::Int(1500), &[Hook::Respond]),
    ModuleDef::new("pace", Priority::Int(1300), &[Hook::Respond]),
    ModuleDef::new("a_cake_between_its_layers", Priority::Int(1200), &[Hook::Respond]),
    ModuleDef::new("words", Priority::Int(1200), &[Hook::Respond]),
    ModuleDef::new("my_work", Priority::Int(1150), &[Hook::Respond]),
    ModuleDef::new("years_in", Priority::Int(1150), &[Hook::Respond]),
    ModuleDef::new("aboutachild", Priority::Int(1100), &[Hook::Respond]),
    ModuleDef::new("familyask", Priority::Int(1100), &[Hook::Respond]),
    ModuleDef::new("night", Priority::Int(1100), &[Hook::Respond]),
    ModuleDef::new("saving_followup", Priority::Int(1100), &[Hook::Respond]),
    ModuleDef::new("how_long_a_stay", Priority::Int(1050), &[Hook::Respond]),
    ModuleDef::new("no_advice", Priority::Int(1050), &[Hook::Respond]),
    ModuleDef::new("a_figure_beside_the_state", Priority::Int(1000), &[Hook::Respond]),
    ModuleDef::new("a_measure_asked_again", Priority::Int(1000), &[Hook::Respond]),
    ModuleDef::new("a_shock_at_a_bill", Priority::Int(1000), &[Hook::Respond]),
    ModuleDef::new("a_will_it_work", Priority::Int(1000), &[Hook::Respond]),
    ModuleDef::new("figure_asked_again", Priority::Int(1000), &[Hook::Respond]),
    ModuleDef::new("taking_asked_again", Priority::Int(1000), &[Hook::Respond]),
    ModuleDef::new("a_limit_over_the_answer", Priority::Int(500), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("already", Priority::Int(400), &[Hook::Respond]),
    ModuleDef::new("doubt", Priority::Int(400), &[Hook::Respond]),
    ModuleDef::new("colour_on_the_body", Priority::Int(350), &[Hook::Respond]),
    ModuleDef::new("said_back", Priority::Int(350), &[Hook::Respond]),
    ModuleDef::new("after_a_lead_in", Priority::Int(320), &[Hook::Respond]),
    ModuleDef::new("chat", Priority::Int(300), &[Hook::After, Hook::Observe, Hook::Respond]),
    ModuleDef::new("their_pastime", Priority::Int(300), &[Hook::Respond]),
    ModuleDef::new("callback_about", Priority::Int(260), &[Hook::Respond]),
    ModuleDef::new("theirperson", Priority::Int(250), &[Hook::Respond]),
    ModuleDef::new("their_life", Priority::Int(100), &[Hook::Respond]),
    ModuleDef::new("answers", Priority::Int(0), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("arbiter", Priority::Int(0), &[Hook::Choose]),
    ModuleDef::new("chat_replies", Priority::Int(0), &[]),
    ModuleDef::new("memory_store", Priority::Int(0), &[Hook::After, Hook::Observe, Hook::Respond]),
    ModuleDef::new("memory_tables", Priority::Int(0), &[]),
    ModuleDef::new("on_record", Priority::Int(0), &[]),
    ModuleDef::new("particles", Priority::Int(0), &[]),
    ModuleDef::new("safety_cards", Priority::Int(0), &[]),
    ModuleDef::new("safety_phrases", Priority::Int(0), &[]),
    ModuleDef::new("safety_tables", Priority::Int(0), &[]),
    ModuleDef::new("ruled_out", Priority::Int(-450), &[Hook::Respond]),
    ModuleDef::new("a_debt_said_flat", Priority::Int(-500), &[Hook::Observe, Hook::Respond]),
    ModuleDef::new("invitation", Priority::Int(-500), &[Hook::Respond]),
    ModuleDef::new("judge_ask", Priority::Int(-500), &[Hook::Respond]),
    ModuleDef::new("record_due", Priority::Int(-500), &[Hook::Respond]),
    ModuleDef::new("talkto", Priority::Int(-500), &[Hook::Respond]),
    ModuleDef::new("a_letter_about_money", Priority::Int(-600), &[Hook::Respond]),
    ModuleDef::new("confusing", Priority::Int(-900), &[Hook::Respond]),
    ModuleDef::new("urgent", Priority::Int(-900), &[Hook::Respond]),
    ModuleDef::new("nudge", Priority::Int(-950), &[Hook::Respond]),
    ModuleDef::new("their_taste", Priority::Int(-960), &[Hook::Respond]),
    ModuleDef::new("hardpast", Priority::Int(-980), &[Hook::Respond]),
    ModuleDef::new("receiving_end", Priority::Int(-980), &[Hook::Respond]),
    ModuleDef::new("ward", Priority::Int(-985), &[Hook::Respond]),
    ModuleDef::new("near_me", Priority::Int(-995), &[Hook::Respond]),
    ModuleDef::new("a_first_time", Priority::Int(-996), &[Hook::Respond]),
    ModuleDef::new("a_figure_of_a_dish", Priority::Int(-997), &[Hook::Respond]),
    ModuleDef::new("chasing_a_body", Priority::Int(-997), &[Hook::Respond]),
    ModuleDef::new("detail_question", Priority::Int(-997), &[Hook::Respond]),
    ModuleDef::new("the_trouble_named", Priority::Int(-997), &[Hook::Respond]),
    ModuleDef::new("carers_turn", Priority::Int(-998), &[Hook::Respond]),
    ModuleDef::new("live_source", Priority::Int(-998), &[Hook::Respond]),
    ModuleDef::new("onsubject", Priority::Int(-998), &[Hook::Respond]),
    ModuleDef::new("what_decides_it", Priority::Int(-998), &[Hook::Respond]),
    ModuleDef::new("a_check_with_you", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("a_day_somebody_else_keeps", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("a_habit_is_raised", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("a_job_that_is_not_a_diy_job", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("a_number_the_bot_knows", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("a_plan_the_thread_holds", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("a_thing_being_weighed", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("a_thing_wants_doing", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("bulky", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("the_detail_the_thread_adds", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("the_kind_one_line_up", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("the_trouble_it_has", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("the_wait_about_a_body", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("where_it_answers", Priority::Int(-999), &[Hook::Respond]),
    ModuleDef::new("first_line_open", Priority::Float(-999.5), &[Hook::Respond]),
    ModuleDef::new("the_shape_of_the_answer", Priority::Float(-999.5), &[Hook::Respond]),
    ModuleDef::new("greeting", Priority::Int(-1000), &[Hook::Respond]),
];

/// Module names, sorted, with their index into [`MODULES`].
///
/// A sorted array, not a map: a lookup is a scan of 109 pairs, and the order is pinned by
/// `tests/registry.rs`, so a binary search could replace the scan without changing behaviour.
#[rustfmt::skip]
pub const NAME_INDEX: &[(&str, u16)] = &[
    ("a_cake_between_its_layers", 31),
    ("a_check_with_you", 92),
    ("a_count_of_packets", 2),
    ("a_count_said", 18),
    ("a_day_somebody_else_keeps", 93),
    ("a_debt_said_flat", 69),
    ("a_fact_asked_again", 11),
    ("a_figure_beside_the_state", 41),
    ("a_figure_of_a_dish", 84),
    ("a_first_time", 83),
    ("a_habit_is_raised", 94),
    ("a_job_that_is_not_a_diy_job", 95),
    ("a_letter_about_money", 74),
    ("a_limit_over_the_answer", 47),
    ("a_measure_asked_again", 42),
    ("a_number_the_bot_knows", 96),
    ("a_plan_the_thread_holds", 97),
    ("a_question_about_somebody", 7),
    ("a_return_brings_back_the_advice", 12),
    ("a_shock_at_a_bill", 43),
    ("a_thing_being_weighed", 98),
    ("a_thing_wants_doing", 99),
    ("a_will_it_work", 44),
    ("aboutachild", 35),
    ("after_a_lead_in", 52),
    ("already", 48),
    ("an_age_ahead", 8),
    ("answers", 58),
    ("arbiter", 59),
    ("attribute_said_again", 21),
    ("bulky", 100),
    ("callback_about", 55),
    ("carers_turn", 88),
    ("chasing_a_body", 85),
    ("chat", 53),
    ("chat_replies", 60),
    ("colour_on_the_body", 50),
    ("confusing", 75),
    ("detail_question", 86),
    ("done_ask", 22),
    ("doubt", 49),
    ("dreams", 26),
    ("familyask", 36),
    ("favourites", 19),
    ("figure_asked_again", 45),
    ("first_line_open", 106),
    ("greeting", 108),
    ("grief", 27),
    ("group_ages", 13),
    ("hardpast", 79),
    ("household", 14),
    ("how_long_a_stay", 39),
    ("intent", 1),
    ("invitation", 70),
    ("judge_ask", 71),
    ("keeping", 6),
    ("kid_tally", 15),
    ("live_source", 89),
    ("live_times", 17),
    ("loss", 28),
    ("memory_store", 61),
    ("memory_tables", 62),
    ("my_work", 33),
    ("name_taken_back", 25),
    ("near_me", 82),
    ("night", 37),
    ("no_advice", 40),
    ("nudge", 77),
    ("on_record", 63),
    ("onsubject", 90),
    ("pace", 30),
    ("particles", 64),
    ("recap", 20),
    ("receiving_end", 80),
    ("record_due", 72),
    ("ruled_out", 68),
    ("safety", 3),
    ("safety_cards", 65),
    ("safety_phrases", 66),
    ("safety_tables", 67),
    ("said_back", 51),
    ("said_fact_back", 16),
    ("saving_followup", 38),
    ("since_it_began", 5),
    ("steps", 9),
    ("taking_asked_again", 46),
    ("talkto", 73),
    ("the_detail_the_thread_adds", 101),
    ("the_kind_one_line_up", 102),
    ("the_month_they_have", 4),
    ("the_shape_of_the_answer", 107),
    ("the_trouble_it_has", 103),
    ("the_trouble_named", 87),
    ("the_wait_about_a_body", 104),
    ("their_life", 57),
    ("their_pastime", 54),
    ("their_taste", 78),
    ("theirperson", 56),
    ("thing_i_did_with", 23),
    ("trip_date_said_again", 24),
    ("understand", 0),
    ("urgent", 76),
    ("ward", 81),
    ("what_decides_it", 91),
    ("what_they_said", 10),
    ("where_it_answers", 105),
    ("who_to_see", 29),
    ("words", 32),
    ("years_in", 34),
];

/// The index of the module, in [`MODULES`] order.
///
/// Matches by name, not by address: `const` tables may be duplicated by the compiler
/// (inlining, static promotion), so two `&ModuleDef` values can be equal in content but differ
/// in address — and two names are never duplicated (a test pins uniqueness).
#[must_use]
pub fn index_of(module: &ModuleDef) -> Option<ModuleId> {
    lookup(module.name)
}

/// A module definition by id (total for ids produced by [`lookup`]/[`all`]).
#[must_use]
pub fn get(id: ModuleId) -> Option<&'static ModuleDef> {
    MODULES.get(id.index())
}

/// A module definition by name (binary search over [`NAME_INDEX`]).
#[must_use]
pub fn find(name: &str) -> Option<&'static ModuleDef> {
    // `NAME_INDEX` is the name -> position map; `lookup` is the one place that reads it.
    lookup(name).and_then(get)
}

/// A module id by name.
#[must_use]
pub fn lookup(name: &str) -> Option<ModuleId> {
    // A linear scan over 109 pairs beats a binary search that must stay perfectly sorted;
    // `tests::the_name_index_is_sorted_and_points_correctly` still pins the order.
    let (_, index) = NAME_INDEX
        .iter()
        .find(|(candidate, _)| *candidate == name)?;
    Some(ModuleId::from_index(*index))
}

/// Every module, in arbitration order.
pub fn all() -> impl Iterator<Item = &'static ModuleDef> {
    MODULES.iter()
}

/// Every module that defines `hook`, in arbitration order — the Python `_OBSERVERS`,
/// `_HANDLERS` and `_AFTER` lists.
pub fn with_hook(hook: Hook) -> impl Iterator<Item = &'static ModuleDef> {
    MODULES.iter().filter(move |module| module.has(hook))
}

/// Number of modules in the registry.
#[must_use]
pub const fn count() -> usize {
    MODULES.len()
}

/// A stand-in definition for code paths that must name an arbiter before one exists.
///
/// The real arbiter is the registry row named `arbiter`; the engine resolves it through
/// [`find`] and only falls back here if the table lost its arbiter, which a unit test forbids.
pub static PLACEHOLDER_ARBITER: ModuleDef =
    ModuleDef::new("arbiter", Priority::Int(0), &[Hook::Choose]);

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_table_holds_all_109_modules_in_arbitration_order() {
        assert_eq!(MODULES.len(), 109);
        assert_eq!(count(), 109);
        for pair in MODULES.windows(2) {
            let (left, right) = (&pair[0], &pair[1]);
            let left_key = (-left.priority.as_f64(), left.name);
            let right_key = (-right.priority.as_f64(), right.name);
            assert!(
                left_key <= right_key,
                "out of order: {left:?} then {right:?}"
            );
        }
    }

    #[test]
    fn the_name_index_is_sorted_unique_and_points_correctly() {
        assert_eq!(NAME_INDEX.len(), MODULES.len());
        for pair in NAME_INDEX.windows(2) {
            assert!(pair[0].0 < pair[1].0, "not sorted: {pair:?}");
        }
        for (name, index) in NAME_INDEX {
            let direct = MODULES.iter().position(|module| module.name == *name);
            assert_eq!(direct, Some(usize::from(*index)), "{name}");
            let module = MODULES.get(usize::from(*index)).expect("index in range");
            assert_eq!(module.name, *name, "the index points at the right row");
        }
    }

    #[test]
    fn every_module_resolves_by_name_and_back() {
        for (index, module) in MODULES.iter().enumerate() {
            let id = lookup(module.name).unwrap_or_else(|| panic!("{} must resolve", module.name));
            assert_eq!(id.index(), index);
            assert_eq!(get(id), Some(module));
            assert_eq!(find(module.name), Some(module));
            assert_eq!(index_of(module), Some(id));
            assert_eq!(module.module_name(), ModuleName::from_static(module.name));
        }
        assert_eq!(lookup("no_such_module"), None);
        assert_eq!(find(""), None);
        assert_eq!(find("Chat"), None, "names are case-sensitive");
        assert_eq!(find("chat "), None, "no trimming");
    }

    #[test]
    fn the_hooks_match_the_reference_bot() {
        // Recon: 97 modules with `respond`, 16 with `observe`, 2 with `after`, 1 arbiter.
        let observers = with_hook(Hook::Observe).count();
        let responders = with_hook(Hook::Respond).count();
        let afters: Vec<&str> = with_hook(Hook::After).map(|module| module.name).collect();
        let arbiters = with_hook(Hook::Choose).count();
        assert_eq!(observers, 16);
        assert_eq!(responders, 97);
        assert_eq!(
            afters,
            vec!["chat", "memory_store"],
            "the two `after` modules"
        );
        assert_eq!(arbiters, 1);
    }

    #[test]
    fn the_arbiter_is_the_only_choose_and_has_no_priority() {
        let arbiter = find("arbiter").unwrap_or_else(|| panic!("arbiter exists"));
        assert!(arbiter.has(Hook::Choose));
        assert!(!arbiter.proposes());
        assert_eq!(arbiter.priority, Priority::Int(0));
    }

    #[test]
    fn priorities_have_the_expected_extremes_and_bands() {
        let highest = MODULES.first().unwrap_or_else(|| panic!("non-empty table"));
        assert_eq!(highest.name, "understand");
        assert_eq!(highest.priority, Priority::Int(10000));
        let lowest = MODULES.last().unwrap_or_else(|| panic!("non-empty table"));
        assert_eq!(lowest.priority, Priority::Int(-1000));
        let priority = |name: &str| {
            find(name).map_or_else(|| panic!("{name} must exist"), |module| module.priority)
        };
        assert_eq!(priority("greeting"), Priority::Int(-1000));
        assert_eq!(priority("safety"), Priority::Int(9000));
        assert_eq!(priority("loss"), Priority::Int(2000));
        assert_eq!(priority("favourites"), Priority::Int(7000));
        // These are the modules whose names show up most in the corpus' candidates.
        assert_eq!(priority("a_check_with_you"), Priority::Int(-999));
        let negative_zero = find("a_module_with_minus_zero").map(|module| module.priority);
        assert_eq!(negative_zero, None, "no module declares -0.0 today");
    }

    #[test]
    fn modules_with_the_same_priority_are_ordered_by_name() {
        // For every priority value, the modules carrying it must appear in name order in the
        // table (this is `load_modules()`'s `(-priority, name)` sort, checked band by band).
        let mut by_priority: std::collections::BTreeMap<String, Vec<&str>> =
            std::collections::BTreeMap::new();
        for module in MODULES {
            by_priority
                .entry(module.priority.repr())
                .or_default()
                .push(module.name);
        }
        assert!(
            by_priority.len() > 10,
            "the bot uses many distinct priorities"
        );
        for (priority, names) in by_priority {
            let mut sorted = names.clone();
            sorted.sort_unstable();
            assert_eq!(names, sorted, "priority {priority} is not name-ordered");
        }
        // And equal priorities are contiguous in the table.
        let mut seen: Vec<String> = Vec::new();
        for module in MODULES {
            let key = module.priority.repr();
            if seen.last() != Some(&key) {
                assert!(!seen.contains(&key), "priority {key} is not contiguous");
                seen.push(key);
            }
        }
    }

    #[test]
    fn names_are_unique_lowercase_snake_case() {
        let mut seen = BTreeSet::new();
        for module in MODULES {
            assert!(seen.insert(module.name), "duplicate module {}", module.name);
            assert!(
                module
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{} is not snake_case",
                module.name
            );
            assert!(!module.name.starts_with('_'));
            assert!(module.hooks.windows(2).all(|pair| pair[0] < pair[1]));
            if module.has(Hook::Choose) {
                assert_eq!(module.name, "arbiter");
            }
        }
    }

    #[test]
    fn all_returns_the_table_in_order_and_ids_are_dense() {
        let from_all: Vec<&str> = all().map(|module| module.name).collect();
        let direct: Vec<&str> = MODULES.iter().map(|module| module.name).collect();
        assert_eq!(from_all, direct);
        for (index, module) in all().enumerate() {
            assert_eq!(index_of(module).map(ModuleId::index), Some(index));
        }
    }
}
