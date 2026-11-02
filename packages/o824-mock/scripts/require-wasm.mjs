#!/usr/bin/env node
// The prepack guard: refuse to build a package that cannot run.
//
// `npm pack` would happily produce a tarball without the WebAssembly module — the
// `files` list is a filter, not a requirement — and that tarball installs cleanly and
// then fails on the first run. A developer on macOS would have no way to tell whether
// the package is broken or their setup is. So the check runs at prepack time, where
// the fix is a rebuild rather than a bug report.

import { existsSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const packageDir = join(here, '..');

const problems = [];

const wasm = join(packageDir, 'o824-mock.wasm');
if (!existsSync(wasm)) {
  problems.push(`o824-mock.wasm is missing from ${packageDir}`);
}

const transcripts = join(packageDir, 'transcripts');
const recordings = existsSync(transcripts)
  ? readdirSync(transcripts).filter((name) => name.endsWith('.jsonl'))
  : [];
if (recordings.length !== 3) {
  problems.push(`transcripts/ holds ${recordings.length} recordings, expected 3`);
}

if (problems.length > 0) {
  process.stderr.write(
    `o824-mock: refusing to pack an unusable package:\n` +
      problems.map((problem) => `  - ${problem}\n`).join('') +
      '\nBuild it with: pwsh -File scripts/build-mock-package.ps1\n',
  );
  process.exit(1);
}
