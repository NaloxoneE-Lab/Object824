//! Starting and stopping the web form of DSH.
//!
//! The web form is a global npm package booted as `dsh --profile web`. Starting it means
//! a **detached** child process — the client that asked for it may exit immediately —
//! writing to a log file this kernel owns, and a record of exactly which process it was.
//!
//! Two rules shape everything here:
//!
//! * **Readiness is the process's own word.** `dsh --profile web` prints
//!   `dsh web: http://host:port/?token=…` when it is listening. Waiting for that line is
//!   honest; waiting for a port to open would also accept whatever else grabs it.
//! * **A pid is not an identity.** The record carries the image and the process start
//!   time as well, and `stop` refuses when they do not match: pids are reused, and
//!   killing on pid alone is how a tool kills something it never started.
//!
//! The URL the form prints carries a trust token. It is returned to the caller and
//! deliberately **not** stored in the record; `url_from_log` reads it back when a
//! caller needs it, from the log this kernel already writes.

// The process queries are three Win32 calls with no standard-library equivalent; the
// raw pointers stay inside them and the rest of the crate is ordinary Rust.
#![deny(unsafe_code)]

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use o824_errs::{Code, Error};
use o824_state::ProcessRecord;

/// How long a start may take before it is considered failed.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);
/// The line `dsh --profile web` prints once it is listening.
pub const READY_MARKER: &str = "dsh web: ";

/// What a successful start produced.
#[derive(Clone, Debug)]
pub struct Started {
    /// The process it spawned.
    pub pid: u32,
    /// The port it listens on.
    pub port: u16,
    /// The URL it printed, including the trust token.
    pub url: String,
    /// The file its output goes to.
    pub log_path: PathBuf,
    /// The record to persist, with no token in it.
    pub record: ProcessRecord,
}

