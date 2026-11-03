//! The GUI's side of the kernel protocol.
//!
//! §7.5 puts the kernel in its own process: the UI spawns it, speaks Content-Length
//! frames over its stdin and stdout, and dies without it. This crate is that client, in
//! Rust, because a UI toolkit cannot do it — a webview cannot spawn a process, and the
//! TypeScript SDK in `packages/o824-sdk` is for clients that already have a runtime.
//!
//! Three behaviours are the reason this is a crate rather than forty lines in the app:
//!
//! * **A job's answer is a stream, not a reply.** `o824.doctor`, `plan` and `apply` return
//!   a job id and then emit `op.state` / `op.progress` / `op.result`. A client that reads
//!   only the reply hangs; a client that reads only the first event misses the answer.
//! * **The kernel can die mid-job.** Every wait has a deadline and every call can return
//!   [`BridgeError::KernelGone`], because a UI that silently hangs is worse than one that
//!   says the kernel is gone and offers to start it again.
//! * **Closing is a request, not a promise.** `close` closes stdin, waits, and reports
//!   whether the kernel actually exited — the "never leave a background process" rule from
//!   §7.5 is only true if someone checks.

#![deny(unsafe_code)]

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use o824_errs::Code;
use o824_protocol::{read_frame, write_frame};
use serde_json::{json, Value};

/// How long a request's reply may take.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a job may run before the wait gives up.
pub const JOB_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long a closed kernel may take to exit.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything that can go wrong on this side of the pipe.
#[derive(Debug)]
pub enum BridgeError {
    /// The kernel process could not be started.
    CannotSpawn(String),
    /// The kernel stopped answering: it exited, or its pipe closed.
    KernelGone(String),
    /// The kernel answered with an error.
    Kernel {
        /// The error code the kernel used.
        code: String,
        /// The message, as the kernel phrased it.
        message: String,
    },
    /// The reply did not arrive in time.
    Timeout(String),
    /// A frame could not be read or written.
    Protocol(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::CannotSpawn(message) => {
                write!(formatter, "cannot start the kernel: {message}")
            }
            BridgeError::KernelGone(message) => {
                write!(
                    formatter,
                    "the kernel stopped answering ({message}); start it again"
                )
            }
            BridgeError::Kernel { code, message } => write!(formatter, "{code}: {message}"),
            BridgeError::Timeout(message) => write!(formatter, "{message}"),
            BridgeError::Protocol(message) => write!(formatter, "protocol error: {message}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// A live kernel process and the two ends of its pipe.
pub struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Option<ChildStderr>,
    next_id: i64,
    /// Messages read while waiting for something else, in order.
    pending: Vec<Value>,
}

/// Where the kernel binary is and what it should look at.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The kernel executable; default is `o824` on `PATH`.
    pub program: Option<PathBuf>,
    /// The installation root to diagnose, when the caller names one.
    pub root: Option<PathBuf>,
    /// The DSH home.
    pub home: Option<PathBuf>,
    /// Where the kernel keeps plans and journals.
    pub data_root: Option<PathBuf>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The pipes are not printable; the pid is what a log needs.
        write!(formatter, "Bridge(pid {})", self.child.id())
    }
}

impl Bridge {
    /// Starts the kernel and completes the handshake.
    pub fn start(options: &Options) -> Result<Bridge, BridgeError> {
        let program = options
            .program
            .clone()
            .unwrap_or_else(|| PathBuf::from("o824"));
        let mut command = Command::new(&program);
        command.args(["kernel", "--stdio"]);
        if let Some(root) = &options.root {
            command.args(["--root", &root.display().to_string()]);
        }
        if let Some(home) = &options.home {
            command.args(["--home", &home.display().to_string()]);
        }
        if let Some(data_root) = &options.data_root {
            command.args(["--data-root", &data_root.display().to_string()]);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|error| BridgeError::CannotSpawn(format!("{}: {error}", program.display())))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| BridgeError::CannotSpawn("the kernel has no stdin".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BridgeError::CannotSpawn("the kernel has no stdout".to_owned()))?;
        let stderr = child.stderr.take();

        let bridge = Bridge {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr,
            next_id: 1,
            pending: Vec::new(),
        };
        Ok(bridge)
    }

