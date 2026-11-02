//! The timed runner: a child that must finish, or be killed.
//!
//! `o824-execx`'s [`run_with_env`](crate::run_with_env) waits as long as the child
//! does, which is right for a short command and wrong for `pnpm install`: a
//! registry that stops answering would otherwise hang a repair forever. This file
//! adds a deadline without changing the untimed path.
//!
//! The child's output is drained on two threads while the deadline is watched. A
//! single thread that waited and then read would deadlock the moment the child
//! filled a pipe buffer, and a large install log does fill one.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use o824_errs::{Code, Error};

use crate::{suppress_console_window, EXIT_UNSTARTED};

/// What a timed run produced.
pub struct TimedOutput {
    /// What the child wrote to stdout.
    pub stdout: String,
    /// What the child wrote to stderr.
    pub stderr: String,
    /// The exit status, or [`EXIT_UNSTARTED`] when there is none.
    pub code: i32,
    /// Set when the process could not start, or was killed at the deadline.
    pub error: Option<Error>,
}

/// Runs `program` with `args` and a deadline.
///
/// A child that is still running at the deadline is killed and reported as an
/// error, not as an exit status: the caller asked for it to finish, and it did
/// not. Everything the child wrote before that is still returned.
pub fn run_with_deadline(
    program: &str,
    args: &[String],
    env: Option<&[(String, String)]>,
    dir: Option<&std::path::Path>,
    deadline: Duration,
) -> TimedOutput {
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
    suppress_console_window(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return TimedOutput {
                stdout: String::new(),
                stderr: String::new(),
                code: EXIT_UNSTARTED,
                error: Some(Error::new(
                    Code::Internal,
                    format!(
                        "execx: cannot run {:?} in {:?}: {error}",
                        program,
                        dir.map(|path| path.display().to_string())
                            .unwrap_or_else(|| ".".to_owned())
                    ),
                )),
            };
        }
    };

    let (sender, receiver) = mpsc::channel::<(bool, Vec<u8>)>();
    if let Some(mut stdout) = child.stdout.take() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = stdout.read_to_end(&mut buffer);
            let _ = sender.send((true, buffer));
        });
    }
    if let Some(mut stderr) = child.stderr.take() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = stderr.read_to_end(&mut buffer);
            let _ = sender.send((false, buffer));
        });
    }
    drop(sender);

    let started = Instant::now();
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return TimedOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    code: EXIT_UNSTARTED,
                    error: Some(Error::new(
                        Code::Internal,
                        format!("execx: cannot wait for {program:?}: {error}"),
                    )),
                };
            }
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    for _ in 0..2 {
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok((true, buffer)) => stdout = buffer,
            Ok((false, buffer)) => stderr = buffer,
            Err(_) => break,
        }
    }

    if timed_out {
        return TimedOutput {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            code: EXIT_UNSTARTED,
            error: Some(Error::new(
                Code::Internal,
                format!(
                    "execx: {program:?} did not finish within {} seconds and was killed",
                    deadline.as_secs()
                ),
            )),
        };
    }

    TimedOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        code: status
            .and_then(|status| status.code())
            .unwrap_or(EXIT_UNSTARTED),
        error: None,
    }
}