/// The program and arguments that boot the web form of one installation.
///
/// The `dsh` shim on PATH is a `.cmd`, which needs a shell to run; the package's own
/// entry point does not, so it is what gets spawned. That also makes the process this
/// kernel records one it can recognize again.
pub fn entry_point(package_dir: &Path) -> Option<PathBuf> {
    let manifest = std::fs::read_to_string(package_dir.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    let entry = value.get("bin")?.get("dsh")?.as_str()?;
    let path = package_dir.join(entry.replace('/', std::path::MAIN_SEPARATOR_STR));
    path.is_file().then_some(path)
}

/// Starts the web form for `package_dir` with `home`, returning once it listens.
///
/// `port` of `None` asks the operating system for a free one, which is what keeps two
/// sessions on one machine from fighting over 3000.
pub fn start(
    package_dir: &Path,
    home: &Path,
    data_root: &Path,
    port: Option<u16>,
    timeout: Duration,
) -> Result<Started, Error> {
    let Some(entry) = entry_point(package_dir) else {
        return Err(Error::new(
            Code::InstallDir,
            format!(
                "web: {:?} has no runnable entry point",
                package_dir.display().to_string()
            ),
        ));
    };

    let logs = data_root.join("logs");
    std::fs::create_dir_all(&logs)
        .map_err(|error| file_error("create the log directory", &logs, &error))?;
    let stamp = o824_model::Timestamp::now()
        .to_rfc3339_seconds()
        .replace(':', "");
    let log_path = logs.join(format!("web-{stamp}.log"));
    let log = std::fs::File::create(&log_path)
        .map_err(|error| file_error("create the log file", &log_path, &error))?;
    let log_for_stderr = log
        .try_clone()
        .map_err(|error| file_error("share the log file", &log_path, &error))?;

    let mut args = vec![
        entry.display().to_string(),
        "--profile".to_owned(),
        "web".to_owned(),
        "--port".to_owned(),
        port.unwrap_or(0).to_string(),
        "--no-open".to_owned(),
    ];
    if args[0].is_empty() {
        args.remove(0);
    }

    let mut child = o824_execx::spawn_detached(
        &node_executable(),
        &args,
        Some(home),
        Some(&[("DSH_HOME".to_owned(), home.display().to_string())]),
        log,
        log_for_stderr,
    )?;
    let pid = child.id();

    match wait_for_ready(&log_path, timeout) {
        Ok(url) => {
            let port = port_of(&url).unwrap_or(0);
            Ok(Started {
                pid,
                port,
                url: url.clone(),
                log_path: log_path.clone(),
                record: ProcessRecord {
                    install_id: String::new(),
                    pid,
                    port: Some(port),
                    home: home.display().to_string(),
                    image: image_of(pid).unwrap_or_default(),
                    created: started_at(pid).unwrap_or(0),
                    started_at: o824_model::Timestamp::now().to_rfc3339_seconds(),
                    log_path: log_path.display().to_string(),
                },
            })
        }
        Err(error) => {
            // A half-started server must not be left behind: it holds a port and a
            // profile lock, and the next attempt would fight it.
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

/// Waits for the readiness line and returns the URL it names.
fn wait_for_ready(log_path: &Path, timeout: Duration) -> Result<String, Error> {
    let deadline = Instant::now() + timeout;
    let mut offset = 0u64;
    let mut collected = String::new();
    loop {
        if let Ok(mut file) = std::fs::File::open(log_path) {
            let _ = file.seek(SeekFrom::Start(offset));
            let mut text = String::new();
            if file.read_to_string(&mut text).is_ok() {
                offset += text.len() as u64;
                collected.push_str(&text);
                if let Some(url) = ready_url(&collected) {
                    return Ok(url);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::new(
                Code::Internal,
                format!(
                    "web: the server did not report a listening port within {} seconds; it was stopped",
                    timeout.as_secs()
                ),
            )
            .with_param("log", log_path.display().to_string()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The URL from the first readiness line in `text`, when there is one.
pub fn ready_url(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix(READY_MARKER) else {
            continue;
        };
        let url = rest.trim();
        if url.starts_with("http://") || url.starts_with("https://") {
            return Some(url.to_owned());
        }
    }
    None
}

/// The URL a running instance printed, read back from its log.
pub fn url_from_log(log_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log_path).ok()?;
    ready_url(&text)
}

/// The port a URL names.
pub fn port_of(url: &str) -> Option<u16> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?;
    authority.rsplit(':').next()?.parse().ok()
}

/// The node that runs the package: the one on PATH, or this process's own.
fn node_executable() -> String {
    std::env::var("O824_NODE").unwrap_or_else(|_| "node".to_owned())
}

/// The image path of a running process, when it can be read.
#[allow(unsafe_code)]
pub fn image_of(pid: u32) -> Option<String> {
    #[cfg(windows)]
    {
        use windows::core::PWSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: the handle is closed before returning, and the buffer outlives the
        // call that writes into it.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = vec![0u16; 1024];
            let mut length = buffer.len() as u32;
            let result = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            );
            let _ = CloseHandle(handle);
            if result.is_err() {
                return None;
            }
            buffer.truncate(length as usize);
            Some(String::from_utf16_lossy(&buffer))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// The instant a process was created, as Windows reports it, or `None` when it is gone.
#[allow(unsafe_code)]
///
/// This is the other half of a pid's identity: two different processes can carry the
/// same pid on different days, and neither can carry the same creation time.
pub fn started_at(pid: u32) -> Option<u64> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, FILETIME};
        use windows::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: every handle is closed and every out-parameter is a local.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            let result = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            let _ = CloseHandle(handle);
            if result.is_err() {
                return None;
            }
            Some(((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// True when `pid` is still the process the record describes.
///
/// Both halves are required: the image path, and the creation time the record captured
/// when it started the process. The creation time is what makes this a real identity
/// check — a pid is reused, and a pid is all a naive check would compare.
pub fn is_the_recorded_process(image: &str, created: u64, pid: u32) -> bool {
    if created == 0 {
        // A record that never captured the creation time cannot prove anything.
        return false;
    }
    let Some(current) = image_of(pid) else {
        return false;
    };
    if !image.is_empty() && !current.eq_ignore_ascii_case(image) {
        return false;
    }
    started_at(pid) == Some(created)
}

/// Stops the process a record describes, if it is still that process.
#[allow(unsafe_code)]
pub fn stop(record: &ProcessRecord) -> Result<(), Error> {
    if !is_the_recorded_process(&record.image, record.created, record.pid) {
        return Err(Error::new(
            Code::StateDrift,
            format!(
                "web: pid {} is not the process this kernel started; refusing to stop it",
                record.pid
            ),
        )
        .with_param("pid", record.pid));
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

        // SAFETY: the handle is closed before returning.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, false, record.pid).map_err(|error| {
                Error::new(
                    Code::Internal,
                    format!("web: cannot open pid {}: {error}", record.pid),
                )
            })?;
            let result = TerminateProcess(handle, 0);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
            result.map_err(|error| {
                Error::new(
                    Code::Internal,
                    format!("web: cannot stop pid {}: {error}", record.pid),
                )
            })?;
        }
    }
    Ok(())
}

fn file_error(operation: &str, path: &Path, cause: &std::io::Error) -> Error {
    Error::new(
        Code::Internal,
        format!(
            "web: cannot {operation} {:?}: {cause}",
            path.display().to_string()
        ),
    )
}

/// Writes a line to a file, used by the tests' stand-in servers.
#[cfg(test)]
fn append(path: &Path, line: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open");
    writeln!(file, "{line}").expect("write");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readiness_line_is_the_one_dsh_prints() {
        let text = "starting\ndsh web: http://127.0.0.1:21846/?token=abc\nmore\n";
        assert_eq!(
            ready_url(text).as_deref(),
            Some("http://127.0.0.1:21846/?token=abc")
        );
    }

    #[test]
    fn noise_without_the_line_is_not_ready() {
        assert!(ready_url("dsh: booting\nlistening on 3000\n").is_none());
    }

    #[test]
    fn the_port_comes_out_of_the_url() {
        assert_eq!(port_of("http://127.0.0.1:21846/?token=x"), Some(21846));
        assert_eq!(port_of("https://[::1]:8443/"), Some(8443));
        assert_eq!(port_of("http://127.0.0.1/"), None);
    }

    #[test]
    fn a_record_without_its_process_is_never_matched() {
        // A pid that certainly does not exist: the image cannot be read, so the answer
        // is no, whatever the record claims.
        assert!(!is_the_recorded_process(
            r"C:\nowhere\node.exe",
            1,
            0x7fff_fffe
        ));
    }

    #[test]
    fn a_record_with_no_creation_time_proves_nothing() {
        // Our own process is certainly alive, but a record that never captured a
        // creation time cannot say which process it means, so it is refused.
        assert!(!is_the_recorded_process(
            &image_of(std::process::id()).unwrap_or_default(),
            0,
            std::process::id()
        ));
    }

    #[test]
    fn waiting_for_ready_gives_up_and_says_where_to_look() {
        let temp = o824_fixtures::TempDir::new("o824-webform-").expect("temp dir");
        let log = temp.path().join("empty.log");
        std::fs::File::create(&log).expect("create");
        let error = wait_for_ready(&log, Duration::from_millis(150)).expect_err("must time out");
        assert!(
            error.fallback.contains("did not report a listening port"),
            "{error}"
        );
    }

    #[test]
    fn the_wait_sees_a_line_that_appears_late() {
        let temp = o824_fixtures::TempDir::new("o824-webform-").expect("temp dir");
        let log = temp.path().join("late.log");
        std::fs::File::create(&log).expect("create");
        let path = log.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            append(&path, "dsh web: http://127.0.0.1:21999/?token=late");
        });
        let url = wait_for_ready(&log, Duration::from_secs(5)).expect("ready");
        assert_eq!(port_of(&url), Some(21999));
    }

    /// Starts the real web form, when this machine has one.
    ///
    /// Skipping rather than failing: CI has no DSH installed, and a test that demands
    /// one would be a test nobody can run.
    #[test]
    fn the_real_web_form_starts_in_a_temporary_home() {
        let Some(package) = crate::tests::installed_package() else {
            eprintln!("skipping: no global @deepseek-ai/dsh on this machine");
            return;
        };
        let temp = o824_fixtures::TempDir::new("o824-webform-start-").expect("temp dir");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).expect("mkdir");

        let started = start(&package, &home, temp.path(), None, Duration::from_secs(60))
            .expect("the web form must start");
        assert!(started.port > 0, "a port was chosen: {started:?}");
        assert!(started.url.contains(&started.port.to_string()));

        // The record is what a later stop relies on, and it must not carry the token.
        assert!(!started.record.log_path.contains("token"));
        assert_eq!(started.record.pid, started.pid);
        assert!(
            !started.record.image.is_empty(),
            "the image is part of the identity"
        );

        // It is the process the record names, by image and by creation time.
        assert!(
            started.record.created > 0,
            "the record captured a creation time"
        );
        assert!(is_the_recorded_process(
            &started.record.image,
            started.record.created,
            started.pid
        ));

        stop(&started.record).expect("stop it again");
        std::thread::sleep(Duration::from_millis(500));
        assert!(image_of(started.pid).is_none(), "the process is gone");
    }

    /// The global package directory, when this machine has the web form installed.
    fn installed_package() -> Option<std::path::PathBuf> {
        for prefix in o824_discovery::global_prefixes() {
            if let Some(package) = o824_discovery::package_in_prefix(&prefix) {
                return Some(package);
            }
        }
        None
    }
}
