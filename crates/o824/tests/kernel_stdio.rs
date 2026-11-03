//! `o824 kernel --stdio` as a process: the transport the SDK and every UI uses.
//!
//! The in-crate tests drive `serve` over in-memory pipes; this one drives the real
//! binary, because the last mile — stdout without a wrapper, exit on EOF — is exactly
//! the part a library test cannot see.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use o824_protocol::{read_frame, write_frame};
use serde_json::{json, Value};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_o824")
}

struct Kernel {
    child: std::process::Child,
    pending: Vec<u8>,
}

impl Kernel {
    fn start(root: &std::path::Path) -> Kernel {
        let child = Command::new(binary())
            .args([
                "kernel",
                "--stdio",
                "--root",
                &root.display().to_string(),
                "--home",
                &root.display().to_string(),
                "--data-root",
                &root.join("data").display().to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn o824 kernel");
        Kernel {
            child,
            pending: Vec::new(),
        }
    }

    fn send(&mut self, message: Value) {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &serde_json::to_vec(&message).expect("serialize"),
        )
        .expect("frame");
        self.child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(&bytes)
            .expect("write the request");
        self.child
            .stdin
            .as_mut()
            .expect("stdin")
            .flush()
            .expect("flush");
    }

    fn next(&mut self) -> Value {
        loop {
            let mut cursor = &self.pending[..];
            if let Ok(body) = read_frame(&mut cursor) {
                let consumed = self.pending.len() - cursor.len();
                self.pending.drain(..consumed);
                return serde_json::from_slice(&body).expect("frame body is JSON");
            }
            let mut chunk = [0u8; 8192];
            let read = self
                .child
                .stdout
                .as_mut()
                .expect("stdout")
                .read(&mut chunk)
                .expect("read");
            assert!(read > 0, "the kernel closed its stdout without answering");
            self.pending.extend_from_slice(&chunk[..read]);
        }
    }

    /// Closes stdin and returns the exit code, with stderr for the failure message.
    fn finish(mut self) -> (i32, String) {
        drop(self.child.stdin.take());
        let output = self.child.wait_with_output().expect("wait");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

#[test]
fn the_binary_serves_a_session_and_exits_when_the_client_closes_the_pipe() {
    let fixture = o824_fixtures::build("ok").expect("fixture");
    let mut kernel = Kernel::start(fixture.root());

    kernel.send(
        json!({"jsonrpc":"2.0","id":1,"method":"o824.info","params":{"clientProtocol":"0.1.0"}}),
    );
    let info = kernel.next();
    assert_eq!(info["result"]["protocolVersion"], "0.1.0");
    assert_eq!(info["result"]["impl"]["name"], "o824");

    kernel.send(json!({"jsonrpc":"2.0","id":2,"method":"o824.discover"}));
    let discover = kernel.next();
    let installs = discover["result"]["installs"].as_array().expect("installs");
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0]["kind"], "desktop");

    // The design's promise: closing the pipe ends the kernel. A kernel that outlived
    // its UI would be the one thing §7.5 says must never happen.
    let (code, stderr) = kernel.finish();
    assert_eq!(code, 0, "the kernel must exit cleanly on EOF: {stderr}");
}

#[test]
fn a_kernel_without_stdio_says_what_it_needs() {
    let output = Command::new(binary())
        .arg("kernel")
        .output()
        .expect("run o824 kernel");
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--stdio"), "{stderr}");
}

/// A job's events must reach a client that is a **separate process**.
///
/// This is the regression test for docs/P3-KERNEL-BUG.md: `main` used to hold
/// `stdout().lock()` for the whole process, so the kernel's job threads deadlocked on a
/// lock the main thread never released. Every in-crate kernel test missed it because they
/// call `serve` with a writer that is not the process's stdout — this one drives the real
/// binary over real pipes, which is where a client actually stands.
#[test]
fn a_job_over_real_pipes_reports_its_result_and_terminates() {
    let fixture = o824_fixtures::build("broken-link").expect("fixture");
    let mut kernel = Kernel::start(fixture.root());

    kernel.send(json!({"jsonrpc":"2.0","id":1,"method":"o824.doctor","params":{"installId":"i1"}}));

    let reply = kernel.next();
    assert_eq!(reply["id"], 1);
    let job_id = reply["result"]["jobId"]
        .as_str()
        .expect("a doctor answers with a job id")
        .to_owned();

    let mut saw_result = false;
    let mut terminal = false;
    // A bounded loop: a kernel that emits nothing fails here instead of hanging the suite.
    for _ in 0..20 {
        let message = kernel.next();
        let params = &message["params"];
        if params["jobId"] != serde_json::json!(job_id) {
            continue;
        }
        match message["method"].as_str() {
            Some("op.result") => {
                saw_result = true;
                assert!(
                    params["result"]["findings"].is_array(),
                    "the result is a doctor envelope: {message}"
                );
            }
            Some("op.state") => {
                let state = params["state"].as_str().unwrap_or_default();
                if state == "done" || state == "failed" || state == "cancelled" {
                    terminal = true;
                    assert_eq!(state, "done", "{message}");
                    break;
                }
            }
            _ => {}
        }
    }

    assert!(saw_result, "the job's result reached the client");
    assert!(terminal, "the job reached a terminal state");
    let (code, stderr) = kernel.finish();
    assert_eq!(code, 0, "{stderr}");
}
