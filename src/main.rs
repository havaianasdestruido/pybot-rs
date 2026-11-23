//! `pybot` — the command line, `pybot.py` ported.
//!
//! ```text
//! pybot                      interactive chat (stdin is a terminal)
//! pybot -p "your message"    one reply, then exit
//! echo "a line" | pybot      one reply per input line
//! ```
//!
//! The CLI holds no chatbot logic: it parses arguments, owns the [`pybot::Session`] and the
//! input/output loop, and delegates every turn to [`pybot::Runtime`]. Anything that can fail
//! (a closed pipe, invalid UTF-8, an unreadable terminal) is mapped to an exit code instead of
//! a panic — see [`pybot::cli`].

fn main() -> std::process::ExitCode {
    // `argv[0]` is the program name, and `parse` takes only the arguments after it, like
    // `argparse.parse_args(sys.argv[1:])`.
    pybot::cli::main_entry(std::env::args_os().skip(1))
}
