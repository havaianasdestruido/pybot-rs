//! End-to-end tests for the differential harness, against the **shipped** corpus.
//!
//! `src/golden.rs` has unit tests for its own pieces; this file treats the harness as a black
//! box and asks the questions a CI reviewer would ask:
//!
//! * does the corpus on disk have the shape the manifest promises (and its stated SHA-256)?
//! * does a full replay produce a report that is *honest* — every turn accounted for, every
//!   block represented, no silent skips, no faults, and percentages that match the counts?
//! * is the report deterministic (twice, and across two processes)?
//! * is the JSON output valid JSON that carries every figure the text report shows?
//! * does the block classifier agree with the generator's own session numbering?
//!
//! The corpus is 3 062 turns / 2 850 sessions, so these tests do real work: they replay the
//! whole thing through the Rust bot, exactly as `cargo run --bin differential` does.

// Integration tests are test code: a failed assertion *should* panic, so the crate's
// production lint set (no unwrap, no expect, no indexing, no panic) does not apply here. The
// allowance is declared once, at the top of the crate root of each test binary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use pybot::golden::{self, Block, Corpus, GoldenReport};
use pybot::json::{self, Json};

/// The corpus directory, resolved at compile time so the tests work from any `cwd`.
fn corpus_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn corpus() -> Corpus {
    golden::load_dir(&corpus_dir()).expect("the shipped corpus must load")
}

fn report() -> GoldenReport {
    GoldenReport::build(&golden::compare_corpus(&corpus()))
}

// ---------------------------------------------------------------------------- the raw corpus

#[test]
fn the_corpus_file_matches_the_manifest_byte_for_byte() {
    // The manifest records the SHA-256 of `corpus.jsonl`. A hand-edited corpus is a corpus
    // whose numbers can no longer be trusted, so this is checked before anything else.
    let bytes = std::fs::read(corpus_dir().join("corpus.jsonl")).expect("read corpus");
    let corpus = corpus();
    let manifest = corpus.manifest.as_ref().expect("manifest is present");
    assert_eq!(
        pybot::hash::sha256(&[&bytes]).hex(),
        manifest.corpus_sha256,
        "the corpus on disk is not the one the manifest describes"
    );
    assert_eq!(manifest.sessions, 2_850);
    assert_eq!(manifest.turns, 3_062);
    assert_eq!(manifest.corpus_seed, 20_261_121);
    assert_eq!(manifest.pythonhashseed, "0");
}

#[test]
fn every_line_of_the_corpus_parses_and_every_field_is_present() {
    let text = std::fs::read_to_string(corpus_dir().join("corpus.jsonl")).expect("read corpus");
    let mut lines = 0;
    for (number, line) in text.lines().enumerate() {
        let parsed = json::parse_str(line).unwrap_or_else(|error| {
            panic!("line {} is not JSON: {error}", number + 1);
        });
        let object = parsed.as_object().expect("an object");
        let keys: Vec<&str> = object.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "session",
                "turn",
                "input",
                "reply",
                "module",
                "candidates",
                "memory_keys",
                "memory_digest",
                "memory"
            ],
            "line {} has the documented field order",
            number + 1
        );
        lines += 1;
    }
    assert_eq!(lines, 3_062);
}

#[test]
fn turn_numbers_restart_at_zero_for_every_session_and_climb_by_one() {
    let corpus = corpus();
    let mut expected: BTreeMap<&str, u32> = BTreeMap::new();
    for turn in &corpus.turns {
        let next = expected.entry(turn.session.as_str()).or_insert(0);
        assert_eq!(turn.turn, *next, "{} turn numbering", turn.session);
        *next += 1;
    }
    assert_eq!(expected.len(), 2_850);
}

#[test]
fn sessions_are_contiguous_and_their_numbers_are_dense() {
    let corpus = corpus();
    let mut seen: Vec<&str> = Vec::new();
    for turn in &corpus.turns {
        if seen.last() != Some(&turn.session.as_str()) {
            assert!(
                !seen.contains(&turn.session.as_str()),
                "session {} is split across the file",
                turn.session
            );
            seen.push(&turn.session);
        }
    }
    assert_eq!(seen.len(), 2_850);
    for (index, session) in seen.iter().enumerate() {
        assert_eq!(*session, format!("s{index:04}"), "dense session ids");
    }
}

