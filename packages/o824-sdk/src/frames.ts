// The frame codec: `Content-Length: <n>\r\n\r\n<json>`.
//
// The decoder is a state machine over a growing buffer for one reason: a pipe has no
// message boundaries. Both of these happen in practice and neither is an error — a
// frame split across three reads, and two frames arriving in one read.

/** One frame could not be read, and the stream cannot be resynchronised after it. */
export class FrameError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'FrameError';
  }
}

/** Bounds one frame body, matching the kernel's own limit. */
export const MAX_FRAME_BYTES = 64 << 20;

/** Encodes one message as a frame. The length is the byte length of the body. */
export function encodeFrame(value: unknown): Buffer {
  const body = Buffer.from(JSON.stringify(value), 'utf8');
  const header = Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, 'ascii');
  return Buffer.concat([header, body]);
}

/**
 * Decodes frames from arbitrarily-split chunks.
 *
 * `push` returns every message that became complete, in order; a partial frame is kept
 * until the rest arrives. A malformed header throws: without a length there is no way
 * to find the next frame boundary, so continuing would decode garbage.
 */
export class FrameDecoder {
  private buffer: Buffer = Buffer.alloc(0);

  push(chunk: Buffer): unknown[] {
    this.buffer = Buffer.concat([this.buffer, chunk]);

    const messages: unknown[] = [];
    for (;;) {
      const headerEnd = this.buffer.indexOf('\r\n\r\n');
      if (headerEnd < 0) {
        return messages;
      }

      const length = parseHeader(this.buffer.subarray(0, headerEnd).toString('ascii'));
      const bodyStart = headerEnd + 4;
      if (this.buffer.length < bodyStart + length) {
        // The header arrived but the body has not: wait for more bytes.
        return messages;
      }

      const body = this.buffer.subarray(bodyStart, bodyStart + length);
      this.buffer = this.buffer.subarray(bodyStart + length);

      let parsed: unknown;
      try {
        parsed = JSON.parse(body.toString('utf8'));
      } catch {
        throw new FrameError('a frame body is not valid JSON');
      }
      messages.push(parsed);
    }
  }
}

/** Reads the length out of the header lines, ignoring every other header. */
function parseHeader(text: string): number {
  let length: number | undefined;
  for (const line of text.split('\r\n')) {
    if (line === '') {
      continue;
    }
    const separator = line.indexOf(':');
    if (separator < 0) {
      throw new FrameError(`frame header line is not a name:value pair: ${JSON.stringify(line)}`);
    }
    const name = line.slice(0, separator).trim();
    const value = line.slice(separator + 1).trim();
    if (name.toLowerCase() === 'content-length') {
      const parsed = Number(value);
      if (!Number.isInteger(parsed) || parsed < 0 || parsed > MAX_FRAME_BYTES) {
        throw new FrameError(`Content-Length is not a usable byte count: ${JSON.stringify(value)}`);
      }
      length = parsed;
    }
  }
  if (length === undefined) {
    throw new FrameError('the frame header has no Content-Length');
  }
  return length;
}
