//! The kernel's stdio transport: LSP-style framing, JSON-RPC envelopes, and the
//! session sequence counter.
//!
//! The frame codec lives here rather than in the mock so the mock reads and writes
//! exactly the frames the real kernel does; a second parser would let the two
//! drift and would make the mock useless for finding the framing bugs a
//! third-party client will hit.
//!
//! Framing is `Content-Length: N\r\n\r\n<json>` (spec §9.1). A frame whose header
//! cannot be read has no usable length, so the stream cannot be resynchronised:
//! that is the one transport failure that ends a session.

#![forbid(unsafe_code)]

use std::io::{self, BufRead, Write};

/// Bounds one frame body so a bogus `Content-Length` cannot make the receiver
/// allocate without limit.
pub const MAX_FRAME_SIZE: usize = 64 << 20;

/// Why a frame could not be read.
#[derive(Debug)]
pub enum FrameError {
    /// The stream ended cleanly, or a frame was truncated mid-body. Neither
    /// leaves a message to answer.
    EndOfStream,
    /// The header cannot be parsed, so the body length is unknown.
    Malformed,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::EndOfStream => formatter.write_str("protocol: end of stream"),
            FrameError::Malformed => formatter.write_str("protocol: malformed frame header"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Reads one `Content-Length` frame.
///
/// The reader is not created here: a caller serves many frames from one buffered
/// reader, and buffering per frame would lose the bytes the buffer had already
/// read ahead.
pub fn read_frame(reader: &mut dyn BufRead) -> Result<Vec<u8>, FrameError> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err(FrameError::EndOfStream),
            Ok(_) => {}
            Err(_) => return Err(FrameError::EndOfStream),
        }
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(FrameError::Malformed);
        };
        if name.trim().eq_ignore_ascii_case("Content-Length") {
            let parsed: i64 = value.trim().parse().map_err(|_| FrameError::Malformed)?;
            if parsed < 0 || parsed as usize > MAX_FRAME_SIZE {
                return Err(FrameError::Malformed);
            }
            length = Some(parsed as usize);
        }
    }
    let Some(length) = length else {
        return Err(FrameError::Malformed);
    };

    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return Err(FrameError::EndOfStream);
    }
    Ok(body)
}

/// Writes one `Content-Length` frame.
///
/// `Content-Length` is the byte length of `body`, which is what makes multi-byte
/// UTF-8 bodies safe.
pub fn write_frame(writer: &mut dyn Write, body: &[u8]) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()
}

/// The session-wide sequence counter.
///
/// It is called once per emitted server message, whether or not the message
/// carries a sequence of its own, so the numbering stays global.
#[derive(Clone, Debug, Default)]
pub struct Seq {
    value: i64,
}

impl Seq {
    /// A counter that starts at zero.
    pub fn new() -> Seq {
        Seq { value: 0 }
    }

    /// The next sequence number.
    pub fn take_next(&mut self) -> i64 {
        self.value += 1;
        self.value
    }

    /// The last number handed out.
    pub fn current(&self) -> i64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_frame_round_trips() {
        let mut buffer: Vec<u8> = Vec::new();
        write_frame(&mut buffer, b"{\"jsonrpc\":\"2.0\"}").expect("write");

        let mut reader = Cursor::new(buffer);
        let body = read_frame(&mut reader).expect("read");
        assert_eq!(body, b"{\"jsonrpc\":\"2.0\"}");
    }

    #[test]
    fn the_length_header_is_the_byte_length() {
        // A multi-byte body must be measured in bytes, not characters.
        let mut buffer: Vec<u8> = Vec::new();
        write_frame(&mut buffer, "{\"m\":\"测试\"}".as_bytes()).expect("write");
        let header = String::from_utf8_lossy(&buffer);
        assert!(header.starts_with("Content-Length: 14\r\n\r\n"), "{header}");
    }

    #[test]
    fn the_header_name_is_case_insensitive() {
        let mut reader = Cursor::new(b"content-length: 2\r\n\r\n{}".to_vec());
        assert_eq!(read_frame(&mut reader).expect("read"), b"{}");
    }

    #[test]
    fn an_unknown_header_is_skipped() {
        let mut reader = Cursor::new(b"X-Ignored: 1\r\nContent-Length: 2\r\n\r\n{}".to_vec());
        assert_eq!(read_frame(&mut reader).expect("read"), b"{}");
    }

    #[test]
    fn a_missing_length_is_malformed() {
        let mut reader = Cursor::new(b"X-Ignored: 1\r\n\r\n{}".to_vec());
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::Malformed)
        ));
    }

    #[test]
    fn a_negative_length_is_malformed() {
        let mut reader = Cursor::new(b"Content-Length: -1\r\n\r\n".to_vec());
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::Malformed)
        ));
    }

    #[test]
    fn a_truncated_body_is_end_of_stream() {
        let mut reader = Cursor::new(b"Content-Length: 10\r\n\r\n{}".to_vec());
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::EndOfStream)
        ));
    }

    #[test]
    fn an_clean_end_of_stream_is_reported_as_such() {
        let mut reader = Cursor::new(Vec::new());
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::EndOfStream)
        ));
    }

    #[test]
    fn the_sequence_counter_is_session_global() {
        let mut seq = Seq::new();
        assert_eq!(seq.take_next(), 1);
        assert_eq!(seq.take_next(), 2);
        assert_eq!(seq.current(), 2);
    }
}
