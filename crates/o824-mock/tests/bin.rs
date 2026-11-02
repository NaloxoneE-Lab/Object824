//! The standalone `o824-mock` entry point, driven the way the npm package drives it:
//! frames on stdin, frames on stdout, and an exit code that means something.
//!
//! The frames come from `o824-protocol`'s codec, the same one the kernel uses, so a
//! regression in the transport shows up here rather than in a third-party client.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use o824_protocol::{read_frame, write_frame};
use serde_json::Value;

/// The binary Cargo built for this crate's `[[bin]]`.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_o824-mock")
}

fn transcript_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("spec")
        .join("transcripts")
}

fn transcripts() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(transcript_dir())
        .expect("read spec/transcripts")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "jsonl").unwrap_or(false))
        .collect();
    paths.sort();
    paths
}

/// Splits a recording into its client requests and the server messages between them.
fn split_recording(text: &str) -> (Vec<Value>, Vec<Value>) {
    let mut requests = Vec::new();
    let mut recorded = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line).expect("a recorded line is JSON");
        let is_request = value.get("method").is_some() && value.get("id").is_some();
        if is_request {
            requests.push(value);
        } else {
            recorded.push(value);
        }
    }
    (requests, recorded)
}

fn frames_of(messages: &[Value]) -> Vec<u8> {
    let mut buffer = Vec::new();
    for message in messages {
        let body = serde_json::to_vec(message).expect("serialize");
        write_frame(&mut buffer, &body).expect("frame");
    }
    buffer
}

fn parse_frames(mut output: &[u8]) -> Vec<Value> {
    let mut frames = Vec::new();
    while let Ok(body) = read_frame(&mut &mut output) {
        frames.push(serde_json::from_slice(&body).expect("frame body is JSON"));
    }
    frames
}

fn run(args: &[&str], input: &[u8]) -> (Option<i32>, Vec<u8>, String) {
    let mut child = Command::new(binary())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn o824-mock");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input)
        .expect("write the payload");
    let output = child.wait_with_output().expect("wait for o824-mock");
    (
        output.status.code(),
        output.stdout,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn every_transcript_replays_frame_for_frame() {
    let paths = transcripts();
    assert_eq!(paths.len(), 3, "{paths:?}");
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).expect("read the transcript");
        let (requests, recorded) = split_recording(&text);

        let (code, stdout, stderr) = run(
            &["--transcript", &path.display().to_string()],
            &frames_of(&requests),
        );
        assert_eq!(code, Some(0), "{name}: {stderr}");
        assert_eq!(
            parse_frames(&stdout),
            recorded,
            "{name}: the replay differs from the recording"
        );
    }
}

#[test]
fn a_usage_error_exits_two() {
    let (code, _stdout, stderr) = run(&[], b"");
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("--transcript"), "{stderr}");
}

#[test]
fn an_unreadable_transcript_exits_eight() {
    let (code, _stdout, stderr) = run(&["--transcript", r"Z:\definitely\not\here.jsonl"], b"");
    assert_eq!(code, Some(8), "{stderr}");
    assert!(stderr.contains("transcript"), "{stderr}");
}

/// A plan result carries a fingerprint and a step list, so a line can be long. The
/// cap is 8 MiB and it must be reported, never silently truncated into a replay that
/// answers the wrong session.
#[test]
fn a_line_longer_than_the_line_cap_is_reported_not_truncated() {
    let fixture = o824_fixtures::TempDir::new("o824-mock-bin-").expect("temp dir");
    let path = fixture.path().join("huge.jsonl");
    let padding = "x".repeat(9 << 20);
    let text = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"m\",\"params\":{{\"pad\":\"{padding}\"}}}}\n"
    );
    std::fs::write(&path, text).expect("write the transcript");

    let (code, _stdout, stderr) = run(&["--transcript", &path.display().to_string()], b"");
    assert_eq!(code, Some(8), "{stderr}");
    assert!(stderr.contains("8 MiB"), "{stderr}");
}