#[test]
fn the_corpus_records_only_modules_the_registry_knows() {
    // The oracle names 18 modules; the Rust registry has 109 rows (the whole Python table).
    // Any name the corpus uses must resolve, otherwise the registry export drifted.
    let corpus = corpus();
    let mut names: Vec<&str> = Vec::new();
    for turn in &corpus.turns {
        if let Some(module) = &turn.module {
            names.push(module);
        }
        for (module, _) in &turn.candidates {
            names.push(module);
        }
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 18, "18 distinct modules appear: {names:?}");
    for name in names {
        assert!(
            pybot::modules::registry::find(name).is_some(),
            "{name} is in the corpus but not in the registry"
        );
    }
}

#[test]
fn memory_keys_are_sorted_and_unique_on_every_turn() {
    let corpus = corpus();
    for turn in &corpus.turns {
        let mut sorted = turn.memory_keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            turn.memory_keys, sorted,
            "{} turn {}",
            turn.session, turn.turn
        );
        assert!(!turn.memory_keys.is_empty(), "every turn leaves memory");
    }
}

#[test]
fn every_digest_is_a_lowercase_sha1_and_every_reply_is_normalised() {
    let corpus = corpus();
    for turn in &corpus.turns {
        assert_eq!(turn.memory_digest.len(), 40, "{}", turn.session);
        assert!(
            turn.memory_digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{}: {}",
            turn.session,
            turn.memory_digest
        );
        // `" ".join(reply.split())`: no runs of spaces, no ends, and no tab/newline survives.
        assert_eq!(pybot::pyutil::collapse_spaces(&turn.reply), turn.reply);
        assert_eq!(pybot::pyutil::strip(&turn.reply), turn.reply);
        assert!(!turn.reply.is_empty(), "the fallback is never empty");
    }
}

#[test]
fn the_hard_inputs_the_generator_threw_at_the_bot_are_all_there() {
    // Spot-check the adversarial tail of the curated block: empty, whitespace, punctuation,
    // emoji, RTL-ish combining text, regex metacharacters, NUL, and the 1 004-character reply.
    let corpus = corpus();
    let inputs: Vec<&str> = corpus
        .turns
        .iter()
        .map(|turn| turn.input.as_str())
        .collect();
    for needle in [
        "",
        "   ",
        "?",
        "!!!",
        "😀😀😀",
        "𝕳𝖊𝖑𝖑𝖔 there",
        "SELECT * FROM users;",
        "<script>alert(1)</script>",
        "{{7*7}}",
        "(?=lookahead)",
        "naïve café résumé",
        "Ångström",
        "quero um café forte",
    ] {
        assert!(
            inputs.contains(&needle),
            "{needle:?} should be in the curated block"
        );
    }
    assert!(
        corpus
            .turns
            .iter()
            .any(|turn| turn.reply.chars().count() >= 1_000),
        "the longest reply (>1 000 chars) is recorded"
    );
}

// ---------------------------------------------------------------------------- the blocks

#[test]
fn blocks_partition_the_corpus_without_overlap_or_gaps() {
    let corpus = corpus();
    let counts = corpus.block_counts();
    assert_eq!(counts.len(), 3);
    assert_eq!(counts.get("curated").copied(), Some(512));
    assert_eq!(counts.get("fuzz").copied(), Some(704));
    assert_eq!(counts.get("knowledge_probes").copied(), Some(1_846));
    assert_eq!(counts.values().sum::<usize>(), 3_062);
}

#[test]
fn the_block_of_a_turn_follows_its_session_id_alone() {
    let corpus = corpus();
    for turn in &corpus.turns {
        let number: u32 = turn.session.trim_start_matches('s').parse().expect("s0123");
        let expected = match number {
            0..=299 => Block::Curated,
            300..=1_003 => Block::Fuzz,
            _ => Block::Knowledge,
        };
        assert_eq!(Block::of_session(number), expected, "{}", turn.session);
    }
}

