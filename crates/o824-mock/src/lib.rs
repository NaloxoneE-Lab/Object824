//! Replays a recorded O824 session, so a third-party UI can be built and tested
//! without DSH installed and without a broken machine (spec §19.5).
//!
//! A transcript is JSONL: one JSON-RPC message per line, in session order. A line
//! with `method` and `id` is a client request; a line with `method` and no `id` is
//! a server notification; a line with `id` and exactly one of `result` or `error`
//! is a server response.
//!
//! Replay answers each client request with the recorded server messages that
//! follow it, up to the next client request. Two values are rewritten so the
//! recording looks like a live session to the client:
//!
//! * the id of every emitted response becomes the incoming request's id (a
//!   transcript is recorded with ids 1, 2, 3…; a client may use any ids);
//! * every emitted sequence number is renumbered to the session-global counter,
//!   whether it sits at the top level of a response or inside a notification's
//!   `params`, so a client that asserts monotonic sequences still passes.
//!
//! The engine never touches the process's own streams: [`Transcript::replay`]
//! takes a reader and a writer, so tests drive it with buffers and the CLI binds
//! it to stdio. Frames come from `o824-protocol`'s codec, so the mock cannot
//! drift from the kernel's framing (spec §9.1).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use o824_errors_compat::code_str;
use o824_protocol::{read_frame, write_frame, FrameError, Seq};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The O824 code a `--fail-at` injection reports. It is mock infrastructure, not
/// a kernel error, so it is spelled here.
pub const CODE_MOCK_FAILURE: &str = "E_MOCK_FAILURE";

/// Bounds one transcript line. A plan result carries a fingerprint and a step
/// list, so lines are longer than a typical JSON-RPC message.
const MAX_TRANSCRIPT_LINE: usize = 8 << 20;

/// One parsed transcript line: the fields the engine classifies on.
///
/// Unknown fields are not captured here, which is why emission works from the
/// raw [`Value`] instead of from this struct.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Message {
    /// Always `"2.0"`.
    #[serde(default)]
    pub jsonrpc: String,
    /// Present on requests and responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Present on requests and notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The request or notification parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// Present on a successful response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Present on a failed response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    /// The session sequence number, top-level on a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
}

impl Message {
    /// True when the line is a client request: `method` and `id`.
    pub fn is_request(&self) -> bool {
        self.method
            .as_deref()
            .map(|m| !m.is_empty())
            .unwrap_or(false)
            && self.id.is_some()
    }
}

/// One recorded client request and the server messages that followed it.
#[derive(Clone, Debug)]
pub struct Exchange {
    /// The request, as recorded.
    pub request: Message,
    /// The server messages that followed, in recorded order.
    pub replies: Vec<Value>,
}

/// A parsed, validated recording.
#[derive(Clone, Debug)]
pub struct Transcript {
    exchanges: Vec<Exchange>,
    by_method: BTreeMap<String, Vec<usize>>,
    methods: Vec<String>,
}

/// Configures one replay session.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Pauses between emitted server messages. Zero emits as fast as the writer
    /// accepts them.
    pub delay: Option<std::time::Duration>,
    /// A 1-based index over emitted server messages. At that message the engine
    /// writes a JSON-RPC `-32603` error with `data.code` [`CODE_MOCK_FAILURE`]
    /// instead of the recorded message, so a UI can exercise its error path.
    pub fail_at: usize,
    /// Overrides the `protocolVersion` a `o824.info` result reports, so a UI can
    /// test negotiation against another version.
    pub protocol: Option<String>,
}

