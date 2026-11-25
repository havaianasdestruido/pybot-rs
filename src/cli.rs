//! The command line: `pybot.py`'s interface, ported.
//!
//! ```text
//! pybot                      interactive chat when stdin is a terminal
//! pybot -p "your message"    one reply, then exit
//! echo "a line" | pybot      one reply per input line
//! ```
//!
//! The Python CLI has four code paths (`chat`, `one_shot`, `pipe`, `main`) plus the opening
//! banner, `/reset`, `/exit` and `/help`, `-v` verbose output and `-s` seeding. The Rust port
//! keeps the same surface with two documented divergences (both required by GOAL.md stability
//! rules — see `docs/fase1.md`):
//!
//! * **invalid UTF-8 on stdin** — Python's text-mode stdin raises `UnicodeDecodeError` and
//!   the process dies mid-conversation; the Rust port folds the bytes lossily (U+FFFD), keeps
//!   answering, and with `-v` says that it did.
//! * **broken pipe** — `pybot | head` makes Python print a `BrokenPipeError` traceback; the
//!   Rust port stops quietly and exits 0, the Unix convention.
//!
//! Everything here is pure I/O plumbing: no chatbot logic, no panics, and every path is
//! exercised by `tests/cli.rs` through the public [`run`] function.

use std::ffi::OsString;
use std::io::{BufRead, IsTerminal, Write};

use crate::error::{Error, Result};
use crate::limits;
use crate::pyutil;
use crate::runtime::Runtime;
use crate::session::Session;
use crate::types::{Reply, Turn};

/// The `FIGlet` "slant" logo, exactly as `pybot.py` prints it (no trailing newline).
pub const LOGO: &str = r"    ____        ____        __
   / __ \__  __/ __ )____  / /_
  / /_/ / / / / __  / __ \/ __/
 / ____/ /_/ / /_/ / /_/ / /_
/_/    \__, /_____/\____/\__/
      /____/";

/// What `PyBot` is, under the logo.
pub const TAGLINE: &str = "A chatbot written in plain Python. No weights, no training, just rules.";

/// The one-line hint under the tagline.
pub const HINT: &str =
    "Type a message and press Enter. /reset starts a new conversation, /exit quits.";

/// The `/help` text (`pybot.py::HELP`).
pub const HELP: &str = "Commands:
  /reset   clear the screen and start again (PyBot forgets what was said)
  /help    show this help
  /exit    quit (Ctrl+D works too)";

/// Prefix of the user's turns in the interactive chat.
pub const USER_PREFIX: &str = "user: ";

/// Prefix of the bot's replies in the interactive chat.
pub const BOT_PREFIX: &str = "pybot: ";

/// The terminal escape that clears screen and scrollback (`pybot.py::clear_screen`).
pub const CLEAR_SCREEN: &str = "\u{1b}[H\u{1b}[2J\u{1b}[3J";

/// The program's version, as `--version` reports it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Exit codes. `pybot.py` only uses 0 (success) and argparse's 2 (usage error).
pub mod exit {
    /// The command ran.
    pub const OK: u8 = 0;
    /// Something failed (I/O error that is not a broken pipe).
    pub const FAILURE: u8 = 1;
    /// The command line was malformed (argparse's code).
    pub const USAGE: u8 = 2;
}

/// Parsed command-line options (`argparse`'s four flags, in the same shapes).
///
/// Five booleans is what `argparse` offers (`-i`, `-v`, `--version`, `-h`, plus "was `-p`
/// given"), so the shape mirrors the reference rather than an invented grouping.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `-p`/`--prompt`: reply to this one message.
    pub prompt: Option<String>,
    /// `-i`/`--interactive`: with `-p`, keep chatting after the first reply.
    pub interactive: bool,
    /// `-s`/`--seed`: accepted for interface compatibility; the port is deterministic and the
    /// Python RNG (Fase 2.4) does not exist yet, so the value is recorded and unused.
    pub seed: Option<i64>,
    /// `-v`/`--verbose`: show the module and the time under each reply.
    pub verbose: bool,
    /// `--version`.
    pub version: bool,
    /// `-h`/`--help`.
    pub help: bool,
}

/// What the process should do, after parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Print and continue.
    Version,
    /// Print and continue.
    Help,
    /// Run the bot.
    Run(Options),
    /// The command line is wrong: print the message and exit with [`exit::USAGE`].
    Usage(String),
}

/// Whether argparse would accept `candidate` as the operand of `-p`/`--prompt`.
///
/// argparse refuses an option-looking token as an operand only when it *could be* an option:
/// a single `-` is a value, and so is a negative number (the parser has no negative-number
/// options, so `-5` is "not an option"). `./--quiet` is a path and therefore a value. Anything
/// else starting with `-` is rejected, and the caller reports "expected one argument".
fn is_prompt_operand(candidate: &str) -> bool {
    if candidate == "-" || !candidate.starts_with('-') {
        return true;
    }
    candidate.parse::<f64>().is_ok()
}

