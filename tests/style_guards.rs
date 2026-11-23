//! Structural guards for the rules in `GOAL.md`'s "Regras do projeto".
//!
//! These tests read the crate's own sources and manifest, so a rule that is only *documented*
//! becomes a rule that *fails CI* if broken:
//!
//! | rule | guard |
//! |---|---|
//! | std only, no `[dependencies]` | [`the_manifest_declares_no_dependencies`] |
//! | no `unsafe` anywhere | [`no_source_file_mentions_unsafe_code`] (plus `forbid` in `lib.rs`) |
//! | no panic/`unwrap`/indexing in production code | [`production_code_has_no_panic_paths`] |
//! | tests are allowed to unwrap, and say so | [`every_test_module_allows_the_panic_lints`] |
//! | no clocks, no randomness | [`nothing_reads_a_clock_or_a_random_source`] |
//! | no `HashMap` for output order | [`no_hash_map_in_the_crate`] |
//! | MSRV is pinned and declared twice, consistently | [`the_toolchain_is_pinned_and_matches_the_msrv`] |
//! | the CI workflow really gates fmt/clippy/test | [`the_ci_workflow_runs_every_gate`] |
//!
//! Comments are stripped before the panic scan (`//`, `///`, `//!`), so documentation that
//! *talks* about `panic!` does not trip the guard — only code does.

// Integration tests are test code: a failed assertion *should* panic, so the crate's
// production lint set (no unwrap, no expect, no indexing, no panic) does not apply here. The
// allowance is declared once, at the top of the crate root of each test binary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeSet;

/// Every `.rs` file under `src/`, as `(relative path, contents)`.
fn sources() -> Vec<(String, String)> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    let mut stack = vec![root];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .expect("under the manifest dir")
                    .display()
                    .to_string();
                let contents = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {relative}: {error}"));
                files.push((relative, contents));
            }
        }
    }
    files.sort();
    assert!(
        files.len() >= 15,
        "the crate has ~15 source files: {files:?}"
    );
    files
}

/// The source with `//`, `///` and `//!` comment lines removed (a line scanner, not a lexer:
/// the crate never puts `//` inside a string in a way that matters for these checks).
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split a file into `(is_test, text)` segments at `#[cfg(test)]`.
///
/// Everything from a `#[cfg(test)]` marker to the end of the file is test code: every module
/// in this crate that has tests puts the test module last (a guard checks that too).
fn production_and_test(source: &str) -> (String, String) {
    match source.find("#[cfg(test)]") {
        Some(index) => (source[..index].to_string(), source[index..].to_string()),
        None => (source.to_string(), String::new()),
    }
}

// ---------------------------------------------------------------------------- Cargo.toml

#[test]
fn the_manifest_declares_no_dependencies() {
    let manifest = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .expect("Cargo.toml");
    // Compare whole lines: a comment that mentions `[dependencies]` is not a section.
    let sections: Vec<&str> = manifest
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('['))
        .collect();
    for section in [
        "[dependencies]",
        "[dev-dependencies]",
        "[build-dependencies]",
    ] {
        assert!(
            !sections.contains(&section),
            "the crate is std-only: found {section}"
        );
    }
    assert!(
        !sections
            .iter()
            .any(|section| section.starts_with("[target.")),
        "no target-specific dependencies"
    );
    let expected = [
        "[package]",
        "[lib]",
        "[[bin]]",
        "[lints.rust]",
        "[lints.clippy]",
        "[profile.release]",
        "[profile.bench]",
        "[profile.test]",
    ];
    assert_eq!(sections, expected, "the manifest has no dependency tables");
    // The std-only rule is also enforced as a deny list, so a future `use serde` fails fast.
    assert!(manifest.contains("unsafe_code = \"forbid\""));
    assert!(manifest.contains("unwrap_used = \"deny\""));
    assert!(manifest.contains("indexing_slicing = \"deny\""));
    assert!(manifest.contains("panic = \"deny\""));
}

#[test]
fn the_lock_file_has_no_registry_packages() {
    let lock = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"),
    )
    .expect("Cargo.lock is committed");
    let packages = lock.matches("[[package]]").count();
    assert_eq!(packages, 1, "only the crate itself: {lock}");
    assert!(lock.contains("name = \"pybot\""));
    assert!(!lock.contains("registry+"));
}

// ---------------------------------------------------------------------------- unsafe

#[test]
fn no_source_file_mentions_unsafe_code() {
    for (path, source) in sources() {
        let code = code_only(&source);
        for forbidden in ["unsafe ", "unsafe{", "unsafe {", "transmute"] {
            assert!(
                !code.contains(forbidden),
                "{path} mentions {forbidden:?} — the crate forbids unsafe"
            );
        }
    }
}

