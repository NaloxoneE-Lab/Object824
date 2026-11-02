//! `o824 status | stop | logs`, and the guards that keep `start` from hurting anything.
//!
//! The safety properties are what these tests are for: a stop that would kill a process
//! the kernel did not start must refuse, and a start against a home that already has a
//! live instance must refuse too. Both are checked with records pointing at processes
//! the test controls, so neither needs DSH installed.

use std::io::Write;

use o824_errs::Code;
use o824_state::{ProcessRecord, State};

fn run(
    command: fn(&[String], &mut dyn Write, &mut dyn std::io::Write) -> i32,
    args: &[&str],
) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    // The signature takes two writers; both are Vec<u8> here.
    let out: &mut dyn Write = &mut stdout;
    let err: &mut dyn Write = &mut stderr;
    let code = command(&args, out, err);
    (
        code,
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

/// A stand-in process that keeps running, so identity checks have something to check.
///
/// The first version of this helper ran the test binary with `--list`, which exits
/// immediately: the tests asserted that a process survived a refusal, and on CI there
/// was no process left to survive.
fn spawn_stand_in() -> std::process::Child {
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("ping");
        command.args(["-n", "60", "127.0.0.1"]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = std::process::Command::new("sleep");
        command.arg("60");
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a stand-in")
}

fn record_for(install_id: &str, pid: u32, image: &str, home: &str) -> ProcessRecord {
    ProcessRecord {
        install_id: install_id.to_owned(),
        pid,
        port: Some(12345),
        home: home.to_owned(),
        image: image.to_owned(),
        created: o824_webform::started_at(pid).unwrap_or(0),
        started_at: "2026-11-03T00:00:00Z".to_owned(),
        log_path: String::new(),
    }
}

fn state_at(dir: &std::path::Path, record: Option<ProcessRecord>) -> std::path::PathBuf {
    let path = dir.join(o824_state::STATE_FILE_NAME);
    let mut state = State::default();
    if let Some(record) = record {
        state.remember_process(record);
    }
    o824_state::save(&path, &state).expect("save");
    path
}

#[test]
fn status_on_a_fresh_kernel_says_nothing_is_running() {
    let temp = o824_fixtures::TempDir::new("o824-status-").expect("temp dir");
    state_at(temp.path(), None);
    let root = temp.path().display().to_string();
    let (code, stdout, stderr) = run(crate::process_cmd::status, &["--data-root", &root]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("nothing started"), "{stdout}");
}

#[test]
fn stop_of_something_never_started_says_so_and_changes_nothing() {
    let temp = o824_fixtures::TempDir::new("o824-stop-").expect("temp dir");
    state_at(temp.path(), None);
    let root = temp.path().display().to_string();
    let home = temp.path().join("home").display().to_string();
    let (code, stdout, stderr) = run(
        crate::process_cmd::stop,
        &["--install", "web", "--home", &home, "--data-root", &root],
    );
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("is not running"), "{stdout}");
}

/// The important one: a record whose pid now belongs to another process must not be
/// killed. Pids are reused, and a kernel that trusts the number would kill a stranger.
#[test]
fn stop_refuses_a_pid_that_is_no_longer_the_recorded_process() {
    let temp = o824_fixtures::TempDir::new("o824-stop-").expect("temp dir");
    let mut stand_in = spawn_stand_in();
    let (pid, image) = (stand_in.id(), image_of_stand_in(&mut stand_in));

    // The image in the record does not match what is running: the same pid, a different
    // process as far as identity is concerned.
    let record = record_for("web", pid, r"C:\somewhere\else\node.exe", "D:\\home");
    state_at(temp.path(), Some(record));
    let root = temp.path().display().to_string();
    let home = temp.path().join("home").display().to_string();

    let (code, _stdout, stderr) = run(
        crate::process_cmd::stop,
        &["--install", "web", "--home", &home, "--data-root", &root],
    );

    let alive = stand_in.try_wait().expect("try_wait").is_none();
    let _ = stand_in.kill();
    let _ = stand_in.wait();

    assert_eq!(
        code,
        crate::exit_code_of(Some(&o824_errs::Error::new(Code::StateDrift, "x"))),
        "{stderr}"
    );
    assert!(
        alive,
        "the process the record did not describe must survive"
    );
    assert!(stderr.contains("refusing to stop"), "{stderr}");
    assert!(!image.is_empty());
}

/// A live record for the same home blocks a second start before anything is spawned,
/// which is what §21.4 asks for: two forms on one home is a hazard, not a convenience.
#[test]
fn start_refuses_when_the_home_already_has_a_live_instance() {
    let temp = o824_fixtures::TempDir::new("o824-start-").expect("temp dir");
    let mut stand_in = spawn_stand_in();
    let image = image_of_stand_in(&mut stand_in);
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("mkdir");

    let record = record_for("web", stand_in.id(), &image, &home.display().to_string());
    state_at(temp.path(), Some(record));

    let root = temp.path().display().to_string();
    let home_text = home.display().to_string();
    let (code, _stdout, stderr) = run(
        crate::process_cmd::start,
        &[
            "--install",
            "web",
            "--home",
            &home_text,
            "--data-root",
            &root,
        ],
    );

    let _ = stand_in.kill();
    let _ = stand_in.wait();

    assert_eq!(
        code,
        crate::exit_code_of(Some(&o824_errs::Error::new(Code::Busy, "x"))),
        "{stderr}"
    );
    assert!(stderr.contains("already running"), "{stderr}");
}

#[test]
fn logs_prints_the_tail_of_the_recorded_file() {
    let temp = o824_fixtures::TempDir::new("o824-logs-").expect("temp dir");
    let log = temp.path().join("web.log");
    std::fs::write(&log, "one\ntwo\nthree\nfour\n").expect("write log");
    let mut record = record_for("web", 1, r"C:\x\node.exe", "D:\\home");
    record.log_path = log.display().to_string();
    record.created = 1;
    state_at(temp.path(), Some(record));

    let root = temp.path().display().to_string();
    let (code, stdout, stderr) = run(
        crate::process_cmd::logs,
        &["--install", "web", "--tail", "2", "--data-root", &root],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "three\nfour\n");
}

/// The image of the spawned stand-in, read after it is surely running.
fn image_of_stand_in(child: &mut std::process::Child) -> String {
    for _ in 0..50 {
        if let Some(image) = o824_webform::image_of(child.id()) {
            if !image.is_empty() {
                return image;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the stand-in never became visible");
}