#[test]
fn the_three_blocks_have_the_character_the_generator_gave_them() {
    let corpus = corpus();
    let curated: Vec<&str> = corpus
        .turns
        .iter()
        .filter(|turn| turn.block() == Block::Curated)
        .map(|turn| turn.input.as_str())
        .collect();
    let fuzz: Vec<&str> = corpus
        .turns
        .iter()
        .filter(|turn| turn.block() == Block::Fuzz)
        .map(|turn| turn.input.as_str())
        .collect();
    let knowledge: Vec<&str> = corpus
        .turns
        .iter()
        .filter(|turn| turn.block() == Block::Knowledge)
        .map(|turn| turn.input.as_str())
        .collect();

    // Curated: hand-written prompts, including every multi-turn session (5 turns at most).
    assert!(curated.iter().any(|line| line.is_empty()));
    assert!(curated.iter().any(|line| line.starts_with("tell me about")));

    // Fuzz: single-turn mutations — a misspelling of a curated prompt is the signature.
    assert_eq!(fuzz.len(), 704);
    assert!(
        fuzz.iter()
            .any(|line| line.contains("abut") || line.contains("teh")),
        "typo'd probes are part of the fuzz block"
    );

    // Knowledge: one question per entry, round-robin over the knowledge base.
    assert_eq!(knowledge.len(), 1_846);
    assert!(knowledge.iter().all(|line| !line.is_empty()));
    // Probes are the knowledge base's own questions, in several shapes.
    let counting = |prefix: &str| {
        knowledge
            .iter()
            .filter(|line| line.starts_with(prefix))
            .count()
    };
    assert!(counting("how do i") > 250, "how-to probes");
    assert!(counting("what do you") > 150, "description probes");
    assert!(counting("tell me about") > 150, "'tell me about' probes");
    // A handful of leading-space probes are deliberate noise (the tail of the fuzz block
    // bleeds into the knowledge range); they must be rare.
    assert!(
        counting(" ") < 20,
        "leading-space probes are the exception: {}",
        counting(" ")
    );
}

#[test]
fn every_session_lives_inside_exactly_one_block() {
    let corpus = corpus();
    let mut blocks: BTreeMap<&str, Block> = BTreeMap::new();
    for turn in &corpus.turns {
        match blocks.get(turn.session.as_str()) {
            Some(existing) => assert_eq!(*existing, turn.block(), "{}", turn.session),
            None => {
                blocks.insert(&turn.session, turn.block());
            }
        }
    }
    assert_eq!(blocks.len(), 2_850);
    assert_eq!(
        blocks
            .values()
            .filter(|block| **block == Block::Curated)
            .count(),
        300,
        "s0000–s0299 are the curated sessions"
    );
}

// ---------------------------------------------------------------------------- the replay

#[test]
fn the_report_accounts_for_every_turn_exactly_once() {
    let report = report();
    assert_eq!(report.turns, 3_062);
    assert_eq!(report.overall.turns, 3_062);
    for field in [
        report.overall.replies,
        report.overall.modules,
        report.overall.candidates,
        report.overall.memory_keys,
        report.overall.memory_digest,
    ] {
        assert!(field <= report.overall.turns, "no count exceeds the total");
    }
    // Blocks add up to the whole corpus, and so do the per-module groups.
    assert_eq!(
        report
            .blocks
            .iter()
            .map(|group| group.agreement.turns)
            .sum::<usize>(),
        3_062
    );
    assert_eq!(
        report
            .by_expected_module
            .iter()
            .map(|group| group.agreement.turns)
            .sum::<usize>(),
        3_062
    );
}

#[test]
fn the_replay_produces_no_faults_on_the_whole_corpus() {
    // A fault here means a handler panicked or a limit was hit — the corpus was built to sit
    // inside the limits, so a fault is a bug in the port, not in the corpus.
    let report = report();
    assert!(
        report.faults.is_empty(),
        "unexpected faults: {:?}",
        report.faults
    );
    assert_eq!(report.overall.faulted, 0);
}

