//! The crate's single error type.
//!
//! GOAL.md: "`Error` (sem `Box<dyn Error>` solto)". Everything that can fail returns
//! [`Result<T>`], and every failure mode is a variant of [`Error`] — never a boxed trait
//! object and never a panic. The enum is `#[non_exhaustive]` so later phases can add
//! variants without breaking callers.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// The crate's result type: [`Error`] as the only error.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything in this crate that can go wrong.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// An I/O operation failed. `operation` names what we were doing ("read corpus",
    /// "write report", ...) so the message is useful without a backtrace.
    Io {
        /// Human-readable description of the operation.
        operation: &'static str,
        /// The underlying OS error.
        source: io::Error,
    },
    /// A path could not be turned into UTF-8, so it cannot be reported or stored.
    PathEncoding {
        /// The offending path, for diagnostics only.
        path: PathBuf,
    },
    /// File contents are not valid UTF-8. `valid_up_to` is the byte offset of the first
    /// invalid byte (from [`std::string::FromUtf8Error`] semantics).
    InvalidUtf8 {
        /// Byte offset of the first invalid sequence.
        valid_up_to: usize,
    },
    /// The JSON document is malformed. `offset` is a byte offset into the input.
    Json {
        /// Byte offset where the parser gave up.
        offset: usize,
        /// What was wrong (a static message: no allocation on the error path).
        message: &'static str,
    },
    /// The golden corpus violates its declared schema.
    Corpus {
        /// 1-based line number in `corpus.jsonl`.
        line: usize,
        /// What was wrong.
        message: &'static str,
    },
    /// The corpus manifest is missing or inconsistent.
    Manifest {
        /// What was wrong.
        message: &'static str,
    },
    /// A registry definition is invalid (only reachable from a programming mistake in the
    /// static module table, never from user input).
    Registry {
        /// What was wrong.
        message: &'static str,
    },
    /// A numeric limit would be exceeded by the operation (`what` names the limit).
    Limit {
        /// Human-readable description of the exceeded limit.
        what: &'static str,
    },
}

impl Error {
    /// Wrap an [`io::Error`] with the name of the operation that failed.
    pub fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }

    /// Build a JSON syntax error at `offset`.
    pub fn json(offset: usize, message: &'static str) -> Self {
        Self::Json { offset, message }
    }

    /// True when the error is the benign `EPIPE` (output consumer went away, e.g.
    /// `pybot | head`). The CLI exits successfully in that case, like Python's `BrokenPipeError`
    /// handling in well-behaved tools.
    #[must_use]
    pub fn is_broken_pipe(&self) -> bool {
        matches!(self, Self::Io { source, .. } if source.kind() == io::ErrorKind::BrokenPipe)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "{operation}: {source}"),
            Self::PathEncoding { path } => {
                write!(f, "path is not valid UTF-8: {}", path.display())
            }
            Self::InvalidUtf8 { valid_up_to } => {
                write!(
                    f,
                    "input is not valid UTF-8 (first invalid byte: offset {valid_up_to})"
                )
            }
            Self::Json { offset, message } => write!(f, "invalid JSON at byte {offset}: {message}"),
            Self::Corpus { line, message } => {
                write!(f, "corpus line {line}: {message}")
            }
            Self::Manifest { message } => write!(f, "corpus manifest: {message}"),
            Self::Registry { message } => write!(f, "module registry: {message}"),
            Self::Limit { what } => write!(f, "limit exceeded: {what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self::Io {
            operation: "I/O",
            source,
        }
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

    fn all_variants() -> Vec<Error> {
        vec![
            Error::io(
                "read corpus",
                io::Error::new(io::ErrorKind::NotFound, "nope"),
            ),
            Error::PathEncoding {
                path: PathBuf::from("x"),
            },
            Error::InvalidUtf8 { valid_up_to: 7 },
            Error::json(42, "expected value"),
            Error::Corpus {
                line: 3,
                message: "missing field",
            },
            Error::Manifest {
                message: "counts disagree",
            },
            Error::Registry {
                message: "duplicate module",
            },
            Error::Limit {
                what: "input chars",
            },
        ]
    }

    #[test]
    fn display_mentions_the_relevant_context_for_every_variant() {
        let expectations = [
            "read corpus",
            "not valid UTF-8",
            "offset 7",
            "byte 42",
            "line 3",
            "manifest",
            "registry",
            "input chars",
        ];
        let rendered: Vec<String> = all_variants().iter().map(ToString::to_string).collect();
        for (text, expected) in rendered.iter().zip(expectations) {
            assert!(
                text.contains(expected),
                "{text:?} must mention {expected:?}"
            );
        }
        assert_eq!(rendered.len(), expectations.len());
    }

    #[test]
    fn display_is_stable_and_never_empty() {
        for error in all_variants() {
            let first = error.to_string();
            let second = error.to_string();
            assert_eq!(first, second);
            assert!(!first.is_empty());
        }
    }

    #[test]
    fn only_io_errors_have_a_source() {
        for error in all_variants() {
            let is_io = matches!(error, Error::Io { .. });
            assert_eq!(std::error::Error::source(&error).is_some(), is_io);
        }
    }

    #[test]
    fn io_errors_are_preserved_and_broken_pipe_is_recognised() {
        let error = Error::from(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
        assert!(error.is_broken_pipe());
        let other = Error::io(
            "write",
            io::Error::new(io::ErrorKind::PermissionDenied, "no"),
        );
        assert!(!other.is_broken_pipe());
        assert!(other.to_string().starts_with("write: "));
    }

    #[test]
    fn error_is_a_std_error_so_callers_can_print_a_chain() {
        fn assert_std_error<T: std::error::Error + Send + Sync + 'static>() {}
        assert_std_error::<Error>();
    }

    #[test]
    fn json_and_corpus_helpers_keep_their_fields() {
        match Error::json(9, "boom") {
            Error::Json { offset, message } => {
                assert_eq!(offset, 9);
                assert_eq!(message, "boom");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
        match (Error::Corpus {
            line: 99,
            message: "m",
        }) {
            Error::Corpus { line, message } => {
                assert_eq!(line, 99);
                assert_eq!(message, "m");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
