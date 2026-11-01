//! The `mock` subcommand: replay a recorded session instead of running a kernel.
//!
//! A UI can be developed and tested against a recording, with no DSH installed and
//! no broken machine (spec §19.5). The mock speaks the same Content-Length framing
//! as the kernel, so the client cannot tell the difference — which is the point.

use std::io::{BufReader, Write};
use std::path::PathBuf;

use o824_errs::{Code, Error};

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_of, EXIT_OK};

/// The help text for the mock subcommand.
pub const USAGE: &str = "\
Usage:
  o824 mock --transcript <path> [--delay <ms>] [--fail-at <n>] [--protocol <version>]

Replays a recorded O824 session over stdio, framed exactly as the kernel frames
it (Content-Length headers). Requests are matched by method, not by position, and
each recorded answer is used once in recorded order; asking for a method the
recording does not cover is answered with -32601 / E_UNSUPPORTED naming the
methods it does cover.

Flags:
  --transcript <path>  the JSONL recording to replay
  --delay <ms>         pause this many milliseconds between emitted messages
  --fail-at <n>        replace the n-th emitted message with a -32603 /
                       E_MOCK_FAILURE error, so a UI can exercise its error path
  --protocol <version> report this protocolVersion from o824.info instead of the
                       recorded one, to test negotiation

Exit code 0 means the client ended the session, 2 means the command line was
unusable, and 8 means the transcript itself is unusable.
";

/// Runs `o824 mock`. Returns the process exit code.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::valued("transcript"),
        FlagSpec::valued("delay"),
        FlagSpec::valued("fail-at"),
        FlagSpec::valued("protocol"),
    ]);
    if let Err(error) = flags.parse("mock", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 mock: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let Some(path) = flags.value("transcript").filter(|path| !path.is_empty()) else {
        let error = Error::new(Code::Usage, "o824 mock: pass --transcript <path>");
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };

    let delay = match flags.value("delay") {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(milliseconds) => Some(std::time::Duration::from_millis(milliseconds)),
            Err(_) => {
                let error = Error::new(
                    Code::Usage,
                    format!("o824 mock: --delay expects milliseconds, got {value:?}"),
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        },
        None => None,
    };
    let fail_at = match flags.value("fail-at") {
        Some(value) => match value.trim().parse::<usize>() {
            Ok(index) => index,
            Err(_) => {
                let error = Error::new(
                    Code::Usage,
                    format!("o824 mock: --fail-at expects a message index, got {value:?}"),
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        },
        None => 0,
    };

    let transcript = match o824_mock::Transcript::load(&PathBuf::from(path)) {
        Ok(transcript) => transcript,
        Err(error) => {
            let error = Error::new(Code::Internal, format!("o824 mock: {error}"));
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    let options = o824_mock::Options {
        delay,
        fail_at,
        protocol: flags.value("protocol").map(str::to_owned),
    };
    let mut input = BufReader::new(std::io::stdin());
    match transcript.replay(&mut input, stdout, &options) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            let error = Error::new(Code::Internal, format!("o824 mock: {error}"));
            let _ = writeln!(stderr, "{error}");
            exit_code_of(Some(&error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_capture(args: &[&str]) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run(&args, &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8(stdout).expect("utf8"),
            String::from_utf8(stderr).expect("utf8"),
        )
    }

    #[test]
    fn a_missing_transcript_is_a_usage_error() {
        let (code, _, stderr) = run_capture(&[]);
        assert_eq!(code, crate::EXIT_USAGE);
        assert!(stderr.contains("--transcript"), "{stderr}");
    }

    #[test]
    fn an_unreadable_transcript_is_an_internal_error() {
        let (code, _, stderr) = run_capture(&["--transcript", r"C:\definitely\not\here.jsonl"]);
        assert_eq!(code, crate::EXIT_INTERNAL);
        assert!(stderr.contains("transcript"), "{stderr}");
    }

    #[test]
    fn the_help_lists_the_flags() {
        let (code, stdout, _) = run_capture(&["--help"]);
        assert_eq!(code, EXIT_OK);
        assert!(stdout.contains("--fail-at"), "{stdout}");
    }

    #[test]
    fn a_bad_delay_is_a_usage_error() {
        let (code, _, stderr) = run_capture(&["--transcript", "x.jsonl", "--delay", "soon"]);
        assert_eq!(code, crate::EXIT_USAGE);
        assert!(stderr.contains("--delay"), "{stderr}");
    }
}
