//! Starting a process the kernel does not wait for.
//!
//! `o824 start` returns while the server it started keeps running, so the child must
//! not be tied to the caller's lifetime. On Windows that means a new process group and
//! no console window; the child's output goes to a file the kernel owns, because a
//! detached process writing to an inherited pipe would block the moment the parent
//! exits.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use o824_errs::{Code, Error};

/// Spawns `program` detached, with its output going to the two files.
///
/// The child keeps running when the caller exits: nothing here waits, and the returned
/// handle is only for the case where the caller decides to stop it immediately.
pub fn spawn_detached(
    program: &str,
    args: &[String],
    dir: Option<&Path>,
    env: Option<&[(String, String)]>,
    stdout: std::fs::File,
    stderr: std::fs::File,
) -> Result<Child, Error> {
    let mut command = Command::new(program);
    command.args(args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::from(stdout));
    command.stderr(Stdio::from(stderr));
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    if let Some(env) = env {
        for (key, value) in env {
            command.env(key, value);
        }
    }
    detach(&mut command);

    command.spawn().map_err(|error| {
        Error::new(
            Code::Internal,
            format!(
                "execx: cannot start {program:?} in {:?}: {error}",
                dir.map(|path| path.display().to_string())
                    .unwrap_or_else(|| ".".to_owned())
            ),
        )
    })
}

/// Flags that make the child outlive the caller and stay invisible.
#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
    };

    // A new group and no console: the server has no window and receives no Ctrl-C from
    // the console the caller happens to share.
    command.creation_flags((CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).0);
}

#[cfg(not(windows))]
fn detach(_command: &mut Command) {}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_detached_child_starts_and_is_not_waited_for() {
        let temp = o824_fixtures::TempDir::new("o824-detached-").expect("temp dir");
        let log = temp.path().join("out.log");
        let out = std::fs::File::create(&log).expect("create");
        let err = out.try_clone().expect("clone");

        let mut child = spawn_detached(
            "cmd",
            &["/c".to_owned(), "echo detached".to_owned()],
            Some(temp.path()),
            None,
            out,
            err,
        )
        .expect("spawn");

        // The kernel does not wait for it; the test does, only to read the log.
        let _ = child.wait();
        let text = std::fs::read_to_string(&log).expect("read log");
        assert!(text.contains("detached"), "{text}");
    }
}
