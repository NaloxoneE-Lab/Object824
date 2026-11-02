#!/usr/bin/env node
// The `o824-mock` launcher: run the real replayer — the same Rust code the kernel's
// own `o824 mock` runs — under Node, on any platform.
//
// The WebAssembly module is the whole program. This file's only job is to make the
// guest's world look like the host's: hand it the process's own stdin/stdout, its
// environment, its exit code, and a directory it can read the transcript from. WASI
// cannot open a host path by itself, so the directory holding the transcript is
// preopened at /t and the argument is rewritten to point inside it (see lib/launcher).

import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describeTranscript, LauncherError, requireNode20 } from '../lib/launcher.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const packageDir = join(here, '..');
const wasmPath = join(packageDir, 'o824-mock.wasm');

const USAGE = `Usage: o824-mock --transcript <path|name> [--delay <ms>] [--fail-at <n>] [--protocol <ver>]

Replays a recorded O824 session over stdio, framed exactly as the kernel frames
it (Content-Length headers). A bare name selects a bundled recording:
  doctor-clean, doctor-findings, plan-apply
`;

// Node announces WASI as experimental on every run. A UI developer running the mock
// should not be told their tool is broken, so that one warning is dropped — every
// other warning and error still reaches the terminal.
const emitWarning = process.emitWarning;
process.emitWarning = (warning, ...rest) => {
  const text = String(warning && warning.message ? warning.message : warning);
  if (text.includes('WASI is an experimental feature')) return;
  emitWarning.call(process, warning, ...rest);
};

function fail(message, exitCode) {
  process.stderr.write(`${message}\n`);
  process.exit(exitCode);
}

const argv = process.argv.slice(2);
if (argv.length === 0) {
  process.stdout.write(USAGE);
  process.exit(2);
}
if (argv.includes('--help') || argv.includes('-h')) {
  process.stdout.write(USAGE);
  process.exit(0);
}

const tooOld = requireNode20(process.versions.node);
if (tooOld) fail(tooOld, 2);

let WASI;
try {
  ({ WASI } = await import('node:wasi'));
} catch (error) {
  fail(
    `o824-mock: this Node has no node:wasi module (${error.message}). ` +
      'It runs the mock as WebAssembly, so it needs Node with WASI support.',
    2,
  );
}

let invocation;
try {
  invocation = describeTranscript(argv, packageDir);
} catch (error) {
  if (error instanceof LauncherError) {
    fail(`o824-mock: ${error.message}`, error.exitCode);
  }
  throw error;
}

// The transcript is checked here, on the host, because WASI's own failure for a
// preopen directory that does not exist is a C-level "UVWASI_ENOENT" and a stack
// trace. A developer reading that learns nothing about their own path.
if (!existsSync(invocation.hostFile)) {
  fail(`o824-mock: cannot read the transcript ${invocation.hostFile}: no such file`, 8);
}

if (!existsSync(wasmPath)) {
  fail(
    `o824-mock: ${wasmPath} is missing. Reinstall the package, or run ` +
      'scripts/build-mock-package.ps1 if you are working from the repository.',
    8,
  );
}

const wasi = new WASI({
  version: 'preview1',
  args: ['o824-mock', ...invocation.argv],
  env: process.env,
  preopens: { '/t': invocation.preopen },
  returnOnExit: true,
});

try {
  const module = new WebAssembly.Module(readFileSync(wasmPath));
  const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
  // The exit code is set, not forced: process.exit() terminates with the guest's
  // stdout writes possibly still queued, and on Windows that surfaces as a libuv
  // assertion (UV_HANDLE_CLOSING) and exit code 127 on a large final frame. Letting
  // the loop drain writes everything before the process ends.
  process.exitCode = wasi.start(instance);
} catch (error) {
  fail(`o824-mock: cannot run the WebAssembly module: ${error.message}`, 8);
}
