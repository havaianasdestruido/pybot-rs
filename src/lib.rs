//! `PyBot`, the rule-based chatbot, rewritten in Rust with **only** the standard library.
//!
//! This crate is the port planned in [`GOAL.md`](../GOAL.md). Fase 1 (this milestone) lays
//! the foundation:
//!
//! * a crate with no dependencies, a fixed MSRV, strict lints and a CI gate;
//! * the central types ([`Turn`], [`Session`], [`Reply`], [`ModuleId`], [`Error`]) — typed,
//!   never a loose `Box<dyn Error>`;
//! * a deterministic **module pipeline** ([`runtime`]) that mirrors `bot/runtime.py`:
//!   `observe` → `respond` on a private copy of the memory → arbiter → the winner's copy
//!   becomes the conversation's memory → `after` → the turn is appended to the log;
//! * a **differential runner** ([`golden`], `src/bin/differential.rs`) that replays the
//!   golden corpus (`tests/golden/corpus.jsonl`, 2 850 sessions / 3 062 turns captured from
//!   the Python bot) and reports agreement by module and directory;
//! * the CLI skeleton (`-p`, safe stdin loop, `/reset`, `/exit`) in [`cli`].
//!
//! Equivalence with the Python bot is *not* expected yet: Fase 1 delivers the harness that
//! measures it. Later phases replace the skeleton modules with the real port.
//!
//! # Stability rules (GOAL.md)
//!
//! Stability > equivalence > performance. Consequently:
//!
//! * [`#![forbid(unsafe_code)]`](https://doc.rust-lang.org/reference/attributes/codegen.html)
//!   at the crate root — the only future exception is the isolated `ffi` module of Fase 11;
//! * no `unwrap`/`expect`/panicking indexing outside tests (enforced by the `[lints]` table in
//!   `Cargo.toml` and re-checked by `tests/style_guards.rs`);
//! * every module call runs inside `catch_unwind`: a module that panics is skipped like in
//!   Python (`except Exception: continue`) and the turn still returns a reply;
//! * explicit limits ([`limits`]) on input size, memory keys and the turn log;
//! * deterministic behaviour: never iterate a `HashMap` to decide output; the ordered
//!   [`memory::Map`] preserves insertion order like a Python `dict`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]
#![warn(clippy::pedantic)]
// Pedantic asks for many things that hurt readability in a standard-library-only port.
// Each exception below is deliberate and documented:
#![allow(clippy::module_name_repetitions)] // `golden::GoldenReport`, `memory::Memory`
#![allow(clippy::must_use_candidate)] // the API is enforced by tests, not by `#[must_use]`
#![allow(clippy::missing_errors_doc)] // every fallible item documents its error enum instead
#![allow(clippy::missing_panics_doc)] // nothing panics: panics are forbidden by the lints
#![allow(clippy::return_self_not_must_use)] // builder-style helpers are used internally only
#![allow(clippy::cast_precision_loss)] // deliberate: durations -> f64 for Python-like ms
#![allow(clippy::cast_possible_truncation)] // deliberate: bounded values (turn indices)
#![allow(clippy::cast_sign_loss)] // deliberate: values are known non-negative
#![allow(clippy::items_after_statements)] // small helper `use`/`fn` next to their use
#![allow(clippy::needless_pass_by_value)] // `Reply`/`Turn` are cheap, owned by design
#![allow(clippy::similar_names)] // `memory`/`memories` and friends read better than synonyms
#![allow(clippy::too_many_lines)] // table-driven code (registry, tests) is one block by design

pub mod arbiter;
pub mod cli;
pub mod engine;
pub mod error;
pub mod golden;
pub mod hash;
pub mod json;
pub mod limits;
pub mod memory;
pub mod modules;
pub mod pyutil;
pub mod rng;
pub mod runtime;
pub mod session;
pub mod types;
pub mod unicode;
pub mod util;

pub use error::{Error, Result};
pub use memory::{Map, Memory, Snapshot, Value};
pub use runtime::{Answer, Candidate, Fault, Faults, Runtime, TurnReport};
pub use session::{LoggedTurn, Session};
pub use types::{ModuleId, ModuleName, Priority, Reply, SessionId, Turn};
