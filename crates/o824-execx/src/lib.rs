//! The kernel's only child-process runner.
//!
//! It takes a program name and an argument slice — never a command string — so a
//! path or an argument that contains a space, a quote, or a non-ASCII character
//! reaches the child exactly as the caller wrote it: the kernel never joins,
//! quotes, or re-tokenizes it, and there is no shell of ours to re-interpret it.
//!
//! A non-zero exit is **data, not a failure**: [`run`] returns the exit status
//! with no error, exactly as a shell would report it, and stdout and stderr are
//! captured into separate buffers and returned unmerged, so a diagnostic a tool
//! writes to stderr can never be mistaken for its output.
//!
//! One limit is worth stating because it is easy to "fix" the wrong way: the
//! runner cannot make `cmd /c <absolute .bat path containing spaces> <quoted
//! argument>` work. The kernel hands `cmd.exe` a correctly quoted command line,
//! but cmd re-parses the text after `/c` with its own rules, and when that text
//! starts with a quoted path and another quoted argument follows, cmd strips the
//! outer quotes and splits the path at its first space. That is cmd.exe's
//! parsing, not the runner's; the answer is to set the working directory and pass
//! the script by its relative name, or to use the interpreter's own argv form —
//! never to concatenate a command string, which is precisely what this crate
//! exists to prevent.

#![forbid(unsafe_code)]

use std::path::Path;
use std::process::{Command, Stdio};

use o824_errs::{Code, Error};

/// The code reported when the process did not run to completion at all.
///
/// It is only ever returned together with an error, and callers must check the
/// error before reading the code.
pub const EXIT_UNSTARTED: i32 = -1;

/// Everything one child process produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// What the child wrote to stdout.
    pub stdout: String,
    /// What the child wrote to stderr.
    pub stderr: String,
    /// The exit status, or [`EXIT_UNSTARTED`] when there is none.
    pub code: i32,
    /// Set only when the process could not be started or did not run to
    /// completion.
    pub error: Option<Error>,
}

impl Output {
    /// True when the process ran and exited with status 0.
    pub fn succeeded(&self) -> bool {
        self.error.is_none() && self.code == 0
    }
}

/// Starts `program` with `args`, in `dir`, and returns everything it wrote.
///
/// `dir` means what `Command::current_dir` means: `None` keeps the current
/// directory. The child's stdin is not connected (it reads an empty device), so a
/// command that prompts fails quickly instead of hanging the CLI.
///
/// A process that ran and exited — successfully or not — returns
/// `error == None`: `code` is its exit status and the two buffers are whatever it
/// wrote, even when that status is non-zero. An error is reserved for a process
/// that could not be started or that the operating system could not deliver
/// output from; then `code` is [`EXIT_UNSTARTED`] and whatever was captured up to
/// that point is still returned.
pub fn run(program: &str, args: &[String], dir: Option<&Path>) -> Output {
    run_with_env(program, args, None, dir)
}