impl Transcript {
    /// Reads and validates the transcript at `path`.
    pub fn load(path: &std::path::Path) -> Result<Transcript, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("transcript {:?}: {error}", path.display().to_string()))?;
        Transcript::parse(&text)
            .map_err(|error| format!("transcript {:?}: {error}", path.display().to_string()))
    }

    /// Parses a transcript. A malformed line is an error that names its number,
    /// so a hand-written transcript is fixable without guessing.
    pub fn parse(text: &str) -> Result<Transcript, String> {
        let mut transcript = Transcript {
            exchanges: Vec::new(),
            by_method: BTreeMap::new(),
            methods: Vec::new(),
        };

        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let raw = line.trim();
            if raw.is_empty() {
                continue;
            }
            if raw.len() > MAX_TRANSCRIPT_LINE {
                return Err(format!("line {number}: the line is longer than 8 MiB"));
            }
            let value: Value = serde_json::from_str(raw)
                .map_err(|error| format!("line {number}: invalid JSON: {error}"))?;
            if !value.is_object() {
                return Err(format!(
                    "line {number}: a transcript line must be a JSON object"
                ));
            }
            let message: Message = serde_json::from_value(value.clone())
                .map_err(|error| format!("line {number}: {error}"))?;
            if message.jsonrpc != "2.0" {
                return Err(format!(
                    "line {number}: a transcript message requires \"jsonrpc\":\"2.0\""
                ));
            }

            if message.is_request() {
                let position = transcript.exchanges.len();
                let method = message.method.clone().unwrap_or_default();
                if !transcript.by_method.contains_key(&method) {
                    transcript.methods.push(method.clone());
                }
                transcript
                    .by_method
                    .entry(method)
                    .or_default()
                    .push(position);
                transcript.exchanges.push(Exchange {
                    request: message,
                    replies: Vec::new(),
                });
                continue;
            }

            // A server message: a notification (method, no id) or a response
            // (id and exactly one of result or error).
            let is_notification = message.method.is_some() && message.id.is_none();
            let is_response =
                message.id.is_some() && (message.result.is_some() != message.error.is_some());
            if !is_notification && !is_response {
                return Err(format!(
                    "line {number}: expected a client request (method and id), a server \
                     notification (method, no id), or a server response (id and exactly one of \
                     result or error)"
                ));
            }
            let Some(last) = transcript.exchanges.last_mut() else {
                return Err(format!(
                    "line {number}: a server message cannot come before the first client request"
                ));
            };
            last.replies.push(value);
        }

        Ok(transcript)
    }

    /// The request methods the transcript covers, in first-appearance order.
    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    /// The number of recorded client requests.
    pub fn len(&self) -> usize {
        self.exchanges.len()
    }

    /// True when the transcript records no requests.
    pub fn is_empty(&self) -> bool {
        self.exchanges.is_empty()
    }

    /// The recorded requests and their server messages.
    pub fn exchanges(&self) -> &[Exchange] {
        &self.exchanges
    }

    /// Serves one session: reads client frames from `input` and writes the
    /// recorded server frames to `output` until the input reaches end of stream.
    ///
    /// Requests are matched by method, not by position. Each recorded exchange for
    /// a method is used once, in recorded order; a client that asks for the same
    /// method more times than the transcript recorded keeps getting the last
    /// recorded answer, so a UI that re-sends `info` or `discover` while
    /// developing does not hit a dead end. A method the transcript does not cover
    /// is the one real error: `-32601` with `data.code` `E_UNSUPPORTED` and
    /// `data.supported` naming the covered methods.
    pub fn replay(
        &self,
        input: &mut dyn BufRead,
        output: &mut dyn Write,
        options: &Options,
    ) -> Result<(), String> {
        let mut state = ReplayState::new(output, options);
        let mut cursors: BTreeMap<String, usize> = BTreeMap::new();

        loop {
            let body = match read_frame(input) {
                Ok(body) => body,
                Err(FrameError::EndOfStream) => return Ok(()),
                Err(FrameError::Malformed) => {
                    // The mock is a transport too: an unparseable header gets one
                    // answer, then the session ends (the boundary is unknown).
                    state
                        .write_protocol_error(
                            None,
                            -32700,
                            "E_PROTOCOL_PARSE",
                            "frame header is not a valid Content-Length header",
                        )
                        .map_err(|error| error.to_string())?;
                    return Ok(());
                }
            };
            self.handle(&body, &mut state, &mut cursors)
                .map_err(|error| error.to_string())?;
        }
    }

    /// Answers one client frame.
    fn handle(
        &self,
        body: &[u8],
        state: &mut ReplayState<'_>,
        cursors: &mut BTreeMap<String, usize>,
    ) -> Result<(), String> {
        let Ok(text) = std::str::from_utf8(body) else {
            return state.write_protocol_error(
                None,
                -32700,
                "E_PROTOCOL_PARSE",
                "frame body is not valid UTF-8",
            );
        };
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return state.write_protocol_error(
                None,
                -32700,
                "E_PROTOCOL_PARSE",
                "frame body is not valid JSON",
            );
        };
        let Ok(request) = serde_json::from_value::<Message>(value) else {
            return state.write_protocol_error(
                None,
                -32600,
                "E_PROTOCOL_INVALID",
                "frame body is not a JSON-RPC 2.0 request object",
            );
        };
        if request.jsonrpc != "2.0"
            || request
                .method
                .as_deref()
                .map(str::trim)
                .map(str::is_empty)
                .unwrap_or(true)
        {
            return state.write_protocol_error(
                request.id,
                -32600,
                "E_PROTOCOL_INVALID",
                "a request requires \"jsonrpc\":\"2.0\" and a non-empty \"method\"",
            );
        }
        // A request without an id is a notification. No method is meaningful as a
        // notification, so it has nothing to answer and is ignored, as in the
        // kernel.
        if request.id.is_none() {
            return Ok(());
        }

        let method = request.method.clone().unwrap_or_default();
        let Some(indexes) = self.by_method.get(&method) else {
            return state.write_unsupported(&self.methods, &request);
        };

        let cursor = cursors.entry(method.clone()).or_insert(0);
        let mut next = *cursor;
        if next >= indexes.len() {
            // The recording has no further answer for this method: replaying the
            // last one keeps a development session usable.
            next = indexes.len() - 1;
        }
        *cursor = next + 1;

        for reply in &self.exchanges[indexes[next]].replies {
            state.write_reply(reply, &request)?;
        }
        Ok(())
    }
}

