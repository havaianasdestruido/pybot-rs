//! Strict, table-driven regression tests for the Phase 2 stdlib substitutes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use pybot::{rng::PythonRandom, unicode, util};

#[test]
fn python_string_split_table() {
    let cases = [
        ("", Vec::<&str>::new()),
        ("   ", vec![]),
        ("a b", vec!["a", "b"]),
        (" a\t\nb ", vec!["a", "b"]),
        ("a\u{1c}b\u{85}c\u{3000}d", vec!["a", "b", "c", "d"]),
    ];
    for (input, expected) in cases {
        assert_eq!(util::split_whitespace(input), expected, "{input:?}");
    }
}

#[test]
fn explicit_separator_split_obeys_maxsplit_and_empty_fields() {
    let cases = [
        ("", ",", -1, vec![""]),
        (",a,,b,", ",", -1, vec!["", "a", "", "b", ""]),
        ("a--b--c", "--", 1, vec!["a", "b--c"]),
        ("a--b--c", "--", 2, vec!["a", "b", "c"]),
        ("a--b--c", "--", 0, vec!["a--b--c"]),
        ("no delimiter", ",", 4, vec!["no delimiter"]),
        (
            "unicode🙂separator🙂x",
            "🙂",
            1,
            vec!["unicode", "separator🙂x"],
        ),
        ("a", "", -1, vec!["a"]),
    ];
    for (text, sep, n, expected) in cases {
        assert_eq!(util::split(text, sep, n), expected);
    }
}

#[test]
fn insertion_map_preserves_order_for_updates_and_reinsertions() {
    let mut map = util::OrderedMap::new();
    for n in 0..100 {
        map.insert(n, n * 10);
    }
    for n in (0..100).step_by(2) {
        assert_eq!(map.insert(n, n * 100), Some(n * 10));
    }
    assert_eq!(map.len(), 100);
    assert_eq!(
        map.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        (0..100).collect::<Vec<_>>()
    );
    for n in (0..100).step_by(3) {
        assert_eq!(
            map.remove(&n),
            Some(if n % 2 == 0 { n * 100 } else { n * 10 })
        );
    }
    for n in (0..100).step_by(3) {
        map.insert(n, n);
    }
    let keys = map.iter().map(|(key, _)| *key).collect::<Vec<_>>();
    assert_eq!(
        &keys[..keys.len() - 34],
        &(0..100).filter(|n| n % 3 != 0).collect::<Vec<_>>()
    );
    assert_eq!(
        &keys[keys.len() - 34..],
        &(0..100).step_by(3).collect::<Vec<_>>()
    );
}

#[test]
fn edit_distance_reference_cases_and_symmetry() {
    let examples = [
        ("", "", 0),
        ("", "abc", 3),
        ("book", "back", 2),
        ("kitten", "sitting", 3),
        ("Saturday", "Sunday", 3),
        ("café", "cafe", 1),
        ("🙂a", "🙂b", 1),
        ("aaaa", "aa", 2),
    ];
    for (a, b, expected) in examples {
        assert_eq!(util::edit_distance(a, b), expected);
        assert_eq!(util::edit_distance(b, a), expected);
        assert_eq!(util::edit_distance(a, a), 0);
    }
}

#[test]
fn sequence_matcher_ratio_known_python_fixtures_and_budget() {
    let examples = [
        ("", "", 1.0),
        ("a", "b", 0.0),
        ("abcd", "abXd", 0.75),
        ("tide", "diet", 0.25),
        ("abc", "abc", 1.0),
        ("abc", "", 0.0),
    ];
    for (a, b, expected) in examples {
        assert_eq!(util::sequence_ratio(a, b, 10_000), Some(expected));
    }
    assert_eq!(util::sequence_ratio("long string", "unrelated", 1), None);
    // Python's default SequenceMatcher autojunk threshold excludes an overrepresented token.
    assert_eq!(
        util::sequence_ratio(&"x".repeat(200), &"x".repeat(200), 50_000),
        Some(0.0)
    );
}

#[test]
fn strip_and_replace_match_python_character_boundaries() {
    assert_eq!(util::strip_chars("...name!!", Some(".!")), "name");
    assert_eq!(util::strip_chars("\u{a0} name \u{3000}", None), "name");
    assert_eq!(util::lstrip_chars("xxnamexx", Some("x")), "namexx");
    assert_eq!(util::rstrip_chars("xxnamexx", Some("x")), "xxname");
    assert_eq!(util::replace("abc", "", "-", 2), "-a-bc");
    assert_eq!(util::replace("a-a-a", "a", "x", 2), "x-x-a");
    assert_eq!(util::replace("a-a-a", "a", "x", -1), "x-x-x");
}

