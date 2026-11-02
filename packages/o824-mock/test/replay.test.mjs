// The package's entry point, driven the way a client drives it: frames in, frames
// out, an exit code that means something.
//
// These tests run the real WebAssembly build under Node. They need `o824-mock.wasm`
// beside the entry point; when it is missing but a cargo build exists, the helper
// copies it in rather than making the test depend on someone having run the packaging
// script first. If neither exists the test says so instead of failing obscurely.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(here, '..');
const repoRoot = resolve(packageDir, '..', '..');
const entry = join(packageDir, 'bin', 'o824-mock.mjs');
const wasm = join(packageDir, 'o824-mock.wasm');
const transcripts = join(packageDir, 'transcripts');

/** The recordings the package ships, straight from the repository's contract. */
const SPEC = join(repoRoot, 'spec', 'transcripts');

function ensureArtifacts() {
  if (!existsSync(wasm)) {
    const built = join(repoRoot, 'target', 'wasm32-wasip1', 'release', 'o824-mock.wasm');
    if (!existsSync(built)) {
      return 'no o824-mock.wasm and no cargo build for wasm32-wasip1; run scripts/build-mock-package.ps1';
    }
    copyFileSync(built, wasm);
  }
  if (!existsSync(transcripts)) {
    mkdirSync(transcripts, { recursive: true });
    for (const name of readdirSync(SPEC).filter((name) => name.endsWith('.jsonl'))) {
      copyFileSync(join(SPEC, name), join(transcripts, name));
    }
  }
  return null;
}

const missing = ensureArtifacts();
// `{ skip: null }` is not the same as no options in every runner version, so the
// options object is built explicitly.
const options = missing ? options : {};

function splitRecording(text) {
  const requests = [];
  const recorded = [];
  for (const line of text.split('\n').filter((line) => line.trim() !== '')) {
    const value = JSON.parse(line);
    if (value.method !== undefined && value.id !== undefined) requests.push(value);
    else recorded.push(value);
  }
  return { requests, recorded };
}

function frame(message) {
  const body = Buffer.from(JSON.stringify(message));
  return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body]);
}

function frames(buffer) {
  const out = [];
  let position = 0;
  while (position < buffer.length) {
    const end = buffer.indexOf('\r\n\r\n', position);
    if (end < 0) break;
    const length = Number.parseInt(buffer.subarray(position, end).toString().split(':')[1], 10);
    out.push(JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString()));
    position = end + 4 + length;
  }
  return out;
}

function run(args, messages = []) {
  const result = spawnSync(process.execPath, [entry, ...args], {
    input: Buffer.concat(messages.map(frame)),
    maxBuffer: 1 << 26,
  });
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr.toString(),
  };
}

test('every bundled transcript replays frame for frame', options, () => {
  for (const name of readdirSync(SPEC).filter((n) => n.endsWith('.jsonl'))) {
    const { requests, recorded } = splitRecording(readFileSync(join(SPEC, name), 'utf8'));
    const result = run(['--transcript', name.replace(/\.jsonl$/, '')], requests);
    assert.equal(result.status, 0, `${name}: ${result.stderr}`);
    assert.deepEqual(frames(result.stdout), recorded, `${name}: the replay differs from the recording`);
  }
});

test('fail-at replaces one message and keeps the sequence gapless', options, () => {
  const { requests } = splitRecording(readFileSync(join(SPEC, 'doctor-clean.jsonl'), 'utf8'));
  const result = run(['--transcript', 'doctor-clean', '--fail-at', '3'], requests);
  const out = frames(result.stdout);
  assert.equal(out.length, 7);
  assert.equal(out[1].seq, 2);
  assert.equal(out[2].error.code, -32603);
  assert.equal(out[2].error.data.code, 'E_MOCK_FAILURE');
  assert.equal(out[2].seq, 3, 'the injected message must not skip a sequence number');
});

test('an unsupported method is answered with -32601 naming what is supported', options, () => {
  const result = run(['--transcript', 'doctor-clean'], [
    { jsonrpc: '2.0', id: 1, method: 'o824.nope' },
  ]);
  const [answer] = frames(result.stdout);
  assert.equal(answer.error.code, -32601);
  assert.equal(answer.error.data.code, 'E_UNSUPPORTED');
  assert.deepEqual(answer.error.data.supported, ['o824.info', 'o824.discover', 'o824.doctor']);
});

test('an unknown recording is a usage error with the list', options, () => {
  const result = run(['--transcript', 'nope']);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /doctor-clean/);
});

test('a transcript path with spaces is passed through untouched', options, () => {
  const { requests, recorded } = splitRecording(
    readFileSync(join(SPEC, 'doctor-clean.jsonl'), 'utf8'),
  );
  const spaced = join(repoRoot, 'spec', 'transcripts', 'doctor-clean.jsonl');
  const result = run(['--transcript', spaced], requests);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(frames(result.stdout), recorded);
});

test('a closed input does not crash the process', options, () => {
  // No requests at all: the client connected and left.
  const result = run(['--transcript', 'doctor-clean'], []);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.length, 0);
});

test('a transcript under a directory that does not exist is a sentence, not a stack', options, () => {
  const nowhere = join(repoRoot, 'no-such-directory-for-the-mock', 'session.jsonl');
  const result = run(['--transcript', nowhere], []);
  assert.equal(result.status, 8, result.stderr);
  assert.match(result.stderr, /no-such-directory-for-the-mock/);
  assert.ok(!result.stderr.includes('    at '), 'a stack trace is not an error message');
  assert.ok(!result.stderr.includes('UVWASI'), 'the WASI host error must not leak');
});

test('a transcript that is not there is named in the error', options, () => {
  const missing = join(repoRoot, 'spec', 'transcripts', 'definitely-not-here.jsonl');
  const result = run(['--transcript', missing], []);
  assert.equal(result.status, 8, result.stderr);
  assert.match(result.stderr, /definitely-not-here/);
  assert.ok(!result.stderr.includes('    at '));
});

test('the wasm and the host binary agree', options, () => {
  const hostBinary = join(repoRoot, 'target', 'release', 'o824-mock.exe');
  if (!existsSync(hostBinary)) {
    return; // The host build is packaging's job, not this test's.
  }
  const { requests, recorded } = splitRecording(
    readFileSync(join(SPEC, 'doctor-findings.jsonl'), 'utf8'),
  );
  const payload = Buffer.concat(requests.map(frame));
  const wasmRun = run(['--transcript', 'doctor-findings'], requests);
  const hostRun = spawnSync(hostBinary, ['--transcript', join(packageDir, 'transcripts', 'doctor-findings.jsonl')], {
    input: payload,
    maxBuffer: 1 << 26,
  });
  assert.deepEqual(frames(wasmRun.stdout), recorded);
  assert.deepEqual(frames(hostRun.stdout), frames(wasmRun.stdout));
});
