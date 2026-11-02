//! The live server, driven the way a client drives it: frames in, frames out.
//!
//! The judge is not "did it answer" but "did it answer what the recordings and the
//! golden files say". A server that agrees with itself and disagrees with
//! `spec/golden/*.doctor.json` would be a second, wrong implementation of the diagnosis.

use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use o824_kernel::{serve, ServerOptions};
use o824_protocol::{read_frame, write_frame};
use serde_json::{json, Value};

/// A reader fed by a channel: the test decides when the next bytes arrive.
struct ChannelReader {
    receiver: Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Read for ChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            match self.receiver.recv() {
                Ok(bytes) => self.pending = bytes,
                Err(_) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.pending.len());
        buffer[..count].copy_from_slice(&self.pending[..count]);
        self.pending.drain(..count);
        Ok(count)
    }
}

/// A writer whose bytes the test can look at while the server is still running.
#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("output lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One running server plus the two ends of its pipe.
struct Harness {
    sender: Sender<Vec<u8>>,
    output: SharedWriter,
    done: Receiver<()>,
}

impl Harness {
    fn start(options: ServerOptions) -> Harness {
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let output = SharedWriter(Arc::new(Mutex::new(Vec::new())));
        let writer = output.clone();
        let (finished, done) = mpsc::channel::<()>();

        std::thread::spawn(move || {
            // The server wants a buffered reader, exactly as it gets one on a real
            // stdio session; `BufReader` also gives it `fill_buf` for the frame header.
            let reader = ChannelReader {
                receiver,
                pending: Vec::new(),
            };
            let mut reader = BufReader::new(reader);
            let _ = serve(&mut reader, Box::new(writer), options);
            let _ = finished.send(());
        });

        Harness {
            sender,
            output,
            done,
        }
    }

    fn send(&self, message: Value) {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &serde_json::to_vec(&message).expect("serialize"),
        )
        .expect("frame");
        self.sender.send(bytes).expect("server is reading");
    }

    /// Reads one message, waiting for it to arrive.
    fn next(&mut self) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let mut bytes = self.output.0.lock().expect("output lock").clone();
            if !bytes.is_empty() {
                let mut cursor = &bytes[..];
                if let Ok(body) = read_frame(&mut cursor) {
                    let consumed = bytes.len() - cursor.len();
                    self.output.0.lock().expect("output lock").drain(..consumed);
                    return serde_json::from_slice(&body).expect("frame body is JSON");
                }
                bytes.clear();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the server said nothing"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Reads until the reply carrying `id`, returning it and everything before it.
    fn next_reply(&mut self, id: i64) -> (Value, Vec<Value>) {
        let mut seen = Vec::new();
        loop {
            let message = self.next();
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                return (message, seen);
            }
            seen.push(message);
        }
    }

    /// Reads until a terminal `op.state` and returns everything read.
    fn drain_until_terminal(&mut self) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            let message = self.next();
            let terminal = message
                .get("params")
                .and_then(|params| params.get("state"))
                .and_then(Value::as_str)
                .map(|state| matches!(state, "done" | "failed" | "cancelled"))
                .unwrap_or(false);
            messages.push(message);
            if terminal {
                return messages;
            }
        }
    }

    fn close(self) {
        drop(self.sender);
        let _ = self.done.recv_timeout(Duration::from_secs(20));
    }
}

fn fixture(kind: &str) -> o824_fixtures::Fixture {
    o824_fixtures::build(kind).expect("build the fixture")
}

fn options_for(fixture: &o824_fixtures::Fixture) -> ServerOptions {
    ServerOptions {
        roots: Some(vec![fixture.root().to_path_buf()]),
        home: Some(fixture.root().display().to_string()),
        data_root: Some(fixture.root().join("data")),
    }
}