/// Parse `argv` (without the program name) exactly like `pybot.py`'s argparse.
///
/// Supported shapes (argparse accepts them all, and so must we):
/// `-p TEXT`, `-pTEXT`, `--prompt TEXT`, `--prompt=TEXT`, `-i`, `-s N`, `-sN`, `-v`,
/// `--version`, `-h`, `--help`, clustered short flags (`-iv`), `--` as end-of-options, and an
/// optional `--seed -5` negative value.
#[must_use]
pub fn parse<I, S>(args: I) -> Action
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut options = Options::default();
    let mut collected: Vec<String> = Vec::new();
    for argument in args {
        let argument: OsString = argument.into();
        match argument.into_string() {
            Ok(text) => collected.push(text),
            Err(_) => return Action::Usage(String::from("error: argument is not valid UTF-8")),
        }
    }
    let args = collected;

    let mut index = 0;
    let mut only_positional = false;
    while index < args.len() {
        let argument = args.get(index).map_or("", String::as_str);
        if only_positional {
            return Action::Usage(format!("error: unrecognized arguments: {argument}"));
        }
        match argument {
            "--" => {
                only_positional = true;
                index += 1;
            }
            "-i" | "--interactive" => {
                options.interactive = true;
                index += 1;
            }
            "-v" | "--verbose" => {
                options.verbose = true;
                index += 1;
            }
            "--version" => {
                options.version = true;
                index += 1;
            }
            "-h" | "--help" => {
                options.help = true;
                index += 1;
            }
            "-p" | "--prompt" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Action::Usage(format!(
                        "error: argument {argument}: expected one argument"
                    ));
                };
                if !is_prompt_operand(value) {
                    // argparse never consumes another option as a value.
                    return Action::Usage(format!(
                        "error: argument {argument}: expected one argument"
                    ));
                }
                options.prompt = Some(value.clone());
                index += 1;
            }
            "-s" | "--seed" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Action::Usage(String::from(
                        "error: argument -s/--seed: expected one argument",
                    ));
                };
                if value.starts_with('-') && value.parse::<i64>().is_err() && value != "-" {
                    return Action::Usage(String::from(
                        "error: argument -s/--seed: expected one argument",
                    ));
                }
                match value.parse::<i64>() {
                    Ok(seed) => options.seed = Some(seed),
                    Err(_) => {
                        return Action::Usage(format!(
                            "error: argument -s/--seed: invalid int value: '{value}'"
                        ))
                    }
                }
                index += 1;
            }
            _ if argument.starts_with("--prompt=") => {
                if let Some(value) = argument.strip_prefix("--prompt=") {
                    options.prompt = Some(value.to_string());
                }
                index += 1;
            }
            _ if argument.starts_with("--seed=") => {
                let value = argument.trim_start_matches("--seed=");
                match value.parse::<i64>() {
                    Ok(seed) => options.seed = Some(seed),
                    Err(_) => {
                        return Action::Usage(format!(
                            "error: argument -s/--seed: invalid int value: '{value}'"
                        ))
                    }
                }
                index += 1;
            }
            _ if argument.starts_with("--") => {
                return Action::Usage(format!("error: unrecognized arguments: {argument}"));
            }
            _ if argument.starts_with('-') && argument.len() > 1 => {
                // Clustered short options: -iv, -vp TEXT, -s5, -p"hey".
                let mut characters = argument.chars().skip(1).peekable();
                let mut consumed = false;
                while let Some(character) = characters.next() {
                    match character {
                        'i' => options.interactive = true,
                        'v' => options.verbose = true,
                        'h' => options.help = true,
                        'p' | 's' => {
                            let rest: String = characters.collect();
                            let value = if rest.is_empty() {
                                index += 1;
                                match args.get(index) {
                                    Some(value) if character != 'p' || is_prompt_operand(value) => {
                                        value.clone()
                                    }
                                    _ => {
                                        return Action::Usage(format!(
                                            "error: argument -{character}: expected one argument"
                                        ))
                                    }
                                }
                            } else {
                                rest
                            };
                            if character == 'p' {
                                options.prompt = Some(value);
                            } else {
                                match value.parse::<i64>() {
                                    Ok(seed) => options.seed = Some(seed),
                                    Err(_) => {
                                        return Action::Usage(format!(
                                        "error: argument -s/--seed: invalid int value: '{value}'"
                                    ))
                                    }
                                }
                            }
                            consumed = true;
                            break;
                        }
                        _ => {
                            return Action::Usage(format!(
                                "error: unrecognized arguments: -{character}"
                            ))
                        }
                    }
                }
                let _ = consumed;
                index += 1;
            }
            _ => {
                return Action::Usage(format!("error: unrecognized arguments: {argument}"));
            }
        }
    }

    if options.help {
        return Action::Help;
    }
    if options.version {
        return Action::Version;
    }
    Action::Run(options)
}

/// The `--version` line (`pybot.py`: `pybot 0.1.0 (bot at commit 24cbc04...)`).
#[must_use]
pub fn version_line() -> String {
    // The Python bot's reference commit, as declared by `pybot.py::SOURCE_COMMIT`.
    const SOURCE_COMMIT: &str = "24cbc0439e07";
    format!("pybot {VERSION} (bot at commit {SOURCE_COMMIT})")
}

/// The `--help` text, character-for-character what `pybot.py --help` prints (captured from
/// the Python CLI; `tests/cli.rs` diff-checks the `usage:` line and every option).
pub const HELP_TEXT: &str = "\
usage: pybot [-h] [-p TEXT] [-i] [-s N] [-v] [--version]

Chat with PyBot, a chatbot written in plain Python: no model, no weights.

