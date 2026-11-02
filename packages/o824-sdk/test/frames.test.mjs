// The transport, tested the way a socket behaves: the bytes arrive however they want.
//
// A frame is `Content-Length: <n>\r\n\r\n<n bytes of JSON>`. Nothing else about the
// header matters, and the split between one read and the next is arbitrary — which is
// the whole reason a decoder is a state machine rather than `JSON.parse` on each chunk.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { encodeFrame, FrameDecoder, FrameError } from '../dist/frames.js';

const frameOf = (value) => {
  const body = Buffer.from(JSON.stringify(value));
  return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body]);
};

test('a frame round-trips', () => {
  const decoder = new FrameDecoder();
  assert.deepEqual(decoder.push(frameOf({ jsonrpc: '2.0', id: 1 })), [{ jsonrpc: '2.0', id: 1 }]);
});

test('a frame split at any point still decodes', () => {
  const frame = frameOf({ jsonrpc: '2.0', id: 7, result: { ok: true } });
  // A fresh decoder per split point: the claim is about one frame arriving in two
  // reads, not about a decoder being reusable across copies of the same frame.
  for (let cut = 1; cut < frame.length; cut += 3) {
    const decoder = new FrameDecoder();
    const received = [
      ...decoder.push(frame.subarray(0, cut)),
      ...decoder.push(frame.subarray(cut)),
    ];
    assert.equal(received.length, 1, `split at ${cut} decoded ${received.length} messages`);
    assert.deepEqual(received[0], { jsonrpc: '2.0', id: 7, result: { ok: true } });
  }
});

test('two frames in one chunk come out as two messages', () => {
  const decoder = new FrameDecoder();
  const messages = decoder.push(Buffer.concat([frameOf({ id: 1 }), frameOf({ id: 2 })]));
  assert.deepEqual(
    messages.map((message) => message.id),
    [1, 2],
  );
});

test('a header without Content-Length is a protocol error with a reason', () => {
  const decoder = new FrameDecoder();
  assert.throws(
    () => decoder.push(Buffer.from('X-Nope: 1\r\n\r\n{}')),
    (error) => error instanceof FrameError && /Content-Length/.test(error.message),
  );
});

test('a negative length is a protocol error', () => {
  const decoder = new FrameDecoder();
  assert.throws(() => decoder.push(Buffer.from('Content-Length: -1\r\n\r\n')), FrameError);
});

test('encoding measures bytes, not characters', () => {
  const frame = encodeFrame({ text: '测试' });
  const header = frame.subarray(0, frame.indexOf('\r\n\r\n')).toString();
  const body = frame.subarray(frame.indexOf('\r\n\r\n') + 4);
  assert.equal(Number(header.split(':')[1]), body.length);
  assert.equal(body.length, Buffer.byteLength(JSON.stringify({ text: '测试' })));
});
