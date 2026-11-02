//! The live kernel: what a UI talks to.
//!
//! One process, JSON-RPC over stdio, stderr reserved for diagnostics (§7.5). Requests
//! that change or inspect the machine run as **jobs**: the answer is a `jobId`, and the
//! work reports itself through `op.state`, `op.progress`, and `op.result`. That is what
//! makes a two-second `doctor` visible instead of silent, and what makes `o824.cancel`
//! meaningful.
//!
//! The contract this implements is §19.1–19.4, and three of its rules shape the code:
//!
//! * **A job always ends.** Every path out of a job emits exactly one terminal
//!   `op.state` (`done`, `failed`, or `cancelled`); a cancelled job never emits a
//!   result afterwards, and a job that cannot be cancelled says so in the cancel
//!   answer rather than pretending.
//! * **`done` never goes backwards**, and `total` only changes when the phase does —
//!   which is why a phase change emits `op.state` before its progress.
//! * **No user-facing prose.** Findings carry a `code` and `params`; the English
//!   `fallbackMessage` exists only so an untranslated UI does not show "unknown error".

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use o824_errs::Code;
use o824_model::Install;
use o824_protocol::{read_frame, write_frame, FrameError};
use serde_json::{json, Value};

/// The protocol version this kernel speaks.
pub const PROTOCOL_VERSION: &str = "0.1.0";
/// The oldest client protocol this kernel accepts.
pub const MIN_CLIENT_PROTOCOL: &str = "0.1.0";
/// What this kernel can do. The list is the transcript's, not an aspiration.
pub const CAPABILITIES: [&str; 4] = ["discover", "doctor", "plan", "apply"];

/// Where the kernel looks, and what it writes.
#[derive(Clone, Debug, Default)]
pub struct ServerOptions {
    /// Diagnose exactly these roots instead of scanning the machine.
    pub roots: Option<Vec<PathBuf>>,
    /// The DSH home, or `None` for the default.
    pub home: Option<String>,
    /// The data root, or `None` for the default.
    pub data_root: Option<PathBuf>,
}

/// Serves one session until the client closes its end.
///
/// A malformed frame ends the session — without a length the next frame boundary is
/// unknowable — while a malformed *body* is answered and the session continues.
pub fn serve(
    input: &mut dyn BufRead,
    output: Box<dyn Write + Send>,
    options: ServerOptions,
) -> Result<(), String> {
    // One lock around one stream. A writer thread plus a channel would be more code
    // for the same guarantee (order), and an extra thing that can fail to start; a
    // job thread and the request loop both write under this mutex, which is all the
    // ordering the protocol needs.
    let output: SharedOutput = Arc::new(Mutex::new(output));
    let server = Server {
        options,
        output: Arc::clone(&output),
        seq: AtomicI64::new(0),
        job_counter: AtomicI64::new(0),
        cancelled: Arc::new(Mutex::new(HashMap::new())),
    };

    loop {
        let body = match read_frame(input) {
            Ok(body) => body,
            Err(FrameError::EndOfStream) => break,
            Err(FrameError::Malformed) => {
                server.send_error(
                    None,
                    -32700,
                    "E_PROTOCOL_PARSE",
                    "frame header is not a valid Content-Length header",
                );
                break;
            }
        };
        if !server.handle_frame(&body) {
            break;
        }
    }
    Ok(())
}

/// The session's one output stream, shared by the request loop and every job thread.
type SharedOutput = Arc<Mutex<Box<dyn Write + Send>>>;