options:
  -h, --help            show this help message and exit
  -p TEXT, --prompt TEXT
                        reply to this one message and exit
  -i, --interactive     with -p: keep chatting after the first reply
  -s N, --seed N        seed for PyBot's choice of wording, to get the same
                        replies again
  -v, --verbose         after each reply, show the module that produced it and
                        the time it took
  --version             show program's version number and exit

With no options, PyBot starts an interactive chat. When input is piped in, it
prints one reply per input line.
";

/// The `--help` text (same bytes as [`HELP_TEXT`]).
#[must_use]
pub fn help_text() -> String {
    String::from(HELP_TEXT)
}

/// Where the CLI reads from and writes to. Tests substitute these; `main_entry` builds them
/// from the process' own handles.
pub struct Streams<'a> {
    /// Standard input.
    pub stdin: &'a mut dyn BufRead,
    /// Standard output.
    pub stdout: &'a mut dyn Write,
    /// Standard error.
    pub stderr: &'a mut dyn Write,
    /// Whether stdin is a terminal (decides chat vs pipe when `-p` is absent).
    pub stdin_is_terminal: bool,
    /// Whether stdout is a terminal (decides screen-wipe vs `New conversation.` on `/reset`).
    pub stdout_is_terminal: bool,
}

impl std::fmt::Debug for Streams<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Streams")
            .field("stdin_is_terminal", &self.stdin_is_terminal)
            .field("stdout_is_terminal", &self.stdout_is_terminal)
            .finish_non_exhaustive()
    }
}

/// Print the opening (logo, tagline, hint) — `pybot.py::opening`.
pub fn opening(out: &mut dyn Write) -> Result<()> {
    writeln!(out, "{LOGO}\n").map_err(|error| Error::io("write opening", error))?;
    writeln!(out, "{TAGLINE}\n").map_err(|error| Error::io("write opening", error))?;
    writeln!(out, "{HINT}\n").map_err(|error| Error::io("write opening", error))
}

/// Run the parsed action against the given streams; returns the exit code.
///
/// The caller (usually [`main_entry`]) passes `Action::Run(options)`; `Help`/`Version`/
/// `Usage` are handled here too so a single function covers the whole surface and tests can
/// drive every path without spawning a process.
pub fn run(action: Action, streams: &mut Streams<'_>) -> Result<u8> {
    match action {
        Action::Version => {
            writeln!(streams.stdout, "{}", version_line())
                .map_err(|error| Error::io("write version", error))?;
            Ok(exit::OK)
        }
        Action::Help => {
            write!(streams.stdout, "{}", help_text())
                .map_err(|error| Error::io("write help", error))?;
            Ok(exit::OK)
        }
        Action::Usage(message) => {
            writeln!(streams.stderr, "{message}")
                .map_err(|error| Error::io("write usage", error))?;
            Ok(exit::USAGE)
        }
        Action::Run(options) => run_bot(&options, streams),
    }
}

fn run_bot(options: &Options, streams: &mut Streams<'_>) -> Result<u8> {
    let runtime = Runtime::new();
    let interactive = (options.prompt.is_none() && streams.stdin_is_terminal)
        || (options.prompt.is_some() && options.interactive);
    if interactive && options.prompt.is_none() {
        // `pybot.py` prints "Loading PyBot..." because Python takes seconds; the Rust port
        // loads instantly, and a status line that costs more than the thing it announces is
        // noise. Documented divergence.
    }
    let mut session = Session::new();
    if interactive {
        chat(&runtime, options, streams, &mut session)?;
    } else if options.prompt.is_some() {
        one_shot(&runtime, options, streams, &mut session)?;
    } else {
        pipe(&runtime, options, streams, &mut session)?;
    }
    Ok(exit::OK)
}

/// Read one line from `input`, keeping at most [`limits::MAX_INPUT_BYTES`] bytes.
///
/// Python's CLI reads a line and the runtime truncates it later; an adversarial pipe can send
/// a line far larger than the budget, and `read_until` would buffer all of it first. This read
/// keeps the first bytes up to the budget (plus the newline) and then **discards the rest of
/// the line** — through the newline or EOF — so memory stays bounded and the next line is read
/// normally.
///
/// `line` ends with `\n` when the input had one, exactly like `read_until(b'\n', ..)`, so
/// callers can keep using [`pyutil::chomp_line`].
fn read_line_bounded(input: &mut dyn BufRead, line: &mut Vec<u8>) -> std::io::Result<usize> {
    line.clear();
    // The budget for the text, plus one byte reserved for the newline when one arrives; a
    // truncated line therefore still ends in `\n` and `chomp_line` still trims it.
    let budget = limits::MAX_INPUT_BYTES.saturating_add(1);
    let mut kept = 0usize;
    let mut total = 0usize;
    loop {
        let available = match input.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            break;
        }
        let take = available.iter().position(|byte| *byte == b'\n');
        let end = take.map_or(available.len(), |position| position + 1);
        if kept < budget {
            // Reserve the last slot for the newline that ends this line (if any), so the kept
            // prefix never swallows it.
            let room = if take.is_some() {
                budget - kept - 1
            } else {
                budget - kept
            };
            let chunk = available.get(..room.min(end)).unwrap_or(available);
            line.extend_from_slice(chunk);
            kept += chunk.len();
        }
        total += end;
        input.consume(end);
        if take.is_some() {
            if kept < budget && kept == budget - 1 {
                line.push(b'\n');
            }
            break;
        }
    }
    Ok(total)
}

