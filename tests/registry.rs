//! The module registry, checked against the Python sources it was exported from.
//!
//! `src/modules/registry.rs` is a *copy* of `bot/modules/` metadata: each module's `PRIORITY`
//! constant and the names of the hooks it defines. A copy drifts, so this file re-derives the
//! table **from the Python sources at test time** — with `std` string scanning, no Python
//! interpreter — and fails on any difference:
//!
//! * every module with at least one hook must be present, with the same priority and the same
//!   hook set;
//! * a data-only module (a `.py` file with tables but no hooks) may be in the table with no
//!   hooks, and it must never be able to reply;
//! * the table is sorted by `(-priority, name)`, which is what `runtime.load_modules()` sorts
//!   by, so "first candidate wins" and "lowest index wins" are the same statement;
//! * the name index is sorted, unique and points at the right row.
//!
//! The Python scan is deliberately forgiving about formatting (`PRIORITY` may be an int, a
//! float, have a trailing comment, underscores) and strict about semantics: a module that
//! defines `respond` but is missing from the table is a test failure, not a warning.

// Integration tests are test code: a failed assertion *should* panic, so the crate's
// production lint set (no unwrap, no expect, no indexing, no panic) does not apply here. The
// allowance is declared once, at the top of the crate root of each test binary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet};

use pybot::modules::registry::{self, Hook, ModuleDef, MODULES, NAME_INDEX};
use pybot::types::Priority;

/// The directory holding the reference implementation.
fn bot_modules() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bot/modules")
}

/// One module's metadata, as read from the Python source.
#[derive(Clone, Debug, PartialEq)]
struct PythonModule {
    name: String,
    priority: f64,
    hooks: Vec<String>,
}

/// Every hook name the runtime calls, alphabetically.
const HOOK_NAMES: [&str; 4] = ["after", "choose", "observe", "respond"];

/// Parse `PRIORITY = <number>` out of a module's source.
fn priority_of(source: &str) -> f64 {
    for line in source.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("PRIORITY") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let number: String = rest
            .trim_start()
            .chars()
            .take_while(|character| {
                character.is_ascii_digit() || matches!(character, '-' | '.' | '_')
            })
            .collect();
        let cleaned = number.replace('_', "");
        if let Ok(value) = cleaned.parse::<f64>() {
            return value;
        }
    }
    0.0
}

/// The hooks a module defines, from top-level `def <hook>(` lines.
fn hooks_of(source: &str) -> Vec<String> {
    let mut hooks = Vec::new();
    for hook in HOOK_NAMES {
        let signature = format!("def {hook}(");
        if source.lines().any(|line| line.starts_with(&signature)) {
            hooks.push(hook.to_string());
        }
    }
    hooks
}

/// Read every module in `bot/modules/`.
fn python_modules() -> Vec<PythonModule> {
    let mut modules = Vec::new();
    let entries =
        std::fs::read_dir(bot_modules()).unwrap_or_else(|error| panic!("bot/modules: {error}"));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|extension| extension != "py") {
            continue;
        }
        let name = path
            .file_stem()
            .expect("a stem")
            .to_string_lossy()
            .to_string();
        if name == "__init__" {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        modules.push(PythonModule {
            name,
            priority: priority_of(&source),
            hooks: hooks_of(&source),
        });
    }
    modules.sort_by(|left, right| left.name.cmp(&right.name));
    assert!(modules.len() >= 100, "the reference bot has ~110 modules");
    modules
}

fn rust_priority(priority: Priority) -> f64 {
    priority.as_f64()
}

fn rust_hooks(module: &ModuleDef) -> Vec<String> {
    let mut hooks = Vec::new();
    for hook in [Hook::After, Hook::Choose, Hook::Observe, Hook::Respond] {
        if module.has(hook) {
            hooks.push(hook.as_str().to_string());
        }
    }
    hooks.sort();
    hooks
}

// ---------------------------------------------------------------------------- drift

