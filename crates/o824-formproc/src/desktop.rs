//! The desktop form: starting the app the user asked for, or saying it is already there.
//!
//! §21.5's rule for this form is "start it if nothing is running, otherwise report what
//! is". Two instances of `desktop` cannot share a profile — DSH takes it exclusively — so
//! the answer to "start" when one is already up is the pid, not a second window.
//!
//! Readiness here is weaker than the web form's, and it is stated rather than faked: the
//! app prints no URL, so "started" means the process it spawned is still alive a moment
//! later. A window on screen is not something a kernel can see.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use o824_errs::{Code, Error};
use o824_state::ProcessRecord;

/// How long the app must stay alive before the start is reported as one.
pub const SETTLE: Duration = Duration::from_millis(1500);

/// A running desktop app whose image is `exe`.
///
/// The weak promise, stated rather than faked: this matches on the **image path only**. The
/// DSH home is passed to the app as an environment variable, so an instance serving a
/// different home looks exactly like this one. Reading another process's environment block
/// would need PEB access, which this tool deliberately does not do — the consequence is that
/// "start" can answer "already running" for a desktop instance that belongs to another home,
/// and that is the trade this accepts.
///
/// It exists because `state.processes` only knows what o824 itself started. A desktop the
/// user opened from the Start Menu is invisible to that record, and before this probe existed
/// the kernel spawned a second instance, watched it exit 0 (DSH is single-instance and hands
/// the request to the running one), and reported a failure for something that was fine.
pub fn running_instance(exe: &Path) -> Option<u32> {
    let want = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let candidates: Vec<u32> = o824_discovery::host_processes()
        .into_iter()
        .filter(|host| {
            let have = Path::new(&host.image);
            have.canonicalize()
                .map(|path| path == want)
                .unwrap_or(false)
        })
        .map(|host| host.pid)
        .collect();

    // The desktop is an Electron app: one launch is several processes sharing one image
    // (main, renderers, GPU, utility). Reporting whichever the enumeration happened to list
    // first would name a renderer. Electron starts its helpers after the main process, so the
    // earliest creation time is the one someone launched — and that is the pid worth showing.
    earliest(candidates, crate::started_at)
}

/// The earliest-created pid among `pids`, by creation time.
///
/// Split out so the choice can be tested without processes: a caller that passes created
/// times gets a deterministic answer, and one that cannot read them falls back to the first
/// candidate rather than guessing.
pub fn earliest(pids: Vec<u32>, created: impl Fn(u32) -> Option<u64>) -> Option<u32> {
    let mut best: Option<(u64, u32)> = None;
    let mut fallback: Option<u32> = None;
    for pid in pids {
        fallback.get_or_insert(pid);
        if let Some(when) = created(pid) {
            if best.map(|(earliest, _)| when < earliest).unwrap_or(true) {
                best = Some((when, pid));
            }
        }
    }
    best.map(|(_, pid)| pid).or(fallback)
}

/// What a desktop start produced.
#[derive(Clone, Debug)]
pub enum StartOutcome {
    /// We spawned it.
    Started(Started),
    /// Something was already running, and nothing was spawned.
    AlreadyRunning(u32),
}

/// What a successful start produced.
#[derive(Clone, Debug)]
pub struct Started {
    /// The process it spawned.
    pub pid: u32,
    /// The record to persist.
    pub record: ProcessRecord,
    /// The file its output goes to.
    pub log_path: PathBuf,
}

/// Starts the desktop app at `exe` with `home`, detached, and records what it started.
///
/// The caller writes the record from `on_spawned` as soon as the pid exists: an
/// interrupted start must leave something `stop` can find.
pub fn start(
    exe: &Path,
    args: &[String],
    home: &Path,
    data_root: &Path,
    on_spawned: &mut dyn FnMut(&ProcessRecord) -> Result<(), Error>,
) -> Result<StartOutcome, Error> {
    start_with(exe, args, home, data_root, on_spawned, &running_instance)
}

