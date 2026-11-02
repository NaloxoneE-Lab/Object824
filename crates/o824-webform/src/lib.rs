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
    /// The port it listens on, when the URL it printed names one.
    ///
    /// `None` is honest: `--public-url` makes the server advertise a URL without a port,
    /// and a made-up `0` would make every later port question answer about port 0.
    pub port: Option<u16>,
    /// The URL it printed, including the trust token.
    pub url: String,
    /// The file its output goes to.
    pub log_path: PathBuf,
    /// The record to persist, with no token in it.
    pub record: ProcessRecord,
}

/// Who is listening on a port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortOwner {
    /// The port, in host byte order.
    pub port: u16,
    /// The owning process.
    pub pid: u32,
    /// The image that process runs, when it can be read.
    pub image: String,
}

/// The process listening on `port`, if one is.
///
/// This is the answer to "the web form was killed but the port is still held": the
/// kernel refuses to guess, and naming the pid is what lets a person decide whether it
/// is the process they just stopped or something they care about.
pub fn port_owner(port: u16) -> Option<PortOwner> {
    #[cfg(windows)]
    {
        let rows = listening_rows()?;
        let (pid, image) = rows
            .into_iter()
            .find(|(local_port, _, _)| *local_port == port)
            .map(|(_, pid, image)| (pid, image))?;
        Some(PortOwner {
            port,
            pid,
            image: image.unwrap_or_default(),
        })
    }
    #[cfg(not(windows))]
    {
        let _ = port;
        None
    }
}

/// True when nothing is listening on `port`.
pub fn port_is_free(port: u16) -> bool {
    port_owner(port).is_none()
}