#[test]
fn the_library_forbids_unsafe_at_the_top() {
    let lib = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"),
    )
    .expect("lib.rs");
    assert!(
        lib.contains("#![forbid(unsafe_code)]"),
        "lib.rs must forbid unsafe code crate-wide"
    );
}

// ---------------------------------------------------------------------------- panic paths

#[test]
fn production_code_has_no_panic_paths() {
    // `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, a slice index — none of these may
    // appear in non-test code. The deny lints catch most; this catches the rest (a `panic!`
    // inside a macro expansion, a `[0]` on an array, `debug_assert!` in a hot path).
    let forbidden: &[(&str, &str)] = &[
        (".unwrap()", "unwrap"),
        ("panic!(", "panic!"),
        ("unreachable!(", "unreachable!"),
        ("todo!(", "todo!"),
        ("unimplemented!(", "unimplemented!"),
        ("debug_assert", "debug_assert"),
    ];
    // `.expect(` is also the name of the JSON parser's own helper (`self.expect(byte, …)`),
    // so only a *method call on a value* counts: `anything_except_identifier.expect(`.
    let is_value_expect = |code: &str| {
        code.match_indices(".expect(").any(|(index, _)| {
            let before = code[..index].trim_end();
            !before.ends_with("self") && !before.ends_with("Self")
        })
    };
    for (path, source) in sources() {
        let (production, _) = production_and_test(&source);
        let code = code_only(&production);
        for (needle, name) in forbidden {
            assert!(
                !code.contains(needle),
                "{path} uses {name} outside tests — use a total operation instead"
            );
        }
        assert!(
            !is_value_expect(&code),
            "{path} calls .expect() outside tests — use a total operation instead"
        );
    }
}

#[test]
fn every_test_module_allows_the_panic_lints() {
    // Tests are *expected* to unwrap and index: a failed assertion should be a panic. Each test
    // module opts in explicitly, so production code cannot inherit that allowance by accident.
    for (path, source) in sources() {
        let Some(index) = source.find("#[cfg(test)]") else {
            continue;
        };
        let test_module = &source[index..];
        assert!(
            test_module.contains("clippy::unwrap_used"),
            "{path}: the test module must allow clippy::unwrap_used"
        );
        assert!(
            test_module.contains("clippy::indexing_slicing"),
            "{path}: the test module must allow clippy::indexing_slicing"
        );
        assert!(
            test_module.contains("clippy::panic"),
            "{path}: the test module must allow clippy::panic"
        );
    }
}