fn golden(kind: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("spec")
        .join("golden")
        .join(format!("{kind}.doctor.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
    serde_json::from_str(&text).expect("golden is JSON")
}

#[test]
fn info_reports_the_protocol_and_what_the_kernel_can_do() {
    let mut harness = Harness::start(ServerOptions::default());
    harness.send(
        json!({"jsonrpc":"2.0","id":1,"method":"o824.info","params":{"clientProtocol":"0.1.0"}}),
    );

    let reply = harness.next();
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["protocolVersion"], "0.1.0");
    assert_eq!(reply["result"]["minClientProtocol"], "0.1.0");
    assert_eq!(reply["result"]["impl"]["name"], "o824");
    assert_eq!(
        reply["result"]["capabilities"],
        json!(["discover", "doctor", "plan", "apply"])
    );
    assert!(
        reply["seq"].as_i64().unwrap() >= 1,
        "a response carries seq"
    );
    harness.close();
}

#[test]
fn a_client_from_another_major_version_is_refused_not_downgraded() {
    let mut harness = Harness::start(ServerOptions::default());
    harness.send(
        json!({"jsonrpc":"2.0","id":2,"method":"o824.info","params":{"clientProtocol":"9.0.0"}}),
    );

    let reply = harness.next();
    assert_eq!(reply["error"]["data"]["code"], "E_PROTOCOL_MISMATCH");
    harness.close();
}

#[test]
fn discover_lists_the_installation_it_was_pointed_at() {
    let fixture = fixture("ok");
    let mut harness = Harness::start(options_for(&fixture));
    harness.send(json!({"jsonrpc":"2.0","id":3,"method":"o824.discover"}));

    let reply = harness.next();
    let installs = reply["result"]["installs"].as_array().expect("installs");
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0]["kind"], "desktop");
    assert_eq!(installs[0]["version"], o824_fixtures::DSH_VERSION);
    harness.close();
}

/// The strongest test in this file: the server's diagnosis must equal the golden file
/// the whole project is measured against.
#[test]
fn doctor_answers_the_same_envelope_as_the_golden_file() {
    for kind in ["ok", "broken-link", "dup-loader", "parse-error", "git-dep"] {
        let fixture = fixture(kind);
        let mut harness = Harness::start(options_for(&fixture));

        harness.send(
            json!({"jsonrpc":"2.0","id":10,"method":"o824.doctor","params":{"installId":"i1"}}),
        );
        let accepted = harness.next();
        let job_id = accepted["result"]["jobId"]
            .as_str()
            .expect("a job id")
            .to_owned();
        assert_eq!(job_id, "j1", "the first job of a session is j1");

        let messages = harness.drain_until_terminal();
        let result = messages
            .iter()
            .find(|message| message["method"] == "op.result")
            .unwrap_or_else(|| panic!("{kind}: no op.result"));
        assert_eq!(
            result["params"]["result"],
            golden(kind),
            "{kind}: the server's envelope differs from spec/golden/{kind}.doctor.json"
        );

        // The phases the recording pins: scan, indeterminate, then the terminal state.
        let state = messages
            .iter()
            .find(|message| message["method"] == "op.state")
            .expect("an op.state");
        assert_eq!(state["params"]["phase"], "scan");
        let progress = messages
            .iter()
            .find(|message| message["method"] == "op.progress")
            .expect("an op.progress");
        assert_eq!(progress["params"]["indeterminate"], true);
        assert_eq!(
            messages.last().expect("terminal")["params"]["state"],
            "done"
        );

        // Sequence numbers are session-global and increase by one per message.
        let seqs: Vec<i64> = messages
            .iter()
            .map(|message| message["params"]["seq"].as_i64().expect("seq"))
            .collect();
        for pair in seqs.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "{kind}: seq must be gapless");
        }
        harness.close();
    }
}

#[test]
fn a_plan_is_a_job_with_steps_and_a_fingerprint() {
    let fixture = fixture("git-dep");
    let mut harness = Harness::start(options_for(&fixture));

    harness.send(json!({"jsonrpc":"2.0","id":11,"method":"o824.plan","params":{"installId":"i1"}}));
    let _accepted = harness.next();
    let messages = harness.drain_until_terminal();
    let result = messages
        .iter()
        .find(|message| message["method"] == "op.result")
        .expect("a plan result");

    let plan = &result["params"]["result"];
    assert_eq!(plan["installId"], "i1");
    assert!(!plan["id"].as_str().expect("plan id").is_empty());
    assert!(
        !plan["steps"].as_array().expect("steps").is_empty(),
        "the git-dep tree needs an allow-builds step"
    );
    assert!(plan["fingerprint"].as_object().is_some());
    harness.close();
}

