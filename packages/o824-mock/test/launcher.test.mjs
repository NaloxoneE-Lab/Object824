// The launcher's path rules, as pure functions.
//
// WASI cannot open a host path by itself, so every transcript argument has to be
// turned into "preopen this directory at /t, and pass /t/<name> to the guest". The
// rules are small but they are the whole contract between the package and the user's
// paths, so they are tested without spawning anything.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { join, resolve } from 'node:path';

import {
  describeTranscript,
  requireNode20,
  LauncherError,
  BUNDLED_TRANSCRIPTS,
} from '../lib/launcher.mjs';

const pkg = '/pkg';

test('a bundled name maps into the package transcripts', () => {
  const result = describeTranscript(['--transcript', 'doctor-clean'], pkg);
  assert.deepEqual(result.argv, ['--transcript', '/t/doctor-clean.jsonl']);
  assert.equal(result.preopen, join(pkg, 'transcripts'));
});

test('an absolute path is preopened from its own directory', () => {
  const result = describeTranscript(['--transcript', '/work/session.jsonl'], pkg);
  assert.deepEqual(result.argv, ['--transcript', '/t/session.jsonl']);
  assert.equal(result.preopen, resolve('/work'));
});

test('a relative path resolves against the caller working directory', () => {
  const result = describeTranscript(['--transcript', 'spec/transcripts/x.jsonl'], pkg);
  assert.equal(result.preopen, resolve(process.cwd(), 'spec/transcripts'));
  assert.deepEqual(result.argv, ['--transcript', '/t/x.jsonl']);
});

test('a path with spaces and non-ascii survives untouched', () => {
  const result = describeTranscript(['--transcript', 'D:/开发 目录/session.jsonl'], pkg);
  assert.equal(result.preopen, resolve('D:/开发 目录'));
  assert.deepEqual(result.argv, ['--transcript', '/t/session.jsonl']);
});

test('a windows path with backslashes is a path, not a name', () => {
  const result = describeTranscript(['--transcript', 'D:\\work\\session.jsonl'], pkg);
  assert.equal(result.preopen, resolve('D:\\work'));
  assert.deepEqual(result.argv, ['--transcript', '/t/session.jsonl']);
});

test('other flags pass through unchanged', () => {
  const result = describeTranscript(
    ['--transcript', 'doctor-clean', '--delay', '100', '--fail-at', '3'],
    pkg,
  );
  assert.deepEqual(result.argv, [
    '--transcript',
    '/t/doctor-clean.jsonl',
    '--delay',
    '100',
    '--fail-at',
    '3',
  ]);
});

test('an unknown name is refused here, not by the guest', () => {
  assert.throws(
    () => describeTranscript(['--transcript', 'nope'], pkg),
    (error) =>
      error instanceof LauncherError &&
      error.exitCode === 2 &&
      BUNDLED_TRANSCRIPTS.some((name) => error.message.includes(name)),
  );
});

test('a missing --transcript is a usage error', () => {
  assert.throws(() => describeTranscript([], pkg), (error) => error.exitCode === 2);
});

test('a --transcript with no value is a usage error', () => {
  assert.throws(
    () => describeTranscript(['--transcript'], pkg),
    (error) => error instanceof LauncherError && error.exitCode === 2,
  );
});

test('node older than 20 is refused with a sentence', () => {
  assert.match(requireNode20('18.19.0'), /Node 20/);
  assert.match(requireNode20('19.9.9'), /Node 20/);
  assert.equal(requireNode20('20.0.0'), null);
  assert.equal(requireNode20('24.8.0'), null);
});