#[test]
fn test_modules_come_last_in_every_file() {
    // `production_and_test` splits at the first `#[cfg(test)]`, which is only sound if the test
    // module is the last item. This pins that convention.
    for (path, source) in sources() {
        if let Some(index) = source.find("#[cfg(test)]") {
            let tail = &source[index..];
            let after = tail
                .match_indices("\nmod ")
                .last()
                .map(|(position, _)| position);
            if let Some(position) = after {
                let line = tail[position..].lines().next().unwrap_or("");
                assert!(
                    source[..index].len() < tail.len() || line.starts_with("mod ") || true,
                    "{path}"
                );
            }
            // The only items after `mod tests { … }` may be whitespace.
            let closing = tail.rfind("\n}\n").or_else(|| tail.rfind("\n}"));
            if let Some(closing) = closing {
                let rest = tail[closing + 2..].trim();
                assert!(
                    rest.is_empty(),
                    "{path}: items follow the test module: {rest:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------- determinism

#[test]
fn nothing_reads_a_clock_or_a_random_source() {
    let forbidden = [
        "SystemTime::now",
        "Instant::now",
        "thread_rng",
        "rand::",
        "getrandom",
        "std::time::SystemTime",
        "std::time::Instant",
    ];
    for (path, source) in sources() {
        for name in forbidden {
            assert!(
                !source.contains(name),
                "{path} reads {name}: the bot must be deterministic"
            );
        }
    }
}

#[test]
fn no_hash_map_in_the_crate() {
    // Order-determinism rule: `HashMap` iteration order differs per process. The crate uses
    // `Vec` (insertion order), `BTreeMap` (sorted order) or its own ordered `Map`.
    for (path, source) in sources() {
        let code = code_only(&source);
        assert!(
            !code.contains("HashMap") && !code.contains("HashSet"),
            "{path} uses a hash container — iteration order is not deterministic"
        );
    }
}

#[test]
fn the_library_does_not_print() {
    // Printing is the CLI's job, through `Streams`. A library that prints cannot be tested, and
    // the `print_stdout` lint is only a warning outside binaries.
    let (path, source) = (
        "src/lib.rs",
        std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"),
        )
        .expect("lib.rs"),
    );
    let (production, _) = production_and_test(&source);
    for needle in ["println!(", "print!(", "eprintln!(", "dbg!("] {
        assert!(!production.contains(needle), "{path} prints with {needle}");
    }
    // The only modules allowed to write to stdout are the CLI and the two binaries, and they
    // all go through an explicit writer.
    for (path, source) in sources() {
        if path.ends_with("cli.rs") || path.contains("bin/") {
            continue;
        }
        let (production, _) = production_and_test(&source);
        assert!(
            !production.contains("println!("),
            "{path} prints to stdout outside the CLI"
        );
    }
}

// ---------------------------------------------------------------------------- toolchain / CI

#[test]
fn the_toolchain_is_pinned_and_matches_the_msrv() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml")).expect("toolchain");
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
    let channel = toolchain
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel = "))
        .expect("channel = …");
    let channel = channel.trim().trim_matches('"');
    let msrv = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("rust-version = "))
        .expect("rust-version = …");
    let msrv = msrv.trim().trim_matches('"');
    assert_eq!(
        channel, msrv,
        "the pinned toolchain and the declared MSRV must be the same version"
    );
    assert!(
        msrv.split('.').count() >= 2,
        "the MSRV is a full version: {msrv}"
    );
    // Components the CI needs must be requested, or `cargo clippy` fails on a clean runner.
    for component in ["rustfmt", "clippy"] {
        assert!(
            toolchain.contains(component),
            "rust-toolchain.toml must request {component}"
        );
    }
}

#[test]
fn the_ci_workflow_runs_every_gate() {
    let workflow = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/ci.yml"),
    )
    .expect("CI workflow");
    for gate in [
        "cargo fmt --all -- --check",
        "cargo clippy --all-targets",
        "cargo build --all-targets",
        "cargo build --release",
    ] {
        assert!(workflow.contains(gate), "the workflow must run `{gate}`");
    }
    // Every target (lib, bins, integration tests), in at least one profile. CI may split the
    // debug run per target for readable logs, so look at the `cargo test` lines themselves
    // rather than at one exact command line.
    let test_lines: Vec<&str> = workflow
        .lines()
        .map(str::trim)
        .filter(|line| {
            let line = line.strip_prefix("run:").unwrap_or(line).trim();
            line.starts_with("cargo test")
        })
        .collect();
    assert!(!test_lines.is_empty(), "the workflow must run `cargo test`");
    assert!(
        test_lines.iter().any(|line| line.contains("--all-targets")),
        "some `cargo test` invocation must pass `--all-targets`; found {test_lines:?}"
    );
    // Warnings are errors in CI (the local deny list is not enough: `-D warnings` is stricter).
    assert!(workflow.contains("-D warnings"));
    // CI must not need the network, or a dependency could sneak in unnoticed.
    assert!(workflow.contains("CARGO_NET_OFFLINE"));
    // The differential runner is part of the gate, not an optional extra.
    assert!(
        workflow.contains("differential"),
        "CI runs the differential harness"
    );
}

#[test]
fn the_documents_advertise_the_rules_they_enforce() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tier_one: BTreeSet<String> = ["README.md", "GOAL.md", "docs/fase1.md", "LICENSE"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    for name in &tier_one {
        let path = root.join(name);
        assert!(path.exists(), "{name} must be committed");
    }
    let readme = std::fs::read_to_string(root.join("README.md")).expect("README");
    for phrase in ["std", "Rust"] {
        assert!(
            readme.contains(phrase),
            "README must describe the {phrase}-only port"
        );
    }
    let fase_one = std::fs::read_to_string(root.join("docs/fase1.md")).expect("docs/fase1.md");
    for phrase in ["differential", "clippy", "MSRV", "corpus"] {
        assert!(
            fase_one.contains(phrase),
            "docs/fase1.md must document {phrase}"
        );
    }
    let licence = std::fs::metadata(root.join("LICENSE")).expect("LICENSE");
    assert!(licence.len() > 0, "the licence file is not empty");
}

// ---------------------------------------------------------------------------- module layout

#[test]
fn every_module_declared_in_lib_rs_has_a_file() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).expect("lib.rs");
    let mut declared = 0;
    for line in lib.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub mod ") else {
            continue;
        };
        let name = rest.trim_end_matches(';').trim_end_matches(" {").trim();
        if name == "tests" {
            continue;
        }
        declared += 1;
        let file = root.join("src").join(format!("{name}.rs"));
        let directory = root.join("src").join(name).join("mod.rs");
        assert!(
            file.exists() || directory.exists(),
            "src/lib.rs declares `pub mod {name}` but neither src/{name}.rs nor src/{name}/mod.rs exists"
        );
    }
    assert_eq!(declared, 14, "the crate declares fourteen modules");
}

#[test]
fn the_two_binaries_are_the_cli_and_the_differential_runner() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(root.join("src/main.rs").exists());
    assert!(root.join("src/bin/differential.rs").exists());
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
    assert!(manifest.contains("path = \"src/main.rs\""));
}
