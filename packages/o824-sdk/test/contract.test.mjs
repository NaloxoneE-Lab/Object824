// The contract test: every recorded message must fit the SDK's types.
//
// The design asks for types generated from a JSON Schema. There is no schema yet, so
// the types are hand-written — and this test is what keeps them from drifting away
// from what the kernel actually sends. When the schema lands, this test becomes
// "validate against the schema" and the types become generated; the assertion does not
// change shape.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { assertMessage } from '../dist/index.js';

const here = dirname(fileURLToPath(import.meta.url));
const specDir = resolve(here, '..', '..', '..', 'spec', 'transcripts');

const recordings = readdirSync(specDir).filter((name) => name.endsWith('.jsonl'));

test('the recordings are where the test expects them', () => {
  assert.equal(recordings.length, 3, recordings.join(', '));
});

for (const name of recordings) {
  test(`every message in ${name} fits the wire types`, () => {
    const lines = readFileSync(join(specDir, name), 'utf8')
      .split('\n')
      .filter((line) => line.trim() !== '');
    assert.ok(lines.length > 0, `${name} is empty`);

    for (const [index, line] of lines.entries()) {
      const message = JSON.parse(line);
      assertMessage(message);

      if (message.method !== undefined && message.id === undefined) {
        // A notification: no id, and its sequence lives inside params.
        assert.equal(typeof message.params?.seq, 'number', `${name}:${index + 1} has no seq`);
      }
      if (message.id !== undefined && message.method === undefined) {
        // A response: seq at the top level.
        assert.equal(typeof message.seq, 'number', `${name}:${index + 1} has no seq`);
        assert.ok(
          ('result' in message) !== ('error' in message),
          `${name}:${index + 1} carries both or neither`,
        );
      }
    }
  });
}