/// One session's write state: the session-global sequence, the number of messages
/// already emitted (for `fail_at`), and the options.
struct ReplayState<'a> {
    output: &'a mut dyn Write,
    options: &'a Options,
    seq: Seq,
    emitted: usize,
}

impl<'a> ReplayState<'a> {
    fn new(output: &'a mut dyn Write, options: &'a Options) -> ReplayState<'a> {
        ReplayState {
            output,
            options,
            seq: Seq::new(),
            emitted: 0,
        }
    }

    /// Emits one recorded server message, rewriting its id to the request's and
    /// its sequence to the session counter.
    fn write_reply(&mut self, raw: &Value, request: &Message) -> Result<(), String> {
        let Some(object) = raw.as_object() else {
            return Err("mock: a recorded message is not a JSON object".to_owned());
        };
        let mut object = object.clone();

        // Decide before decorating: both paths consume exactly one sequence
        // number, and the injected message must not skip one.
        if self.options.fail_at > 0 && self.emitted + 1 == self.options.fail_at {
            let injected = self.injected_error(&object, request);
            return self.write(&injected);
        }
        self.decorate(&mut object, request);
        self.write(&Value::Object(object))
    }

    /// Rewrites the session-owned fields of one recorded server message.
    fn decorate(&mut self, object: &mut Map<String, Value>, request: &Message) {
        let seq = self.seq.take_next();
        if object.contains_key("id") {
            object.insert("id".to_owned(), request.id.clone().unwrap_or(Value::Null));
        }
        if object.contains_key("seq") {
            object.insert("seq".to_owned(), Value::from(seq));
        }
        if let Some(Value::Object(params)) = object.get_mut("params") {
            if params.contains_key("seq") {
                params.insert("seq".to_owned(), Value::from(seq));
            }
        }
        let Some(protocol) = &self.options.protocol else {
            return;
        };
        // Only `o824.info` reports a protocolVersion; matching the field rather
        // than the method keeps a later result shape out of this decision.
        if let Some(Value::Object(result)) = object.get_mut("result") {
            if result.contains_key("protocolVersion") {
                result.insert("protocolVersion".to_owned(), Value::from(protocol.clone()));
            }
        }
    }

    /// Builds the `-32603` / `E_MOCK_FAILURE` message that replaces the
    /// `fail_at`-th recorded message.
    fn injected_error(&mut self, recorded: &Map<String, Value>, request: &Message) -> Value {
        let message = format!(
            "mock: injected failure at server message {}",
            self.options.fail_at
        );
        let mut object = Map::new();
        object.insert("jsonrpc".to_owned(), Value::from("2.0"));
        if recorded.contains_key("id") {
            object.insert("id".to_owned(), request.id.clone().unwrap_or(Value::Null));
        }
        object.insert("seq".to_owned(), Value::from(self.seq.take_next()));
        object.insert(
            "error".to_owned(),
            serde_json::json!({
                "code": -32603,
                "message": message,
                "data": {
                    "code": CODE_MOCK_FAILURE,
                    "message": message,
                    "failAt": self.options.fail_at,
                    "method": request.method.clone().unwrap_or_default(),
                },
            }),
        );
        Value::Object(object)
    }

    /// Answers a method the transcript does not cover.
    fn write_unsupported(&mut self, methods: &[String], request: &Message) -> Result<(), String> {
        let message = format!(
            "unsupported method {:?}",
            request.method.clone().unwrap_or_default()
        );
        let object = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request.id.clone().unwrap_or(Value::Null),
            "seq": self.seq.take_next(),
            "error": {
                "code": -32601,
                "message": message,
                "data": {
                    "code": code_str(o824_errs::Code::Unsupported),
                    "message": message,
                    "supported": methods,
                },
            },
        });
        self.write(&object)
    }

    /// Answers a frame the mock cannot read as a request.
    fn write_protocol_error(
        &mut self,
        id: Option<Value>,
        code: i64,
        o824_code: &str,
        message: &str,
    ) -> Result<(), String> {
        let object = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "seq": self.seq.take_next(),
            "error": {
                "code": code,
                "message": message,
                "data": { "code": o824_code, "message": message },
            },
        });
        self.write(&object)
    }

    /// Emits one server message: the pause between messages lives here so every
    /// path (recorded, injected, protocol error) sees the same delay.
    fn write(&mut self, message: &Value) -> Result<(), String> {
        if let Some(delay) = self.options.delay {
            if self.emitted > 0 {
                std::thread::sleep(delay);
            }
        }
        self.emitted += 1;
        let body = serde_json::to_vec(message).map_err(|error| error.to_string())?;
        write_frame(self.output, &body).map_err(|error| error.to_string())
    }
}

