#!/usr/bin/env node
// Refuses to pack a tarball that has no compiled output: `files` is a filter, not a
// requirement, so `npm pack` would cheerfully ship an empty package that installs and
// then fails to import.

import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, '..', 'dist');

for (const required of ['index.js', 'index.d.ts', 'client.js', 'frames.js', 'protocol.js']) {
  if (!existsSync(join(dist, required))) {
    process.stderr.write(
      `o824-sdk: refusing to pack without dist/${required}.\n` +
        'Build it with: npm run build\n',
    );
    process.exit(1);
  }
}