/// Waits until `port` has no listener, and says whether it got there.
pub fn wait_for_port_release(port: u16, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if port_is_free(port) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Every TCP listener, as `(port, pid, image)`.
#[cfg(windows)]
#[allow(unsafe_code)]
fn listening_rows() -> Option<Vec<(u16, u32, Option<String>)>> {
    use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    // Both families: a listener on [::1]:N has no row in the IPv4 table, and DSH's
    // --host explicitly accepts an IPv6 literal, so reading only AF_INET would call a
    // busy port free.
    let mut listeners = Vec::new();
    for family in [AF_INET.0 as u32, AF_INET6.0 as u32] {
        // SAFETY: the table is sized by a first call, allocated here, and only read as
        // far as the second call says it wrote. Every pointer is a local.
        unsafe {
            let mut size = 0u32;
            let first = GetExtendedTcpTable(
                None,
                &mut size,
                false,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
            if first != ERROR_INSUFFICIENT_BUFFER.0 && first != NO_ERROR.0 {
                continue;
            }
            let mut buffer = vec![0u8; size as usize];
            let status = GetExtendedTcpTable(
                Some(buffer.as_mut_ptr() as *mut _),
                &mut size,
                false,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
            if status != NO_ERROR.0 {
                continue;
            }

            // The table starts with an entry count, then that many rows of the same
            // shape for either family.
            let count = *(buffer.as_ptr() as *const u32) as usize;
            let rows = buffer.as_ptr().add(4) as *const MIB_TCPROW_OWNER_PID;
            for index in 0..count {
                let row = &*rows.add(index);
                // The port is stored in network byte order.
                let port = u16::from_be(row.dwLocalPort as u16);
                let pid = row.dwOwningPid;
                listeners.push((port, pid, image_of(pid)));
            }
        }
    }
    Some(listeners)
}

#[cfg(not(windows))]
fn listening_rows() -> Option<Vec<(u16, u32, Option<String>)>> {
    None
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
    on_spawned: &mut dyn FnMut(&ProcessRecord) -> Result<(), Error>,
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

    // The record is written as soon as the pid exists, not after readiness: an
    // interrupted start must leave something `stop` can find, or it leaves a server
    // nobody can stop.
    let Some(created) = started_at(pid) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::new(
            Code::Internal,
            format!("web: the started process (pid {pid}) has no readable creation time; it was stopped"),
        ));
    };
    let provisional = ProcessRecord {
        install_id: String::new(),
        pid,
        port: None,
        home: home.display().to_string(),
        image: image_of(pid).unwrap_or_default(),
        created,
        started_at: o824_model::Timestamp::now().to_rfc3339_seconds(),
        log_path: log_path.display().to_string(),
    };
    if let Err(error) = on_spawned(&provisional) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    match wait_for_ready(&log_path, timeout, Some(&mut child)) {
        Ok(url) => {
            let port = port_of(&url);
            Ok(Started {
                pid,
                port,
                url: url.clone(),
                log_path: log_path.clone(),
                record: ProcessRecord {
                    port,
                    ..provisional
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
fn wait_for_ready(
    log_path: &Path,
    timeout: Duration,
    mut child: Option<&mut std::process::Child>,
) -> Result<String, Error> {
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
        if let Some(child) = child.as_deref_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                // The server died before it could say a port. Saying so at once, with
                // its own log, is the difference between a diagnosable failure and half
                // a minute of nothing.
                return Err(Error::new(
                    Code::Internal,
                    format!(
                        "web: the server exited ({status}) before reporting a listening port; see {}",
                        log_path.display()
                    ),
                )
                .with_param("log", log_path.display().to_string()));
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::new(
                Code::Internal,
                format!(
                    "web: the server did not report a listening port within {} seconds; it was stopped; see {}",
                    timeout.as_secs(),
                    log_path.display()
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
        use windows::Win32::System::Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        };

        // SAFETY: the handle is closed on every path below.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, record.pid)
                .map_err(|error| {
                    Error::new(
                        Code::Internal,
                        format!("web: cannot open pid {}: {error}", record.pid),
                    )
                })?;
            let terminated = TerminateProcess(handle, 0);
            // The process object outlives the process while any handle is open — the
            // caller's own `Child` is one — so "the creation time still reads" is not
            // the question. Waiting on the handle is what answers "is it gone".
            let waited = WaitForSingleObject(handle, 10_000);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
            terminated.map_err(|error| {
                Error::new(
                    Code::Internal,
                    format!("web: cannot stop pid {}: {error}", record.pid),
                )
            })?;
            if waited != windows::Win32::Foundation::WAIT_OBJECT_0 {
                return Err(Error::new(
                    Code::Internal,
                    format!(
                        "web: pid {} was asked to stop but is still running after 10 seconds",
                        record.pid
                    ),
                )
                .with_param("pid", record.pid));
            }
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
    fn the_process_listening_on_a_port_is_named() {
        // Our own listener: no dependency on DSH or node, and the pid is certainly the
        // one that owns it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let owner = port_owner(port).expect("a listener is there");
        assert_eq!(owner.pid, std::process::id());
        assert!(
            !owner.image.is_empty(),
            "the image is what makes it actionable"
        );
        drop(listener);
    }

    #[test]
    fn a_port_with_nothing_on_it_is_free() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        // The kernel may hold a TIME_WAIT, but nothing is *listening*.
        assert!(port_is_free(port));
    }

    #[test]
    fn waiting_for_a_release_sees_it_happen() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(listener);
        });
        assert!(wait_for_port_release(port, Duration::from_secs(5)));
    }

    #[test]
    fn waiting_for_a_release_that_never_comes_gives_up() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert!(!wait_for_port_release(port, Duration::from_millis(300)));
        drop(listener);
    }

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
        let error =
            wait_for_ready(&log, Duration::from_millis(150), None).expect_err("must time out");
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
        let url = wait_for_ready(&log, Duration::from_secs(5), None).expect("ready");
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

        let started = start(
            &package,
            &home,
            temp.path(),
            None,
            Duration::from_secs(60),
            &mut |_record: &ProcessRecord| Ok(()),
        )
        .expect("the web form must start");
        let port = started.port.expect("the default start prints a port");
        assert!(port > 0, "a real port was chosen: {started:?}");
        assert!(started.url.contains(&port.to_string()));

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
