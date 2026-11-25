//! `differential` — replay the golden corpus on the Rust bot and report agreement.
//!
//! ```text
//! cargo run --release --bin differential -- tests/golden
//! cargo run --release --bin differential -- tests/golden --json report.json --text report.txt
//! cargo run --release --bin differential -- tests/golden --quiet
//! ```
//!
//! Exit codes: `0` when the harness ran and produced a report (Fase 1: any agreement level is
//! acceptable — GOAL.md makes the harness the deliverable), `2` when the corpus cannot be read
//! or parsed, `1` for output errors. `--min-module-percent N` turns the tool into a CI gate
//! for later phases (the caller decides the threshold; nothing is hard-coded).

// A command-line tool reports on stdout/stderr by design: the crate-wide
// `print_stdout`/`print_stderr` warns exist so the *library* never prints, and this binary is
// the one place allowed to. The library binary (`src/main.rs`) goes through `Streams` instead.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::Write;

use pybot::error::{Error, Result};
use pybot::golden::{self, GoldenReport};

/// Exit codes of the differential runner.
mod exit {
    /// The report was produced.
    pub const OK: u8 = 0;
    /// Writing the report failed.
    pub const FAILURE: u8 = 1;
    /// The corpus could not be loaded.
    pub const BAD_CORPUS: u8 = 2;
    /// An optional gate (`--min-module-percent`) was not met.
    pub const GATE: u8 = 3;
}

#[derive(Debug)]
struct Options {
    directory: std::path::PathBuf,
    json: Option<std::path::PathBuf>,
    text: Option<std::path::PathBuf>,
    quiet: bool,
    min_module_percent: Option<f64>,
    min_reply_percent: Option<f64>,
}

fn parse_options(args: &[String]) -> std::result::Result<Options, String> {
    let mut directory: Option<std::path::PathBuf> = None;
    let mut options = Options {
        directory: std::path::PathBuf::from("tests/golden"),
        json: None,
        text: None,
        quiet: false,
        min_module_percent: None,
        min_reply_percent: None,
    };
    let mut index = 0;
    while index < args.len() {
        let argument = args.get(index).map(String::as_str).unwrap_or("");
        let mut value = |name: &str| -> std::result::Result<String, String> {
            index += 1;
            match args.get(index) {
                // An option-looking token is not an operand: report the missing value instead
                // of silently consuming the next flag. A path spelled `./--text` and a negative
                // number (a legal gate) still work, because neither can be a flag here.
                Some(candidate)
                    if candidate.starts_with('-')
                        && candidate != "-"
                        && candidate.parse::<f64>().is_err() =>
                {
                    Err(format!("{name} expects a value"))
                }
                Some(candidate) => Ok(candidate.clone()),
                None => Err(format!("{name} expects a value")),
            }
        };
        match argument {
            "-h" | "--help" => return Err(String::from("help")),
            "--quiet" | "-q" => options.quiet = true,
            "--json" => options.json = Some(std::path::PathBuf::from(value("--json")?)),
            "--text" => options.text = Some(std::path::PathBuf::from(value("--text")?)),
            "--min-module-percent" => {
                options.min_module_percent = Some(threshold(
                    "--min-module-percent",
                    &value("--min-module-percent")?,
                )?);
            }
            "--min-reply-percent" => {
                options.min_reply_percent = Some(threshold(
                    "--min-reply-percent",
                    &value("--min-reply-percent")?,
                )?);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option: {other}"));
            }
            other => directory = Some(std::path::PathBuf::from(other)),
        }
        index += 1;
    }
    if let Some(directory) = directory {
        options.directory = directory;
    }
    Ok(options)
}

/// Parse a `--min-*-percent` gate.
///
/// `f64`'s `FromStr` accepts `inf`, `NaN` and `+Infinity`; a gate of `NaN` compares false
/// against everything and would silently disable itself, so non-finite values are a usage
/// error. Any *finite* value is accepted, including values outside 0–100 (a gate above 100 is
/// simply unreachable, and the caller may rely on that to assert "never").
fn threshold(name: &str, raw: &str) -> std::result::Result<f64, String> {
    let value: f64 = raw
        .parse()
        .map_err(|_| format!("{name} expects a number"))?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{name} expects a finite number"))
    }
}

fn usage() -> String {
    String::from(
        "usage: differential [CORPUS_DIR] [--json FILE] [--text FILE] [--quiet]\n\
         \x20      [--min-module-percent N] [--min-reply-percent N]\n\
         \n\
         Replays tests/golden/corpus.jsonl (2 850 sessions, 3 062 turns captured from the\n\
         Python bot) on the Rust bot and reports agreement by block and by module.\n\
         Exit 0 when a report was produced; 3 when a --min-* gate is not met.\n",
    )
}