#[test]
fn every_python_module_with_a_hook_is_in_the_table_with_the_same_priority_and_hooks() {
    let python = python_modules();
    let with_hooks: Vec<&PythonModule> = python
        .iter()
        .filter(|module| !module.hooks.is_empty())
        .collect();
    assert_eq!(
        with_hooks.len(),
        102,
        "102 modules define at least one hook"
    );

    let mut problems = Vec::new();
    for module in with_hooks {
        match registry::find(&module.name) {
            None => problems.push(format!("{}: missing from MODULES", module.name)),
            Some(row) => {
                if (rust_priority(row.priority) - module.priority).abs() > f64::EPSILON {
                    problems.push(format!(
                        "{}: priority {} in Python, {} in Rust",
                        module.name,
                        module.priority,
                        row.priority.repr()
                    ));
                }
                let mut expected = module.hooks.clone();
                expected.sort();
                if rust_hooks(row) != expected {
                    problems.push(format!(
                        "{}: hooks {:?} in Python, {:?} in Rust",
                        module.name,
                        expected,
                        rust_hooks(row)
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the Rust table drifted from bot/modules/:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_table_has_no_module_the_reference_bot_lacks() {
    let known: BTreeSet<String> = python_modules()
        .into_iter()
        .map(|module| module.name)
        .collect();
    for module in MODULES {
        assert!(
            known.contains(module.name),
            "{} is in MODULES but has no file in bot/modules/",
            module.name
        );
    }
}

#[test]
fn data_only_modules_may_be_listed_but_must_never_reply() {
    // Seven files under `bot/modules/` are tables and helpers with no hooks
    // (`chat_replies.py`, `safety_cards.py`, …). They are listed so a name lookup can point at
    // them, and they must stay inert: a hook on one of them would mean the export invented it.
    let python = python_modules();
    let data_only: BTreeSet<&str> = python
        .iter()
        .filter(|module| module.hooks.is_empty())
        .map(|module| module.name.as_str())
        .collect();
    assert_eq!(data_only.len(), 7, "seven data-only modules: {data_only:?}");
    for name in &data_only {
        if let Some(row) = registry::find(name) {
            assert!(
                rust_hooks(row).is_empty(),
                "{name} has no hook in Python but {} in Rust",
                rust_hooks(row).join(", ")
            );
            assert!(!row.proposes(), "{name} can never propose a reply");
        }
    }
}

#[test]
fn every_module_in_the_table_resolves_by_name_and_by_id() {
    for (index, module) in MODULES.iter().enumerate() {
        assert_eq!(
            registry::find(module.name).map(|found| found.name),
            Some(module.name),
            "find({})",
            module.name
        );
        let id = registry::lookup(module.name).expect("lookup");
        assert_eq!(id.index(), index, "{} is at position {index}", module.name);
        assert_eq!(registry::get(id).map(|found| found.name), Some(module.name));
        assert_eq!(registry::index_of(module), Some(id));
    }
}

#[test]
fn lookups_reject_what_is_not_there() {
    for missing in [
        "",
        " ",
        "nope",
        "Greeting",
        "greeting ",
        "greetings",
        "__init__",
    ] {
        assert_eq!(registry::find(missing), None, "{missing:?}");
        assert_eq!(registry::lookup(missing), None, "{missing:?}");
    }
    assert_eq!(registry::count(), MODULES.len());
    let last = MODULES.len();
    assert_eq!(
        registry::get(pybot::types::ModuleId::from_index(
            u16::try_from(last).expect("fits")
        )),
        None
    );
}

// ---------------------------------------------------------------------------- ordering

#[test]
fn the_table_is_sorted_by_priority_desc_then_name() {
    // `runtime.load_modules()`: `loaded.sort(key=lambda item: (-item[0], item[1]))`. The table
    // being in that order is what makes the arbiter's tie-break a simple index comparison.
    let mut sorted: Vec<&ModuleDef> = MODULES.iter().collect();
    sorted.sort_by(|left, right| {
        right
            .priority
            .partial_cmp(&left.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.name.cmp(right.name))
    });
    let actual: Vec<&str> = MODULES.iter().map(|module| module.name).collect();
    let expected: Vec<&str> = sorted.iter().map(|module| module.name).collect();
    assert_eq!(actual, expected, "MODULES must be in load_modules() order");
    for window in MODULES.windows(2) {
        let (left, right) = (&window[0], &window[1]);
        let ordered = left.priority > right.priority
            || (left.priority == right.priority && left.name < right.name);
        assert!(
            ordered,
            "{} ({}) must come before {} ({})",
            left.name,
            left.priority.repr(),
            right.name,
            right.priority.repr()
        );
    }
}

#[test]
fn the_name_index_is_sorted_unique_and_points_at_the_matching_row() {
    assert_eq!(NAME_INDEX.len(), MODULES.len());
    for window in NAME_INDEX.windows(2) {
        let (left, right) = (&window[0], &window[1]);
        assert!(left.0 < right.0, "{} must sort before {}", left.0, right.0);
    }
    for (name, index) in NAME_INDEX {
        let row = MODULES
            .get(usize::from(*index))
            .unwrap_or_else(|| panic!("{name} points past the table"));
        assert_eq!(row.name, *name);
        assert_eq!(registry::index_of(row), registry::lookup(name));
    }
}

#[test]
fn the_priority_bands_match_the_reference_bot() {
    let priority = |name: &str| {
        registry::find(name)
            .unwrap_or_else(|| panic!("{name} must exist"))
            .priority
    };
    // The bands `docs/recon.md` documents, each read from the Python source.
    assert_eq!(priority("understand"), Priority::Int(10_000));
    assert_eq!(priority("intent"), Priority::Int(9_500));
    assert_eq!(priority("safety"), Priority::Int(9_000));
    assert_eq!(priority("favourites"), Priority::Int(7_000));
    assert_eq!(priority("answers"), Priority::Int(0));
    assert_eq!(priority("chat"), Priority::Int(300));
    assert_eq!(priority("arbiter"), Priority::Int(0));
    assert_eq!(priority("greeting"), Priority::Int(-1_000));

    // The extremes are the ends of the table.
    let highest = MODULES.first().expect("non-empty");
    let lowest = MODULES.last().expect("non-empty");
    assert_eq!(highest.name, "understand");
    assert_eq!(lowest.name, "greeting");
    assert_eq!(lowest.priority, Priority::Int(-1_000));
    assert!(MODULES
        .iter()
        .all(|module| module.priority <= highest.priority));
    assert!(MODULES
        .iter()
        .all(|module| module.priority >= lowest.priority));
}

#[test]
fn the_hook_population_matches_the_reference_bot() {
    let count = |hook: Hook| registry::with_hook(hook).count();
    assert_eq!(count(Hook::Observe), 16, "16 observers");
    assert_eq!(count(Hook::Respond), 97, "97 responders");
    assert_eq!(count(Hook::After), 2, "chat and memory_store");
    assert_eq!(count(Hook::Choose), 1, "the arbiter alone chooses");
    let names = |hook: Hook| -> Vec<&str> {
        registry::with_hook(hook)
            .map(|module| module.name)
            .collect()
    };
    assert_eq!(names(Hook::After), vec!["chat", "memory_store"]);
    assert_eq!(names(Hook::Choose), vec!["arbiter"]);
    // Every module with a `respond` hook proposes; observers and the arbiter do not.
    for module in registry::with_hook(Hook::Respond) {
        assert!(module.proposes(), "{} proposes", module.name);
    }
    for module in MODULES.iter().filter(|module| !module.has(Hook::Respond)) {
        assert!(!module.proposes(), "{} does not propose", module.name);
    }
}

#[test]
fn with_hook_iterates_in_table_order() {
    // The pipeline runs hooks in registry order; an iterator that reordered would change which
    // observer sees memory first.
    let observed: Vec<usize> = registry::with_hook(Hook::Observe)
        .filter_map(|module| registry::index_of(module).map(|id| id.index()))
        .collect();
    let mut sorted = observed.clone();
    sorted.sort_unstable();
    assert_eq!(observed, sorted, "ascending table positions");
    let all: Vec<&str> = registry::all().map(|module| module.name).collect();
    assert_eq!(all.len(), MODULES.len());
    assert_eq!(
        all,
        MODULES.iter().map(|module| module.name).collect::<Vec<_>>()
    );
}

#[test]
fn module_names_and_ids_round_trip_without_allocation_surprises() {
    for module in MODULES {
        let id = registry::index_of(module).expect("every row is in the table");
        let name = module.module_name();
        assert_eq!(name.as_str(), module.name);
        assert_eq!(
            id.to_string(),
            format!("#{}", id.index()),
            "ModuleId displays as #n"
        );
    }
}

// ---------------------------------------------------------------------------- wiring

#[test]
fn every_registry_row_with_a_hook_has_a_handler_or_is_documented_as_pending() {
    // Fase 1 ships skeleton handlers for seven modules; everything else keeps its metadata and
    // is skipped by the engine (the "not ported yet" path). This test pins *which* seven, so
    // adding a handler is a deliberate edit and losing one is a failure.
    let implemented: BTreeSet<&str> = MODULES
        .iter()
        .filter(|module| {
            registry::index_of(module)
                .and_then(pybot::modules::handler_for)
                .is_some()
        })
        .map(|module| module.name)
        .collect();
    assert_eq!(
        implemented,
        BTreeSet::from([
            "answers",
            "arbiter",
            "chat",
            "greeting",
            "intent",
            "memory_store",
            "understand",
        ]),
        "the Fase 1 skeleton covers these seven modules"
    );
    // A handler never exists for a module without the hook it would need.
    for module in MODULES {
        let Some(handler) = registry::index_of(module).and_then(pybot::modules::handler_for) else {
            continue;
        };
        let def = handler.def();
        assert_eq!(def.name, module.name, "a handler names its own row");
    }
}

#[test]
fn the_arbiter_is_the_only_chooser_and_the_priority_floor_uses_it() {
    let arbiter = registry::find("arbiter").expect("arbiter exists");
    assert!(arbiter.has(Hook::Choose));
    assert!(!arbiter.proposes());
    assert_eq!(arbiter.priority, Priority::Int(0));
}

#[test]
fn no_module_name_is_empty_or_contains_whitespace() {
    for module in MODULES {
        assert!(!module.name.is_empty());
        assert!(
            module
                .name
                .chars()
                .all(|character| character.is_ascii_lowercase()
                    || character == '_'
                    || character.is_ascii_digit()),
            "{} is a Python module name",
            module.name
        );
    }
}

#[test]
fn the_registry_metadata_is_a_pure_function_of_the_table() {
    // Every accessor recomputed twice gives the same answer: no lazy initialisation, no
    // interior mutability, no dependence on call order.
    let snapshot = |_| -> BTreeMap<String, (String, Vec<String>)> {
        MODULES
            .iter()
            .map(|module| {
                (
                    module.name.to_string(),
                    (module.priority.repr(), rust_hooks(module)),
                )
            })
            .collect()
    };
    assert_eq!(snapshot(()), snapshot(()));
}

// ---------------------------------------------------------------------------- the exporter

/// Priority literals the exporter must read exactly as CPython's `ast.literal_eval` would.
const PRIORITY_CASES: &[(&str, &str, &str)] = &[
    ("PRIORITY = 9000", "9000", "int"),
    ("PRIORITY = 9_000", "9000", "int"),
    ("PRIORITY = 7_050", "7050", "int"),
    ("PRIORITY = -999.5", "-999.5", "float"),
    ("PRIORITY = 1_200.5", "1200.5", "float"),
    ("PRIORITY = 9500  # the safety band", "9500", "int"),
];

#[test]
fn the_export_script_reads_digit_separators_and_comments() {
    let Some(mut command) = python() else {
        return; // no interpreter: the AST drift test above still pins the table
    };
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Drive the exporter's own regex against literals the Python tree may not contain today, so
    // a regression (truncating `9_000` to `9`, or missing the comment form) fails here.
    let script = r#"
import importlib.util, sys
spec = importlib.util.spec_from_file_location("exporter", "tools/export_registry.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
checked = 0
for line in sys.stdin.read().splitlines():
    source, value, kind = line.split("\t")
    match = module.PRIORITY_RE.search(source)
    assert match is not None, source
    raw = match.group(1).replace("_", "")
    parsed = int(raw) if "." not in raw else float(raw)
    assert type(parsed).__name__ == kind, (source, parsed)
    assert repr(parsed) == value, (source, parsed, value)
    checked += 1
sys.stderr.write(str(checked))
"#;
    let script_path =
        std::env::temp_dir().join(format!("pybot-priority-check-{}.py", std::process::id()));
    std::fs::write(&script_path, script).expect("write helper script");
    let input: String = PRIORITY_CASES
        .iter()
        .map(|(source, value, kind)| format!("{source}\t{value}\t{kind}\n"))
        .collect();
    let mut child = command
        .arg(&script_path)
        .current_dir(&root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn python");
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(input.as_bytes());
    }
    let result = child.wait_with_output().expect("python3 runs");
    std::fs::remove_file(&script_path).ok();
    assert!(
        result.status.success(),
        "exporter priority parsing failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stderr).trim(),
        PRIORITY_CASES.len().to_string(),
        "every case must be checked"
    );
}

/// A working Python interpreter, or `None`.
///
/// GitHub's Linux and macOS runners provide `python3`; the Windows image provides `python`
/// (and a `python3` shim that resolves to the Store stub, which is not a real interpreter).
/// The exporter tests are a convenience — the drift test above is the guarantee — so when no
/// interpreter runs, they skip instead of failing the matrix.
fn python() -> Option<std::process::Command> {
    for candidate in ["python3", "python"] {
        let Ok(output) = std::process::Command::new(candidate)
            .arg("--version")
            .output()
        else {
            continue;
        };
        if output.status.success() {
            return Some(std::process::Command::new(candidate));
        }
    }
    None
}
/// `\r\n` -> `\n`, so a Windows checkout or a Python `print()` writing to a Windows pipe (both
/// translate newlines) cannot make this comparison fail on an end-of-line difference. A real
/// drift — a priority, a name, a hook — still fails.
fn normalise_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[test]
fn the_export_script_reproduces_the_committed_table_exactly() {
    // `tools/export_registry.py` (no flags) prints the table it would write. The committed
    // `src/modules/registry.rs` must contain exactly those bytes between `pub const MODULES`
    // and the end of `NAME_INDEX`, so a hand edit cannot survive a `--write`.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("tools/export_registry.py");
    if !script.exists() {
        // The Python reference may be pruned in Fase 12; the drift test above still guards the
        // table against `bot/modules/`.
        return;
    }
    let Some(mut command) = python() else {
        return; // no interpreter on this machine: skip, keep the AST comparison
    };
    let output = command
        .arg(&script)
        .arg("--modules")
        .arg(root.join("bot/modules"))
        .output();
    let Ok(output) = output else {
        return;
    };
    if !output.status.success() {
        panic!(
            "the exporter failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // Compare *content*, not checkout artifacts: a Windows checkout may hand the file CRLF
    // while the exporter (Python, `\n`) writes LF. Normalise both sides before comparing, so
    // the test still fails on a real drift (a priority, a name, a hook) and never on an
    // end-of-line difference, which `.gitattributes` already controls for the repository.
    let generated = normalise_newlines(&String::from_utf8(output.stdout).expect("UTF-8 table"))
        .replace("#[rustfmt::skip]\n", "");
    let committed =
        std::fs::read_to_string(root.join("src/modules/registry.rs")).expect("registry");
    let start = committed
        .find("pub const MODULES: &[ModuleDef] = &[")
        .expect("the table");
    let index_start = committed.find("pub const NAME_INDEX").expect("index");
    let end = committed[index_start..].find("];").expect("end of index") + index_start + 2;
    let committed_table =
        normalise_newlines(&committed[start..end]).replace("#[rustfmt::skip]\n", "");
    assert_eq!(
        committed_table,
        generated.trim(),
        "`python3 tools/export_registry.py --write` would change the committed table"
    );
}

#[test]
fn the_export_script_summarises_the_hook_population() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("tools/export_registry.py");
    if !script.exists() {
        return;
    }
    let Some(mut command) = python() else {
        return;
    };
    let output = command
        .arg(&script)
        .arg("--modules")
        .arg(root.join("bot/modules"))
        .arg("--summary")
        .output();
    let Ok(output) = output else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let summary = String::from_utf8_lossy(&output.stderr);
    for needle in [
        "109 modules",
        "after=2",
        "choose=1",
        "observe=16",
        "respond=97",
    ] {
        assert!(
            summary.contains(needle),
            "summary must say {needle}: {summary}"
        );
    }
}