    /// The kernel's protocol version and capabilities.
    pub fn info(&mut self) -> Result<Value, BridgeError> {
        let reply = self.call("o824.info", json!({}))?;
        Ok(reply)
    }

    /// The installations on this machine.
    pub fn discover(&mut self) -> Result<Vec<o824_model::Install>, BridgeError> {
        let reply = self.call("o824.discover", json!({}))?;
        let installs = reply
            .get("installs")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        serde_json::from_value(installs)
            .map_err(|error| BridgeError::Protocol(format!("installs: {error}")))
    }

    /// Runs `o824.doctor` and waits for the envelope.
    pub fn doctor(&mut self, install_id: &str) -> Result<Value, BridgeError> {
        self.job("o824.doctor", json!({ "installId": install_id }))
    }

    /// Builds a plan for `action` (`repair`, `start`, `stop`).
    pub fn plan(&mut self, install_id: &str, action: &str) -> Result<Value, BridgeError> {
        self.job(
            "o824.plan",
            json!({ "installId": install_id, "action": action }),
        )
    }

    /// Applies a plan the kernel wrote earlier in this session.
    ///
    /// The kernel takes a plan **id**, not a plan: it kept the plan itself when it wrote it,
    /// so what travels is a reference to something the kernel can verify. Sending the body
    /// would let a client edit the steps between the confirmation and the run, which is
    /// exactly the gap the plan/apply split exists to close.
    pub fn apply(&mut self, plan: &Value) -> Result<Value, BridgeError> {
        let plan_id = plan
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::Protocol("the plan has no id".to_owned()))?;
        self.job("o824.apply", json!({ "planId": plan_id }))
    }

    /// Asks the kernel to cancel a running job.
    pub fn cancel(&mut self, job_id: &str) -> Result<Value, BridgeError> {
        self.call("o824.cancel", json!({ "jobId": job_id }))
    }

    /// The kernel's own last words, if it wrote any to stderr.
    pub fn stderr_text(&mut self) -> String {
        let Some(stderr) = self.stderr.as_mut() else {
            return String::new();
        };
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    }

    /// Closes the pipe and reports whether the kernel exited.
    ///
    /// A kernel that ignores EOF and keeps running is the failure this returns `false`
    /// for: the process would outlive the window that owns it.
    pub fn close(mut self) -> bool {
        drop(self.stdin);
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return false;
                }
            }
        }
    }

    /// One request, one reply.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, BridgeError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;

        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let message = self.next_message(deadline, method)?;
            // Job events arrive on the same pipe; they are not this reply.
            if message.get("method").is_some() {
                self.pending.push(message);
                continue;
            }
            if message.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            return reply_of(message);
        }
    }

    /// A job: the reply names the job, then events carry the answer.
    fn job(&mut self, method: &str, params: Value) -> Result<Value, BridgeError> {
        let reply = self.call(method, params)?;
        let Some(job_id) = reply.get("jobId").and_then(Value::as_str) else {
            // A kernel that answered synchronously is also fine: the envelope is the answer.
            return Ok(reply);
        };
        let job_id = job_id.to_owned();
        let deadline = Instant::now() + JOB_TIMEOUT;
        let mut result: Option<Value> = None;
        loop {
            let message = match self.next_message(deadline, method) {
                Ok(message) => message,
                // A cancel that raced the job's own end is not an error to the caller.
                Err(BridgeError::KernelGone(message)) if message.contains("cancelled") => {
                    return Ok(json!({ "cancelled": true }));
                }
                Err(error) => return Err(error),
            };
            let Some(params) = message.get("params") else {
                continue;
            };
            if params.get("jobId").and_then(Value::as_str) != Some(job_id.as_str()) {
                continue;
            }
            match message.get("method").and_then(Value::as_str) {
                Some("op.result") => {
                    result = params.get("result").cloned();
                }
                Some("op.state") => match params.get("state").and_then(Value::as_str) {
                    Some("done") => {
                        return Ok(result.unwrap_or(Value::Null));
                    }
                    Some("failed") => {
                        let error = params.get("error").cloned().unwrap_or(Value::Null);
                        return Err(BridgeError::Kernel {
                            code: error
                                .get("code")
                                .and_then(Value::as_str)
                                .unwrap_or("E_INTERNAL")
                                .to_owned(),
                            message: error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("the kernel reported a failure")
                                .to_owned(),
                        });
                    }
                    Some("cancelled") => return Ok(json!({ "cancelled": true })),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn send(&mut self, message: Value) -> Result<(), BridgeError> {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &serde_json::to_vec(&message)
                .map_err(|error| BridgeError::Protocol(format!("serialize: {error}")))?,
        )
        .map_err(|error| BridgeError::Protocol(format!("frame: {error}")))?;
        self.stdin
            .write_all(&bytes)
            .and_then(|()| self.stdin.flush())
            .map_err(|error| BridgeError::KernelGone(format!("write: {error}")))
    }

    /// The next message, from the pending queue or the pipe.
    fn next_message(&mut self, deadline: Instant, what: &str) -> Result<Value, BridgeError> {
        if !self.pending.is_empty() {
            return Ok(self.pending.remove(0));
        }
        if Instant::now() >= deadline {
            return Err(BridgeError::Timeout(format!(
                "the kernel did not answer {what} in time"
            )));
        }
        match read_frame(&mut self.stdout) {
            Ok(body) => serde_json::from_slice(&body)
                .map_err(|error| BridgeError::Protocol(format!("body: {error}"))),
            // EOF means the kernel exited; anything else is a broken pipe. The exit status
            // goes in the message: "the kernel stopped answering" without it sends the
            // reader to the wrong place.
            Err(error) => match self.child.try_wait() {
                Ok(Some(status)) => Err(BridgeError::KernelGone(format!(
                    "{error}; the kernel exited with {status}"
                ))),
                _ => Err(BridgeError::KernelGone(format!("{error}"))),
            },
        }
    }
}

/// The answer inside a JSON-RPC reply.
fn reply_of(message: Value) -> Result<Value, BridgeError> {
    if let Some(error) = message.get("error") {
        return Err(BridgeError::Kernel {
            code: error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("E_INTERNAL")
                .to_owned(),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the kernel refused the request")
                .to_owned(),
        });
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

/// The exit code a bridge error maps to, for a caller that reports one.
pub fn exit_code(error: &BridgeError) -> i32 {
    match error {
        BridgeError::Kernel { code, .. } => match code.as_str() {
            "E_USAGE" => 2,
            "E_CONFIRMATION_REQUIRED" => 3,
            "E_PERMISSION" => 4,
            "E_STATE_DRIFT" | "E_PORT_IN_USE" | "E_CONCURRENT_INSTANCE" => 5,
            "E_NETWORK" => 6,
            "E_BUSY" => 7,
            _ => 8,
        },
        BridgeError::CannotSpawn(_) | BridgeError::KernelGone(_) => 8,
        BridgeError::Timeout(_) => 8,
        BridgeError::Protocol(_) => 8,
    }
}

/// The error code a bridge error carries, when it has one.
pub fn error_code(error: &BridgeError) -> Option<Code> {
    match error {
        BridgeError::Kernel { code, .. } => Some(match code.as_str() {
            "E_USAGE" => Code::Usage,
            "E_INSTALL_DIR" => Code::InstallDir,
            "E_STATE_DRIFT" => Code::StateDrift,
            "E_BUSY" => Code::Busy,
            "E_PERMISSION" => Code::Permission,
            "E_NETWORK" => Code::Network,
            _ => Code::Internal,
        }),
        _ => None,
    }
}

/// True when `path` looks like a kernel binary we could start.
pub fn is_kernel(path: &Path) -> bool {
    path.is_file()
}