fn main_impl(args: &[String]) -> Result<u8> {
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(message) if message == "help" => {
            print!("{}", usage());
            return Ok(exit::OK);
        }
        Err(message) => {
            eprintln!("differential: {message}");
            eprint!("{}", usage());
            return Ok(exit::BAD_CORPUS);
        }
    };

    let corpus = match golden::load_dir(&options.directory) {
        Ok(corpus) => corpus,
        Err(error) => {
            eprintln!("differential: {error}");
            return Ok(exit::BAD_CORPUS);
        }
    };

    // Writing both reports to one path would mean whichever lands last silently wins.
    if let (Some(text), Some(json)) = (&options.text, &options.json) {
        if text == json {
            eprintln!(
                "differential: --text and --json point at the same file ({})",
                text.display()
            );
            return Ok(exit::BAD_CORPUS);
        }
    }

    let report = GoldenReport::build(&golden::compare_corpus(&corpus));
    let text = report.render_text();
    if !options.quiet {
        // Fallible, so a closed stdout reaches the `exit::FAILURE` handler instead of
        // panicking inside `print!`. `--quiet` stays independent of stdout entirely.
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(text.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|error| Error::io("write report", error))?;
    }
    if let Some(path) = &options.text {
        write_file(path, text.as_bytes())?;
    }
    if let Some(path) = &options.json {
        let encoded = pybot::json::to_string(&report.to_json())?;
        write_file(path, encoded.as_bytes())?;
    }

    let mut gate_failed = false;
    if let Some(minimum) = options.min_module_percent {
        if report.overall.module_percent() < minimum {
            eprintln!(
                "differential: module agreement {:.2}% is below the gate {:.2}%",
                report.overall.module_percent(),
                minimum
            );
            gate_failed = true;
        }
    }
    if let Some(minimum) = options.min_reply_percent {
        if report.overall.reply_percent() < minimum {
            eprintln!(
                "differential: reply agreement {:.2}% is below the gate {:.2}%",
                report.overall.reply_percent(),
                minimum
            );
            gate_failed = true;
        }
    }
    Ok(if gate_failed { exit::GATE } else { exit::OK })
}

fn write_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let mut file =
        std::fs::File::create(path).map_err(|error| Error::io("create report", error))?;
    file.write_all(bytes)
        .map_err(|error| Error::io("write report", error))
}