/// `start`, with the probe injected so the "already running" path can be tested without a
/// real desktop on the machine.
pub fn start_with(
    exe: &Path,
    args: &[String],
    home: &Path,
    data_root: &Path,
    on_spawned: &mut dyn FnMut(&ProcessRecord) -> Result<(), Error>,
    probe: &dyn Fn(&Path) -> Option<u32>,
) -> Result<StartOutcome, Error> {
    if !exe.is_file() {
        return Err(Error::new(
            Code::InstallDir,
            format!(
                "desktop: {:?} is not an executable",
                exe.display().to_string()
            ),
        ));
    }

    // Before the log file is created: a start that does not happen must not leave the
    // zero-byte `desktop-*.log` that made the original failure look like a crash.
    if let Some(pid) = probe(exe) {
        return Ok(StartOutcome::AlreadyRunning(pid));
    }

    let log_path = crate::log_file(data_root, "desktop")?;
    let log = std::fs::File::create(&log_path)
        .map_err(|error| crate::file_error("create the log file", &log_path, &error))?;
    let log_for_stderr = log
        .try_clone()
        .map_err(|error| crate::file_error("share the log file", &log_path, &error))?;

    // The app runs from its own directory and is told which home to use, so the desktop it
    // boots is the one the kernel is reasoning about.
    let Some(install_dir) = exe.parent() else {
        return Err(Error::new(
            Code::InstallDir,
            format!("desktop: {:?} has no directory", exe.display().to_string()),
        ));
    };
    let mut child = o824_execx::spawn_detached(
        &exe.display().to_string(),
        args,
        Some(install_dir),
        Some(&[("DSH_HOME".to_owned(), home.display().to_string())]),
        log,
        log_for_stderr,
    )?;
    let pid = child.id();

    let Some(created) = crate::started_at(pid) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::new(
            Code::Internal,
            format!("desktop: the started process (pid {pid}) has no readable creation time; it was stopped"),
        ));
    };
    let record = ProcessRecord {
        install_id: String::new(),
        pid,
        port: None,
        home: home.display().to_string(),
        image: crate::image_of(pid).unwrap_or_default(),
        created,
        started_at: o824_model::Timestamp::now().to_rfc3339_seconds(),
        log_path: log_path.display().to_string(),
    };
    if let Err(error) = on_spawned(&record) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    // "Still alive" is the only readiness this form can report, and it is worth reporting:
    // an app that dies on startup must not be recorded as running.
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(Error::new(
                Code::Internal,
                format!(
                    "desktop: the app exited ({status}) right after starting; see {}",
                    log_path.display()
                ),
            )
            .with_param("log", log_path.display().to_string()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(StartOutcome::Started(Started {
        pid,
        record,
        log_path,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A directory that removes itself.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path = std::env::temp_dir().join(format!(
                "o824-desktop-{name}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_nanos())
                    .unwrap_or_default()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("scratch");
            Scratch(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A stand-in for the desktop executable: a file, not a program, because the
    /// already-running path must not reach the point of spawning anything.
    fn fake_exe(scratch: &Scratch) -> PathBuf {
        let exe = scratch.path().join("DeepSeek Harness.exe");
        fs::write(&exe, "not a program").expect("write");
        exe
    }

    fn logs(scratch: &Scratch) -> Vec<PathBuf> {
        let directory = scratch.path().join("data").join("logs");
        fs::read_dir(directory)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn an_existing_instance_is_reported_and_nothing_is_started() {
        let scratch = Scratch::new("running");
        let exe = fake_exe(&scratch);
        let home = scratch.path().join("home");
        let data = scratch.path().join("data");
        fs::create_dir_all(&home).expect("home");

        let outcome = start_with(&exe, &[], &home, &data, &mut |_record| Ok(()), &|_| {
            Some(4242)
        })
        .expect("start");

        match outcome {
            StartOutcome::AlreadyRunning(pid) => assert_eq!(pid, 4242),
            StartOutcome::Started(_) => panic!("it should not have started anything"),
        }
        // The whole point of probing first: no log file, no record, no second window.
        assert!(
            logs(&scratch).is_empty(),
            "a start that did not happen left a log file"
        );
    }

    #[test]
    fn with_nothing_running_the_check_under_test_is_the_probe_itself() {
        let scratch = Scratch::new("none");
        let exe = fake_exe(&scratch);

        // `None` means "carry on", so the probe's answer is what decides. Asserting that the
        // probe was consulted is the part this test can hold: spawning a fake exe would be
        // asserting on Windows' error message instead of on our own behaviour.
        let consulted = std::cell::Cell::new(0);
        let probe = |_path: &Path| {
            consulted.set(consulted.get() + 1);
            None
        };
        let outcome = start_with(
            &exe,
            &[],
            &scratch.path().join("home"),
            &scratch.path().join("data"),
            &mut |_record| Ok(()),
            &probe,
        );
        assert_eq!(
            consulted.get(),
            1,
            "the probe runs once, before anything else"
        );
        // It failed at spawn (the file is not a program), which is the expected path here.
        assert!(
            outcome.is_err(),
            "a file that is not a program cannot start"
        );
    }

    #[test]
    fn a_path_that_differs_does_not_count_as_the_same_image() {
        let scratch = Scratch::new("image");
        let first = fake_exe(&scratch);
        let other = scratch.path().join("Another.exe");
        fs::write(&other, "different").expect("write");

        assert_ne!(
            first.canonicalize().expect("first"),
            other.canonicalize().expect("other")
        );
        // The comparison inside `running_instance` canonicalises both sides; this is the
        // property it relies on.
        assert!(!first.ends_with("Another.exe"));
    }

    #[test]
    fn the_earliest_process_wins() {
        // An Electron launch is several processes with one image; the main one started first.
        let created = |pid: u32| match pid {
            30 => Some(1_700_000_300),
            10 => Some(1_700_000_100),
            20 => Some(1_700_000_200),
            _ => None,
        };
        assert_eq!(earliest(vec![30, 10, 20], created), Some(10));

        // A pid whose creation time cannot be read is not discarded: if nothing is readable,
        // the first candidate is still better than "nothing is running".
        assert_eq!(earliest(vec![99, 98], |_| None), Some(99));

        // And a readable one beats an unreadable one, whichever comes first in the list.
        assert_eq!(earliest(vec![99, 10], created), Some(10));
        assert_eq!(earliest(vec![], created), None);
    }

    /// The real probe, against whatever is actually running on this machine.
    ///
    /// Ignored by default: it needs a DSH desktop to exist, and CI has none.
    #[test]
    #[ignore]
    fn the_real_probe_finds_a_running_desktop() {
        // The path comes from the running processes rather than from a guess: this machine has
        // the app under %LOCALAPPDATA%\Programs, and a hardcoded Program Files path would
        // "prove" the probe broken when it is only looking in the wrong place.
        let hosts = o824_discovery::host_processes();
        let Some(host) = hosts.first() else {
            println!("no desktop is running; nothing to probe");
            return;
        };
        let exe = PathBuf::from(&host.image);
        let found = running_instance(&exe);
        println!(
            "running_instance({}) = {found:?} (first process of that image: pid {})",
            exe.display(),
            host.pid
        );
        assert!(
            found.is_some(),
            "the app is running, so the probe must find it"
        );
    }
}
