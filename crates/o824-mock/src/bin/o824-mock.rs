//! The `o824-mock` standalone entry point: the transcript replayer over stdio.
//!
//! This is what the `o824-mock` npm package runs. It is deliberately thin — parse the
//! flags, load the transcript, hand the process's own streams to the same
//! [`Transcript::replay`](o824_mock::Transcript::replay) the kernel's tests use — so
//! the packaged mock and the crate cannot diverge.
//!
//! Exit codes follow the CLI contract: `0` the client ended the session, `2` the
//! command line was unusable, `8` the transcript itself is unusable.

use std::io::{BufReader, BufWriter};
use std::path::Path;

use o824_mock::{Options, Transcript};

const USAGE: &str = "\
Usage: o824-mock --transcript <path> [--delay <ms>] [--fail-at <n>] [--protocol <ver>]

Replays a recorded O824 session over stdio, framed exactly as the kernel frames it
(Content-Length headers). Requests are matched by method, not by position.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        return;
    }

    let mut transcript_path: Option<String> = None;
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args.get(index + 1).cloned();
        match (flag, value) {
            ("--transcript", Some(value)) => transcript_path = Some(value),
            ("--protocol", Some(value)) => options.protocol = Some(value),
            ("--delay", Some(value)) => match value.trim().parse::<u64>() {
                Ok(milliseconds) => {
                    options.delay = Some(std::time::Duration::from_millis(milliseconds));
                }
                Err(_) => usage_error(&format!("--delay expects milliseconds, got {value:?}")),
            },
            ("--fail-at", Some(value)) => match value.trim().parse::<usize>() {
                Ok(position) => options.fail_at = position,
                Err(_) => usage_error(&format!("--fail-at expects a message index, got {value:?}")),
            },
            (other, _) => usage_error(&format!("unexpected argument {other:?}")),
        }
        index += 2;
    }

    let Some(path) = transcript_path else {
        usage_error("pass --transcript <path>");
    };
    let transcript = match Transcript::load(Path::new(&path)) {
        Ok(transcript) => transcript,
        Err(error) => {
            eprintln!("o824-mock: {error}");
            std::process::exit(8);
        }
    };

    let mut input = BufReader::new(std::io::stdin());
    let mut output = BufWriter::new(std::io::stdout());
    if let Err(error) = transcript.replay(&mut input, &mut output, &options) {
        eprintln!("o824-mock: {error}");
        std::process::exit(8);
    }
}

fn usage_error(message: &str) -> ! {
    eprintln!("o824-mock: {message}\n\n{USAGE}");
    std::process::exit(2);
}