/// `-p` without `-i`: one reply, then exit.
fn one_shot(
    runtime: &Runtime,
    options: &Options,
    streams: &mut Streams<'_>,
    session: &mut Session,
) -> Result<()> {
    let prompt = options.prompt.as_deref().unwrap_or("");
    let answer = runtime.answer(prompt, session);
    writeln!(streams.stdout, "{}", answer.reply.as_str())
        .map_err(|error| Error::io("write reply", error))?;
    if options.verbose {
        writeln!(
            streams.stderr,
            "{}",
            details(answer.module.as_ref().map(crate::types::ModuleName::as_str))
        )
        .map_err(|error| Error::io("write details", error))?;
    }
    Ok(())
}

/// Piped input: one reply per line, sharing one session (like `python -m bot`).
fn pipe(
    runtime: &Runtime,
    options: &Options,
    streams: &mut Streams<'_>,
    session: &mut Session,
) -> Result<()> {
    let mut line = Vec::new();
    loop {
        match read_line_bounded(streams.stdin, &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => return Err(Error::io("read input", error)),
        }
        let turn = Turn::from_bytes(&line);
        if turn.is_lossy() {
            report_lossy(options, streams)?;
        }
        let text = pyutil::chomp_line(turn.text());
        let answer = runtime.answer(text, session);
        writeln!(streams.stdout, "{}", answer.reply.as_str())
            .map_err(|error| Error::io("write reply", error))?;
        if options.verbose {
            writeln!(
                streams.stderr,
                "{}",
                details(answer.module.as_ref().map(crate::types::ModuleName::as_str))
            )
            .map_err(|error| Error::io("write details", error))?;
        }
    }
    Ok(())
}

/// The interactive chat: banner, `user: ` prompts, `/reset`, `/help`, `/exit`, Ctrl-D.
fn chat(
    runtime: &Runtime,
    options: &Options,
    streams: &mut Streams<'_>,
    session: &mut Session,
) -> Result<()> {
    opening(streams.stdout)?;
    let mut first = options.prompt.clone();
    let mut line = Vec::new();
    loop {
        let text = if let Some(text) = first.take() {
            writeln!(streams.stdout, "{USER_PREFIX}{text}")
                .map_err(|error| Error::io("write prompt", error))?;
            text
        } else {
            write!(streams.stdout, "{USER_PREFIX}")
                .map_err(|error| Error::io("write prompt", error))?;
            streams
                .stdout
                .flush()
                .map_err(|error| Error::io("flush prompt", error))?;
            match read_line_bounded(streams.stdin, &mut line) {
                Ok(0) => {
                    writeln!(streams.stdout).map_err(|error| Error::io("write newline", error))?;
                    break;
                }
                Ok(_) => {}
                Err(error) => return Err(Error::io("read input", error)),
            }
            let turn = Turn::from_bytes(&line);
            if turn.is_lossy() {
                report_lossy(options, streams)?;
            }
            pyutil::chomp_line(turn.text()).to_string()
        };

        let command = pyutil::strip(&text).to_lowercase();
        if ["/exit", "/quit", "/bye"].contains(&command.as_str()) {
            break;
        }
        if command == "/reset" {
            session.reset();
            if streams.stdout_is_terminal {
                write!(streams.stdout, "{CLEAR_SCREEN}")
                    .map_err(|error| Error::io("write clear", error))?;
                opening(streams.stdout)?;
            } else {
                writeln!(streams.stdout, "New conversation.\n")
                    .map_err(|error| Error::io("write reset", error))?;
            }
            continue;
        }
        if command == "/help" {
            writeln!(streams.stdout, "{HELP}\n").map_err(|error| Error::io("write help", error))?;
            continue;
        }
        if command.is_empty() {
            continue;
        }
        let answer = runtime.answer(&text, session);
        writeln!(streams.stdout, "{BOT_PREFIX}{}", answer.reply.as_str())
            .map_err(|error| Error::io("write reply", error))?;
        if options.verbose {
            writeln!(
                streams.stdout,
                "{}",
                details(answer.module.as_ref().map(crate::types::ModuleName::as_str))
            )
            .map_err(|error| Error::io("write details", error))?;
        }
        writeln!(streams.stdout).map_err(|error| Error::io("write newline", error))?;
    }
    Ok(())
}

/// `[module: <name> | <ms> ms]` — `pybot.py::details`. The port has no timing here; the
/// differential and benchmark binaries report real timings.
/// Tell the user (on stderr, only with `-v`) that a line was not valid UTF-8.
///
/// The Python CLI crashes on such input. The port stays usable — the invalid bytes become
/// U+FFFD in the prompt and the turn proceeds — but the substitution is not silent, so a
/// corpus mismatch traced to it is explainable.
fn report_lossy(options: &Options, streams: &mut Streams<'_>) -> Result<()> {
    if options.verbose {
        writeln!(
            streams.stderr,
            "[input: invalid UTF-8 replaced with U+FFFD]"
        )
        .map_err(|error| Error::io("write details", error))?;
    }
    Ok(())
}

fn details(module: Option<&str>) -> String {
    format!("[module: {} | 0 ms]", module.unwrap_or("none"))
}