#[test]
fn an_unknown_method_is_answered_with_the_list_of_known_ones() {
    let mut harness = Harness::start(ServerOptions::default());
    harness.send(json!({"jsonrpc":"2.0","id":12,"method":"o824.start"}));

    let reply = harness.next();
    assert_eq!(reply["error"]["code"], -32601);
    assert_eq!(reply["error"]["data"]["code"], "E_UNSUPPORTED");
    harness.close();
}

#[test]
fn cancelling_a_job_that_does_not_exist_says_so_and_cancels_nothing() {
    let mut harness = Harness::start(ServerOptions::default());
    harness.send(json!({"jsonrpc":"2.0","id":13,"method":"o824.cancel","params":{"jobId":"nope"}}));

    let reply = harness.next();
    assert_eq!(reply["result"]["cancellable"], false);
    assert!(reply["result"]["reason"].as_str().is_some());
    harness.close();
}

/// §19.2: a job never stays running after a cancel. Whatever the race decides — the
/// job finished first or the cancel landed — the stream ends in a terminal state, and a
/// cancelled job never sends a result afterwards.
#[test]
fn cancelling_a_running_job_still_reaches_a_terminal_state() {
    let fixture = fixture("git-dep");
    let mut harness = Harness::start(options_for(&fixture));

    harness
        .send(json!({"jsonrpc":"2.0","id":14,"method":"o824.doctor","params":{"installId":"i1"}}));
    let accepted = harness.next();
    let job_id = accepted["result"]["jobId"]
        .as_str()
        .expect("job id")
        .to_owned();

    harness.send(json!({"jsonrpc":"2.0","id":15,"method":"o824.cancel","params":{"jobId":job_id}}));
    // The cancel reply and the job's own notifications interleave, so the reply is
    // found by id rather than assumed to be next.
    let (_cancel, mut messages) = harness.next_reply(15);

    // The job may have finished before the cancel arrived; both answers are legitimate,
    // and both must be followed by a terminal state — which may already be in hand.
    let terminal_seen = messages.iter().any(|message| {
        message["params"]["state"]
            .as_str()
            .map(|state| matches!(state, "done" | "failed" | "cancelled"))
            .unwrap_or(false)
    });
    if !terminal_seen {
        messages.extend(harness.drain_until_terminal());
    }
    let terminal = messages.last().expect("terminal state");
    let state = terminal["params"]["state"].as_str().expect("state");
    assert!(matches!(state, "done" | "failed" | "cancelled"), "{state}");

    if state == "cancelled" {
        assert!(
            !messages
                .iter()
                .any(|message| message["method"] == "op.result"),
            "a cancelled job must not also send a result"
        );
    }
    harness.close();
}

#[test]
fn a_body_that_is_not_json_is_answered_without_ending_the_session() {
    let mut harness = Harness::start(ServerOptions::default());
    let mut bytes = Vec::new();
    write_frame(&mut bytes, b"not json").expect("frame");
    harness.sender.send(bytes).expect("server is reading");

    let reply = harness.next();
    assert_eq!(reply["error"]["code"], -32700);
    assert_eq!(reply["error"]["data"]["code"], "E_PROTOCOL_PARSE");

    // And the session still works afterwards.
    harness.send(json!({"jsonrpc":"2.0","id":16,"method":"o824.info"}));
    let after = harness.next();
    assert_eq!(after["result"]["protocolVersion"], "0.1.0");
    harness.close();
}

/// A path helper the fixtures use for their own assertions.
#[allow(dead_code)]
fn clean(path: &Path) -> String {
    o824_model::path::clean(path).display().to_string()
}