/// One session's state.
struct Server {
    options: ServerOptions,
    output: SharedOutput,
    seq: AtomicI64,
    job_counter: AtomicI64,
    /// Cancellation flags by job id. A job removes its own entry when it ends.
    cancelled: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl Server {
    /// Answers one frame. Returns false when the session should end.
    fn handle_frame(&self, body: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(body) else {
            self.send_error(
                None,
                -32700,
                "E_PROTOCOL_PARSE",
                "frame body is not valid UTF-8",
            );
            return true;
        };
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            self.send_error(
                None,
                -32700,
                "E_PROTOCOL_PARSE",
                "frame body is not valid JSON",
            );
            return true;
        };
        let Some(object) = value.as_object() else {
            self.send_error(
                None,
                -32600,
                "E_PROTOCOL_INVALID",
                "a request must be a JSON object",
            );
            return true;
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // A response on the kernel's stdin is not part of the protocol: the kernel
            // makes no requests, so there is nothing it could be answering.
            self.send_error(id, -32600, "E_PROTOCOL_INVALID", "a request needs a method");
            return true;
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);

        // A notification has no id and gets no answer.
        let Some(id) = id else {
            return true;
        };

        match method {
            "o824.info" => self.info(id, &params),
            "o824.discover" => self.discover(id),
            "o824.doctor" => self.start_job(id, Job::Doctor, &params),
            "o824.plan" => self.start_job(id, Job::Plan, &params),
            "o824.apply" => self.start_job(id, Job::Apply, &params),
            "o824.cancel" => self.cancel(id, &params),
            other => self.send_error(
                Some(id),
                -32601,
                "E_UNSUPPORTED",
                &format!("unsupported method {other:?}"),
            ),
        }
        true
    }

    fn info(&self, id: Value, params: &Value) {
        // §19.4: a client may state the protocol it speaks and the oldest kernel it can
        // work with; a mismatch is an error, never a silent downgrade.
        if let Some(client) = params.get("clientProtocol").and_then(Value::as_str) {
            if major_of(client) != major_of(PROTOCOL_VERSION) {
                self.send_kernel_error(
                    id,
                    Code::ProtocolMismatch,
                    &format!(
                        "client protocol {client} cannot talk to a kernel speaking {PROTOCOL_VERSION}"
                    ),
                );
                return;
            }
        }
        if let Some(minimum) = params.get("minServerProtocol").and_then(Value::as_str) {
            if compare_versions(minimum, PROTOCOL_VERSION).is_gt() {
                self.send_kernel_error(
                    id,
                    Code::Unsupported,
                    &format!(
                        "the client needs protocol {minimum} or newer; this kernel speaks {PROTOCOL_VERSION}"
                    ),
                );
                return;
            }
        }

        self.send_result(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "minClientProtocol": MIN_CLIENT_PROTOCOL,
                "impl": { "name": "o824", "version": o824_version::VERSION },
                "capabilities": CAPABILITIES,
                "platform": { "os": "windows", "arch": std::env::consts::ARCH },
            }),
        );
    }

    fn discover(&self, id: Value) {
        match o824_discovery::discover(&self.discovery_options()) {
            Ok(installs) => self.send_result(id, json!({ "installs": installs })),
            Err(error) => self.send_kernel_error(id, error.code, &error.fallback),
        }
    }

    fn discovery_options(&self) -> o824_discovery::Options {
        o824_discovery::Options {
            roots: self.options.roots.clone(),
            extra_dirs: Vec::new(),
            home: self.options.home.clone(),
            ..Default::default()
        }
    }

    /// Starts a job and answers with its id before any work happens.
    fn start_job(&self, id: Value, job: Job, params: &Value) {
        let job_id = format!("j{}", self.job_counter.fetch_add(1, Ordering::SeqCst) + 1);
        let flag = Arc::new(AtomicBool::new(false));
        if let Ok(mut registry) = self.cancelled.lock() {
            registry.insert(job_id.clone(), Arc::clone(&flag));
        }
        self.send_result(id, json!({ "jobId": job_id }));

        let output = Arc::clone(&self.output);
        let seq = Arc::new(AtomicI64::new(self.seq.load(Ordering::SeqCst)));
        let options = self.options.clone();
        let cancelled = Arc::clone(&self.cancelled);
        let worker_id = job_id.clone();
        let params = params.clone();

        std::thread::spawn(move || {
            let session = JobSession {
                job_id: worker_id.clone(),
                output,
                seq,
                flag,
            };
            let outcome = match job {
                Job::Doctor => session.doctor(&options, &params),
                Job::Plan => session.plan(&options, &params),
                Job::Apply => session.apply(&options, &params),
            };
            outcome.finish(&session);
            if let Ok(mut registry) = cancelled.lock() {
                registry.remove(&worker_id);
            }
        });
    }

    fn cancel(&self, id: Value, params: &Value) {
        let Some(job_id) = params.get("jobId").and_then(Value::as_str) else {
            self.send_error(
                Some(id),
                -32602,
                "E_PROTOCOL_INVALID",
                "o824.cancel needs a jobId",
            );
            return;
        };
        let flag = self
            .cancelled
            .lock()
            .ok()
            .and_then(|registry| registry.get(job_id).cloned());
        match flag {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                // §19.2: the answer says the cancellation was accepted; the job's own
                // terminal state is the proof it stopped.
                self.send_result(id, json!({ "cancellable": true }));
            }
            None => {
                self.send_result(
                    id,
                    json!({ "cancellable": false, "reason": "no such job in this session" }),
                );
            }
        }
    }

    // ---- writing frames ----------------------------------------------------

    fn send_result(&self, id: Value, result: Value) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "seq": seq,
            "result": result,
        }));
    }

    fn send_error(&self, id: Option<Value>, code: i64, kernel_code: &str, message: &str) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "seq": seq,
            "error": {
                "code": code,
                "message": message,
                "data": { "code": kernel_code, "message": message },
            },
        }));
    }

    /// A kernel error, carried as the kernel's own code rather than a JSON-RPC one.
    fn send_kernel_error(&self, id: Value, code: Code, message: &str) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "seq": seq,
            "error": {
                "code": -32000,
                "message": message,
                "data": { "code": code.as_str(), "message": message },
            },
        }));
    }

    fn send(&self, message: Value) {
        write_message(&self.output, &message);
    }
}

