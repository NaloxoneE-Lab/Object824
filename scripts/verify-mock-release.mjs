#!/usr/bin/env node
// Verifies a built tarball the way a user meets it: install it into an empty project,
// run it through `npx`, and check that the frames it answers match the recording.
//
// This exists because "the tests pass" and "the package works when installed by
// someone who does not have the repository" are different claims. It is used by CI
// after the package is built, and by hand before publishing a release:
//
//   node scripts/verify-mock-release.mjs dist/o824-mock-0.1.2.tgz
//
// A frame is written before reading, so the check never depends on the runner's stdin
// being at end-of-file; a mock with no input at all would exit zero without proving
// anything.

import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const tarball = process.argv[2];
if (!tarball) {
  console.error('usage: node scripts/verify-mock-release.mjs <tarball>');
  process.exit(2);
}

const recording = join(repoRoot, 'spec', 'transcripts', 'doctor-clean.jsonl');
const lines = readFileSync(recording, 'utf8')
  .split('\n')
  .filter((line) => line.trim() !== '')
  .map((line) => JSON.parse(line));
const requests = lines.filter((message) => message.method !== undefined && message.id !== undefined);
const recorded = lines.filter((message) => !(message.method !== undefined && message.id !== undefined));

const frame = (message) => {
  const body = Buffer.from(JSON.stringify(message));
  return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body]);
};

const frames = (buffer) => {
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
};

const work = mkdtempSync(join(tmpdir(), 'o824-mock-release-'));
let failures = 0;
const check = (name, ok, detail) => {
  if (ok) {
    console.log(`${name.padEnd(44)} PASS`);
  } else {
    console.log(`${name.padEnd(44)} FAIL  ${detail}`);
    failures += 1;
  }
};

try {
  writeFileSync(join(work, 'package.json'), '{"name":"consumer","private":true}\n');

  // The tarball is copied in first: this repository lives under a path with
  // spaces, and a shell-expanded absolute path would be split into several
  // arguments. A relative name inside the work directory has neither problem.
  copyFileSync(resolve(tarball), join(work, basename(tarball)));

  const installed = spawnSync('npm', ['i', '-D', './' + basename(tarball)], {
    cwd: work,
    shell: true,
    encoding: 'utf8',
  });
  check('the tarball installs into an empty project', installed.status === 0,
    `${installed.stdout ?? ''}${installed.stderr ?? ''}`);

  const ran = spawnSync('npx', ['o824-mock', '--transcript', 'doctor-clean'], {
    cwd: work,
    shell: true,
    input: Buffer.concat(requests.map(frame)),
    maxBuffer: 1 << 26,
  });
  check('npx runs the installed package', ran.status === 0, String(ran.stderr ?? ''));

  const answered = frames(ran.stdout ?? Buffer.alloc(0));
  check('it answers the recording frame for frame',
    JSON.stringify(answered) === JSON.stringify(recorded),
    `got ${answered.length} frames, expected ${recorded.length}`);

  const failed = spawnSync('npx', ['o824-mock', '--transcript', 'no-such-recording'], {
    cwd: work,
    shell: true,
    encoding: 'utf8',
  });
  check('an unknown recording is refused with the list',
    failed.status === 2 && /doctor-clean/.test(`${failed.stderr ?? ''}`),
    `exit ${failed.status}`);
} finally {
  rmSync(work, { recursive: true, force: true });
}

console.log(failures === 0 ? '\nthe release asset works as installed' : `\n${failures} check(s) failed`);
process.exit(failures === 0 ? 0 : 1);