#[test]
fn field_formatting_handles_braces_and_missing_fields() {
    assert_eq!(
        util::format_fields("{}-{1}-{name}-{{ok}}", &["a", "b"], &[("name", "c")]),
        Ok("a-b-c-{ok}".into())
    );
    assert_eq!(
        util::format_fields("{0}", &[], &[]),
        Err(util::FormatError::MissingField)
    );
    assert_eq!(
        util::format_fields("x}", &[], &[]),
        Err(util::FormatError::UnmatchedBrace)
    );
}

#[test]
fn unicode_casefold_reference_matrix() {
    let pairs = [
        ("Straße", "strasse"),
        ("ẞ", "ss"),
        ("İ", "i\u{307}"),
        ("Σςσ", "σσσ"),
        ("K", "k"),
        ("ﬃ", "ffi"),
        ("ΜΆΙΟΣ", "μάιοσ"),
        ("Ａ", "ａ"),
        ("𐐀", "𐐨"),
    ];
    for (input, expected) in pairs {
        assert_eq!(unicode::casefold(input), expected, "{input:?}");
    }
}

#[test]
fn unicode_diacritic_folding_preserves_non_latin_and_removes_marks() {
    let cases = [
        ("Crème brûlée", "creme brulee"),
        ("Æther", "aether"),
        ("Łódź", "łodz"),
        ("Cafe\u{301}", "cafe"),
        ("İ ① Ａ ﬃ", "i 1 a ffi"),
        ("東京", "東京"),
        ("नमस्ते", "नमसते"),
    ];
    for (input, expected) in cases {
        assert_eq!(unicode::fold_diacritics(input), expected);
    }
}

#[test]
fn unicode_regex_character_classes_do_not_conflate_categories() {
    for ch in ['a', 'é', '٣', '𝟡', '_'] {
        assert!(unicode::is_word(ch), "{ch:?}");
    }
    for ch in ['-', '\u{301}', ' ', '²', 'Ⅷ'] {
        assert!(!unicode::is_word(ch), "{ch:?}");
    }
    for ch in ['0', '٣', '𝟡', '१', '０'] {
        assert!(unicode::is_decimal_digit(ch), "{ch:?}");
    }
    for ch in ['²', 'Ⅷ', 'a', '_'] {
        assert!(!unicode::is_decimal_digit(ch), "{ch:?}");
    }
}

#[test]
fn cpython_random_seeded_random_stream_fixtures() {
    let fixtures: &[(i128, &[f64])] = &[
        (
            0,
            &[0.8444218515250481, 0.7579544029403025, 0.420571580830845],
        ),
        (
            1,
            &[0.13436424411240122, 0.8474337369372327, 0.763774618976614],
        ),
        (
            42,
            &[
                0.6394267984578837,
                0.025010755222666936,
                0.27502931836911926,
            ],
        ),
        (
            2024,
            &[0.47009071843107064, 0.7282642914232076, 0.3037513583913575],
        ),
        (-17, &[0.5219839097124932, 0.8066907771186791]),
    ];
    for (seed, expected) in fixtures {
        let mut rng = PythonRandom::new(*seed);
        for (index, expected_value) in expected.iter().enumerate() {
            assert_eq!(rng.random(), *expected_value, "seed {seed}, draw {index}");
        }
    }
}

#[test]
fn cpython_getrandbits_choice_and_shuffle_fixtures() {
    let mut rng = PythonRandom::new(42);
    assert_eq!(rng.getrandbits(0), Some(0));
    assert_eq!(rng.getrandbits(5), Some(20));
    assert_eq!(rng.getrandbits(32), Some(478_163_327));
    assert_eq!(rng.getrandbits(129), None);
    let mut rng = PythonRandom::new(42);
    let choices = (0..10).collect::<Vec<_>>();
    assert_eq!(rng.choice(&choices), Some(&1));
    let mut shuffled = (0..10).collect::<Vec<_>>();
    PythonRandom::new(42).shuffle(&mut shuffled);
    assert_eq!(shuffled, [7, 3, 2, 8, 5, 6, 9, 4, 0, 1]);
}

#[test]
fn rng_is_reproducible_bounded_and_shuffle_is_permutation_for_many_seeds() {
    for seed in 0..256_i128 {
        let mut left = PythonRandom::new(seed);
        let mut right = PythonRandom::new(seed);
        let mut numbers = Vec::new();
        for _ in 0..128 {
            let value = left.randbelow(17).expect("positive bound");
            assert!(value < 17);
            assert_eq!(value, right.randbelow(17).expect("positive bound"));
            numbers.push(value);
        }
        assert_eq!(numbers.len(), 128);
        let mut values = (0..64).collect::<Vec<_>>();
        PythonRandom::new(seed).shuffle(&mut values);
        values.sort_unstable();
        assert_eq!(values, (0..64).collect::<Vec<_>>());
    }
}