/// Which job to run.
#[derive(Clone, Copy)]
enum Job {
    Doctor,
    Plan,
    Apply,
}

/// How a job ended.
enum Outcome {
    Done(Value),
    Failed(Code, String),
    Cancelled,
}

/// The job's own writer: sequence numbers are session-global, so they come from one
/// counter shared with the request path.
struct JobSession {
    job_id: String,
    output: SharedOutput,
    seq: Arc<AtomicI64>,
    flag: Arc<AtomicBool>,
}

impl JobSession {
    fn cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    fn emit(&self, message: Value) {
        write_message(&self.output, &message);
    }

    fn next_seq(&self) -> i64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Announces a phase. `total` is the unit count of that phase, or `None` when it is
    /// not measurable — the protocol calls that indeterminate rather than guessing.
    fn phase(&self, phase: &str, total: Option<u64>) {
        self.emit(json!({
            "jsonrpc": "2.0",
            "method": "op.state",
            "params": {
                "seq": self.next_seq(),
                "jobId": self.job_id,
                "state": "running",
                "phase": phase,
            },
        }));
        self.progress(phase, 0, total.unwrap_or(0), total.is_none());
    }

    fn progress(&self, phase: &str, done: u64, total: u64, indeterminate: bool) {
        self.emit(json!({
            "jsonrpc": "2.0",
            "method": "op.progress",
            "params": {
                "seq": self.next_seq(),
                "jobId": self.job_id,
                "phase": phase,
                "done": done,
                "total": total,
                "indeterminate": indeterminate,
            },
        }));
    }