#[test]
fn the_percentages_are_derived_from_the_counts_not_guessed() {
    let report = report();
    let agreement = &report.overall;
    let percent = |count: usize| {
        if agreement.turns == 0 {
            0.0
        } else {
            count as f64 * 100.0 / agreement.turns as f64
        }
    };
    assert!((agreement.reply_percent() - percent(agreement.replies)).abs() < 1e-9);
    assert!((agreement.module_percent() - percent(agreement.modules)).abs() < 1e-9);
    assert!((agreement.candidate_percent() - percent(agreement.candidates)).abs() < 1e-9);
    assert!((agreement.memory_keys_percent() - percent(agreement.memory_keys)).abs() < 1e-9);
    assert!((agreement.digest_percent() - percent(agreement.memory_digest)).abs() < 1e-9);
    for value in [
        agreement.reply_percent(),
        agreement.module_percent(),
        agreement.candidate_percent(),
        agreement.memory_keys_percent(),
        agreement.digest_percent(),
    ] {
        assert!((0.0..=100.0).contains(&value), "{value} is a percentage");
    }
}

#[test]
fn the_phase_one_report_shows_a_real_gap_and_real_agreement() {
    // GOAL.md Fase 1: 0 % equivalence is acceptable, but the harness must be *honest*. The
    // skeleton reproduces the greeting line, so reply agreement is neither 0 nor 100.
    let report = report();
    let reply = report.overall.reply_percent();
    assert!(
        reply > 0.5 && reply < 50.0,
        "the skeleton gets some replies right and misses most: {reply:.2}%"
    );
    // The winner is almost never right: only `greeting` is implemented.
    let module = report.overall.module_percent();
    assert!(
        module < 5.0,
        "module agreement must stay visibly low until Fase 7: {module:.2}%"
    );
    // The fuzz block is harder than the curated one for the reply signal.
    let curated = report
        .blocks
        .iter()
        .find(|group| group.name == "curated")
        .expect("curated block");
    let fuzz = report
        .blocks
        .iter()
        .find(|group| group.name == "fuzz")
        .expect("fuzz block");
    assert!(
        curated.agreement.reply_percent() > fuzz.agreement.reply_percent(),
        "curated {:.2}% vs fuzz {:.2}%",
        curated.agreement.reply_percent(),
        fuzz.agreement.reply_percent()
    );
}

#[test]
fn the_greeting_module_is_the_one_thing_that_already_agrees() {
    let report = report();
    let chat = report
        .by_expected_module
        .iter()
        .find(|group| group.name == "chat")
        .expect("chat group: the seed replies to greetings");
    assert!(
        chat.agreement.reply_percent() > 10.0,
        "the skeleton reproduces the greeting line: {:.2}%",
        chat.agreement.reply_percent()
    );
    // Every expected module in the corpus gets a group.
    let names: Vec<&str> = report
        .by_expected_module
        .iter()
        .map(|group| group.name.as_str())
        .collect();
    for expected in [
        "answers",
        "chat",
        "greeting",
        "memory_store",
        "safety",
        "favourites",
        "loss",
    ] {
        assert!(names.contains(&expected), "missing group {expected}");
    }
}

#[test]
fn the_answers_module_is_the_largest_gap_in_the_phase_one_port() {
    // 1 571 turns expect `answers`, the knowledge base (Fase 6). The skeleton cannot answer
    // any of them, and the report says so with zero agreement rather than hiding the block.
    let report = report();
    let answers = report
        .by_expected_module
        .iter()
        .find(|group| group.name == "answers")
        .expect("answers group");
    assert_eq!(answers.agreement.turns, 1_571);
    assert!((answers.agreement.reply_percent()).abs() < 1e-9);
    assert!((answers.agreement.module_percent()).abs() < 1e-9);
    assert!(!answers.examples.is_empty(), "the gap comes with examples");
    assert!(answers.examples.len() <= golden::MAX_GROUPED_DIFFS);
}