#[test]
fn cpython_rng_full_seed_corpus_differential() {
    let corpus = include_str!("fixtures/phase2_rng.tsv");
    let mut records = 0usize;
    for line in corpus.lines().filter(|line| !line.starts_with('#')) {
        let mut columns = line.split('\t');
        let seed = columns
            .next()
            .expect("seed column")
            .parse::<i128>()
            .expect("integer seed");
        let expected_floats = columns
            .next()
            .expect("float column")
            .split(',')
            .map(|v| v.parse::<f64>().expect("valid float"));
        let mut rng = PythonRandom::new(seed);
        for expected in expected_floats {
            assert_eq!(rng.random(), expected, "random seed {seed}");
        }
        let expected_bits = columns
            .next()
            .expect("bits column")
            .split(',')
            .map(|v| v.parse::<u128>().expect("valid bits"));
        let mut rng = PythonRandom::new(seed);
        for (width, expected) in [1, 5, 32, 33, 64, 127].into_iter().zip(expected_bits) {
            assert_eq!(
                rng.getrandbits(width),
                Some(expected),
                "getrandbits({width}) seed {seed}"
            );
        }
        let expected_choices = columns
            .next()
            .expect("choice column")
            .split(',')
            .map(|v| v.parse::<u32>().expect("valid choice"));
        let choice_values = (0..101_u32).collect::<Vec<_>>();
        let mut rng = PythonRandom::new(seed);
        for expected in expected_choices {
            assert_eq!(
                rng.choice(&choice_values),
                Some(&expected),
                "choice seed {seed}"
            );
        }
        let expected_shuffle = columns
            .next()
            .expect("shuffle column")
            .split(',')
            .map(|v| v.parse::<u32>().expect("valid shuffle"));
        let mut values = (0..24_u32).collect::<Vec<_>>();
        PythonRandom::new(seed).shuffle(&mut values);
        assert_eq!(
            values,
            expected_shuffle.collect::<Vec<_>>(),
            "shuffle seed {seed}"
        );
        let expected_ranges = columns
            .next()
            .expect("randrange column")
            .split(',')
            .map(|v| v.parse::<i64>().expect("valid range value"));
        let mut rng = PythonRandom::new(seed);
        for expected in expected_ranges {
            assert_eq!(
                rng.randrange_step(-50, 100, 7),
                Some(expected),
                "randrange seed {seed}"
            );
        }
        let expected_pool = columns
            .next()
            .expect("small-pool sample column")
            .split(',')
            .map(|v| v.parse::<u32>().expect("valid sample"));
        assert_eq!(
            PythonRandom::new(seed).sample(&(0..20_u32).collect::<Vec<_>>(), 5),
            Some(expected_pool.collect())
        );
        let expected_small = columns
            .next()
            .expect("small sample column")
            .split(',')
            .map(|v| v.parse::<u32>().expect("valid sample"));
        assert_eq!(
            PythonRandom::new(seed).sample(&(0..30_u32).collect::<Vec<_>>(), 5),
            Some(expected_small.collect())
        );
        let expected_large = columns
            .next()
            .expect("large sample column")
            .split(',')
            .map(|v| v.parse::<u32>().expect("valid sample"));
        assert_eq!(
            PythonRandom::new(seed).sample(&(0..1_000_u32).collect::<Vec<_>>(), 7),
            Some(expected_large.collect())
        );
        assert!(
            columns.next().is_none(),
            "unexpected extra field for seed {seed}"
        );
        records += 1;
    }
    assert_eq!(
        records, 260,
        "fixture corpus was truncated or expanded without updating this guard"
    );
}

#[test]
fn difflib_ratio_exhaustive_short_string_differential() {
    let corpus = include_str!("fixtures/phase2_difflib.tsv");
    let mut records = 0usize;
    for line in corpus.lines().filter(|line| !line.starts_with('#')) {
        let mut columns = line.split('\t');
        let left = columns.next().expect("left field");
        let right = columns.next().expect("right field");
        let expected = columns
            .next()
            .expect("ratio field")
            .parse::<f64>()
            .expect("valid ratio");
        assert!(columns.next().is_none());
        assert_eq!(
            util::sequence_ratio(left, right, 10_000),
            Some(expected),
            "left={left:?}, right={right:?}"
        );
        records += 1;
    }
    assert_eq!(records, 3_969);
}

#[test]
fn python_alnum_distinguishes_numbers_from_punctuation() {
    for text in ["café٣", "漢字", "Ⅷ", "𝟡"] {
        assert!(util::is_alnum(text), "{text:?}");
    }
    for text in ["", "a_b", "a-b", "é "] {
        assert!(!util::is_alnum(text), "{text:?}");
    }
}