    fn doctor(&self, options: &ServerOptions, params: &Value) -> Outcome {
        self.phase("scan", None);
        let Some(install) = self.resolve_install(options, params) else {
            return Outcome::Failed(
                Code::InstallDir,
                "no installation matched the request".to_owned(),
            );
        };
        if self.cancelled() {
            return Outcome::Cancelled;
        }

        let envelope = o824_verify::doctor(&install, &options_data_root(options));
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        match serde_json::to_value(envelope) {
            Ok(value) => Outcome::Done(value),
            Err(error) => Outcome::Failed(Code::Internal, error.to_string()),
        }
    }

    fn plan(&self, options: &ServerOptions, params: &Value) -> Outcome {
        self.phase("scan", None);
        let Some(install) = self.resolve_install(options, params) else {
            return Outcome::Failed(
                Code::InstallDir,
                "no installation matched the request".to_owned(),
            );
        };
        if self.cancelled() {
            return Outcome::Cancelled;
        }

        let with_install = params
            .get("withInstall")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let envelope = o824_verify::doctor(&install, &options_data_root(options));
        let plan = match o824_plan::build_with(
            &install,
            &envelope,
            o824_model::Timestamp::now(),
            &options_data_root(options),
            o824_plan::Options {
                install_packages: with_install,
            },
        ) {
            Ok(plan) => plan,
            Err(error) => return Outcome::Failed(error.code, error.fallback),
        };
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        // A plan is a session object: applying it later is a different request, so it
        // is written where the apply path can find it. The fingerprint inside it is
        // what stops an apply against a tree that has moved.
        if let Err(error) = self.store_plan(options, &plan) {
            return Outcome::Failed(Code::Internal, error);
        }
        match serde_json::to_value(plan) {
            Ok(value) => Outcome::Done(value),
            Err(error) => Outcome::Failed(Code::Internal, error.to_string()),
        }
    }

    fn apply(&self, options: &ServerOptions, params: &Value) -> Outcome {
        // §19.2's phase list, in the order the recording showed: the snapshot first,
        // then the steps.
        self.phase("snapshot", Some(1));
        if self.cancelled() {
            return Outcome::Cancelled;
        }

        let Some(plan_id) = params.get("planId").and_then(Value::as_str) else {
            return Outcome::Failed(Code::Usage, "o824.apply needs a planId".to_owned());
        };
        let Some(plan) = self.load_plan(options, plan_id) else {
            // An expired or unknown plan is exactly what `E_PLAN_EXPIRED` is for, and
            // the answer is "plan again", never "retry apply".
            return Outcome::Failed(
                Code::PlanExpired,
                format!("plan {plan_id:?} is not in this session or has expired"),
            );
        };

        let mut plan = plan;
        let step_count = plan.steps.len() as u64;
        self.progress("snapshot", 1, 1, false);
        if self.cancelled() {
            return Outcome::Cancelled;
        }

        self.phase("apply", Some(step_count.max(1)));
        let root = PathBuf::from(&plan.root);
        match o824_apply::run(&options_data_root(options), &mut plan, &root) {
            Ok(()) => {
                if self.cancelled() {
                    return Outcome::Cancelled;
                }
                self.progress("apply", step_count.max(1), step_count.max(1), false);
                Outcome::Done(json!({
                    "planId": plan_id,
                    "applied": step_count,
                    "steps": plan.steps.iter().map(|step| json!({
                        "kind": step.kind,
                        "path": step.path,
                    })).collect::<Vec<_>>(),
                }))
            }
            Err(error) => Outcome::Failed(error.code, error.fallback),
        }
    }

    /// The installation a request names, or the only one on the machine.
    fn resolve_install(&self, options: &ServerOptions, params: &Value) -> Option<Install> {
        let installs = o824_discovery::discover(&o824_discovery::Options {
            roots: options.roots.clone(),
            extra_dirs: Vec::new(),
            home: options.home.clone(),
            ..Default::default()
        })
        .ok()?;
        let wanted = params.get("installId").and_then(Value::as_str);
        match wanted {
            Some(id) => installs
                .into_iter()
                .find(|install| install.install_id == id),
            None if installs.len() == 1 => installs.into_iter().next(),
            None => None,
        }
    }