fn main() -> std::process::ExitCode {
    // OS strings, so a non-UTF-8 path is a path and not a decode error. Only text operands
    // (flags, thresholds) are decoded; a non-UTF-8 one is a usage error.
    let mut args: Vec<String> = Vec::new();
    for argument in std::env::args_os().skip(1) {
        match argument.into_string() {
            Ok(text) => args.push(text),
            Err(raw) => {
                eprintln!(
                    "differential: argument is not valid UTF-8: {}",
                    raw.to_string_lossy()
                );
                return std::process::ExitCode::from(exit::BAD_CORPUS);
            }
        }
    }
    match main_impl(&args) {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            eprintln!("differential: {error}");
            std::process::ExitCode::from(exit::FAILURE)
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

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn the_default_directory_is_the_shipped_corpus() {
        let options = parse_options(&[]).unwrap();
        assert_eq!(options.directory, std::path::PathBuf::from("tests/golden"));
        assert!(options.json.is_none());
        assert!(options.text.is_none());
        assert!(!options.quiet);
        assert!(options.min_module_percent.is_none());
    }

    #[test]
    fn options_parse_in_any_order() {
        let options = parse_options(&args(&[
            "--json",
            "a.json",
            "some/dir",
            "--quiet",
            "--text",
            "b.txt",
            "--min-module-percent",
            "12.5",
            "--min-reply-percent",
            "3",
        ]))
        .unwrap();
        assert_eq!(options.directory, std::path::PathBuf::from("some/dir"));
        assert_eq!(options.json, Some(std::path::PathBuf::from("a.json")));
        assert_eq!(options.text, Some(std::path::PathBuf::from("b.txt")));
        assert!(options.quiet);
        assert_eq!(options.min_module_percent, Some(12.5));
        assert_eq!(options.min_reply_percent, Some(3.0));
    }

    #[test]
    fn bad_arguments_are_rejected_with_a_message() {
        assert_eq!(
            parse_options(&args(&["--json"])).unwrap_err(),
            "--json expects a value"
        );
        assert_eq!(
            parse_options(&args(&["--min-module-percent", "abc"])).unwrap_err(),
            "--min-module-percent expects a number"
        );
        assert_eq!(
            parse_options(&args(&["--nope"])).unwrap_err(),
            "unknown option: --nope"
        );
        assert_eq!(parse_options(&args(&["--help"])).unwrap_err(), "help");
    }

    #[test]
    fn the_usage_text_mentions_the_corpus_and_the_gate() {
        let usage = usage();
        assert!(usage.contains("differential [CORPUS_DIR]"));
        assert!(usage.contains("2 850 sessions"));
        assert!(usage.contains("--min-module-percent"));
    }

    #[test]
    fn a_missing_corpus_is_reported_as_exit_two() {
        let code = main_impl(&args(&["does/not/exist", "--quiet"])).unwrap();
        assert_eq!(code, exit::BAD_CORPUS);
        let code = main_impl(&args(&["--nope"])).unwrap();
        assert_eq!(code, exit::BAD_CORPUS);
    }

    #[test]
    fn the_shipped_corpus_produces_a_report_and_writes_both_formats() {
        let directory =
            std::env::temp_dir().join(format!("pybot-differential-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let json_path = directory.join("report.json");
        let text_path = directory.join("report.txt");
        let code = main_impl(&args(&[
            &format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR")),
            "--quiet",
            "--json",
            json_path.to_str().unwrap(),
            "--text",
            text_path.to_str().unwrap(),
        ]))
        .unwrap();
        assert_eq!(code, exit::OK);
        let json_text = std::fs::read_to_string(&json_path).unwrap();
        let text = std::fs::read_to_string(&text_path).unwrap();
        assert!(json_text.starts_with('{'));
        assert!(text.contains("differential report"));
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_gate_that_cannot_be_met_exits_three() {
        let code = main_impl(&args(&[
            &format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR")),
            "--quiet",
            "--min-module-percent",
            "101",
        ]))
        .unwrap();
        assert_eq!(code, exit::GATE);
        let code = main_impl(&args(&[
            &format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR")),
            "--quiet",
            "--min-module-percent",
            "0",
        ]))
        .unwrap();
        assert_eq!(code, exit::OK);
    }

    #[test]
    fn an_option_token_is_not_eaten_as_a_value() {
        for flag in [
            "--json",
            "--text",
            "--min-module-percent",
            "--min-reply-percent",
        ] {
            assert_eq!(
                parse_options(&args(&[flag, "--quiet"])).unwrap_err(),
                format!("{flag} expects a value"),
                "{flag}"
            );
        }
        // A path that merely starts with a dash is still a value when spelled unambiguously.
        let options = parse_options(&args(&["--text", "./--quiet", "--quiet"])).unwrap();
        assert_eq!(options.text, Some(std::path::PathBuf::from("./--quiet")));
        assert!(options.quiet);
    }

    #[test]
    fn thresholds_reject_non_finite_values() {
        assert_eq!(
            parse_options(&args(&["--min-reply-percent", "inf"])).unwrap_err(),
            "--min-reply-percent expects a finite number"
        );
        assert_eq!(
            parse_options(&args(&["--min-module-percent", "NaN"])).unwrap_err(),
            "--min-module-percent expects a finite number"
        );
        // Finite values outside 0–100 stay valid, and so do ordinary ones.
        assert_eq!(
            parse_options(&args(&["--min-reply-percent", "-5"]))
                .unwrap()
                .min_reply_percent,
            Some(-5.0)
        );
        assert_eq!(
            parse_options(&args(&["--min-reply-percent", "101"]))
                .unwrap()
                .min_reply_percent,
            Some(101.0)
        );
    }

    #[test]
    fn one_destination_for_both_reports_is_refused_before_anything_is_written() {
        let directory =
            std::env::temp_dir().join(format!("pybot-differential-same-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("both.report");
        let code = main_impl(&args(&[
            &format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR")),
            "--quiet",
            "--text",
            path.to_str().unwrap(),
            "--json",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert_eq!(code, exit::BAD_CORPUS);
        assert!(
            !path.exists(),
            "neither artifact may be written when the destinations collide"
        );
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn distinct_destinations_still_write_both_reports() {
        let directory =
            std::env::temp_dir().join(format!("pybot-differential-both-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let text_path = directory.join("report.txt");
        let json_path = directory.join("report.json");
        let code = main_impl(&args(&[
            &format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR")),
            "--quiet",
            "--text",
            text_path.to_str().unwrap(),
            "--json",
            json_path.to_str().unwrap(),
        ]))
        .unwrap();
        assert_eq!(code, exit::OK);
        assert!(text_path.exists() && json_path.exists());
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn help_prints_and_exits_zero() {
        assert_eq!(main_impl(&args(&["--help"])).unwrap(), exit::OK);
    }
}