/// The process entry point: build streams from the real handles and map errors to exit codes.
///
/// `args` holds the arguments *after* `argv[0]` (like `sys.argv[1:]`); [`crate::cli::parse`]
/// does the parsing.
///
/// Never panics. A broken pipe (`pybot | head`) ends the process with success, like a
/// well-behaved Unix filter; any other I/O failure prints one line to stderr and exits 1.
#[must_use]
pub fn main_entry<I, S>(args: I) -> std::process::ExitCode
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let action = parse(args);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let stdin_is_terminal = stdin.is_terminal();
    let stdout_is_terminal = stdout.is_terminal();
    let mut stdin_lock = std::io::BufReader::new(stdin.lock());
    let mut stdout_lock = stdout.lock();
    let mut stderr_lock = stderr.lock();
    let mut streams = Streams {
        stdin: &mut stdin_lock,
        stdout: &mut stdout_lock,
        stderr: &mut stderr_lock,
        stdin_is_terminal,
        stdout_is_terminal,
    };
    match run(action, &mut streams) {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) if error.is_broken_pipe() => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(streams.stderr, "pybot: {error}");
            std::process::ExitCode::from(exit::FAILURE)
        }
    }
}

/// Normalise a reply exactly as `pybot.py::answer` does (`" ".join(str(reply).split())`,
/// falling back to `"Hello!"` for an empty line).
#[must_use]
pub fn normalise_reply(reply: &Reply) -> Reply {
    let normalised = pyutil::collapse_spaces(reply.as_str());
    if normalised.is_empty() {
        Reply::fallback()
    } else {
        Reply::new(normalised.into_owned())
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

    fn parse_args(args: &[&str]) -> Action {
        parse(args.iter().copied())
    }

    // ------------------------------------------------------------------ constants

    #[test]
    fn the_logo_is_the_python_one_and_fits_a_narrow_terminal() {
        let lines: Vec<&str> = LOGO.split('\n').collect();
        assert_eq!(lines.len(), 6);
        for line in lines {
            assert!(line.len() <= 80);
            assert_eq!(line, line.trim_end());
            assert!(line.is_ascii());
        }
        assert!(LOGO.contains("____"));
    }

    #[test]
    fn the_tagline_and_hint_match_pybot_py() {
        assert_eq!(
            TAGLINE,
            "A chatbot written in plain Python. No weights, no training, just rules."
        );
        assert!(HINT.contains("/reset"));
        assert!(HINT.contains("/exit"));
        assert_eq!(USER_PREFIX, "user: ");
        assert_eq!(BOT_PREFIX, "pybot: ");
        assert!(HELP.contains("/help"));
    }

    #[test]
    fn version_line_reports_the_crate_version_and_reference_commit() {
        assert_eq!(version_line(), "pybot 0.1.0 (bot at commit 24cbc0439e07)");
        assert!(help_text().contains("usage: pybot"));
        assert!(help_text().contains("--version"));
    }

    // ------------------------------------------------------------------ parsing

    #[test]
    fn no_arguments_means_run_with_defaults() {
        assert_eq!(parse_args(&[]), Action::Run(Options::default()));
        assert_eq!(
            parse_args(&["-i"]),
            Action::Run(Options {
                interactive: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["-v"]),
            Action::Run(Options {
                verbose: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["-s", "7", "-i", "-v"]),
            Action::Run(Options {
                seed: Some(7),
                interactive: true,
                verbose: true,
                ..Options::default()
            })
        );
    }

    #[test]
    fn prompt_accepts_every_argparse_shape() {
        let expected = Some(String::from("hello there"));
        assert_eq!(
            parse_args(&["-p", "hello there"]),
            Action::Run(Options {
                prompt: expected.clone(),
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["--prompt", "hello there"]),
            Action::Run(Options {
                prompt: expected.clone(),
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["--prompt=hello there"]),
            Action::Run(Options {
                prompt: expected.clone(),
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["-phello there"]),
            Action::Run(Options {
                prompt: expected.clone(),
                ..Options::default()
            })
        );
        // argparse refuses an option token as the operand of `-p` and reports the missing
        // argument instead of consuming `-i` as the prompt (verified against CPython 3.11).
        match parse_args(&["-p", "-i"]) {
            Action::Usage(message) => {
                assert_eq!(message, "error: argument -p: expected one argument");
            }
            other => panic!("expected a usage error, got {other:?}"),
        }
        // A negative number and a bare `-` are not options, so they stay valid prompts.
        assert_eq!(
            parse_args(&["-p", "-5"]),
            Action::Run(Options {
                prompt: Some(String::from("-5")),
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["-p", "-"]),
            Action::Run(Options {
                prompt: Some(String::from("-")),
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["-p", ""]),
            Action::Run(Options {
                prompt: Some(String::new()),
                ..Options::default()
            })
        );
    }

    #[test]
    fn seed_accepts_int_shapes_and_rejects_junk() {
        assert_eq!(
            parse_args(&["-s0"]),
            Action::Run(Options {
                seed: Some(0),
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["--seed", "-5"]),
            Action::Run(Options {
                seed: Some(-5),
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["--seed=42"]),
            Action::Run(Options {
                seed: Some(42),
                ..Options::default()
            })
        );
        match parse_args(&["-s", "abc"]) {
            Action::Usage(message) => assert!(message.contains("invalid int value: 'abc'")),
            other => panic!("{other:?}"),
        }
        match parse_args(&["-s"]) {
            Action::Usage(message) => assert!(message.contains("expected one argument")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn clustered_short_flags_work_like_argparse() {
        assert_eq!(
            parse_args(&["-iv"]),
            Action::Run(Options {
                interactive: true,
                verbose: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_args(&["-vip", "hey"]),
            Action::Run(Options {
                interactive: true,
                verbose: true,
                prompt: Some(String::from("hey")),
                ..Options::default()
            })
        );
        match parse_args(&["-x"]) {
            Action::Usage(message) => assert!(message.contains("unrecognized arguments: -x")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn help_and_version_win_over_running() {
        assert_eq!(parse_args(&["-h"]), Action::Help);
        assert_eq!(parse_args(&["--help"]), Action::Help);
        assert_eq!(parse_args(&["--version"]), Action::Version);
        assert_eq!(parse_args(&["-p", "hi", "--help"]), Action::Help);
        assert_eq!(parse_args(&["--version", "-p", "hi"]), Action::Version);
    }

    #[test]
    fn positionals_and_unknown_options_are_usage_errors() {
        match parse_args(&["hello"]) {
            Action::Usage(message) => assert_eq!(message, "error: unrecognized arguments: hello"),
            other => panic!("{other:?}"),
        }
        match parse_args(&["--nope"]) {
            Action::Usage(message) => assert_eq!(message, "error: unrecognized arguments: --nope"),
            other => panic!("{other:?}"),
        }
        match parse_args(&["--", "-p"]) {
            Action::Usage(message) => assert_eq!(message, "error: unrecognized arguments: -p"),
            other => panic!("{other:?}"),
        }
        match parse_args(&["--prompt"]) {
            Action::Usage(message) => assert!(message.contains("expected one argument")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn seed_rejects_overflow_and_empty() {
        match parse_args(&["--seed=99999999999999999999"]) {
            Action::Usage(message) => assert!(message.contains("invalid int value")),
            other => panic!("{other:?}"),
        }
        match parse_args(&["--seed="]) {
            Action::Usage(message) => assert!(message.contains("invalid int value")),
            other => panic!("{other:?}"),
        }
    }

    // ------------------------------------------------------------------ streams

    struct Harness {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        code: u8,
    }

    fn drive(action: Action, input: &[u8], stdin_terminal: bool, stdout_terminal: bool) -> Harness {
        let mut stdin = std::io::BufReader::new(std::io::Cursor::new(input.to_vec()));
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = {
            let mut streams = Streams {
                stdin: &mut stdin,
                stdout: &mut stdout,
                stderr: &mut stderr,
                stdin_is_terminal: stdin_terminal,
                stdout_is_terminal: stdout_terminal,
            };
            run(action, &mut streams).expect("in-memory streams never fail")
        };
        Harness {
            stdout,
            stderr,
            code,
        }
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    // ------------------------------------------------------------------ one-shot

    #[test]
    fn one_shot_prints_exactly_one_reply_line() {
        let harness = drive(
            Action::Run(Options {
                prompt: Some(String::from("hello")),
                ..Options::default()
            }),
            b"",
            false,
            false,
        );
        assert_eq!(harness.code, exit::OK);
        let output = text(&harness.stdout);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "Hi there — what do you need?");
        assert!(harness.stderr.is_empty());
    }

    #[test]
    fn one_shot_of_unknown_input_prints_the_fallback() {
        let harness = drive(
            Action::Run(Options {
                prompt: Some(String::from("how do i boil an egg")),
                ..Options::default()
            }),
            b"",
            false,
            false,
        );
        assert_eq!(text(&harness.stdout), "Hello!\n");
    }

    #[test]
    fn verbose_one_shot_puts_details_on_stderr() {
        let harness = drive(
            Action::Run(Options {
                prompt: Some(String::from("hello")),
                verbose: true,
                ..Options::default()
            }),
            b"",
            false,
            false,
        );
        assert_eq!(text(&harness.stdout), "Hi there — what do you need?\n");
        assert_eq!(text(&harness.stderr), "[module: greeting | 0 ms]\n");
    }

    // ------------------------------------------------------------------ pipe

    #[test]
    fn pipe_prints_one_reply_per_line_and_collapses_blank_lines() {
        let harness = drive(
            Action::Run(Options::default()),
            b"hello\n\nhi\n",
            false,
            false,
        );
        assert_eq!(harness.code, exit::OK);
        let output = text(&harness.stdout);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Hi there — what do you need?");
        assert_eq!(
            lines[1], "Hello!",
            "an empty line still gets a reply (Python does)"
        );
        assert_eq!(lines[2], "Hi there — what do you need?");
    }

    #[test]
    fn pipe_handles_crlf_and_a_missing_final_newline() {
        let harness = drive(
            Action::Run(Options::default()),
            b"hello\r\nhello",
            false,
            false,
        );
        assert_eq!(text(&harness.stdout).lines().count(), 2);
        assert!(harness.stdout.ends_with(b"\n"));
        assert!(!text(&harness.stdout).contains('\r'));
    }

    #[test]
    fn pipe_survives_invalid_utf8_instead_of_dying() {
        let harness = drive(
            Action::Run(Options::default()),
            b"hi \xff\xfe there\nhello\n",
            false,
            false,
        );
        assert_eq!(harness.code, exit::OK);
        assert_eq!(text(&harness.stdout).lines().count(), 2);
    }

    #[test]
    fn pipe_shares_one_session_across_lines() {
        let harness = drive(
            Action::Run(Options::default()),
            b"hi, i'm Sam\nwhat's my name?\n",
            false,
            false,
        );
        let output = text(&harness.stdout);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "Nice to meet you, Sam. What can I help with?");
        assert_eq!(
            lines[1], "You're Sam.",
            "the second line sees the name the first stored"
        );
    }

    #[test]
    fn pipe_with_empty_input_prints_nothing() {
        let harness = drive(Action::Run(Options::default()), b"", false, false);
        assert_eq!(harness.stdout, Vec::<u8>::new());
        assert_eq!(harness.code, exit::OK);
    }

    #[test]
    fn pipe_verbose_reports_every_module_on_stderr() {
        let harness = drive(
            Action::Run(Options {
                verbose: true,
                ..Options::default()
            }),
            b"hello\ncook pasta\n",
            false,
            false,
        );
        let output = text(&harness.stderr);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(
            lines,
            vec!["[module: greeting | 0 ms]", "[module: none | 0 ms]"]
        );
    }

    #[test]
    fn args_after_the_program_name_are_what_parse_sees() {
        // `argv[0]` is dropped by `main`; `parse` only ever sees the rest. A positional here is
        // a usage error, exactly as `pybot.py` reports it.
        assert_eq!(
            parse(["pybot"]),
            Action::Usage(String::from("error: unrecognized arguments: pybot"))
        );
        assert_eq!(
            parse(["-p", "hello"]),
            Action::Run(Options {
                prompt: Some(String::from("hello")),
                ..Options::default()
            })
        );
    }

    // ------------------------------------------------------------------ chat

    #[test]
    fn chat_prints_the_opening_then_labelled_turns() {
        let harness = drive(
            Action::Run(Options::default()),
            b"hello\n/exit\n",
            true,
            false,
        );
        let output = text(&harness.stdout);
        assert!(output.starts_with(LOGO), "the opening is printed first");
        assert!(output.contains(TAGLINE));
        assert!(output.contains(HINT));
        // The prompt carries the label; the typed text is not echoed (a real terminal echoes
        // it, but `stdin`/`stdout` here are pipes — Python behaves the same way).
        assert_eq!(output.matches(USER_PREFIX).count(), 2, "{output}");
        assert!(
            !output.contains("user: hello"),
            "input is never echoed by pybot itself"
        );
        assert!(
            output.contains(&format!("{BOT_PREFIX}Hi there — what do you need?\n\n")),
            "{output}"
        );
        assert_eq!(harness.code, exit::OK);
    }

    #[test]
    fn chat_survives_eof_without_exit() {
        let harness = drive(Action::Run(Options::default()), b"hello\n", true, false);
        assert!(text(&harness.stdout).contains("pybot: "));
        assert_eq!(harness.code, exit::OK);
    }

    #[test]
    fn chat_exit_commands_all_quit() {
        for command in ["/exit", "/quit", "/bye", "  /exit  ", "/EXIT"] {
            let harness = drive(
                Action::Run(Options::default()),
                format!("hello\n{command}\nhi\n").as_bytes(),
                true,
                false,
            );
            let output = text(&harness.stdout);
            assert_eq!(output.matches("pybot: ").count(), 1, "{command}");
            assert!(!output.contains("user: hi"), "{command}");
        }
    }

    #[test]
    fn chat_reset_forgets_and_prints_a_line_when_not_a_terminal() {
        let harness = drive(
            Action::Run(Options::default()),
            b"hi, i'm Sam\n/reset\nwhat's my name?\n/exit\n",
            true,
            false,
        );
        let output = text(&harness.stdout);
        assert!(output.contains("New conversation.\n\n"), "{output}");
        assert!(!output.contains('\u{1b}'), "no escape sequence on a pipe");
        assert!(
            output.ends_with(&format!(
                "{BOT_PREFIX}I don't know it yet.\n\n{USER_PREFIX}"
            )),
            "the name is gone after /reset: {output}"
        );
    }

    #[test]
    fn chat_reset_wipes_the_screen_on_a_terminal() {
        let harness = drive(
            Action::Run(Options::default()),
            b"hello\n/reset\n/exit\n",
            true,
            true,
        );
        let output = text(&harness.stdout);
        let Some((before, after)) = output.split_once(CLEAR_SCREEN) else {
            unreachable!("the terminal reset must clear the screen: {output}");
        };
        assert!(before.contains("pybot: "));
        assert!(after.starts_with(LOGO));
        assert!(!after.contains("pybot: "));
        assert!(!after.contains("New conversation."));
    }

    #[test]
    fn chat_help_prints_the_commands_and_gets_no_reply() {
        let harness = drive(
            Action::Run(Options::default()),
            b"/help\n/exit\n",
            true,
            false,
        );
        let output = text(&harness.stdout);
        assert!(output.contains("Commands:"));
        assert!(output.contains("/reset"));
        assert!(!output.contains("pybot: "));
    }

    #[test]
    fn chat_skips_blank_lines() {
        let harness = drive(
            Action::Run(Options::default()),
            b"\n   \nhello\n/exit\n",
            true,
            false,
        );
        let output = text(&harness.stdout);
        assert_eq!(output.matches("pybot: ").count(), 1);
    }

    #[test]
    fn chat_with_prompt_and_interactive_shows_the_prompt_as_a_user_turn() {
        let harness = drive(
            Action::Run(Options {
                prompt: Some(String::from("hello")),
                interactive: true,
                ..Options::default()
            }),
            b"/exit\n",
            false,
            false,
        );
        let output = text(&harness.stdout);
        assert!(output.contains("user: hello"));
        assert_eq!(output.matches("pybot: ").count(), 1);
    }

    // ------------------------------------------------------------------ usage/help/version

    #[test]
    fn help_and_version_actions_print_and_exit_zero() {
        let harness = drive(Action::Help, b"", false, false);
        assert_eq!(harness.code, exit::OK);
        assert!(text(&harness.stdout).starts_with("usage: pybot"));
        let harness = drive(Action::Version, b"", false, false);
        assert_eq!(harness.code, exit::OK);
        assert_eq!(
            text(&harness.stdout),
            "pybot 0.1.0 (bot at commit 24cbc0439e07)\n"
        );
    }

    #[test]
    fn usage_errors_go_to_stderr_with_exit_two() {
        let harness = drive(Action::Usage(String::from("error: bad")), b"", false, false);
        assert_eq!(harness.code, exit::USAGE);
        assert_eq!(text(&harness.stderr), "error: bad\n");
        assert!(harness.stdout.is_empty());
    }

    // ------------------------------------------------------------------ normalisation

    #[test]
    fn normalise_reply_matches_pybot_join_split() {
        assert_eq!(normalise_reply(&Reply::new("  a \t b ")).as_str(), "a b");
        assert_eq!(normalise_reply(&Reply::new("")).as_str(), "Hello!");
        assert_eq!(normalise_reply(&Reply::new("   ")).as_str(), "Hello!");
        assert_eq!(normalise_reply(&Reply::new("Hello!")).as_str(), "Hello!");
    }

    #[test]
    fn read_line_bounded_keeps_the_budget_and_discards_the_rest() {
        let budget = limits::MAX_INPUT_BYTES;
        // A line far over budget, then a normal line: the first is cut at the budget (plus the
        // newline) and the second is read normally, so a long line never eats its successor.
        let mut input =
            std::io::Cursor::new([vec![b'a'; budget * 3], b"\nsecond\n".to_vec()].concat());
        let mut line = Vec::new();
        let first = read_line_bounded(&mut input, &mut line).unwrap();
        assert_eq!(first, budget * 3 + 1, "the whole line was consumed");
        assert_eq!(
            line.len(),
            budget + 1,
            "the budget of text plus the kept newline"
        );
        assert_eq!(line.last(), Some(&b'\n'));

        let second = read_line_bounded(&mut input, &mut line).unwrap();
        assert_eq!(second, 7);
        assert_eq!(line, b"second\n");

        let third = read_line_bounded(&mut input, &mut line).unwrap();
        assert_eq!(third, 0, "EOF");
        assert!(line.is_empty());
    }

    #[test]
    fn read_line_bounded_handles_no_trailing_newline_and_crlf() {
        let mut input = std::io::Cursor::new(b"no newline".to_vec());
        let mut line = Vec::new();
        assert_eq!(read_line_bounded(&mut input, &mut line).unwrap(), 10);
        assert_eq!(line, b"no newline");
        assert_eq!(read_line_bounded(&mut input, &mut line).unwrap(), 0);
        assert!(line.is_empty(), "the buffer is cleared on every call");

        let mut input = std::io::Cursor::new(b"crlf\r\nnext\n".to_vec());
        assert_eq!(read_line_bounded(&mut input, &mut line).unwrap(), 6);
        assert_eq!(line, b"crlf\r\n");
        assert_eq!(pyutil::chomp_line(str::from_utf8(&line).unwrap()), "crlf");
    }

    #[test]
    fn a_prompt_operand_may_not_be_another_option() {
        // argparse: `-p -i` and `-p --verbose` are missing operands, not an `-i` prompt.
        assert!(!is_prompt_operand("-i"));
        assert!(!is_prompt_operand("--verbose"));
        assert!(!is_prompt_operand("--"));
        // …but a number, a lone dash and a relative path are values.
        assert!(is_prompt_operand("-5"));
        assert!(is_prompt_operand("-1.5e3"));
        assert!(is_prompt_operand("-"));
        assert!(is_prompt_operand("./--quiet"));
        assert!(is_prompt_operand("hello"));
        assert!(is_prompt_operand(""));
    }

    #[test]
    fn details_always_names_a_module_or_none() {
        assert_eq!(details(Some("chat")), "[module: chat | 0 ms]");
        assert_eq!(details(None), "[module: none | 0 ms]");
    }

    #[test]
    fn the_streams_debug_impl_exists_for_assertions() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let mut input = std::io::BufReader::new(std::io::Cursor::new(Vec::<u8>::new()));
        let streams = Streams {
            stdin: &mut input,
            stdout: &mut out,
            stderr: &mut err,
            stdin_is_terminal: false,
            stdout_is_terminal: false,
        };
        let rendered = format!("{streams:?}");
        assert!(rendered.contains("stdin_is_terminal"));
    }
}