/// A module alias so the error-code spelling comes from the kernel's own
/// vocabulary instead of a second copy.
mod o824_errors_compat {
    /// The wire spelling of a kernel error code.
    pub fn code_str(code: o824_errs::Code) -> &'static str {
        code.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame_line(line: &str) -> Vec<u8> {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, line.as_bytes()).expect("frame");
        buffer
    }

    fn transcript_text() -> String {
        [
            r#"{"jsonrpc":"2.0","id":1,"method":"o824.info","params":{"clientProtocol":"0.1.0"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"seq":1,"result":{"protocolVersion":"0.1.0"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"o824.discover"}"#,
            r#"{"jsonrpc":"2.0","id":2,"seq":2,"result":{"installs":[]}}"#,
            r#"{"jsonrpc":"2.0","method":"op.state","params":{"seq":3,"jobId":"j1","state":"done"}}"#,
        ]
        .join("\n")
    }

    fn replay(text: &str, requests: &[&str], options: &Options) -> Vec<Value> {
        let transcript = Transcript::parse(text).expect("parse");
        let mut input: Vec<u8> = Vec::new();
        for request in requests {
            input.extend(frame_line(request));
        }
        let mut output: Vec<u8> = Vec::new();
        transcript
            .replay(&mut Cursor::new(input), &mut output, options)
            .expect("replay");

        let mut frames = Vec::new();
        let mut reader = Cursor::new(output);
        while let Ok(body) = read_frame(&mut reader) {
            frames.push(serde_json::from_slice(&body).expect("frame json"));
        }
        frames
    }

    #[test]
    fn the_three_line_classes_are_recognised() {
        let transcript = Transcript::parse(&transcript_text()).expect("parse");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript.methods(), ["o824.info", "o824.discover"]);
        // The notification after the discover response belongs to discover.
        assert_eq!(transcript.exchanges()[1].replies.len(), 2);
    }

    #[test]
    fn a_server_message_before_any_request_is_rejected() {
        let error =
            Transcript::parse(r#"{"jsonrpc":"2.0","method":"op.state"}"#).expect_err("must fail");
        assert!(error.contains("before the first client request"), "{error}");
    }

    #[test]
    fn a_malformed_line_names_its_number() {
        let text = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"o824.info\"}\n",
            "not json\n"
        );
        let error = Transcript::parse(text).expect_err("must fail");
        assert!(error.starts_with("line 2:"), "{error}");
    }

    #[test]
    fn replay_answers_each_request_in_recorded_order() {
        let frames = replay(
            &transcript_text(),
            &[
                r#"{"jsonrpc":"2.0","id":7,"method":"o824.info"}"#,
                r#"{"jsonrpc":"2.0","id":8,"method":"o824.discover"}"#,
            ],
            &Options::default(),
        );
        assert_eq!(frames.len(), 3);
        // The id is rewritten to the caller's, the seq is renumbered.
        assert_eq!(frames[0]["id"], 7);
        assert_eq!(frames[0]["seq"], 1);
        assert_eq!(frames[1]["id"], 8);
        assert_eq!(frames[1]["seq"], 2);
        assert_eq!(frames[2]["params"]["seq"], 3);
    }

    #[test]
    fn an_unsupported_method_names_what_is_supported() {
        let frames = replay(
            &transcript_text(),
            &[r#"{"jsonrpc":"2.0","id":1,"method":"o824.nope"}"#],
            &Options::default(),
        );
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["error"]["code"], -32601);
        assert_eq!(frames[0]["error"]["data"]["code"], "E_UNSUPPORTED");
        assert_eq!(frames[0]["error"]["data"]["supported"][0], "o824.info");
    }

    #[test]
    fn fail_at_replaces_exactly_one_message() {
        let frames = replay(
            &transcript_text(),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"o824.info"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"o824.discover"}"#,
            ],
            &Options {
                fail_at: 2,
                ..Options::default()
            },
        );
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0]["result"]["protocolVersion"], "0.1.0");
        assert_eq!(frames[1]["error"]["code"], -32603);
        assert_eq!(frames[1]["error"]["data"]["code"], CODE_MOCK_FAILURE);
        // The sequence numbers stay gapless.
        assert_eq!(frames[0]["seq"], 1);
        assert_eq!(frames[1]["seq"], 2);
        assert_eq!(frames[2]["params"]["seq"], 3);
    }

    #[test]
    fn a_protocol_override_rewrites_the_info_result() {
        let frames = replay(
            &transcript_text(),
            &[r#"{"jsonrpc":"2.0","id":1,"method":"o824.info"}"#],
            &Options {
                protocol: Some("9.9.9".to_owned()),
                ..Options::default()
            },
        );
        assert_eq!(frames[0]["result"]["protocolVersion"], "9.9.9");
    }

    #[test]
    fn a_notification_gets_no_answer() {
        let frames = replay(
            &transcript_text(),
            &[r#"{"jsonrpc":"2.0","method":"o824.info"}"#],
            &Options::default(),
        );
        assert!(frames.is_empty(), "{frames:?}");
    }

    #[test]
    fn a_non_json_body_is_a_protocol_parse_error() {
        let transcript = Transcript::parse(&transcript_text()).expect("parse");
        let mut input: Vec<u8> = Vec::new();
        write_frame(&mut input, b"not json").expect("frame");
        let mut output: Vec<u8> = Vec::new();
        transcript
            .replay(&mut Cursor::new(input), &mut output, &Options::default())
            .expect("replay");
        let mut reader = Cursor::new(output);
        let body = read_frame(&mut reader).expect("frame");
        let value: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["error"]["code"], -32700);
        assert_eq!(value["error"]["data"]["code"], "E_PROTOCOL_PARSE");
    }

    #[test]
    fn a_repeated_method_keeps_replaying_its_last_answer() {
        let frames = replay(
            &transcript_text(),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"o824.info"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"o824.info"}"#,
            ],
            &Options::default(),
        );
        assert_eq!(frames.len(), 2, "{frames:?}");
        assert_eq!(frames[1]["id"], 2);
    }
}
