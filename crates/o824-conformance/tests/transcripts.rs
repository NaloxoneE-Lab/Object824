//! The three transcripts, replayed frame by frame.
//!
//! This is gate G4: driving the mock with a transcript's own client requests must
//! emit exactly the server frames the transcript recorded. Because the requests
//! carry the recording's ids and arrive in the recording's order, the mock's two
//! rewrites (id and sequence) are the identity here — which is the point: a
//! client that speaks the recorded session sees the recorded session.

use std::io::Cursor;
use std::path::PathBuf;

use o824_conformance::{load, transcript_dir};
use o824_mock::{Options, Transcript};
use o824_protocol::{read_frame, write_frame};
use serde_json::Value;

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

/// Frames the transcript's own client requests, in recorded order.
fn client_input(transcript: &Transcript) -> Vec<u8> {
    let mut input: Vec<u8> = Vec::new();
    for exchange in transcript.exchanges() {
        let body = serde_json::to_vec(&exchange.request).expect("serialize the request");
        write_frame(&mut input, &body).expect("frame the request");
    }
    input
}

/// Parses every frame a replay wrote.
fn frames_of(bytes: Vec<u8>) -> Vec<Value> {
    let mut reader = Cursor::new(bytes);
    let mut frames = Vec::new();
    while let Ok(body) = read_frame(&mut reader) {
        frames.push(serde_json::from_slice(&body).expect("frame body is JSON"));
    }
    frames
}

#[test]
fn the_transcript_directory_holds_three_recordings() {
    let paths = transcripts();
    assert_eq!(paths.len(), 3, "{paths:?}");
}

#[test]
fn every_transcript_replays_frame_for_frame() {
    for path in transcripts() {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let transcript = Transcript::load(&path).expect("load");
        assert!(!transcript.is_empty(), "{name} records no requests");

        let mut output: Vec<u8> = Vec::new();
        transcript
            .replay(
                &mut Cursor::new(client_input(&transcript)),
                &mut output,
                &Options::default(),
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let actual = frames_of(output);

        let expected: Vec<Value> = transcript
            .exchanges()
            .iter()
            .flat_map(|exchange| exchange.replies.iter().cloned())
            .collect();

        assert_eq!(
            actual.len(),
            expected.len(),
            "{name}: the replay emitted a different number of frames"
        );
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                actual,
                expected,
                "{name}: frame {} differs\n  actual:   {actual}\n  recorded: {expected}",
                index + 1
            );
        }
    }
}

/// The transcripts pin two shapes that are easy to get wrong: a response carries
/// `seq` at the top level, a notification carries it inside `params`, and a
/// result always arrives before the `done` state.
#[test]
fn the_sequence_and_job_shapes_are_the_documented_ones() {
    for path in transcripts() {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let transcript = Transcript::load(&path).expect("load");
        let mut saw_result_before_done = false;

        for exchange in transcript.exchanges() {
            let mut result_seen = false;
            for reply in &exchange.replies {
                let object = reply.as_object().expect("object");
                let method = object.get("method").and_then(Value::as_str);

                match method {
                    // A notification: no id, seq inside params.
                    Some(_) => {
                        assert!(
                            !object.contains_key("id"),
                            "{name}: a notification must not carry an id: {reply}"
                        );
                        let params = object.get("params").expect("params");
                        assert!(
                            params.get("seq").is_some(),
                            "{name}: a notification carries its seq inside params: {reply}"
                        );
                        match method {
                            Some("op.result") => result_seen = true,
                            Some("op.state")
                                if params.get("state").and_then(Value::as_str) == Some("done") =>
                            {
                                assert!(
                                    result_seen,
                                    "{name}: op.result must come before op.state(done)"
                                );
                                saw_result_before_done = true;
                            }
                            _ => {}
                        }
                    }
                    // A response: id at the top level, seq at the top level.
                    None => {
                        assert!(
                            object.contains_key("id"),
                            "{name}: a response carries an id"
                        );
                        assert!(
                            object.get("seq").is_some(),
                            "{name}: a response carries seq at the top level: {reply}"
                        );
                    }
                }
            }
        }
        if name.starts_with("doctor") || name.starts_with("plan") {
            assert!(saw_result_before_done, "{name}: no job reached a result");
        }
    }
}

/// The same session replayed twice emits the same frames: the reply cursor
/// belongs to the session, not to the transcript.
#[test]
fn a_transcript_can_replay_more_than_once() {
    for path in transcripts() {
        let transcript = Transcript::load(&path).expect("load");
        let mut first: Vec<u8> = Vec::new();
        transcript
            .replay(
                &mut Cursor::new(client_input(&transcript)),
                &mut first,
                &Options::default(),
            )
            .expect("first replay");
        let mut second: Vec<u8> = Vec::new();
        transcript
            .replay(
                &mut Cursor::new(client_input(&transcript)),
                &mut second,
                &Options::default(),
            )
            .expect("second replay");
        assert_eq!(frames_of(first), frames_of(second));
    }
}

#[test]
fn the_mock_is_the_only_thing_the_test_needs() {
    let path = transcript_dir().join("doctor-clean.jsonl");
    let text = load(&path);
    let transcript = Transcript::parse(&text).expect("parse");
    // The recording covers exactly the methods a client may ask for.
    assert_eq!(
        transcript.methods(),
        ["o824.info", "o824.discover", "o824.doctor"]
    );
}