#[test]
fn grouped_diffs_are_capped_and_deterministic() {
    let report = report();
    for group in report
        .blocks
        .iter()
        .chain(&report.by_expected_module)
        .chain(&report.by_candidate_module)
    {
        assert!(
            group.examples.len() <= golden::MAX_GROUPED_DIFFS,
            "{} has {} examples",
            group.name,
            group.examples.len()
        );
        // Examples are the first N differing turns in file order, so replaying yields the same
        // ones — no dependence on a hash map's iteration order.
        let mut previous: Option<(String, u32)> = None;
        for example in &group.examples {
            let position = (example.session.clone(), example.turn);
            if let Some(previous) = previous {
                assert!(previous < position, "{} is out of order", group.name);
            }
            previous = Some(position);
        }
    }
}

#[test]
fn the_candidate_module_groups_mention_every_module_either_side_proposed() {
    let corpus = corpus();
    let mut expected: Vec<&str> = Vec::new();
    for turn in &corpus.turns {
        if let Some(module) = &turn.module {
            expected.push(module);
        }
        for (module, _) in &turn.candidates {
            expected.push(module);
        }
    }
    expected.sort_unstable();
    expected.dedup();
    let report = report();
    let names: Vec<&str> = report
        .by_candidate_module
        .iter()
        .map(|group| group.name.as_str())
        .collect();
    for name in expected {
        assert!(
            names.contains(&name),
            "{name} should have a candidate group"
        );
    }
    // Alphabetical, and no duplicates.
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(names, sorted);
}

#[test]
fn the_replay_is_deterministic_across_runs_and_instances() {
    let first = goldon_text();
    let second = goldon_text();
    assert_eq!(first, second, "two runs in one process must agree");
    // A second fresh `Corpus` and `Runtime` (no shared state) must give the same report.
    let other = GoldenReport::build(&golden::compare_corpus(&corpus()));
    assert_eq!(first, other.render_text());
}

fn goldon_text() -> String {
    report().render_text()
}

// ---------------------------------------------------------------------------- the outputs

#[test]
fn the_text_report_has_one_section_per_question_a_reviewer_asks() {
    let text = report().render_text();
    for needle in [
        "differential report — 3062 turns",
        "overall: reply",
        "by block (directory):",
        "by expected module:",
        "by candidate module (turns it appears in):",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    assert!(text.contains("curated"), "the blocks are named");
    assert!(text.contains("fuzz"));
    assert!(text.contains("knowledge_probes"));
    assert!(text.contains("e.g. s"), "the diffs carry examples");
}

#[test]
fn the_json_report_is_valid_json_and_carries_every_figure() {
    let report = report();
    let encoded = json::to_string(&report.to_json());
    let parsed = json::parse_str(&encoded).expect("the report must be valid JSON");
    let object = parsed.as_object().expect("an object");
    let keys: Vec<&str> = object.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "turns",
            "overall",
            "blocks",
            "by_expected_module",
            "by_candidate_module",
            "faults"
        ],
        "stable key order: a report is an interface"
    );
    assert_eq!(parsed.get("turns").and_then(Json::as_i64), Some(3_062));
    let overall = parsed.get("overall").expect("overall");
    for key in [
        "turns",
        "replies",
        "modules",
        "candidates",
        "memory_keys",
        "memory_digest",
        "faulted",
    ] {
        assert!(overall.get(key).is_some(), "overall.{key}");
    }
    assert_eq!(overall.get("turns").and_then(Json::as_i64), Some(3_062));
    let blocks = parsed
        .get("blocks")
        .and_then(Json::as_array)
        .expect("blocks array");
    assert_eq!(blocks.len(), 3);
    let names: Vec<&str> = blocks
        .iter()
        .map(|block| block.get("name").and_then(Json::as_str).expect("name"))
        .collect();
    assert_eq!(names, vec!["curated", "fuzz", "knowledge_probes"]);
    // Every example inside is a well-formed object with the five documented fields.
    for group in parsed
        .get("by_expected_module")
        .and_then(Json::as_array)
        .expect("groups")
    {
        let examples = group
            .get("examples")
            .and_then(Json::as_array)
            .expect("examples");
        for example in examples {
            for key in [
                "session",
                "turn",
                "input",
                "expected_module",
                "actual_module",
                "expected_reply",
                "actual_reply",
            ] {
                assert!(example.get(key).is_some(), "example.{key}");
            }
        }
    }
    let faults = parsed
        .get("faults")
        .and_then(Json::as_object)
        .expect("faults");
    assert!(
        faults.is_empty(),
        "no fault on the whole corpus: {faults:?}"
    );
}

