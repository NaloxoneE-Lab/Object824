// The SDK against the recordings.
//
// The mock is a real protocol server: same frames, same job notifications, same error
// codes. That makes it the right target for this test — a client that only ever met a
// hand-written fake would agree with itself and disagree with the kernel.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { connect, RpcError, ConnectionClosedError } from '../dist/index.js';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '..', '..', '..');
const mockEntry = join(repoRoot, 'packages', 'o824-mock', 'bin', 'o824-mock.mjs');

const skip = existsSync(mockEntry)
  ? {}
  : { skip: 'packages/o824-mock is not built; run its build first' };

/** Points the client at the mock, which speaks the kernel's protocol. */
function connectToMock(transcript, extraArgs = []) {
  return connect({
    command: process.execPath,
    args: [mockEntry, '--transcript', transcript, ...extraArgs],
  });
}

test('info reports the protocol version and capabilities', skip, async () => {
  const client = connectToMock('doctor-clean');
  const info = await client.info();
  assert.equal(info.protocolVersion, '0.1.0');
  assert.deepEqual(info.capabilities, ['discover', 'doctor', 'plan', 'apply']);
  await client.close();
});

test('capabilities answers as a lookup, not a version check', skip, async () => {
  const client = connectToMock('doctor-clean');
  const capabilities = await client.capabilities();
  assert.equal(capabilities.doctor, true);
  assert.equal(capabilities.start, undefined);
  await client.close();
});

test('discover returns the installations the recording holds', skip, async () => {
  const client = connectToMock('doctor-clean');
  const installs = await client.discover();
  assert.equal(installs.length, 1);
  assert.equal(installs[0].installId, 'i1');
  assert.equal(installs[0].kind, 'desktop');
  await client.close();
});

test('doctor resolves with the envelope and reports progress on the way', skip, async () => {
  const client = connectToMock('doctor-findings');
  const seen = [];
  client.on((event) => seen.push(event));

  const envelope = await client.doctor({ installId: 'i1', profile: 'desktop' });

  assert.equal(envelope.envelope, 'dsh-doctor/v1');
  assert.equal(envelope.findings[0].code, 'E_DUP_LOADER');
  const progress = seen.filter((event) => event.kind === 'progress');
  assert.ok(progress.length >= 1, 'the job reported progress');
  assert.equal(progress[0].progress.indeterminate, true);
  assert.ok(
    seen.some((event) => event.kind === 'state' && event.state.state === 'done'),
    'the job reported that it finished',
  );
  await client.close();
});

test('a kernel error is rethrown as the kernel sent it', skip, async () => {
  const client = connectToMock('doctor-clean');
  await assert.rejects(
    () => client.request('o824.start', { installId: 'i1' }),
    (error) =>
      error instanceof RpcError &&
      error.code === -32601 &&
      error.kernelCode === 'E_UNSUPPORTED' &&
      Array.isArray(error.data.supported),
  );
  await client.close();
});

test('cancellation is sent, and an unsupporting kernel says so', skip, async () => {
  const client = connectToMock('doctor-clean');
  // The recording never covers o824.cancel, so the mock answers -32601. That is the
  // honest outcome to pin: cancel resolves when the request was accepted, and a kernel
  // that cannot cancel must be visible as an error rather than a silent no-op.
  await assert.rejects(
    () => client.cancel('j1'),
    (error) => error instanceof RpcError && error.kernelCode === 'E_UNSUPPORTED',
  );
  await client.close();
});

test('a dead kernel rejects what was in flight, with a way forward', async () => {
  // A process that never speaks the protocol: every request stays pending until it
  // dies, which is exactly the state a crashed kernel leaves a UI in.
  // Node itself, told to say nothing: a leaf process that is guaranteed to exist and
  // to have no grandchildren, so killing it cannot leave a pipe open behind it.
  const client = connect({
    command: process.execPath,
    args: ['-e', 'setTimeout(() => {}, 20000)'],
  });
  // The handler is attached before the process is killed, not after: a rejection that
  // lands while nothing is listening is an unhandled rejection, and on Linux the child
  // dies fast enough for that race to happen every time.
  const rejected = assert.rejects(
    client.request('o824.info'),
    (error) => error instanceof ConnectionClosedError && /connect again/.test(error.message),
  );
  await client.kill();
  await rejected;
});

test('plan and apply carry their recorded results', skip, async () => {
  const client = connectToMock('plan-apply');
  const plan = await client.plan({ installId: 'i1', profile: 'desktop', withInstall: true });
  assert.equal(plan.id, 'plan-20261101T120000Z-1a2b3c4d');
  assert.equal(plan.steps.length, 2);

  const applied = await client.apply({
    planId: plan.id,
    confirm: true,
    idempotencyKey: 'test-1',
  });
  assert.equal(applied.applied, 2);
  await client.close();
});