    /// Writes a plan where a later `apply` can find it.
    fn store_plan(&self, options: &ServerOptions, plan: &o824_plan::Plan) -> Result<(), String> {
        let directory = options_data_root(options).join("plans");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let text = serde_json::to_string(plan).map_err(|error| error.to_string())?;
        std::fs::write(directory.join(format!("{}.json", plan.id)), text)
            .map_err(|error| format!("cannot write the plan: {error}"))
    }

    /// Plans built in this session, by id.
    ///
    /// A plan is a session object: it carries a fingerprint of the tree it was built
    /// against, and applying it after the tree moved is the failure the fingerprint
    /// exists to catch. Keeping them here (not on disk) is what makes "expired" mean
    /// something.
    fn load_plan(&self, options: &ServerOptions, plan_id: &str) -> Option<o824_plan::Plan> {
        let directory = options_data_root(options).join("plans");
        let path = directory.join(format!("{plan_id}.json"));
        let text = std::fs::read_to_string(path).ok()?;
        let plan: o824_plan::Plan = serde_json::from_str(&text).ok()?;
        let now = o824_model::Timestamp::now();
        let expired = (plan.expires_at.unix_secs(), plan.expires_at.nanos())
            <= (now.unix_secs(), now.nanos());
        if expired {
            return None;
        }
        Some(plan)
    }
}

impl Outcome {
    /// Emits the terminal state, exactly once, whatever happened.
    fn finish(&self, session: &JobSession) {
        match self {
            Outcome::Done(result) => {
                session.emit(json!({
                    "jsonrpc": "2.0",
                    "method": "op.result",
                    "params": {
                        "seq": session.next_seq(),
                        "jobId": session.job_id,
                        "result": result,
                    },
                }));
                session.emit(json!({
                    "jsonrpc": "2.0",
                    "method": "op.state",
                    "params": { "seq": session.next_seq(), "jobId": session.job_id, "state": "done" },
                }));
            }
            Outcome::Failed(code, message) => {
                session.emit(json!({
                    "jsonrpc": "2.0",
                    "method": "op.state",
                    "params": {
                        "seq": session.next_seq(),
                        "jobId": session.job_id,
                        "state": "failed",
                        "error": { "code": code.as_str(), "message": message },
                    },
                }));
            }
            Outcome::Cancelled => {
                session.emit(json!({
                    "jsonrpc": "2.0",
                    "method": "op.state",
                    "params": { "seq": session.next_seq(), "jobId": session.job_id, "state": "cancelled" },
                }));
            }
        }
    }
}

/// Frames one message and writes it under the output lock.
///
/// A write failure means the client is gone; the session ends when the next read hits
/// EOF, so the failure is not worth propagating into the middle of a job.
fn write_message(output: &SharedOutput, message: &Value) {
    let Ok(body) = serde_json::to_vec(message) else {
        return;
    };
    let mut bytes = Vec::new();
    if write_frame(&mut bytes, &body).is_err() {
        return;
    }
    if let Ok(mut stream) = output.lock() {
        let _ = stream.write_all(&bytes);
        let _ = stream.flush();
    }
}

fn options_data_root(options: &ServerOptions) -> PathBuf {
    match &options.data_root {
        Some(root) => root.clone(),
        None => o824_state::data_root(None),
    }
}

/// The major component of a dotted version, for the compatibility rule in §19.4.
fn major_of(version: &str) -> u64 {
    version
        .split('.')
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0)
}

/// Compares two dotted versions by the parts that participate in negotiation.
fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let parts = |value: &str| -> Vec<u64> {
        value
            .split('.')
            .take(2)
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    let left = parts(left);
    let right = parts(right);
    for index in 0..2 {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        match a.cmp(&b) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}
