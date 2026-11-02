//! The `kernel` subcommand: the live stdio server a UI talks to.
//!
//! This is the承重墙 of §7.5: the kernel is always a separate process, its only surface
//! is JSON-RPC over stdin/stdout, and stderr is reserved for diagnostics. Everything
//! the CLI does from the command line, a UI does through this.

use std::io::{BufReader, Write};

use o824_errs::{Code, Error};

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_of, EXIT_INTERNAL, EXIT_OK};

/// The help text for the kernel subcommand.
pub const USAGE: &str = "\
Usage: o824 kernel --stdio [--root <path>] [--home <path>]

Serves the protocol on stdin/stdout until the client closes the pipe. One JSON-RPC
message per Content-Length frame; stderr carries diagnostics and never protocol.

Methods: o824.info, o824.discover, o824.doctor, o824.plan, o824.apply, o824.cancel.
Work that touches the machine runs as a job: the answer is a jobId, and the result
arrives as op.state / op.progress / op.result notifications.

Flags:
  --stdio            required; the only transport this kernel speaks
  --root <path>      diagnose exactly this installation instead of scanning
  --home <path>      the DSH home whose profiles are reported
  --data-root <path> snapshots, journals and plans (defaults to O824_DATA_ROOT)
";

/// Runs `o824 kernel`. Returns the process exit code.
pub fn run(args: &[String], stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stderr.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("stdio"),
        FlagSpec::valued("root"),
        FlagSpec::valued("home"),
        FlagSpec::valued("data-root"),
    ]);
    if let Err(error) = flags.parse("kernel", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if !flags.flag("stdio") {
        let error = Error::new(
            Code::Usage,
            "o824 kernel: pass --stdio; it is the only transport this kernel speaks",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 kernel: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let options = o824_kernel::ServerOptions {
        roots: flags
            .value("root")
            .map(|root| vec![std::path::PathBuf::from(root)]),
        home: flags.value("home").map(str::to_owned),
        data_root: flags.value("data-root").map(std::path::PathBuf::from),
    };

    // The server writes frames on stdout directly rather than through the CLI's writer:
    // it owns that stream for the whole session, and a buffered wrapper around a
    // writer the caller keeps would interleave.
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    match o824_kernel::serve(&mut input, Box::new(std::io::stdout()), options) {
        Ok(()) => EXIT_OK,
        Err(error) => {
            let _ = writeln!(stderr, "o824 kernel: {error}");
            EXIT_INTERNAL
        }
    }
}