#[test]
fn the_json_report_round_trips_through_the_parser_and_the_writer() {
    let first = json::to_string(&report().to_json());
    let parsed = json::parse_str(&first).expect("parse");
    let second = json::to_string(&parsed);
    assert_eq!(
        first, second,
        "writer and parser agree on the same document"
    );
    // And the document is ASCII-safe? No — non-ASCII is written raw, like Python's
    // `ensure_ascii=False`, so the corpus' em dashes come through as UTF-8.
    assert!(first.contains('—') || !first.contains("\\u2014"));
}

#[test]
fn the_report_shrinks_to_a_single_turn_without_losing_its_shape() {
    let corpus = corpus();
    let one = GoldenReport::build(&golden::compare_corpus(&Corpus {
        turns: vec![corpus.turns.first().cloned().expect("non-empty")],
        manifest: None,
    }));
    assert_eq!(one.turns, 1);
    assert_eq!(one.overall.turns, 1);
    assert_eq!(one.blocks.len(), 1, "one block, not three empty ones");
    let text = one.render_text();
    assert!(text.contains("1 turns") || text.contains("1 turn"));
    let parsed = json::parse_str(&json::to_string(&one.to_json())).expect("valid");
    assert_eq!(parsed.get("turns").and_then(Json::as_i64), Some(1));
}

// ---------------------------------------------------------------------------- the binary

#[test]
fn the_differential_binary_runs_end_to_end_against_the_shipped_corpus() {
    // The same command CI runs: `differential tests/golden --json … --text …`. The helper
    // binary is built by cargo next to the test binary, so this exercises the real CLI.
    let directory = std::env::temp_dir().join(format!("pybot-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temp dir");
    let json_path = directory.join("report.json");
    let text_path = directory.join("report.txt");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_differential"))
        .arg(corpus_dir())
        .arg("--json")
        .arg(&json_path)
        .arg("--text")
        .arg(&text_path)
        .arg("--quiet")
        .output()
        .expect("run differential");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "--quiet prints nothing");
    let json_text = std::fs::read_to_string(&json_path).expect("report.json");
    let text = std::fs::read_to_string(&text_path).expect("report.txt");
    assert_eq!(
        json::parse_str(&json_text)
            .expect("valid JSON")
            .get("turns")
            .and_then(Json::as_i64),
        Some(3_062)
    );
    assert!(text.contains("differential report — 3062 turns"));
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn the_differential_binary_gate_passes_at_zero_and_fails_above_the_real_score() {
    let binary = env!("CARGO_BIN_EXE_differential");
    let run = |minimum: &str| {
        std::process::Command::new(binary)
            .arg(corpus_dir())
            .arg("--quiet")
            .arg("--min-module-percent")
            .arg(minimum)
            .status()
            .expect("run differential")
            .code()
    };
    assert_eq!(run("0"), Some(0), "a satisfied gate exits 0");
    assert_eq!(run("100"), Some(3), "an impossible gate exits 3");
}

#[test]
fn the_differential_binary_reports_a_bad_corpus_with_exit_two() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_differential"))
        .arg("this/directory/does/not/exist")
        .arg("--quiet")
        .output()
        .expect("run differential");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("differential:"),
        "the error is labelled"
    );
}

#[test]
fn the_harness_never_touches_the_clock_or_a_random_source() {
    // A cheap structural guard: the golden module and its binary must not mention `Instant`,
    // `SystemTime`, `rand` or a hash map iteration. (The crate rule is determinism by
    // construction; this catches a future accidental import.)
    let sources = [
        include_str!("../src/golden.rs"),
        include_str!("../src/bin/differential.rs"),
    ];
    for source in sources {
        for forbidden in [
            "Instant::now",
            "SystemTime::now",
            "HashMap",
            "thread_rng",
            "rand::",
        ] {
            assert!(
                !source.contains(forbidden),
                "the harness must stay deterministic: found {forbidden}"
            );
        }
    }
}
