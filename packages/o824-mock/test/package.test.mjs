// What the tarball actually contains.
//
// A package that silently ships without its WebAssembly module is worse than one that
// fails to build: it installs, and then the first `npx` run says "cannot find
// o824-mock.wasm" on a machine the developer cannot debug. So the prepack guard is a
// feature, and it is tested by taking the module away.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, renameSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(here, '..');
const wasm = join(packageDir, 'o824-mock.wasm');
const manifest = JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf8'));

const skip = existsSync(wasm)
  ? {}
  : { skip: 'no o824-mock.wasm; run scripts/build-mock-package.ps1 first' };

function pack() {
  const result = spawnSync('npm', ['pack', '--dry-run', '--json'], {
    cwd: packageDir,
    shell: true,
    encoding: 'utf8',
  });
  return {
    status: result.status,
    stdout: result.stdout ?? '',
    output: `${result.stdout ?? ''}${result.stderr ?? ''}`,
  };
}

test('the tarball carries the module, the entry, the rules and the recordings', skip, () => {
  const result = pack();
  assert.equal(result.status, 0, result.output);

  const [entry] = JSON.parse(result.stdout);
  const files = entry.files.map((file) => file.path.replace(/\\/g, '/'));
  for (const required of [
    'o824-mock.wasm',
    'bin/o824-mock.mjs',
    'lib/launcher.mjs',
    'package.json',
  ]) {
    assert.ok(files.includes(required), `missing ${required} in ${files.join(', ')}`);
  }
  const recordings = files.filter((file) => file.startsWith('transcripts/'));
  assert.equal(recordings.length, 3, recordings.join(', '));
  assert.equal(entry.version, manifest.version);
});

test('a package without the module refuses to build', { skip: skip.skip }, () => {
  const backup = `${wasm}.bak`;
  renameSync(wasm, backup);
  try {
    const result = pack();
    assert.notEqual(result.status, 0, 'packing without the module must fail');
    assert.match(result.output, /build-mock-package\.ps1/);
  } finally {
    renameSync(backup, wasm);
  }
});
