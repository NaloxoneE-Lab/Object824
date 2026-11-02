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
