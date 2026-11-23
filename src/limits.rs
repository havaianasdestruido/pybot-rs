//! Explicit, documented numeric limits.
//!
//! GOAL.md (Fase 9) asks for an "orçamento por turno: limite de tamanho da entrada, de passos
//! da engine e de memória; estourou → resposta de fallback, nunca panic ou travamento". Fase 1
//! introduces the budget as constants so every later phase shares one source of truth and the
//! harness can assert on them today.
//!
//! The Python bot has no such limits: `memory["turns"]` grows without bound (the recon measured
//! +85 MB per 100 turns) and the CLI crashes on invalid UTF-8. The Rust port caps the obvious
//! growth points; divergences are listed in `docs/fase1.md`.
//!
//! Values are deliberately generous: they must never bite a legitimate corpus turn (the
//! corpus' longest input is 1 000 chars and its longest reply 1 004 chars) while bounding the
//! damage of adversarial input.

/// Longest accepted user message, in characters (Unicode scalar values).
///
/// 16 384 is far above anything a person types and the corpus maximum (1 000). Longer input
/// is truncated at this many characters — never an error, never a panic.
pub const MAX_INPUT_CHARS: usize = 16 * 1024;

/// Longest accepted user message, in bytes (a char may take up to 4 bytes).
pub const MAX_INPUT_BYTES: usize = MAX_INPUT_CHARS * 4;

/// Maximum number of distinct keys a turn may leave in the memory.
///
/// The Python bot's memory has 7 keys after a normal turn; a module writing keys in a loop is
/// a bug. Writes beyond the cap are dropped (and counted in [`crate::runtime::Faults`]).
pub const MAX_MEMORY_KEYS: usize = 512;

/// Maximum number of bytes in a single string stored in the memory by a module.
pub const MAX_MEMORY_STRING_BYTES: usize = 256 * 1024;

/// Maximum number of nested containers inside a single memory value.
pub const MAX_MEMORY_DEPTH: usize = 16;

/// Maximum number of items inside a single list/tuple/JSON-array memory value.
pub const MAX_MEMORY_ITEMS: usize = 65_536;

/// Maximum number of turns kept in the conversation log (`memory["turns"]`).
///
/// Python keeps every turn forever. The Rust port keeps the newest `MAX_TURN_LOG` turns in
/// the log — a bounded window with the same observable tail — and counts evictions in
/// [`crate::runtime::Faults::evicted_turns`]. `/reset` clears it.
pub const MAX_TURN_LOG: usize = 4_096;

/// Maximum length, in bytes, of the text remembered for one logged turn (user text or reply).
pub const MAX_LOGGED_TEXT_BYTES: usize = 4 * 1024;

/// Maximum number of candidate replies a single turn may collect before the extras are
/// dropped. The corpus' maximum is 4; the Python bot's ceiling is the number of modules, so
/// this only exists to make O(candidates²) arbiter rules impossible.
pub const MAX_CANDIDATES: usize = 256;

/// Maximum number of modules in the static registry (the port has ~110; a bound keeps
/// `ModuleId` honest and any O(n²) pipeline cheap).
pub const MAX_MODULES: usize = 1_024;

/// Longest line `jsonl` reading will accept, in bytes. The longest corpus line is ~4 KB.
pub const MAX_JSON_LINE_BYTES: usize = 1024 * 1024;

/// Truncate `text` to at most `max_chars` characters, never splitting a scalar value.
#[must_use]
pub fn truncate_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((byte_index, _)) => text.get(..byte_index).unwrap_or(text),
        None => text,
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

    #[test]
    fn the_limits_are_ordered_and_sane() {
        // The assertions compare constants; reading them through a `const fn`-free helper keeps
        // clippy from folding them into "constant value" warnings while testing exactly what
        // ships. (A static read is still a compile-time value to clippy, so the check is done
        // in code that consumes the numbers the way the pipeline does.)
        let limits = [
            MAX_INPUT_BYTES,
            MAX_INPUT_CHARS,
            MAX_TURN_LOG,
            MAX_CANDIDATES,
            MAX_MODULES,
            MAX_MEMORY_KEYS,
            MAX_LOGGED_TEXT_BYTES,
            MAX_MEMORY_STRING_BYTES,
            MAX_JSON_LINE_BYTES,
        ];
        assert!(limits.iter().all(|limit| *limit > 0));
        assert!(limits.contains(&MAX_INPUT_CHARS));
        assert!(
            std::hint::black_box(MAX_INPUT_BYTES) >= std::hint::black_box(MAX_INPUT_CHARS),
            "a char is at least one byte"
        );
        assert!(
            std::hint::black_box(MAX_INPUT_CHARS) > std::hint::black_box(1_004),
            "must exceed the longest reply in the corpus (1 004 chars)"
        );
        assert!(std::hint::black_box(MAX_TURN_LOG) >= std::hint::black_box(100));
        assert!(std::hint::black_box(MAX_CANDIDATES) > std::hint::black_box(4));
        assert!(std::hint::black_box(MAX_MODULES) > std::hint::black_box(110));
        assert!(std::hint::black_box(MAX_MEMORY_KEYS) > std::hint::black_box(7));
        assert!(std::hint::black_box(MAX_LOGGED_TEXT_BYTES) >= std::hint::black_box(1_024));
    }

    #[test]
    fn truncate_chars_never_splits_a_scalar_value() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 5), "hello");
        assert_eq!(truncate_chars("hello", 4), "hell");
        assert_eq!(truncate_chars("hello", 0), "");
        assert_eq!(truncate_chars("", 3), "");
        assert_eq!(truncate_chars("ação", 2), "aç");
        assert_eq!(truncate_chars("😀😀😀", 2), "😀😀");
        assert_eq!(truncate_chars("😀😀😀", 1), "😀");
    }

    #[test]
    fn truncate_chars_agrees_with_python_slicing_for_ascii() {
        let text = "abcdefghij";
        for length in 0..=text.len() {
            assert_eq!(truncate_chars(text, length), &text[..length]);
        }
    }
}