/// [`run`] with an explicit child environment.
///
/// `None` inherits the current environment; `Some` is used as the child's
/// complete environment. It exists because a command that must see one extra
/// variable — Electron's `ELECTRON_RUN_AS_NODE`, which makes the host executable
/// behave as Node so it can run its own packaged CLI — must not mutate the
/// parent's environment to get it, and must still be started as argv rather than
/// through a shell.
pub fn run_with_env(
    program: &str,
    args: &[String],
    env: Option<&[(String, String)]>,
    dir: Option<&Path>,
) -> Output {
    let mut command = Command::new(program);
    command.args(args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    if let Some(env) = env {
        command.env_clear();
        for (key, value) in env {
            command.env(key, value);
        }
    }
    // A child run from a GUI process must not flash a console window.
    suppress_console_window(&mut command);

    match command.output() {
        // The process ran and exited on its own: its status is data.
        Ok(output) => Output {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(EXIT_UNSTARTED),
            error: None,
        },
        Err(error) => Output {
            stdout: String::new(),
            stderr: String::new(),
            code: EXIT_UNSTARTED,
            error: Some(run_error(program, dir, &error)),
        },
    }
}

/// Builds the kernel error for a process that did not run to completion.
///
/// It names the program and its working directory and deliberately omits the
/// argument list, which may contain long or non-ASCII paths.
fn run_error(program: &str, dir: Option<&Path>, cause: &std::io::Error) -> Error {
    let dir_text = display_dir(dir);
    Error::new(
        Code::Internal,
        format!(
            "execx: cannot run {} in {}: {cause}",
            quote_path(program),
            quote_path(&dir_text)
        ),
    )
    // Keys are inserted in the order the reference implementation's Go map
    // marshals them: alphabetical.
    .with_param("dir", dir_text)
    .with_param("error", cause.to_string())
    .with_param("program", program)
}

/// Renders a path for a diagnostic.
///
/// Deliberately not a debug-printed string: that escapes backslashes, so the
/// directory would read `"C:\\Users\\..."` — the opposite of naming the path for
/// the Windows user reading the error. Windows paths cannot contain a double
/// quote, so wrapping is unambiguous.
fn quote_path(path: &str) -> String {
    format!("\"{path}\"")
}

/// Names the child's working directory in a message. An unset directory means
/// "whatever the current directory is at the moment of the call".
fn display_dir(dir: Option<&Path>) -> String {
    match dir {
        Some(dir) => dir.display().to_string(),
        None => ".".to_owned(),
    }
}

#[cfg(windows)]
fn suppress_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    /// Windows' CREATE_NO_WINDOW: a GUI process has no console of its own, so
    /// Windows would create one for a console-subsystem child.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn suppress_console_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_exe() -> String {
        std::env::current_exe()
            .expect("current exe")
            .display()
            .to_string()
    }

    #[test]
    fn a_zero_exit_reports_no_error() {
        let output = run(&current_exe(), &["--list".to_owned()], None);
        assert!(output.error.is_none(), "{:?}", output.error);
        assert_eq!(output.code, 0);
        assert!(
            output.stdout.contains("tests::"),
            "the child's stdout was not captured: {}",
            output.stdout
        );
    }

    /// The program path itself contains spaces on this machine, and so does the
    /// skip pattern. If the runner split or re-quoted either of them, the child
    /// harness would reject the argument and exit non-zero.
    #[test]
    fn spaces_and_non_ascii_survive_the_runner() {
        let args = [
            "--list".to_owned(),
            "--skip".to_owned(),
            "no such test 测试".to_owned(),
        ];
        let output = run(&current_exe(), &args, None);
        assert_eq!(output.code, 0, "stderr: {}", output.stderr);
        assert!(output.stderr.is_empty(), "stderr: {}", output.stderr);
    }

    #[test]
    fn a_non_zero_exit_is_data_not_an_error() {
        let args = ["/c".to_owned(), "exit".to_owned(), "3".to_owned()];
        let output = run("cmd", &args, None);
        assert!(output.error.is_none(), "{:?}", output.error);
        assert_eq!(output.code, 3);
    }

    #[test]
    fn stdout_and_stderr_stay_separate() {
        let args = ["/c".to_owned(), "echo out& echo err 1>&2".to_owned()];
        let output = run("cmd", &args, None);
        assert!(output.stdout.contains("out"), "{:?}", output.stdout);
        assert!(output.stderr.contains("err"), "{:?}", output.stderr);
        assert!(
            !output.stdout.contains("err"),
            "streams merged: {:?}",
            output.stdout
        );
    }

    #[test]
    fn a_missing_program_is_an_internal_error() {
        let output = run("o824-no-such-program", &[], None);
        assert_eq!(output.code, EXIT_UNSTARTED);
        let error = output.error.expect("an error");
        assert_eq!(error.code, Code::Internal);
        assert!(
            error.fallback.contains("o824-no-such-program"),
            "{}",
            error.fallback
        );
        assert!(error.params.contains_key("program"));
        assert!(error.params.contains_key("dir"));
        assert!(error.params.contains_key("error"));
    }

    #[test]
    fn the_working_directory_is_honoured() {
        let directory = o824_fixtures::TempDir::new("o824-execx-cwd-").expect("temp dir");
        let args = ["/c".to_owned(), "cd".to_owned()];
        let output = run("cmd", &args, Some(directory.path()));
        assert_eq!(
            output.code, 0,
            "stdout: {:?}, stderr: {:?}",
            output.stdout, output.stderr
        );
        // `cmd /c cd` prints exactly the child's working directory.
        assert!(
            output
                .stdout
                .to_lowercase()
                .contains(&directory.path().display().to_string().to_lowercase()),
            "stdout: {:?}, expected the child to run in {}",
            output.stdout,
            directory.path().display()
        );
    }
}
